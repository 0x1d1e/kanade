//! Which Banners show and for how long (ADR 0007): at most `MOST` at a time, the rest queued in
//! arrival order with Critical ones ahead. Low shows 4 s, normal 6 s, Critical until closed. The
//! pointer on any Banner pauses every timer. Time is an input; this never reads a clock.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::sources::notifications::Urgency;

use crate::island::activity::Toast;

// ADR 0007: more would cover too much of the focused output
pub const MOST: usize = 3;

const LOW: Duration = Duration::from_secs(4);
const NORMAL: Duration = Duration::from_secs(6);

// one notification as a Banner shows it
#[derive(Debug, Clone, PartialEq)]
pub struct Banner {
    pub id: u32,
    pub toast: Toast,
    pub urgency: Urgency,

    // a click on the Banner runs it; without one the click only closes the Banner
    pub default: bool,

    // key and label
    pub actions: Vec<(String, String)>,
}

impl Banner {
    pub fn critical(&self) -> bool {
        self.urgency == Urgency::Critical
    }

    // none is sticky
    fn length(&self) -> Option<Duration> {
        match self.urgency {
            Urgency::Low => Some(LOW),
            Urgency::Normal => Some(NORMAL),
            Urgency::Critical => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Timer {
    Sticky,
    Running(Instant),

    // what was left when the pointer came on
    Paused(Duration),
}

#[derive(Debug, Clone, PartialEq)]
struct Shown {
    banner: Banner,
    timer: Timer,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stack {
    // oldest first
    shown: Vec<Shown>,

    queued: VecDeque<Banner>,

    // the pointer is on a Banner
    hovered: bool,
}

impl Stack {
    /*
     * a new notification, or a new version of one, which replaces it where it is and starts its
     * time again. DND drops a non-Critical one, and the version it replaces
     */
    pub fn arrive(&mut self, banner: Banner, dnd: bool, now: Instant) {
        if dnd && !banner.critical() {
            self.close(banner.id, now);
            return;
        }

        if let Some(shown) = self
            .shown
            .iter_mut()
            .find(|shown| shown.banner.id == banner.id)
        {
            shown.timer = timer(&banner, self.hovered, now);
            shown.banner = banner;
            return;
        }

        // a waiting one keeps its place, unless it crossed into or out of Critical, which waits first
        if let Some(queued) = self
            .queued
            .iter_mut()
            .find(|queued| queued.id == banner.id && queued.critical() == banner.critical())
        {
            *queued = banner;
            return;
        }

        self.queued.retain(|queued| queued.id != banner.id);
        self.queue(banner);
        self.promote(now);
    }

    // its sender or the user closed it; false when it was not here
    pub fn close(&mut self, id: u32, now: Instant) -> bool {
        let before = (self.shown.len(), self.queued.len());

        self.shown.retain(|shown| shown.banner.id != id);
        self.queued.retain(|queued| queued.id != id);

        let closed = before != (self.shown.len(), self.queued.len());

        self.promote(now);
        closed
    }

    // DND came on: only Critical Banners stay; false when there was nothing to drop
    pub fn silence(&mut self, now: Instant) -> bool {
        let before = (self.shown.len(), self.queued.len());

        self.shown.retain(|shown| shown.banner.critical());
        self.queued.retain(Banner::critical);

        let dropped = before != (self.shown.len(), self.queued.len());

        self.promote(now);
        dropped
    }

    // anything DND would drop
    pub fn silences(&self) -> bool {
        self.shown.iter().any(|shown| !shown.banner.critical())
            || self.queued.iter().any(|queued| !queued.critical())
    }

    pub fn hovered(&self) -> bool {
        self.hovered
    }

    // the pointer came on or left the Banners; false when it already was
    pub fn hover(&mut self, inside: bool, now: Instant) -> bool {
        // nothing shows to be on
        let inside = inside && !self.shown.is_empty();

        if inside == self.hovered {
            return false;
        }

        self.hovered = inside;

        for shown in &mut self.shown {
            shown.timer = match (shown.timer, inside) {
                (Timer::Running(until), true) => {
                    Timer::Paused(until.saturating_duration_since(now))
                }
                (Timer::Paused(left), false) => Timer::Running(now + left),
                (timer, _) => timer,
            };
        }

        true
    }

    // takes down every Banner whose time ran out, and shows queued ones in their place
    pub fn expire(&mut self, now: Instant) {
        self.shown
            .retain(|shown| !matches!(shown.timer, Timer::Running(until) if until <= now));

        self.promote(now);
    }

    // when a Banner's time runs out next
    pub fn deadline(&self) -> Option<Instant> {
        self.shown
            .iter()
            .filter_map(|shown| match shown.timer {
                Timer::Running(until) => Some(until),
                _ => None,
            })
            .min()
    }

    // newest first
    pub fn shown(&self) -> impl Iterator<Item = &Banner> {
        self.shown.iter().rev().map(|shown| &shown.banner)
    }

    pub fn contains(&self, id: u32) -> bool {
        self.shown.iter().any(|shown| shown.banner.id == id)
            || self.queued.iter().any(|queued| queued.id == id)
    }

    // in arrival order, a Critical one after the Critical ones already waiting
    fn queue(&mut self, banner: Banner) {
        let at = if banner.critical() {
            self.queued
                .iter()
                .position(|queued| !queued.critical())
                .unwrap_or(self.queued.len())
        } else {
            self.queued.len()
        };

        self.queued.insert(at, banner);
    }

    // fills the free places from the queue, each with its full time
    fn promote(&mut self, now: Instant) {
        while self.shown.len() < MOST
            && let Some(banner) = self.queued.pop_front()
        {
            let timer = timer(&banner, self.hovered, now);

            self.shown.push(Shown { banner, timer });
        }

        // with nothing showing, nothing is under the pointer
        if self.shown.is_empty() {
            self.hovered = false;
        }
    }
}

// a full time, held while the pointer is on the Banners
fn timer(banner: &Banner, hovered: bool, now: Instant) -> Timer {
    match banner.length() {
        None => Timer::Sticky,
        Some(length) if hovered => Timer::Paused(length),
        Some(length) => Timer::Running(now + length),
    }
}
