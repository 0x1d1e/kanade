//! The key that wakes the monitors (ADR 0065): not typed into the password field. The shell says
//! when it powered them off and when input lit them; this decides which key was the waking one.
//! Pure: time is an input, never read here.

use std::time::{Duration, Instant};

/*
 * how long after the shell says input lit the monitors the key that did may still arrive here: the
 * shell hears the compositor's idle notification on one connection and this the key on another, so
 * either may come first. Short, so a password typed at once after a wake that sent no key here is
 * not eaten
 */
pub(crate) const LATE: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    // nothing to swallow
    #[default]
    Awake,

    // the monitors are off
    Dark,

    // input lit them at this moment, and the key that did may still come
    Lit(Instant),
}

#[derive(Debug, Default)]
pub(crate) struct Wake {
    state: State,

    // the key swallowed, held down: its repeats are swallowed too
    held: Option<u32>,
}

impl Wake {
    // the monitors went off
    pub(crate) fn dark(&mut self) {
        self.state = State::Dark;
    }

    // input lit the monitors, at `now`
    pub(crate) fn lit(&mut self, now: Instant) {
        if self.state == State::Dark {
            self.state = State::Lit(now);
        }
    }

    // the lock ended or began: nothing waits for a waking key
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /*
     * the first input after the monitors went off was what woke them. Whether the key `code`,
     * pressed or repeating at `now`, is that one and not to be typed: it, and its repeats while it
     * is held, are not
     */
    pub(crate) fn key(&mut self, code: u32, now: Instant) -> bool {
        if self.held == Some(code) {
            return true;
        }

        if self.armed(now) {
            self.state = State::Awake;
            self.held = Some(code);
            return true;
        }

        false
    }

    // the key `code` went up
    pub(crate) fn released(&mut self, code: u32) {
        if self.held == Some(code) {
            self.held = None;
        }
    }

    // the keyboard left the lock surface: no key stays held
    pub(crate) fn left(&mut self) {
        self.held = None;
    }

    // the pointer moved, pressed or scrolled: it woke them, and nothing of it is typed
    pub(crate) fn pointer(&mut self) {
        self.state = State::Awake;
    }

    // whether the waking key is still to come at `now`
    fn armed(&self, now: Instant) -> bool {
        match self.state {
            State::Awake => false,
            State::Dark => true,
            State::Lit(at) => now.saturating_duration_since(at) < LATE,
        }
    }
}

// how far, in logical pixels, the pointer must go from where it was to count as moved by a hand
const MOVED: f64 = 4.0;

/*
 * whether the pointer moved by a hand: a lock surface appearing under a still pointer, a pointer
 * crossing to another output and a few pixels of jitter are not. Pure, so it is tested
 */
#[derive(Debug, Default)]
pub(crate) struct Pointer {
    at: Option<(f64, f64)>,
}

impl Pointer {
    // the pointer arrived at `at` on a surface, which is no movement
    pub(crate) fn entered(&mut self, at: (f64, f64)) {
        self.at = Some(at);
    }

    pub(crate) fn left(&mut self) {
        self.at = None;
    }

    // whether the pointer, now at `at`, moved far enough from where it was last counted
    pub(crate) fn moved(&mut self, at: (f64, f64)) -> bool {
        let Some(from) = self.at else {
            // none known: this is where it is, which is no movement
            self.at = Some(at);
            return false;
        };

        let far = (at.0 - from.0).hypot(at.1 - from.1) >= MOVED;
        if far {
            self.at = Some(at);
        }

        far
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pointer_counts_as_moved_only_by_a_few_pixels_from_where_it_was() {
        let mut pointer = Pointer::default();

        // under a lock surface that appeared, or on another output: where it is, not a move
        assert!(!pointer.moved((100.0, 100.0)));
        pointer.entered((50.0, 50.0));
        assert!(!pointer.moved((51.0, 50.5)));
        assert!(!pointer.moved((49.0, 52.0)));

        // jitter is measured from where the last move was counted; a hand's move counts
        assert!(!pointer.moved((53.0, 50.0)));
        assert!(pointer.moved((54.0, 50.0)));
        assert!(!pointer.moved((54.0, 51.0)));

        pointer.left();
        assert!(!pointer.moved((400.0, 400.0)));
    }

    // the first key after the monitors went off woke them, whichever way the news and it race
    #[test]
    fn the_first_key_after_the_monitors_went_off_is_not_typed() {
        let start = Instant::now();
        let after = |millis| start + Duration::from_millis(millis);

        // the key first, then the news
        let mut wake = Wake::default();
        wake.dark();
        assert!(wake.key(30, after(1_000)));
        wake.lit(after(1_005));
        assert!(!wake.key(31, after(1_100)));

        // the news first, the key soon after
        let mut wake = Wake::default();
        wake.dark();
        wake.lit(after(1_000));
        assert!(wake.key(30, after(1_010)));
        assert!(!wake.key(31, after(1_020)));
    }

    // a key that never came here leaves nothing to eat once the news is old
    #[test]
    fn a_key_long_after_the_monitors_lit_is_typed() {
        let start = Instant::now();
        let mut wake = Wake::default();

        wake.dark();
        wake.lit(start);
        assert!(!wake.key(30, start + LATE));

        // and nothing is armed by news of light with the monitors on
        let mut wake = Wake::default();
        wake.lit(start);
        assert!(!wake.key(30, start));
    }

    // the pointer wakes them as well, and no key after it is lost
    #[test]
    fn a_pointer_wake_swallows_no_key() {
        let start = Instant::now();
        let mut wake = Wake::default();

        wake.dark();
        wake.pointer();
        assert!(!wake.key(30, start));
    }

    #[test]
    fn the_waking_key_held_is_swallowed_until_it_is_released() {
        let start = Instant::now();
        let mut wake = Wake::default();

        wake.dark();
        assert!(wake.key(57, start));

        // repeats, and another key typed meanwhile
        assert!(wake.key(57, start + Duration::from_secs(1)));
        assert!(!wake.key(30, start + Duration::from_secs(1)));
        assert!(wake.key(57, start + Duration::from_secs(2)));

        wake.released(57);
        assert!(!wake.key(57, start + Duration::from_secs(3)));

        // a reset forgets all of it
        wake.dark();
        assert!(wake.key(57, start));
        wake.reset();
        assert!(!wake.key(57, start));
    }
}
