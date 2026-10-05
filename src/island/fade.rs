//! Content crossfade (CONTEXT.md: a Presentation change is geometry plus content crossfade in one
//! motion), over anything the body shows: a Presentation, or what one shows of an Activity.
//! Driven by the morph's progress, not a timer of its own: the old content fades out over
//! the first half of the morph and the new fades in over the second, so at most one shows at once.
//! A morph that changes its mind takes over whatever shows, at its current opacity, so nothing pops.
//!
//! A `Dissolve` is for content that changes inside a form that stays put, like a new track's art:
//! the new rises over the old on its own spring, so the old still shows beneath it and nothing dips.

use std::time::{Duration, Instant};

use super::motion::{Mode, Spring};

#[derive(Debug, Clone)]
pub struct Crossfade<T> {
    // what showed when the current leg began, and how strongly
    from: Option<(T, f32)>,

    to: T,

    // how strongly `to` already showed when the leg began, nonzero only on a way back
    start: f32,
}

// what a Crossfade can show
pub trait InPlace: Clone + PartialEq {
    // `next` is this content changed where it stands, like a level that moved, so it does not fade
    fn in_place(&self, next: &Self) -> bool {
        self == next
    }
}

impl<T: InPlace> Crossfade<T> {
    // settled on `content`
    pub fn new(content: T) -> Self {
        Self {
            from: None,
            to: content,
            start: 1.0,
        }
    }

    pub fn target(&self) -> &T {
        &self.to
    }

    /*
     * a new leg toward `next`, begun at `progress` of the current one, and whether it began one:
     * content changed in place takes over the current leg's place as it is
     */
    pub fn to(&mut self, next: T, progress: f32) -> bool {
        if self.to.in_place(&next) {
            self.to = next;
            return false;
        }

        let showing = self.shown(progress).into_iter().flatten().next();

        (self.from, self.start) = match showing {
            // back to what still shows, it only has to grow back from here
            Some((content, opacity)) if content.in_place(&next) => (None, opacity),
            showing => (showing, 0.0),
        };

        self.to = next;
        true
    }

    // what shows at `progress` of the current leg, and how strongly; never both at once
    pub fn shown(&self, progress: f32) -> [Option<(T, f32)>; 2] {
        let out = (1.0 - 2.0 * progress).clamp(0.0, 1.0);
        let into = (2.0 * progress - 1.0).clamp(0.0, 1.0);

        let from = self
            .from
            .clone()
            .map(|(content, opacity)| (content, opacity * out));
        let to = (self.to.clone(), self.start + (1.0 - self.start) * into);

        [from, Some(to)].map(|shown| shown.filter(|&(_, opacity)| opacity > 0.0))
    }
}

// an untouched island rests, with nothing to show
impl<T: InPlace + Default> Default for Crossfade<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

/*
 * the new content rising over the old in place, both showing until it arrives. Its own spring,
 * since the body does not move; under reduced motion it still fades, see motion::REDUCED_FADE
 */
#[derive(Debug, Clone, PartialEq)]
pub struct Dissolve<T> {
    // what shows beneath, until the new one covers it
    from: Option<T>,

    to: T,

    // 0 to 1, how far `to` has risen
    rise: Spring<1>,
}

impl<T: InPlace> Dissolve<T> {
    // settled on `content`
    pub fn new(content: T, mode: Mode) -> Self {
        Self {
            from: None,
            to: content,
            rise: Spring::new([1.0], mode),
        }
    }

    pub fn target(&self) -> &T {
        &self.to
    }

    /*
     * a new rise toward `next`, and whether it started the rise over: content changed in place
     * is swapped where it stands. A change of mind before the rise is halfway keeps its old one
     * beneath and its rise, so the old one still fades as it did; past halfway, the one mostly
     * showing goes beneath and the rise starts over
     */
    pub fn to(&mut self, next: T, response: Duration, now: Instant) -> bool {
        if self.to.in_place(&next) {
            self.to = next;
            return false;
        }

        let previous = std::mem::replace(&mut self.to, next);

        if self.from(now).is_some() && self.rise(now) < 0.5 {
            return false;
        }

        self.from = Some(previous);
        self.rise = Spring::new([0.0], self.rise.mode());
        self.rise.to([1.0], response, now);

        true
    }

    // what still shows beneath at `now`, none once the new one covers it
    pub fn from(&self, now: Instant) -> Option<&T> {
        self.from.as_ref().filter(|_| !self.rise.settled(now))
    }

    // how strongly the new one shows over it, 0 to 1
    pub fn rise(&self, now: Instant) -> f32 {
        self.rise.progress(now)
    }

    pub fn settled(&self, now: Instant) -> bool {
        self.rise.settled(now)
    }
}

/*
 * one thing that changes where it stands, like a title, swapped through nothing: the old fades out
 * over the first half of the rise, the new in over the second
 */
pub fn swap<T>(from: Option<T>, to: T, rise: f32) -> (T, f32) {
    match from {
        Some(from) if rise < 0.5 => (from, 1.0 - 2.0 * rise),
        Some(_) => (to, 2.0 * rise - 1.0),
        None => (to, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::presentation::{Presentation, Surface};

    use Presentation::{Compact, Expanded, Peek, Rest};

    impl InPlace for Presentation {}

    const MEDIA: Presentation = Expanded(Surface::Media);

    fn shown(fade: &Crossfade<Presentation>, progress: f32) -> Vec<(Presentation, f32)> {
        fade.shown(progress).into_iter().flatten().collect()
    }

    // the opacity `presentation` shows with, 0 when it does not show
    fn opacity(fade: &Crossfade<Presentation>, presentation: Presentation, progress: f32) -> f32 {
        shown(fade, progress)
            .into_iter()
            .find(|&(shown, _)| shown == presentation)
            .map_or(0.0, |(_, opacity)| opacity)
    }

    fn steps() -> impl Iterator<Item = f32> {
        (0..=100).map(|step| step as f32 / 100.0)
    }

    #[test]
    fn settled_shows_only_its_presentation() {
        let fade = Crossfade::new(Compact);

        assert_eq!(shown(&fade, 1.0), [(Compact, 1.0)]);
        assert_eq!(shown(&fade, 0.0), [(Compact, 1.0)]);
    }

    #[test]
    fn old_fades_out_then_new_fades_in() {
        let mut fade = Crossfade::new(Compact);

        fade.to(MEDIA, 1.0);

        assert_eq!(shown(&fade, 0.0), [(Compact, 1.0)]);
        assert_eq!(shown(&fade, 0.25), [(Compact, 0.5)]);
        assert_eq!(shown(&fade, 0.5), []);
        assert_eq!(shown(&fade, 0.75), [(MEDIA, 0.5)]);
        assert_eq!(shown(&fade, 1.0), [(MEDIA, 1.0)]);
    }

    #[test]
    fn never_two_at_once() {
        let mut fade = Crossfade::new(Rest);

        for (next, at) in [(Compact, 0.3), (Peek, 0.8), (MEDIA, 0.6), (Compact, 0.1)] {
            fade.to(next, at);

            for progress in steps() {
                assert!(shown(&fade, progress).len() <= 1, "{fade:?} at {progress}");
            }
        }
    }

    // a changed mind continues from what shows, whatever the new target
    #[test]
    fn retargeting_never_pops() {
        let targets = [Rest, Compact, Peek, MEDIA];

        for first in targets {
            // a leg already headed for Peek does not start over
            for second in targets.into_iter().filter(|&second| second != Peek) {
                for at in steps() {
                    let mut fade = Crossfade::new(Rest);

                    fade.to(first, 1.0);
                    fade.to(second, 0.0);

                    let before = shown(&fade, at);

                    fade.to(Peek, at);

                    let after = shown(&fade, 0.0);

                    assert_eq!(before, after, "{first:?} -> {second:?} -> Peek at {at}");
                }
            }
        }
    }

    #[test]
    fn way_back_grows_what_still_shows() {
        let mut fade = Crossfade::new(Compact);

        fade.to(MEDIA, 1.0);
        fade.to(Compact, 0.2);

        // Compact showed at 0.6 when the mind changed, it only grows from there
        let mut last = opacity(&fade, Compact, 0.0);
        assert!((last - 0.6).abs() < 1e-6, "{last}");

        for progress in steps() {
            let now = opacity(&fade, Compact, progress);

            assert!(now >= last, "dipped at {progress}: {now}");
            assert_eq!(opacity(&fade, MEDIA, progress), 0.0);

            last = now;
        }

        assert_eq!(last, 1.0);
    }

    #[test]
    fn same_target_changes_nothing() {
        let mut fade = Crossfade::new(Compact);

        fade.to(MEDIA, 1.0);
        fade.to(MEDIA, 0.7);

        assert_eq!(shown(&fade, 0.25), [(Compact, 0.5)]);
    }

    // a level and the Activity it belongs to; same Activity, other level, is in place
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct Level(char, u8);

    impl InPlace for Level {
        fn in_place(&self, next: &Self) -> bool {
            self.0 == next.0
        }
    }

    #[test]
    fn in_place_takes_over_without_a_leg() {
        let mut fade = Crossfade::new(Level('v', 40));

        fade.to(Level('m', 0), 1.0);
        assert!(!fade.to(Level('m', 10), 0.6));

        // still fading the same leg, now toward the moved level
        assert_eq!(fade.shown(0.25), [Some((Level('v', 40), 0.5)), None]);
        assert_eq!(fade.shown(0.75), [None, Some((Level('m', 10), 0.5))]);
    }

    #[test]
    fn way_back_to_a_moved_level_grows_it() {
        let mut fade = Crossfade::new(Level('v', 40));

        fade.to(Level('m', 0), 1.0);
        assert!(fade.to(Level('v', 45), 0.2));

        assert_eq!(fade.shown(0.0), [None, Some((Level('v', 45), 0.6))]);
    }

    const RISE: Duration = Duration::from_millis(300);

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn a_new_dissolve_rests_on_its_content() {
        let now = Instant::now();
        let dissolve = Dissolve::new(Level('a', 0), Mode::Spring);

        assert_eq!(dissolve.from(now), None);
        assert_eq!(dissolve.rise(now), 1.0);
        assert!(dissolve.settled(now));
    }

    #[test]
    fn the_new_one_rises_over_the_old() {
        let now = Instant::now();
        let mut dissolve = Dissolve::new(Level('a', 0), Mode::Spring);

        assert!(dissolve.to(Level('b', 0), RISE, now));
        assert_eq!(dissolve.target(), &Level('b', 0));
        assert_eq!(dissolve.from(now), Some(&Level('a', 0)));
        assert_eq!(dissolve.rise(now), 0.0);

        let mut last = 0.0;
        for step in 1..=20 {
            let rise = dissolve.rise(now + ms(step * 15));
            assert!(rise >= last, "dipped at {step}: {rise}");
            last = rise;
        }

        let late = now + ms(2_000);
        assert!(dissolve.settled(late));
        assert_eq!(dissolve.from(late), None);
        assert_eq!(dissolve.rise(late), 1.0);
    }

    #[test]
    fn in_place_swaps_without_a_rise() {
        let now = Instant::now();
        let mut dissolve = Dissolve::new(Level('a', 0), Mode::Spring);

        assert!(!dissolve.to(Level('a', 9), RISE, now));
        assert_eq!(dissolve.target(), &Level('a', 9));
        assert!(dissolve.settled(now));
    }

    #[test]
    fn a_change_of_mind_before_halfway_keeps_the_old_one_beneath() {
        let now = Instant::now();
        let mut dissolve = Dissolve::new(Level('a', 0), Mode::Spring);
        dissolve.to(Level('b', 0), RISE, now);

        let early = now + ms(10);
        let rise = dissolve.rise(early);
        assert!(rise < 0.5, "{rise}");

        assert!(!dissolve.to(Level('c', 0), RISE, early));
        assert_eq!(dissolve.target(), &Level('c', 0));
        assert_eq!(dissolve.from(early), Some(&Level('a', 0)));
        assert_eq!(dissolve.rise(early), rise);
    }

    #[test]
    fn a_change_of_mind_past_halfway_starts_over_from_the_new_one() {
        let now = Instant::now();
        let mut dissolve = Dissolve::new(Level('a', 0), Mode::Spring);
        dissolve.to(Level('b', 0), RISE, now);

        let late = (1..)
            .map(|step| now + ms(step))
            .find(|&at| dissolve.rise(at) >= 0.5)
            .unwrap();
        assert!(!dissolve.settled(late));

        assert!(dissolve.to(Level('c', 0), RISE, late));
        assert_eq!(dissolve.from(late), Some(&Level('b', 0)));
        assert_eq!(dissolve.rise(late), 0.0);
    }

    #[test]
    fn reduced_motion_still_dissolves() {
        let now = Instant::now();
        let mut dissolve = Dissolve::new(Level('a', 0), Mode::Reduced);
        dissolve.to(Level('b', 0), RISE, now);

        let rise = dissolve.rise(now + ms(40));
        assert!(0.0 < rise && rise < 1.0, "{rise}");
        assert!(dissolve.settled(now + crate::island::motion::REDUCED_FADE));
    }

    #[test]
    fn swap_goes_through_nothing() {
        assert_eq!(swap(Some('a'), 'b', 0.0), ('a', 1.0));
        assert_eq!(swap(Some('a'), 'b', 0.25), ('a', 0.5));
        assert_eq!(swap(Some('a'), 'b', 0.5), ('b', 0.0));
        assert_eq!(swap(Some('a'), 'b', 0.75), ('b', 0.5));
        assert_eq!(swap(Some('a'), 'b', 1.0), ('b', 1.0));
        assert_eq!(swap(None, 'b', 0.0), ('b', 1.0));
    }
}
