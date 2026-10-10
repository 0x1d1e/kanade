//! Damped spring, closed form (plan 6.4), so there is no integrator to jitter. Critically damped
//! by default, `x(t) = target - (A + B t) e^(-wt)` per dimension; with a damping below 1 it is
//! underdamped, `x(t) = target - (A cos(dt) + B sin(dt)) e^(-zwt)` with `d = w sqrt(1 - z^2)`, and
//! passes its target a little and comes back, like macOS's bouncy springs. Only the body's shape
//! bounces; opacity and the Satellites stay critical. Time is always passed in, never read.
//!
//! The dimensions of one `Spring` share `w`, so a group (width, height, radius) moves as one
//! object. Content opacity and translation derive from `progress`, not from timers of their own.
//! Each leg takes its own response, so a collapse and a Surface change can take different times.

use std::time::{Duration, Instant};

/*
 * from rest, the share of the way still left is (1 + wt) e^(-wt); it falls to 5% at wt = 4.744,
 * so a spring with a given response covers 95% of any move in that time
 */
const RESPONSE_FACTOR: f32 = 4.744;

// closer than this in every dimension and slower than SETTLE_SPEED, it snaps and rests
const SETTLE_DISTANCE: f32 = 0.5;
const SETTLE_SPEED: f32 = 5.0;

// and not before a move from rest would be this close, below one step of 8-bit opacity
const SETTLE_SHARE: f32 = 1.0 / 256.0;

// reduced motion: geometry snaps, only the derived opacity still fades (plan 6.4)
pub const REDUCED_FADE: Duration = Duration::from_millis(80);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Spring,
    Reduced,
}

// critically damped: the fastest settle that never passes the target
pub const CRITICAL: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring<const N: usize> {
    mode: Mode,

    // the damping ratio legs start with, CRITICAL or below; a leg keeps the one it started with
    damping: f32,

    // of the current leg, from its response; 0 under reduced motion, which never moves
    w: f32,

    // the current leg's damping ratio
    z: f32,

    target: [f32; N],

    // the current leg: x(t) = target - (a + b t) e^(-wt), t counted from `start`
    a: [f32; N],
    b: [f32; N],

    // none until the first `to`, a new spring is at rest
    start: Option<Instant>,
}

impl<const N: usize> Spring<N> {
    // at rest on `value`, nothing moves until `to`
    pub fn new(value: [f32; N], mode: Mode) -> Self {
        Self {
            mode,
            damping: CRITICAL,
            w: 0.0,
            z: CRITICAL,
            target: value,
            a: [0.0; N],
            b: [0.0; N],
            start: None,
        }
    }

    // the damping ratio of legs from the next `to` on, clamped to (0.2, CRITICAL)
    pub fn damp(&mut self, damping: f32) {
        self.damping = damping.clamp(0.2, CRITICAL);
    }

    /*
     * a new leg starts from where it is and how fast it moves now, so a changed mind never jumps.
     * No overshoot from rest or on a reversal toward the other endpoint. A same-direction target
     * closer than speed / w may be passed, since keeping the velocity leaves no other way to stop.
     * The same target keeps the motion but still starts a leg, for content that changes in place.
     * The leg covers 95% of its way in `response`; reduced motion ignores it
     */
    pub fn to(&mut self, target: [f32; N], response: Duration, now: Instant) {
        let position = self.at(now);
        let velocity = self.velocity(now);

        self.w = match self.mode {
            Mode::Spring => RESPONSE_FACTOR / response.as_secs_f32(),
            Mode::Reduced => 0.0,
        };
        self.z = self.damping;

        for i in 0..N {
            self.a[i] = target[i] - position[i];

            self.b[i] = if self.critical() {
                // x(0) = target - a, x'(0) = w a - b
                self.w * self.a[i] - velocity[i]
            } else {
                // x(0) = target - a, x'(0) = z w a - d b
                (self.z * self.w * self.a[i] - velocity[i]) / self.damped()
            };
        }

        self.target = target;
        self.start = Some(now);
    }

    fn critical(&self) -> bool {
        self.z >= CRITICAL
    }

    // the underdamped leg's angular frequency
    fn damped(&self) -> f32 {
        self.w * (1.0 - self.z * self.z).max(0.0).sqrt()
    }

    // dimension i's way still to go at t, and how fast that shrinks
    fn left(&self, i: usize, t: f32) -> (f32, f32) {
        let (a, b, w, z) = (self.a[i], self.b[i], self.w, self.z);

        if self.critical() {
            let decay = (-w * t).exp();

            // d/dt of -(a + b t) e^(-wt)
            ((a + b * t) * decay, (w * (a + b * t) - b) * decay)
        } else {
            let d = self.damped();
            let decay = (-z * w * t).exp();
            let (sin, cos) = (d * t).sin_cos();

            (
                (a * cos + b * sin) * decay,
                ((z * w * a - b * d) * cos + (z * w * b + a * d) * sin) * decay,
            )
        }
    }

    // at most the share of a move from rest still to go at t
    fn envelope(&self, t: f32) -> f32 {
        if self.critical() {
            (1.0 + self.w * t) * (-self.w * t).exp()
        } else {
            (-self.z * self.w * t).exp() / (1.0 - self.z * self.z).max(1e-4).sqrt()
        }
    }

    // where the current leg is going
    pub fn target(&self) -> [f32; N] {
        self.target
    }

    // the damping ratio the next leg starts with
    pub fn damping(&self) -> f32 {
        self.damping
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn at(&self, now: Instant) -> [f32; N] {
        if self.mode == Mode::Reduced || self.settled(now) {
            return self.target;
        }

        let t = self.elapsed(now);

        std::array::from_fn(|i| self.target[i] - self.left(i, t).0)
    }

    pub fn velocity(&self, now: Instant) -> [f32; N] {
        if self.mode == Mode::Reduced || self.settled(now) {
            return [0.0; N];
        }

        let t = self.elapsed(now);

        std::array::from_fn(|i| self.left(i, t).1)
    }

    // false while the view should keep asking for frames
    pub fn settled(&self, now: Instant) -> bool {
        if self.start.is_none() {
            return true;
        }

        let t = self.elapsed(now);

        if self.mode == Mode::Reduced {
            return t >= REDUCED_FADE.as_secs_f32();
        }

        if self.envelope(t) > SETTLE_SHARE {
            return false;
        }

        (0..N).all(|i| {
            let (left, speed) = self.left(i, t);

            left.abs() <= SETTLE_DISTANCE && speed.abs() <= SETTLE_SPEED
        })
    }

    /*
     * 0 when the current leg starts, 1 once it arrives; measured on the dimension that has the
     * furthest to go, and starting over at 0 on every `to`, when the content changes anyway.
     * A leg that goes nowhere runs as a move from rest would, so content still fades in step.
     * Reduced motion: geometry is already there, this fades linearly over REDUCED_FADE
     */
    pub fn progress(&self, now: Instant) -> f32 {
        if self.settled(now) {
            return 1.0;
        }

        let t = self.elapsed(now);

        if self.mode == Mode::Reduced {
            return t / REDUCED_FADE.as_secs_f32();
        }

        let furthest = (0..N)
            .max_by(|&i, &j| self.a[i].abs().total_cmp(&self.a[j].abs()))
            .filter(|&i| self.a[i] != 0.0);

        let Some(i) = furthest else {
            return (1.0 - self.envelope(t)).clamp(0.0, 1.0);
        };

        (1.0 - self.left(i, t).0 / self.a[i]).clamp(0.0, 1.0)
    }

    fn elapsed(&self, now: Instant) -> f32 {
        self.start.map_or(0.0, |start| {
            now.saturating_duration_since(start).as_secs_f32()
        })
    }
}
