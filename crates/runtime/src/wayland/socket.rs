use std::fs::{self, File, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use smithay_client_toolkit::reexports::calloop::{
    Interest, LoopHandle, Mode, PostAction, generic::Generic, ping::Ping,
};

use super::WaylandState;
use crate::ipc::{IpcCall, IpcHandlers, IpcReply, ipc_socket};

// the lock that keeps a second shell of the session out, held until this process exits
static CLAIM: OnceLock<File> = OnceLock::new();

// A client that stalls must not create a thread per connection or an unbounded backlog.
const READERS: usize = 8;
const READY_CALLS: usize = 16;

// Calls read off the loop's thread, waiting for it to take them.
type ReadyCall = (IpcCall, IpcReply);
type ReadyQueue = (SyncSender<ReadyCall>, Mutex<Receiver<ReadyCall>>);

static READY: LazyLock<ReadyQueue> = LazyLock::new(|| {
    let (send, receive) = mpsc::sync_channel(READY_CALLS);
    (send, Mutex::new(receive))
});
static READER_SLOTS: LazyLock<Slots> = LazyLock::new(|| Slots::new(READERS));

// the loop, woken when a call is ready
static WAKE: OnceLock<Ping> = OnceLock::new();

pub fn insert(handle: &LoopHandle<'static, WaylandState>, handlers: IpcHandlers) {
    let source = Generic::new(listen(), Interest::READ, Mode::Level);

    handle
        .insert_source(source, move |_, listener, _| {
            // a failed call only loses that call, the shell keeps running; the call itself is
            // read off this thread, since a client that never finishes sending would otherwise
            // freeze drawing, input and frame callbacks for its whole read timeout
            if let Ok((stream, _)) = listener.accept() {
                request(stream);
            }

            Ok(PostAction::Continue)
        })
        .expect("failed to insert ipc socket");

    let (ping, source) = smithay_client_toolkit::reexports::calloop::ping::make_ping()
        .expect("failed to create ipc wake ping");

    handle
        .insert_source(source, move |(), _, state| {
            for (call, reply) in taken() {
                let draws = handlers.run(&call, reply);

                /*
                 * a `kanade ui` call was queued for the windows, which this has; an act pressed,
                 * typed or scrolled in one asks for that window's own frame there, while a describe
                 * or a find only read them. A call that drew nothing itself left every change to a
                 * Service, whose write already asked for the frames of the windows that read it
                 */
                state.serve_ui();

                if draws.any() {
                    state.request_frames();
                }
            }
        })
        .expect("failed to insert ipc wake ping");

    // a call read before this point is taken by the first wake, which this asks for
    let _ = WAKE.set(ping.clone());

    ping.ping();
}

/*
 * reads one call on a thread of its own and hands it to the loop: a client that sends nothing, or
 * drips its call, costs the shell a thread and its own deadline, never a frame
 */
fn request(stream: UnixStream) {
    let Some(slot) = READER_SLOTS.try_acquire() else {
        // Close overloaded clients immediately; a refusal payload would add another protocol.
        drop(stream);
        return;
    };

    thread::spawn(move || {
        let _slot = slot;
        // a client that never finishes sending must not hold this thread either
        if stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .is_err()
        {
            return;
        }

        let Ok(call) = IpcCall::read(&stream) else {
            return;
        };

        if READY.0.try_send((call, IpcReply::new(stream))).is_err() {
            return;
        }

        if let Some(ping) = WAKE.get() {
            ping.ping();
        }
    });
}

// what the threads read since the loop last looked
fn taken() -> Vec<(IpcCall, IpcReply)> {
    let ready = READY.1.lock().unwrap_or_else(|error| error.into_inner());

    let mut calls = Vec::new();

    while let Ok(call) = ready.try_recv() {
        calls.push(call);
    }

    calls
}

struct Slots {
    available: Mutex<usize>,
}

impl Slots {
    fn new(count: usize) -> Self {
        Self {
            available: Mutex::new(count),
        }
    }

    #[cfg(test)]
    fn available(&self) -> usize {
        *self
            .available
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn try_acquire(&self) -> Option<Slot<'_>> {
        let mut available = self
            .available
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if *available == 0 {
            return None;
        }
        *available -= 1;
        Some(Slot(self))
    }
}

struct Slot<'a>(&'a Slots);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        *self
            .0
            .available
            .lock()
            .unwrap_or_else(|error| error.into_inner()) += 1;
    }
}

fn listen() -> UnixListener {
    let path = ipc_socket();

    // a socket that still answers belongs to a shell of an older build, which holds no lock
    if UnixStream::connect(&path).is_ok() {
        panic!("failed to listen: kanade is already running");
    }

    claim(&path);

    /*
     * bound under a name of its own and renamed into place: the socket at the path never holds the
     * umask's mode, which would let another user connect before it is made private
     */
    let staging = path.with_extension("binding");

    let _ = fs::remove_file(&staging);

    let listener = UnixListener::bind(&staging).expect("failed to create ipc socket");

    // whoever can connect can act on the shell (`kanade ui` presses and types), so only its user can
    fs::set_permissions(&staging, Permissions::from_mode(0o600))
        .expect("failed to make ipc socket private");

    // what is left at the path is the socket of a shell that did not exit cleanly
    let _ = fs::remove_file(&path);

    fs::rename(&staging, &path).expect("failed to place ipc socket");

    // the event loop only wakes this up when a client is waiting, so accept must never block
    listener
        .set_nonblocking(true)
        .expect("failed to make ipc socket non-blocking");

    listener
}

/*
 * one shell per session: the lock is held for as long as this process runs, so two shells starting
 * together cannot both claim the socket, and neither removes the other's live one
 */
fn claim(path: &Path) {
    let lock = path.with_extension("lock");

    let file = File::create(&lock).expect("failed to create ipc lock");

    file.try_lock()
        .expect("failed to listen: kanade is already running");

    // kept until the process exits, which is what holds the lock
    let _ = CLAIM.set(file);
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::time::Instant;

    use super::*;
    use std::sync::{Mutex, MutexGuard};

    fn the_queue_turn() -> MutexGuard<'static, ()> {
        static CALLS: Mutex<()> = Mutex::new(());
        CALLS.lock().unwrap_or_else(|error| error.into_inner())
    }

    /*
     * the loop accepted the stream and answered at once, while the call itself is still being
     * read on another thread: a client that sends nothing used to freeze every frame, key and
     * pointer event for the whole read timeout, once per connect
     */
    #[test]
    fn a_client_that_sends_nothing_does_not_block_the_loop() {
        let _turn = the_queue_turn();

        let (shell, client) = UnixStream::pair().unwrap();

        let start = Instant::now();

        let held = (0..READERS - 1)
            .map(|_| READER_SLOTS.try_acquire().expect("reader slot available"))
            .collect::<Vec<_>>();
        request(shell);

        assert!(
            start.elapsed() < Duration::from_millis(100),
            "accepting waited {:?} on a client that sends nothing",
            start.elapsed()
        );

        // The read owns its last slot while the peer stays open and has sent nothing.
        assert_eq!(READER_SLOTS.available(), 0);
        assert!(taken().is_empty());

        drop(client);
        drop(held);
        wait_for_reader_slot();
    }

    fn wait_for_no_reader_slot() {
        let start = Instant::now();
        while READER_SLOTS.available() != 0 {
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "readers did not occupy their slots"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn wait_for_reader_slot() {
        let start = Instant::now();
        while READER_SLOTS.try_acquire().is_none() {
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "reader slot was not released"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn stalled_clients_fill_the_reader_bound_and_extra_clients_are_closed() {
        let _turn = the_queue_turn();
        let mut clients = Vec::new();

        for _ in 0..READERS {
            let (shell, client) = UnixStream::pair().unwrap();
            clients.push(client);
            request(shell);
        }

        wait_for_no_reader_slot();

        let (shell, mut refused) = UnixStream::pair().unwrap();
        request(shell);
        refused
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut byte = [0];
        assert_eq!(refused.read(&mut byte).unwrap(), 0);
        assert!(taken().is_empty());

        drop(clients);
        wait_for_reader_slot();
    }

    #[test]
    fn a_call_reaches_the_loop() {
        let _turn = the_queue_turn();

        let (shell, mut client) = UnixStream::pair().unwrap();

        request(shell);

        IpcCall::new("describe", &[String::from("dock")])
            .write(&mut client)
            .unwrap();

        client.shutdown(std::net::Shutdown::Write).unwrap();

        let start = Instant::now();

        let calls = loop {
            let calls = taken();

            if !calls.is_empty() {
                break calls;
            }

            assert!(
                start.elapsed() < Duration::from_secs(5),
                "the call never reached the loop"
            );

            thread::sleep(Duration::from_millis(5));
        };

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0.name, "describe");
        assert_eq!(calls[0].0.arguments, ["dock"]);
    }
}
