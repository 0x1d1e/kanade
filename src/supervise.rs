//! Restarting a source thread that panicked (docs/archived/design.md Constraints + SLOs). The runtime does
//! this for a Service's `listen()`; Kanade's own source threads come back the same way, so one bad
//! reading costs a pause rather than the source for good. What a source has posted lives outside
//! what restarts, so a restart neither posts it again nor forgets to withdraw it. A source's thread
//! starts with `spawn`, so its whole body is supervised, setup too. What went wrong with each
//! source, a panic or giving up, is kept for `kanade status`.
//!
//! A source that returns is not restarted: some are done once their work is finished (`seed`), and
//! one that ends because its bus went away must not spin. So a return is recorded like a panic,
//! which `kanade status` shows, and the source that can go on after one runs its own loop with
//! `crate::backoff` (as `battery`, `seat` and `wallpaper::watch` do). `niri` is one such: after ADR
//! 0071 it reconnects its stream in its own loop, so the supervised run stays alive across a lost
//! stream and a return there is still final.
//!
//! A return is a fault only for a source meant to run for the session. One whose returning is its
//! whole design, as `wallpaper::seed` filling an empty folder once, starts with `spawn_once` and
//! records nothing: `kanade status` is how the user finds real trouble, so a standing line for a
//! source that did exactly what it is for would drown it. What is worth saying without being
//! trouble, as hardware the machine does not have, `note` says once.

use std::any::Any;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use kanade_runtime::{MutexExt, service};

// how long a source that panicked waits before it runs again, as the runtime waits for a Service
pub const RESTART: Duration = service::RESTART_DELAY;

/*
 * runs `run` until it returns; a panic, already printed by the panic hook, is logged and `run`
 * runs again after `RESTART`, so a source that keeps panicking never spins. A return ends it, and
 * says so once
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
    spawn_ending(name, source, End::Early);
}

/*
 * a source whose returning is its own end, not a fault: a one-shot that does its work and is done
 * (`wallpaper-seed`), or a holder that hands over to a daemon already running. A panic still
 * restarts it, and a source that has to say why it ended says it itself with `stopped`; a plain
 * return records nothing, so `status` keeps no standing line for a source that did what it is for
 */
pub fn spawn_once(name: &str, source: impl FnMut() + Send + 'static) {
    spawn_ending(name, source, End::ByDesign);
}

fn spawn_ending(name: &str, source: impl FnMut() + Send + 'static, end: End) {
    if let Err(error) = start(name, source, thread::sleep, end) {
        let why = format!("cannot start a thread: {error}");

        eprintln!("kanade: {name} {why}");
        stopped(name, why);
    }
}

// whether a source that returns ended early, which `status` says, or as designed, which it does not
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Early,
    ByDesign,
}

fn start(
    name: &str,
    source: impl FnMut() + Send + 'static,
    sleep: impl FnMut(Duration) + Send + 'static,
    end: End,
) -> io::Result<JoinHandle<()>> {
    let owned = name.to_owned();

    thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || keep_ending(&owned, source, sleep, end))
}

fn keep(name: &str, run: impl FnMut(), sleep: impl FnMut(Duration)) {
    keep_ending(name, run, sleep, End::Early);
}

fn keep_ending(name: &str, mut run: impl FnMut(), mut sleep: impl FnMut(Duration), end: End) {
    loop {
        match panic::catch_unwind(AssertUnwindSafe(&mut run)) {
            Err(payload) => {
                panicked(name, payload.as_ref());
                eprintln!("kanade: {name} panicked, running it again in {RESTART:?}");

                sleep(RESTART);
            }

            Ok(()) => {
                if end == End::Early {
                    returned(name);
                }

                return;
            }
        }
    }
}

// a source's run ended on its own, which `status` says; said once, as a restart is recorded as a
// panic
fn returned(name: &str) {
    if trouble(name, |trouble| trouble.stopped.is_some()) {
        return;
    }

    let why = String::from("it ended");

    eprintln!("kanade: {name} ended");
    stopped(name, why);
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

// what is worth saying of a source that is not in trouble, by name, in the order noted
static NOTES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

// `change` is handed the source's Trouble, and says something back to the caller
fn trouble<T>(name: &str, change: impl FnOnce(&mut Trouble) -> T) -> T {
    let mut troubles = TROUBLES.locked();

    let at = match troubles.iter().position(|(each, _)| each == name) {
        Some(at) => at,
        None => {
            troubles.push((name.to_owned(), Trouble::default()));
            troubles.len() - 1
        }
    };

    change(&mut troubles[at].1)
}

// a Service's panics, which the runtime restarts, are kept for `status` like a source's
pub fn follow_services() {
    service::on_panic(panicked);
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

/*
 * what is worth knowing of a source that is not in trouble, said once, as hardware the machine
 * does not have: a feature silently absent is not damage, but it is not silence either. Said once
 * however often the source says it, as a source may run again
 */
pub fn note(name: &str, what: String) {
    let mut notes = NOTES.locked();

    if notes.iter().any(|(each, _)| each == name) {
        return;
    }

    eprintln!("kanade: {name}: {what}");
    notes.push((name.to_owned(), what));
}

// a line for each panic count and each stop, the sources in the order they first went wrong, then
// a note per source that is not in trouble
pub fn status() -> Vec<String> {
    let mut lines: Vec<String> = TROUBLES
        .locked()
        .iter()
        .flat_map(|(name, trouble)| trouble.lines(name))
        .collect();

    lines.extend(
        NOTES
            .locked()
            .iter()
            .map(|(name, what)| format!("source {name}: {what}")),
    );

    lines
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

        start(name, source, |_| {}, End::Early)
            .unwrap()
            .join()
            .unwrap();

        assert_eq!(setups.iter().collect::<Vec<_>>(), [1, 2]);
        assert_eq!(
            status()
                .into_iter()
                .filter(|line| line.starts_with("source test setup "))
                .collect::<Vec<_>>(),
            [
                "source test setup panicked once: setup failed",
                // its second run finishes, which is a stop now, not silence
                "source test setup stopped: it ended",
            ]
        );
    }

    #[test]
    fn a_source_that_returns_is_done() {
        let mut runs = 0;

        keep("test", || runs += 1, |_| panic!("no restart"));

        assert_eq!(runs, 1);
    }

    // a source that ended rather than panicked is said once, not at every restart that ends again
    #[test]
    fn a_source_that_returns_is_reported_once() {
        let name = "test returned";
        let lines = || {
            status()
                .into_iter()
                .filter(|line| line.starts_with(&format!("source {name} ")))
                .collect::<Vec<_>>()
        };

        keep(name, || {}, |_| panic!("no restart"));

        assert_eq!(lines(), [format!("source {name} stopped: it ended")]);

        keep(name, || {}, |_| panic!("no restart"));

        assert_eq!(lines(), [format!("source {name} stopped: it ended")]);
    }

    // a source that is done by design records nothing, while one that ends early still does: the
    // same return, told apart by how its spawn said its return should be read
    #[test]
    fn a_one_shot_that_returns_is_no_trouble_but_an_early_end_is() {
        let once = "test one-shot";
        let early = "test ended early";

        start(once, || {}, |_| panic!("no restart"), End::ByDesign)
            .unwrap()
            .join()
            .unwrap();
        start(early, || {}, |_| panic!("no restart"), End::Early)
            .unwrap()
            .join()
            .unwrap();

        let lines: Vec<String> = status()
            .into_iter()
            .filter(|line| line.contains(once) || line.contains(early))
            .collect();

        assert_eq!(lines, [format!("source {early} stopped: it ended")]);
    }

    // said once, and not as trouble: a machine without the hardware is not a broken source
    #[test]
    fn a_note_is_said_once_and_is_not_trouble() {
        let name = "test note";

        note(name, String::from("this machine has none"));
        note(name, String::from("this machine has none"));

        let lines: Vec<String> = status()
            .into_iter()
            .filter(|line| line.contains(name))
            .collect();

        assert_eq!(lines, [format!("source {name}: this machine has none")]);
    }
}
