//! How long to wait before trying again: `first`, then twice as long after each failure in a row,
//! up to `longest`.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    first: Duration,
    longest: Duration,
    failures: u32,
}

impl Backoff {
    pub const fn new(first: Duration, longest: Duration) -> Self {
        Self {
            first,
            longest,
            failures: 0,
        }
    }

    /// Whether the last tries failed, so the next waits `wait`.
    pub fn failing(&self) -> bool {
        self.failures > 0
    }

    /// How many tries failed in a row.
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// How long the next try waits.
    pub fn wait(&self) -> Duration {
        let doublings = self.failures.saturating_sub(1).min(16);

        self.first.saturating_mul(1 << doublings).min(self.longest)
    }

    /// A try failed: how long until the next.
    pub fn failed(&mut self) -> Duration {
        self.failures = self.failures.saturating_add(1);
        self.wait()
    }

    /// A run that lasted `ran` ended. One that lasted `healthy` was no flapping one, so it starts
    /// over. How long until the next run.
    pub fn after(&mut self, ran: Duration, healthy: Duration) -> Duration {
        if ran >= healthy {
            self.reset();
        }

        self.failed()
    }

    /// A try worked.
    pub fn reset(&mut self) {
        self.failures = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn each_failure_waits_twice_as_long_up_to_the_longest_and_a_good_run_starts_over() {
        let mut backoff = Backoff::new(SECOND, 5 * SECOND);

        assert!(!backoff.failing());
        assert_eq!(backoff.wait(), SECOND);

        let waits: Vec<_> = (0..5).map(|_| backoff.failed()).collect();
        assert_eq!(waits, [1, 2, 4, 5, 5].map(|seconds| seconds * SECOND));
        assert!(backoff.failing());

        assert_eq!(backoff.after(SECOND, 60 * SECOND), 5 * SECOND);
        assert_eq!(backoff.after(60 * SECOND, 60 * SECOND), SECOND);

        backoff.reset();
        assert!(!backoff.failing());

        for _ in 0..100 {
            backoff.failed();
        }
        assert_eq!(backoff.wait(), 5 * SECOND);
    }
}
