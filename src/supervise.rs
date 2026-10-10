//! Restarting a source thread that panicked (#95, docs/design.md Constraints + SLOs). The runtime does
//! this for a Service's `listen()`; Kanade's own source threads come back the same way, so one bad
//! reading costs a pause rather than the source for good. What a source has posted lives outside
//! what restarts, so a restart neither posts it again nor forgets to withdraw it. A source's thread
//! starts with `spawn`, so its whole body is supervised, setup too. What went wrong with each
//! source, a panic or giving up, is kept for `kanade status`.

use std::any::Any;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

// how long a source that panicked waits before it runs again, as the runtime waits for a Service
pub const RESTART: Duration = Duration::from_secs(5);

/*
 * runs `run` until it returns; a panic, already printed by the panic hook, is logged and `run`
 * runs again after `RESTART`, so a source that keeps panicking never spins
 */
pub fn run(name: &str, run: impl FnMut()) {
    keep(name, run, thread::sleep);
}

/*
 * a source's own thread, named after it so `/proc/<pid>/task/<tid>/comm` says which run, under `run`
 * from its first line: a panic before a restart the source keeps inside, like in its setup, runs
 * it again whole. One that cannot start a thread is stopped
 */
pub fn spawn(name: &str, source: impl FnMut() + Send + 'static) {
    if let Err(error) = start(name, source, thread::sleep) {
        let why = format!("cannot start a thread: {error}");

        eprintln!("kanade: {name} {why}");
        stopped(name, why);
    }
}

fn start(
    name: &str,
    source: impl FnMut() + Send + 'static,
    sleep: impl FnMut(Duration) + Send + 'static,
) -> io::Result<JoinHandle<()>> {
    let owned = name.to_owned();

    thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || keep(&owned, source, sleep))
}

fn keep(name: &str, mut run: impl FnMut(), mut sleep: impl FnMut(Duration)) {
    while let Err(payload) = panic::catch_unwind(AssertUnwindSafe(&mut run)) {
        panicked(name, payload.as_ref());
        eprintln!("kanade: {name} panicked, running it again in {RESTART:?}");

        sleep(RESTART);
    }
}

// what went wrong with one source, by its name; several threads may share one
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Trouble {
    panics: u32,

    // the last panic's message
    last: String,

    // why it gave up for good, if it did
    stopped: Option<String>,
}

impl Trouble {
    fn lines(&self, name: &str) -> impl Iterator<Item = String> {
        let panics = match self.panics {
            0 => None,
            1 => Some(format!("source {name} panicked once: {}", self.last)),
            times => Some(format!(
                "source {name} panicked {times} times, last: {}",
                self.last
            )),
        };
        let stopped = self
            .stopped
            .as_ref()
            .map(|why| format!("source {name} stopped: {why}"));

        panics.into_iter().chain(stopped)
    }
}

// in the order each first went wrong; a source that never did is not here
static TROUBLES: Mutex<Vec<(String, Trouble)>> = Mutex::new(Vec::new());

fn trouble(name: &str, change: impl FnOnce(&mut Trouble)) {
    let mut troubles = TROUBLES.lock().unwrap_or_else(PoisonError::into_inner);

    let at = match troubles.iter().position(|(each, _)| each == name) {
        Some(at) => at,
        None => {
            troubles.push((name.to_owned(), Trouble::default()));
            troubles.len() - 1
        }
    };

    change(&mut troubles[at].1);
}

// a source's panic, caught by whatever runs it again
pub fn panicked(name: &str, payload: &(dyn Any + Send)) {
    let message = payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| String::from("no message"));

    trouble(name, |trouble| {
        trouble.panics += 1;
        trouble.last = message;
    });
}

// a source that gave up for good, and why
pub fn stopped(name: &str, why: String) {
    trouble(name, |trouble| trouble.stopped = Some(why));
}

// a line for each panic count and each stop, the sources in the order they first went wrong
pub fn status() -> Vec<String> {
    TROUBLES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .flat_map(|(name, trouble)| trouble.lines(name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_that_panics_once_restarts_and_keeps_posting() {
        let mut runs = 0;
        let mut posts = Vec::new();
        let mut sleeps = Vec::new();

        keep(
            "test",
            || {
                runs += 1;
                posts.push(runs);
                assert!(runs > 1, "a reading that panics once");
                posts.push(runs * 10);
            },
            |pause| sleeps.push(pause),
        );

        assert_eq!(posts, [1, 2, 20]);
        assert_eq!(sleeps, [RESTART]);
    }

    #[test]
    fn restarts_no_faster_than_every_five_seconds() {
        let mut runs = 0;
        let mut sleeps = Vec::new();

        keep(
            "test",
            || {
                runs += 1;
                assert!(runs > 3, "a source that keeps panicking");
            },
            |pause| sleeps.push(pause),
        );

        assert_eq!(runs, 4);
        assert_eq!(sleeps, [RESTART; 3]);
        assert!(RESTART >= Duration::from_secs(5));
    }

    #[test]
    fn panics_and_stops_are_kept_by_source() {
        let name = "test troubles";

        keep(
            name,
            || {
                let runs = TROUBLES
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(each, _)| each == name)
                    .map_or(0, |(_, trouble)| trouble.panics);
                assert!(runs >= 2, "panics so far: {runs}");
            },
            |_| {},
        );
        stopped(name, String::from("cannot run pw-dump"));

        let lines: Vec<String> = status()
            .into_iter()
            .filter(|line| line.starts_with("source test troubles "))
            .collect();

        assert_eq!(
            lines,
            [
                "source test troubles panicked 2 times, last: panics so far: 1",
                "source test troubles stopped: cannot run pw-dump",
            ]
        );

        let once = Trouble {
            panics: 1,
            last: String::from("boom"),
            stopped: None,
        };
        assert_eq!(
            once.lines("media").collect::<Vec<_>>(),
            ["source media panicked once: boom"]
        );
        assert_eq!(Trouble::default().lines("media").count(), 0);
    }

    // a panic before the source's own `run`, which nothing inside catches, still restarts it whole
    #[test]
    fn a_panic_in_a_sources_setup_is_kept_and_restarts_it() {
        let name = "test setup";
        let (sender, setups) = std::sync::mpsc::channel();
        let mut runs = 0;

        let source = move || {
            runs += 1;
            sender.send(runs).unwrap();
            assert!(runs > 1, "setup failed");

            run(name, || {});
        };

        start(name, source, |_| {}).unwrap().join().unwrap();

        assert_eq!(setups.iter().collect::<Vec<_>>(), [1, 2]);
        assert_eq!(
            status()
                .into_iter()
                .filter(|line| line.starts_with("source test setup "))
                .collect::<Vec<_>>(),
            ["source test setup panicked once: setup failed"]
        );
    }

    #[test]
    fn a_source_that_returns_is_done() {
        let mut runs = 0;

        keep("test", || runs += 1, |_| panic!("no restart"));

        assert_eq!(runs, 1);
    }
}
