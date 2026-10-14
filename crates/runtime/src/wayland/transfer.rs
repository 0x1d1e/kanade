use std::cell::RefCell;
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::rc::Rc;
use std::time::{Duration, Instant};

use smithay_client_toolkit::data_device_manager::ReadPipe;
use smithay_client_toolkit::reexports::calloop::{
    PostAction, RegistrationToken,
    generic::NoIoDrop,
    timer::{TimeoutAction, Timer},
};

use super::WaylandState;

// the other program chose to offer the data, and may be hostile or stuck: it gets this long
const DEADLINE: Duration = Duration::from_secs(2);

// runs once with everything the source wrote, or none when it was too much, too slow or failed
pub type Done = Box<dyn FnOnce(&mut WaylandState, Option<Vec<u8>>)>;

// what a source has written so far, never more than `limit`
struct Bounded {
    data: Vec<u8>,
    limit: usize,
}

#[derive(Debug, PartialEq, Eq)]
enum Step {
    More,
    Done,
    Rejected,
}

impl Bounded {
    fn new(limit: usize) -> Self {
        Self {
            data: Vec::new(),
            limit,
        }
    }

    // takes what `from` has now, which is at most one read
    fn fill(&mut self, from: &mut impl Read) -> Step {
        let mut chunk = [0; 8192];

        match from.read(&mut chunk) {
            Ok(0) => Step::Done,

            // a source that goes over is dropped whole, not cut at a limit that may split a letter
            Ok(count) if self.data.len() + count > self.limit => Step::Rejected,

            Ok(count) => {
                self.data.extend_from_slice(&chunk[..count]);

                Step::More
            }

            Err(error) if error.kind() == ErrorKind::Interrupted => Step::More,

            Err(_) => Step::Rejected,
        }
    }
}

// the two sources a transfer waits on, each removing the other when it ends the transfer
struct Pending {
    bytes: Bounded,
    done: Done,
    read: Option<RegistrationToken>,
    timer: Option<RegistrationToken>,
}

type Shared = Rc<RefCell<Option<Pending>>>;

impl WaylandState {
    /*
     * reads a pipe on the event loop a chunk at a time, so a source that stalls costs the shell
     * nothing but its own deadline
     */
    pub(super) fn receive(&mut self, pipe: ReadPipe, limit: usize, done: Done) {
        let pending: Shared = Rc::new(RefCell::new(Some(Pending {
            bytes: Bounded::new(limit),
            done,
            read: None,
            timer: None,
        })));

        let handle = self.loop_handle.clone();

        let on_read = {
            let pending = pending.clone();
            let handle = handle.clone();

            move |(), file: &mut NoIoDrop<File>, state: &mut WaylandState| {
                let mut slot = pending.borrow_mut();

                let Some(open) = slot.as_mut() else {
                    return PostAction::Remove;
                };

                // the pipe is readable, so this read does not wait
                let step = open.bytes.fill(&mut &**file);

                if step == Step::More {
                    return PostAction::Continue;
                }

                let mut open = slot.take().expect("checked above");

                drop(slot);

                // this source is removed by the return below
                open.read = None;

                if let Some(timer) = open.timer.take() {
                    handle.remove(timer);
                }

                let bytes = (step == Step::Done).then_some(open.bytes.data);

                (open.done)(state, bytes);

                PostAction::Remove
            }
        };

        let Ok(read) = handle.insert_source(pipe, on_read) else {
            return self.fail(&pending);
        };

        let on_timeout = {
            let pending = pending.clone();
            let handle = handle.clone();

            move |_: Instant, _: &mut (), state: &mut WaylandState| {
                let open = pending.borrow_mut().take();

                if let Some(mut open) = open {
                    // this source is dropped by the return below
                    open.timer = None;

                    if let Some(read) = open.read.take() {
                        handle.remove(read);
                    }

                    (open.done)(state, None);
                }

                TimeoutAction::Drop
            }
        };

        let Ok(timer) = handle.insert_source(Timer::from_duration(DEADLINE), on_timeout) else {
            handle.remove(read);

            return self.fail(&pending);
        };

        if let Some(open) = pending.borrow_mut().as_mut() {
            open.read = Some(read);
            open.timer = Some(timer);
        }
    }

    fn fail(&mut self, pending: &Shared) {
        let open = pending.borrow_mut().take();

        if let Some(open) = open {
            (open.done)(self, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn drain(bounded: &mut Bounded, bytes: &[u8]) -> Step {
        let mut from = Cursor::new(bytes.to_vec());

        loop {
            match bounded.fill(&mut from) {
                Step::More => {}
                step => return step,
            }
        }
    }

    #[test]
    fn what_fits_the_limit_is_kept() {
        let mut bounded = Bounded::new(20_000);

        assert_eq!(drain(&mut bounded, &[b'a'; 20_000]), Step::Done);
        assert_eq!(bounded.data.len(), 20_000);
    }

    #[test]
    fn one_byte_over_the_limit_rejects_all_of_it() {
        let mut bounded = Bounded::new(20_000);

        assert_eq!(drain(&mut bounded, &[b'a'; 20_001]), Step::Rejected);
    }

    #[test]
    fn a_failed_read_rejects() {
        struct Broken;

        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(ErrorKind::BrokenPipe.into())
            }
        }

        assert_eq!(Bounded::new(10).fill(&mut Broken), Step::Rejected);
    }
}
