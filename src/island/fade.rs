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

    /*
     * the current leg, at `progress`, made the start of a new one with the same content: what shows
     * keeps its opacity, so a body re-aimed mid-leg or settled shows no fade again
     */
    pub fn rebase(&mut self, progress: f32) {
        let [from, to] = self.shown(progress);

        self.from = from;
        self.start = to.map_or(0.0, |(_, opacity)| opacity);
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
     * a new rise toward `next`: content changed in place is swapped where it stands. A change of mind before the rise is halfway keeps its old one
     * beneath and its rise, so the old one still fades as it did; past halfway, the one mostly
     * showing goes beneath and the rise starts over
     */
    pub fn to(&mut self, next: T, response: Duration, now: Instant) {
        if self.to.in_place(&next) {
            self.to = next;
            return;
        }

        let previous = std::mem::replace(&mut self.to, next);

        if self.from(now).is_some() && self.rise(now) < 0.5 {
            return;
        }

        self.from = Some(previous);
        self.rise = Spring::new([0.0], self.rise.mode());
        self.rise.to([1.0], response, now);
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
