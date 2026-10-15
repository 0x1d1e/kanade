//! Confirm by holding (ADR 0085): a destructive press that counts only when kept for `HOLD`. A target
//! pressed with the pointer, or a key held, fills toward the end of it, which then does the act; let
//! go before it, the fill drains and nothing happens, so a tap or a stray key destroys nothing. The
//! act runs on the timer's thread, never in a view.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::service::Service;
use kanade_runtime::{Color, Rectangle, Stack, Widget, children, request_frame};

use crate::config;
use crate::island::motion::Mode;
use crate::island::service::IslandService;

// how long a press is kept before it acts
pub const HOLD: Duration = Duration::from_millis(700);

// an early release drains the fill back in this time
const DRAIN: Duration = Duration::from_millis(180);

// the fill's strength over a Surface's own colors
const STRENGTH: u8 = 110;

// one hold at a time: only one pointer button or key is held at once
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Confirm {
    // counts presses, so a timer that slept through a release and a newer press is stale
    serial: u64,

    // what is held, by hash
    what: u64,

    // when the press began, none once it is let go or acted
    since: Option<Instant>,

    // how full it was when it ended and when, draining from there
    drain: Option<(f32, Instant)>,
}

impl Service for Confirm {
    fn new() -> Self {
        Confirm::default()
    }

    fn listen() {}
}

impl Confirm {
    fn pressed(&mut self, what: u64, now: Instant) -> u64 {
        self.serial += 1;
        self.what = what;
        self.since = Some(now);
        self.drain = None;

        self.serial
    }

    fn released(&mut self, now: Instant) -> bool {
        let Some(since) = self.since.take() else {
            return false;
        };

        self.drain = Some((progress(since, now), now));

        true
    }

    // whether press `serial` has been kept for `HOLD`, and so acts: once, it ends there
    fn due(&mut self, serial: u64, now: Instant) -> bool {
        let kept = self.serial == serial && self.since.is_some_and(|since| now >= since + HOLD);

        if kept {
            self.since = None;
            self.drain = Some((1.0, now));
        }

        kept
    }

    // how full `what` is: 0 to 1, and whether it still moves. Reduced motion has no fill to move,
    // it is full while held
    fn fill_at(&self, what: u64, now: Instant, reduced: bool) -> (f32, bool) {
        if self.what != what {
            return (0.0, false);
        }

        if let Some(since) = self.since {
            return if reduced {
                (1.0, false)
            } else {
                let fill = progress(since, now);

                (fill, fill < 1.0)
            };
        }

        match self.drain {
            Some((from, at)) if !reduced => {
                let left =
                    1.0 - (now.saturating_duration_since(at).as_secs_f32() / DRAIN.as_secs_f32());
                let fill = from * left.clamp(0.0, 1.0);

                (fill, fill > 0.0)
            }
            _ => (0.0, false),
        }
    }
}

fn progress(since: Instant, now: Instant) -> f32 {
    (now.saturating_duration_since(since).as_secs_f32() / HOLD.as_secs_f32()).clamp(0.0, 1.0)
}

fn name(what: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();

    what.hash(&mut hasher);

    hasher.finish()
}

/*
 * starts holding `what`: `then` runs once `HOLD` has passed with no release, no other key and no
 * new visit of the Surface between. Pointer and key alike end it with `release`
 */
pub fn press(what: impl Hash, then: Arc<dyn Fn() + Send + Sync>) {
    let visit = IslandService::read().visit();
    let serial = Confirm::write().pressed(name(what), Instant::now());

    // a timer that cannot start leaves a hold that never acts
    let _ = thread::Builder::new()
        .name(String::from("hold"))
        .spawn(move || {
            thread::sleep(HOLD);

            let mut hold = Confirm::write();
            let kept = hold.due(serial, Instant::now());

            if !kept {
                hold.quiet();
            }

            drop(hold);

            if kept && IslandService::read().visit() == visit {
                then();
            }
        });
}

// the press ends: the pointer or a key came up, the pointer left or another key came down
pub fn release() {
    let mut hold = Confirm::write();

    if !hold.released(Instant::now()) {
        hold.quiet();
    }
}

// how full `what` is for the view, which keeps drawing while it moves
pub fn fill(what: impl Hash) -> f32 {
    let reduced = config::get().island.motion == Mode::Reduced;
    let (fill, moving) = Confirm::read().fill_at(name(what), Instant::now(), reduced);

    if moving {
        request_frame();
    }

    fill
}

/*
 * `target` held with the pointer acts as `press` says. A tap does nothing but claim the click
 * (and a right click pins, as on any target of a Surface)
 */
pub fn target(
    target: Rectangle,
    what: impl Hash + Clone + 'static,
    then: Arc<dyn Fn() + Send + Sync>,
) -> Rectangle {
    target
        .on_hold(move |down| {
            if down {
                crate::view::claim();
                press(what.clone(), then.clone());
            } else {
                release();
            }
        })
        .on_click(super::on_left(|| {}))
}

/*
 * `content` over the fill of `what`, growing from the start: for a target clipped to its rounded
 * shape, `ink` the color it fills with
 */
pub fn behind(
    what: impl Hash,
    width: f32,
    height: f32,
    ink: Color,
    content: impl Widget + 'static,
) -> Stack {
    let filled = Rectangle::new()
        .width(width * fill(what))
        .height(height)
        .fill(Color::rgba(ink.red(), ink.green(), ink.blue(), STRENGTH));

    Stack::new(children![filled, content])
}

#[cfg(test)]
mod tests {
    use super::*;

    // the act runs only for a press kept the whole time: a release, or a newer press, makes the
    // timer of the old one stale
    #[test]
    fn only_a_press_kept_for_the_hold_acts_and_only_once() {
        let start = Instant::now();
        let mut hold = Confirm::default();

        let tap = hold.pressed(1, start);
        assert!(hold.released(start + HOLD / 2));
        assert!(!hold.due(tap, start + HOLD));

        let replaced = hold.pressed(1, start);
        let newer = hold.pressed(1, start + HOLD / 2);
        assert!(!hold.due(replaced, start + HOLD));
        assert!(!hold.due(newer, start + HOLD - Duration::from_millis(1)));
        assert!(hold.due(newer, start + HOLD / 2 + HOLD));
        assert!(!hold.due(newer, start + HOLD * 2));
    }

    // an early release drains from where it was, and reduced motion shows no stages
    #[test]
    fn the_fill_grows_drains_and_stays_still_under_reduced_motion() {
        let start = Instant::now();
        let mut hold = Confirm::default();

        hold.pressed(7, start);

        assert_eq!(hold.fill_at(7, start + HOLD / 2, false), (0.5, true));
        assert_eq!(hold.fill_at(8, start + HOLD / 2, false), (0.0, false));
        assert_eq!(hold.fill_at(7, start + HOLD / 2, true), (1.0, false));

        hold.released(start + HOLD / 2);

        assert_eq!(hold.fill_at(7, start + HOLD / 2, false), (0.5, true));
        assert_eq!(
            hold.fill_at(7, start + HOLD / 2 + DRAIN, false),
            (0.0, false)
        );
        assert_eq!(hold.fill_at(7, start + HOLD / 2, true), (0.0, false));
    }
}
