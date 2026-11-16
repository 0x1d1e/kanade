//! The instant one frame samples, and the per-window clock that predicts when it reaches the
//! screen (ADR 0154). Views read [`now`]; deadlines and input keep `Instant::now`.

use std::cell::Cell;
use std::time::{Duration, Instant};

thread_local! {
    // set for one window's view and drawing, so every sample in the frame agrees
    static FROZEN: Cell<Option<Instant>> = const { Cell::new(None) };
}

// the instant the frame being drawn samples; real time outside a frozen frame
pub fn now() -> Instant {
    FROZEN.with(Cell::get).unwrap_or_else(Instant::now)
}

// freezes the samples this thread takes until `clear`, such as one frame's predicted present
pub fn set(at: Instant) {
    FROZEN.with(|frozen| frozen.set(Some(at)));
}

// real time again, after the frame that froze it
pub fn clear() {
    FROZEN.with(|frozen| frozen.set(None));
}

// the interval between frames until a window says how often it draws: 60 Hz
const DEFAULT_INTERVAL: Duration = Duration::from_micros(16_667);

// a frame interval a window can present at; slower is idle, faster is a redraw within a frame
const MIN_INTERVAL: Duration = Duration::from_millis(4);
const MAX_INTERVAL: Duration = Duration::from_millis(50);

// a gap below this cannot be a frame apart, and one above it is an idle stretch, not a frame's
const MIN_GAP: Duration = Duration::from_millis(4);
const MAX_GAP: Duration = Duration::from_millis(200);

// how much one frame moves the smoothed interval toward its gap
const WEIGHT: f32 = 0.2;

/*
 * one window's frame clock: the last frame start and the smoothed interval between frames, so a
 * view samples at the time its frame is likely to reach the screen rather than at the callback
 */
pub struct FrameClock {
    last: Option<Instant>,
    interval: Duration,
}

impl FrameClock {
    pub fn new() -> Self {
        Self {
            last: None,
            interval: DEFAULT_INTERVAL,
        }
    }

    /*
     * the instant this frame should sample, from when it started; a frame that follows another
     * plausibly gets the time it is likely to present at, while an idle stretch samples real time
     * and only a plausible consecutive gap moves the interval
     */
    pub fn next(&mut self, now: Instant) -> Instant {
        let Some(last) = self.last.replace(now) else {
            return now;
        };

        let gap = now.saturating_duration_since(last);

        if !(MIN_GAP..=MAX_GAP).contains(&gap) {
            return now;
        }

        let smoothed = self.interval.mul_f32(1.0 - WEIGHT) + gap.mul_f32(WEIGHT);
        self.interval = smoothed.clamp(MIN_INTERVAL, MAX_INTERVAL);

        now + self.interval
    }

    // a frame that was hidden or unconfigured ends the chain: no gap across it teaches the interval
    pub fn reset(&mut self) {
        self.last = None;
    }
}

impl Default for FrameClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frozen_instant_is_sampled_until_cleared() {
        clear();

        let before = Instant::now();
        let at = before
            .checked_sub(Duration::from_millis(9))
            .expect("an instant before now");

        set(at);
        assert_eq!(now(), at);
        assert_eq!(now(), at);

        clear();
        assert!(now() > at);
    }

    #[test]
    fn the_frame_clock_never_samples_before_it_started() {
        let start = Instant::now();
        let mut clock = FrameClock::new();

        assert_eq!(clock.next(start), start);

        let second = start + Duration::from_micros(16_667);
        let target = clock.next(second);

        assert!(target >= second);
        assert!(target <= second + MAX_INTERVAL);
    }

    #[test]
    fn a_hundred_and_sixty_five_hertz_becomes_the_interval() {
        let start = Instant::now();
        let mut clock = FrameClock::new();
        let mut now = start;

        clock.next(now);

        for _ in 0..200 {
            now += Duration::from_micros(6_060);
            clock.next(now);
        }

        assert!(clock.interval < Duration::from_millis(7));
        assert!(clock.interval >= MIN_INTERVAL);

        let target = clock.next(now);

        assert!(target >= now);
        assert!(target <= now + MAX_INTERVAL);
    }

    #[test]
    fn sixty_hertz_stays_the_interval() {
        let start = Instant::now();
        let mut clock = FrameClock::new();
        let mut now = start;

        clock.next(now);

        for _ in 0..60 {
            now += Duration::from_micros(16_667);
            clock.next(now);
        }

        assert!(clock.interval.abs_diff(DEFAULT_INTERVAL) < Duration::from_micros(20));
    }

    #[test]
    fn an_idle_gap_is_not_a_frame_and_teaches_nothing() {
        let start = Instant::now();
        let mut clock = FrameClock::new();

        clock.next(start);
        let idle = start + Duration::from_secs(3);

        assert_eq!(clock.next(idle), idle);
        assert_eq!(clock.interval, DEFAULT_INTERVAL);

        // the first frame after the idle stretch is the one the next gap is measured from
        let resumed = idle + Duration::from_micros(16_667);
        let target = clock.next(resumed);

        assert!(target >= resumed);
        assert!(target <= resumed + MAX_INTERVAL);
        assert!(clock.interval.abs_diff(DEFAULT_INTERVAL) < Duration::from_micros(20));
    }

    #[test]
    fn a_hidden_or_unconfigured_period_teaches_nothing() {
        let start = Instant::now();
        let mut clock = FrameClock::new();

        clock.next(start);
        clock.reset();

        let after = start + Duration::from_micros(6_000);

        assert_eq!(clock.next(after), after);
        assert_eq!(clock.interval, DEFAULT_INTERVAL);
    }

    #[test]
    fn the_interval_is_clamped_to_what_a_screen_can_present() {
        let start = Instant::now();
        let mut clock = FrameClock::new();

        clock.next(start);

        let mut now = start;

        for _ in 0..100 {
            now += Duration::from_millis(200);
            clock.next(now);
        }

        assert_eq!(clock.interval, MAX_INTERVAL);

        let target = clock.next(now + Duration::from_millis(200));

        assert!(target <= now + Duration::from_millis(200) + MAX_INTERVAL);
    }
}
