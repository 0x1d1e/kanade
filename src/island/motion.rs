//! Damped spring, closed form, so there is no integrator to jitter. Critically damped
//! by default, `x(t) = target - (A + B t) e^(-wt)` per dimension; with a damping below 1 it is
//! underdamped, `x(t) = target - (A cos(dt) + B sin(dt)) e^(-zwt)` with `d = w sqrt(1 - z^2)`, and
//! passes its target a little and comes back, like macOS's bouncy springs. Only the body's shape
//! bounces; opacity and the Satellites stay critical. Time is always passed in, never read.
//!
//! Dimensions within a `Spring` share a response. `Body` gives width, height and corners their
//! own springs; a critical phase keeps the content handoff monotonic even when geometry bounces.
//! Each leg takes its response from the configured expand, change or collapse timing.

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

// reduced motion: geometry snaps, only the derived opacity still fades
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

    // the current leg's response; in Reduced mode this is the content fade duration
    response: Duration,

    target: [f32; N],

    // Carried state is returned exactly at the leg's start, without reconstructing it.
    initial_position: [f32; N],
    initial_velocity: [f32; N],

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
            response: REDUCED_FADE,
            target: value,
            initial_position: value,
            initial_velocity: [0.0; N],
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
     * each leg starts at the current position and velocity, so retargeting never jumps. reversals
     * do not overshoot, but a closer same-direction target may; a leg aimed at the same target still
     * starts, so content changing in place tracks the motion. `response` covers 95% of the leg, while
     * Reduced snaps geometry and uses it for opacity
     */
    pub fn to(&mut self, target: [f32; N], response: Duration, now: Instant) {
        self.aim(target, response, self.mode, now);
    }

    // Retarget while changing motion mode; sample the old leg before applying the new mode.
    pub fn aim(&mut self, target: [f32; N], response: Duration, mode: Mode, now: Instant) {
        let (position, velocity) = self.sample(now);
        self.mode = mode;

        self.response = response;
        self.w = if self.mode == Mode::Spring && !response.is_zero() {
            RESPONSE_FACTOR / response.as_secs_f32()
        } else {
            0.0
        };
        self.z = self.damping;

        for i in 0..N {
            self.a[i] = target[i] - position[i];

            self.b[i] = if self.w == 0.0 {
                0.0
            } else if self.critical() {
                // x(0) = target - a, x'(0) = w a - b
                self.w * self.a[i] - velocity[i]
            } else {
                // x(0) = target - a, x'(0) = z w a - d b
                (self.z * self.w * self.a[i] - velocity[i]) / self.damped()
            };
        }

        self.target = target;
        self.initial_position = position;
        self.initial_velocity = velocity;
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
        self.sample(now).0
    }

    pub fn velocity(&self, now: Instant) -> [f32; N] {
        self.sample(now).1
    }

    fn sample(&self, now: Instant) -> ([f32; N], [f32; N]) {
        if self.mode == Mode::Reduced || self.response.is_zero() {
            return (self.target, [0.0; N]);
        }

        let t = self.elapsed(now);
        if t == 0.0 {
            return (self.initial_position, self.initial_velocity);
        }
        if self.settled(now) {
            return (self.target, [0.0; N]);
        }

        let mut position = [0.0; N];
        let mut velocity = [0.0; N];
        for i in 0..N {
            let (left, speed) = self.left(i, t);
            position[i] = self.target[i] - left;
            velocity[i] = speed;
        }
        (position, velocity)
    }

    // false while the view should keep asking for frames
    pub fn settled(&self, now: Instant) -> bool {
        if self.start.is_none() || self.response.is_zero() {
            return true;
        }

        let t = self.elapsed(now);

        if self.mode == Mode::Reduced {
            return t >= self.response.as_secs_f32();
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
     * Reduced motion: geometry is already there, this fades linearly over the response
     */
    pub fn progress(&self, now: Instant) -> f32 {
        if self.settled(now) {
            return 1.0;
        }

        let t = self.elapsed(now);

        if self.mode == Mode::Reduced {
            return t / self.response.as_secs_f32();
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

impl Spring<1> {
    // Express a blend from its other endpoint, including its velocity, when two segments trade.
    pub fn complement(&mut self) {
        self.target[0] = 1.0 - self.target[0];
        self.initial_position[0] = 1.0 - self.initial_position[0];
        self.initial_velocity[0] = -self.initial_velocity[0];
        self.a[0] = -self.a[0];
        self.b[0] = -self.b[0];
    }
}

// Body geometry has separate responses without changing shared Spring consumers such as the Dock.
#[derive(Debug, Clone)]
pub struct Body {
    axes: [Spring<1>; 3],
    phase: Spring<1>,
    mode: Mode,
}

impl Body {
    pub fn new(value: [f32; 3], mode: Mode) -> Self {
        Self {
            axes: value.map(|value| Spring::new([value], mode)),
            phase: Spring::new([1.0], mode),
            mode,
        }
    }

    pub fn configure(&mut self, damping: f32, mode: Mode) {
        self.mode = mode;
        for axis in &mut self.axes {
            axis.damp(damping);
        }
    }

    pub fn to(&mut self, target: [f32; 3], response: Duration, now: Instant) {
        // Width makes room first; height reveals the content; corners finish the silhouette.
        for ((axis, target), pace) in self.axes.iter_mut().zip(target).zip([0.85, 1.0, 1.15]) {
            let pace = if self.mode == Mode::Reduced {
                1.0
            } else {
                pace
            };
            axis.aim([target], response.mul_f32(pace), self.mode, now);
        }
        self.phase = Spring::new([0.0], self.mode);
        self.phase.to([1.0], response, now);
    }

    pub fn at(&self, now: Instant) -> [f32; 3] {
        std::array::from_fn(|i| self.axes[i].at(now)[0])
    }

    pub fn target(&self) -> [f32; 3] {
        std::array::from_fn(|i| self.axes[i].target()[0])
    }

    pub fn velocity(&self, now: Instant) -> [f32; 3] {
        std::array::from_fn(|i| self.axes[i].velocity(now)[0])
    }

    pub fn progress(&self, now: Instant) -> f32 {
        self.phase.progress(now)
    }

    pub fn settled(&self, now: Instant) -> bool {
        self.phase.settled(now) && self.axes.iter().all(|axis| axis.settled(now))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    // a leg turned around, or aimed elsewhere, at any point of its way starts from where it is, as fast as it moves
    #[test]
    fn a_new_leg_keeps_position_and_velocity() {
        for damping in [CRITICAL, 0.72] {
            for share in [25, 50, 75] {
                let start = Instant::now();
                let mut spring = Spring::new([0.0, 0.0], Mode::Spring);

                spring.damp(damping);
                spring.to([200.0, 100.0], ms(180), start);

                let now = start + ms(180 * share / 100);
                let (position, velocity) = (spring.at(now), spring.velocity(now));

                spring.to([0.0, 0.0], ms(180), now);

                for i in 0..2 {
                    assert!((spring.at(now)[i] - position[i]).abs() < 1e-3);
                    assert!((spring.velocity(now)[i] - velocity[i]).abs() < 1e-2);
                }
            }
        }
    }

    #[test]
    fn body_retarget_preserves_every_axis_and_settles() {
        let start = Instant::now();
        let mut body = Body::new([100.0, 30.0, 15.0], Mode::Spring);
        body.configure(0.72, Mode::Spring);
        body.to([500.0, 400.0, 28.0], ms(180), start);
        for elapsed in [40, 80, 120] {
            let now = start + ms(elapsed);
            let position = body.at(now);
            let velocity = body.velocity(now);
            body.to([200.0, 50.0, 25.0], ms(140), now);
            for i in 0..3 {
                assert!((body.at(now)[i] - position[i]).abs() < 1e-3);
                assert!((body.velocity(now)[i] - velocity[i]).abs() < 1e-2);
            }
        }
        let settled = start + ms(2000);
        assert!(body.settled(settled));
        assert_eq!(body.at(settled), body.target());
        assert_eq!(body.velocity(settled), [0.0; 3]);
    }

    #[test]
    fn body_axes_arrive_in_order_while_content_progress_stays_monotonic() {
        let start = Instant::now();
        let mut body = Body::new([0.0; 3], Mode::Spring);
        body.to([1.0; 3], ms(180), start);
        let position = body.at(start + ms(60));
        assert!(position[0] > position[1] && position[1] > position[2]);
        body.configure(0.72, Mode::Spring);
        body.to([0.0; 3], ms(180), start + ms(60));
        let mut last = 0.0;
        for elapsed in 60..1000 {
            let progress = body.progress(start + ms(elapsed));
            assert!(progress >= last);
            last = progress;
        }
        assert_eq!(last, 1.0);
    }

    #[test]
    fn swapping_segment_endpoints_preserves_pose_and_speed() {
        let start = Instant::now();
        let mut swap = Spring::new([1.0], Mode::Spring);
        swap.to([0.0], ms(180), start);
        let now = start + ms(60);
        let position = swap.at(now)[0];
        let velocity = swap.velocity(now)[0];
        swap.complement();
        swap.aim([0.0], ms(180), Mode::Spring, now);
        assert!((swap.at(now)[0] - (1.0 - position)).abs() < 1e-6);
        assert!((swap.velocity(now)[0] + velocity).abs() < 1e-5);
    }
}
