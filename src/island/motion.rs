//! Critically damped spring, closed form (plan 6.4), so there is no integrator to jitter:
//! `x(t) = target - (A + B t) e^(-wt)` per dimension. Time is always passed in, never read.
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

#[derive(Debug, Clone, Copy)]
pub struct Spring<const N: usize> {
    mode: Mode,

    // of the current leg, from its response; 0 under reduced motion, which never moves
    w: f32,

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
            w: 0.0,
            target: value,
            a: [0.0; N],
            b: [0.0; N],
            start: None,
        }
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

        for i in 0..N {
            // x(0) = target - a, x'(0) = w a - b
            self.a[i] = target[i] - position[i];
            self.b[i] = self.w * self.a[i] - velocity[i];
        }

        self.target = target;
        self.start = Some(now);
    }

    pub fn at(&self, now: Instant) -> [f32; N] {
        if self.mode == Mode::Reduced || self.settled(now) {
            return self.target;
        }

        let t = self.elapsed(now);
        let decay = (-self.w * t).exp();

        std::array::from_fn(|i| self.target[i] - (self.a[i] + self.b[i] * t) * decay)
    }

    pub fn velocity(&self, now: Instant) -> [f32; N] {
        if self.mode == Mode::Reduced || self.settled(now) {
            return [0.0; N];
        }

        let t = self.elapsed(now);
        let decay = (-self.w * t).exp();

        // d/dt of -(a + b t) e^(-wt)
        std::array::from_fn(|i| (self.w * (self.a[i] + self.b[i] * t) - self.b[i]) * decay)
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

        let decay = (-self.w * t).exp();

        if (1.0 + self.w * t) * decay > SETTLE_SHARE {
            return false;
        }

        (0..N).all(|i| {
            let left = (self.a[i] + self.b[i] * t) * decay;
            let speed = (self.w * (self.a[i] + self.b[i] * t) - self.b[i]) * decay;

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

        let decay = (-self.w * t).exp();

        let furthest = (0..N)
            .max_by(|&i, &j| self.a[i].abs().total_cmp(&self.a[j].abs()))
            .filter(|&i| self.a[i] != 0.0);

        let Some(i) = furthest else {
            return 1.0 - (1.0 + self.w * t) * decay;
        };

        let left = (self.a[i] + self.b[i] * t) * decay;

        (1.0 - left / self.a[i]).clamp(0.0, 1.0)
    }

    fn elapsed(&self, now: Instant) -> f32 {
        self.start.map_or(0.0, |start| {
            now.saturating_duration_since(start).as_secs_f32()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESPONSE: Duration = Duration::from_millis(180);
    const STEP: Duration = Duration::from_millis(1);

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    // every millisecond from `from` until the spring has settled, with a hard cap
    fn walk<const N: usize>(spring: &Spring<N>, from: Instant) -> Vec<Instant> {
        let mut times = vec![from];

        while !spring.settled(*times.last().unwrap()) {
            times.push(*times.last().unwrap() + STEP);
            assert!(times.len() < 5_000, "never settled");
        }

        times
    }

    #[test]
    fn at_rest_until_retargeted() {
        let now = Instant::now();
        let spring = Spring::new([150.0, 32.0], Mode::Spring);

        assert!(spring.settled(now));
        assert_eq!(spring.at(now + ms(500)), [150.0, 32.0]);

        // reduced motion too, no fade before the first move
        assert!(Spring::new([150.0], Mode::Reduced).settled(now));
        assert_eq!(spring.progress(now), 1.0);
    }

    #[test]
    fn same_target_keeps_the_motion_and_starts_a_leg() {
        let now = Instant::now();
        let mut spring = Spring::new([150.0], Mode::Spring);

        spring.to([440.0], RESPONSE, now);

        let turn = now + ms(60);
        let later = now + ms(120);
        let before = spring.at(later);

        spring.to([440.0], RESPONSE, turn);

        assert_eq!(spring.progress(turn), 0.0);
        assert!((spring.at(later)[0] - before[0]).abs() < 1e-2);
    }

    // content that changes within one shape fades as a move from rest of that shape would
    #[test]
    fn leg_in_place_runs_like_a_move_from_rest() {
        let now = Instant::now();
        let mut moving = Spring::new([150.0], Mode::Spring);
        let mut still = Spring::new([150.0], Mode::Spring);

        moving.to([440.0], RESPONSE, now);
        still.to([150.0], RESPONSE, now);

        assert_eq!(still.progress(now), 0.0);

        for &t in &walk(&moving, now) {
            assert_eq!(still.at(t), [150.0]);
            assert!(
                (still.progress(t) - moving.progress(t)).abs() <= SETTLE_SHARE,
                "at {:?}: {} vs {}",
                t - now,
                still.progress(t),
                moving.progress(t)
            );
        }

        let end = *walk(&still, now).last().unwrap();

        assert!(end - now > RESPONSE, "{:?}", end - now);
        assert_eq!(still.progress(end), 1.0);
    }

    #[test]
    fn covers_95_percent_in_each_legs_response() {
        let now = Instant::now();
        let mut spring = Spring::new([0.0], Mode::Spring);

        spring.to([100.0], RESPONSE, now);

        let [x] = spring.at(now + RESPONSE);

        assert!((x - 95.0).abs() < 0.1, "{x}");

        // a slower leg from rest again
        let later = now + ms(1_000);
        spring.to([0.0], ms(220), later);

        let [x] = spring.at(later + ms(220));

        assert!((x - 5.0).abs() < 0.1, "{x}");
    }

    #[test]
    fn moves_without_overshoot_and_settles_on_the_target() {
        let now = Instant::now();
        let mut spring = Spring::new([150.0, 32.0, 16.0], Mode::Spring);

        let target = [440.0, 160.0, 32.0];
        spring.to(target, RESPONSE, now);

        let times = walk(&spring, now);
        let mut last = spring.at(now);

        for &t in &times {
            let x = spring.at(t);

            for i in 0..3 {
                assert!(x[i] <= target[i], "overshoot at {t:?}: {x:?}");
                assert!(x[i] >= last[i], "moved backwards at {t:?}: {x:?}");
            }

            last = x;
        }

        let end = *times.last().unwrap();

        assert_eq!(spring.at(end), target);
        assert_eq!(spring.velocity(end), [0.0; 3]);

        // the tail is sub-pixel, the whole move rests within about twice the response
        assert!(end - now < RESPONSE * 2 + ms(50), "{:?}", end - now);
    }

    #[test]
    fn group_dimensions_arrive_together() {
        let now = Instant::now();
        let mut spring = Spring::new([150.0, 32.0, 16.0], Mode::Spring);

        spring.to([440.0, 160.0, 32.0], RESPONSE, now);

        // from rest, every dimension is the same share of its way at any moment
        for t in (0..400).step_by(20) {
            let x = spring.at(now + ms(t));
            let shares = [
                (x[0] - 150.0) / 290.0,
                (x[1] - 32.0) / 128.0,
                (x[2] - 16.0) / 16.0,
            ];

            if !spring.settled(now + ms(t)) {
                assert!((shares[0] - shares[1]).abs() < 1e-4, "{shares:?}");
                assert!((shares[0] - shares[2]).abs() < 1e-4, "{shares:?}");
            }
        }
    }

    // a leg of another response too, like a Surface change taking over an expand
    #[test]
    fn retarget_keeps_position_and_velocity() {
        let now = Instant::now();
        let mut spring = Spring::new([150.0, 32.0], Mode::Spring);

        spring.to([440.0, 160.0], RESPONSE, now);

        let turn = now + ms(70);
        let position = spring.at(turn);
        let velocity = spring.velocity(turn);

        spring.to([150.0, 32.0], ms(220), turn);

        assert_eq!(spring.at(turn), position);

        for (now, before) in spring.velocity(turn).into_iter().zip(velocity) {
            assert!((now - before).abs() < 1e-2, "{now} vs {before}");
        }

        // and stays continuous just after the turn, no kink
        let after = spring.velocity(turn + STEP);

        for (after, before) in after.into_iter().zip(velocity) {
            assert!(
                (after - before).abs() < before.abs() * 0.2,
                "velocity jumped {before} -> {after}"
            );
        }
    }

    #[test]
    fn reversal_mid_flight_does_not_overshoot() {
        let now = Instant::now();
        let mut spring = Spring::new([150.0], Mode::Spring);

        spring.to([440.0], RESPONSE, now);

        let turn = now + ms(60);
        spring.to([150.0], RESPONSE, turn);

        // still moving out, it turns once and then comes straight back, never past 150
        let mut turns = 0;
        let mut last = spring.at(turn)[0];
        let mut rising = true;

        for &t in &walk(&spring, turn) {
            let [x] = spring.at(t);

            assert!(x >= 150.0, "overshoot past the new target at {t:?}: {x}");

            if rising && x < last {
                rising = false;
                turns += 1;
            }

            assert!(rising || x <= last, "turned again at {t:?}: {x}");

            last = x;
        }

        assert_eq!(turns, 1);
        assert_eq!(last, 150.0);
    }

    #[test]
    fn progress_runs_from_zero_to_one_per_leg() {
        let now = Instant::now();
        let mut spring = Spring::new([150.0, 32.0], Mode::Spring);

        spring.to([440.0, 160.0], RESPONSE, now);
        assert_eq!(spring.progress(now), 0.0);

        let mut last = 0.0;

        for &t in &walk(&spring, now) {
            let progress = spring.progress(t);

            assert!((0.0..=1.0).contains(&progress));
            assert!(progress >= last);

            last = progress;
        }

        assert_eq!(last, 1.0);

        spring.to([150.0, 32.0], RESPONSE, now + ms(1_000));
        assert_eq!(spring.progress(now + ms(1_000)), 0.0);
    }

    #[test]
    fn reduced_motion_snaps_geometry_and_fades_progress() {
        let now = Instant::now();
        let mut spring = Spring::new([150.0, 32.0], Mode::Reduced);

        spring.to([440.0, 160.0], RESPONSE, now);

        assert_eq!(spring.at(now), [440.0, 160.0]);
        assert_eq!(spring.velocity(now), [0.0; 2]);

        assert_eq!(spring.progress(now), 0.0);
        assert!((spring.progress(now + ms(40)) - 0.5).abs() < 1e-3);
        assert!(!spring.settled(now + ms(79)));

        assert_eq!(spring.progress(now + REDUCED_FADE), 1.0);
        assert!(spring.settled(now + REDUCED_FADE));
    }

    #[test]
    fn time_before_the_leg_counts_as_its_start() {
        let now = Instant::now();
        let mut spring = Spring::new([0.0], Mode::Spring);

        spring.to([100.0], RESPONSE, now + ms(10));

        assert_eq!(spring.at(now), [0.0]);
    }
}
