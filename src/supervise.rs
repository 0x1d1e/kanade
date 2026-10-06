//! Restarting a source thread that panicked (#95, docs/design.md Constraints + SLOs). Amane does
//! this for a Service's `listen()`; Kanade's own source threads come back the same way, so one bad
//! reading costs a pause rather than the source for good. What a source has posted lives outside
//! what restarts, so a restart neither posts it again nor forgets to withdraw it.

use std::panic::{self, AssertUnwindSafe};
use std::thread;
use std::time::Duration;

// how long a source that panicked waits before it runs again, as Amane waits for a Service
pub const RESTART: Duration = Duration::from_secs(5);

/*
 * runs `run` until it returns; a panic, already printed by the panic hook, is logged and `run`
 * runs again after `RESTART`, so a source that keeps panicking never spins
 */
pub fn run(name: &str, run: impl FnMut()) {
    keep(name, run, thread::sleep);
}

fn keep(name: &str, mut run: impl FnMut(), mut sleep: impl FnMut(Duration)) {
    while panic::catch_unwind(AssertUnwindSafe(&mut run)).is_err() {
        eprintln!("kanade: {name} panicked, running it again in {RESTART:?}");

        sleep(RESTART);
    }
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
    fn a_source_that_returns_is_done() {
        let mut runs = 0;

        keep("test", || runs += 1, |_| panic!("no restart"));

        assert_eq!(runs, 1);
    }
}
