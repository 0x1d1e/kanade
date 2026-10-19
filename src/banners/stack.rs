//! Which Banners show and for how long (ADR 0007): at most `MOST` at a time, the rest queued in
//! arrival order with Critical ones ahead. Low shows 4 s, normal 6 s, Critical until closed, unless
//! its notification's rule gives a time. The pointer on any Banner pauses every timer. Under DND
//! only Critical ones show, and those whose rule bypasses DND. Time is an input; this never reads a
//! clock.

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

    // its notification's version, so leaving closes only the version it showed
    pub version: u64,

    pub toast: Toast,
    pub urgency: Urgency,

    // a click on the Banner runs it; without one the click only closes the Banner
    pub default: bool,

    // key and label
    pub actions: Vec<(String, String)>,

    // the sender takes a typed reply (ADR 0079): the placeholder of the field under the actions
    pub reply: Option<String>,

    // from its notification's rule: kept in the history once the Banner is gone, shown under DND,
    // and how long it shows over its urgency's time
    pub history: bool,
    pub bypass_dnd: bool,
    pub timeout: Option<Duration>,
}

impl Banner {
    pub fn critical(&self) -> bool {
        self.urgency == Urgency::Critical
    }

    // shows under DND
    pub fn loud(&self) -> bool {
        self.critical() || self.bypass_dnd
    }

    // none is sticky
    fn length(&self) -> Option<Duration> {
        self.timeout.or(match self.urgency {
            Urgency::Low => Some(LOW),
            Urgency::Normal => Some(NORMAL),
            Urgency::Critical => None,
        })
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

    // a reply is being typed, so it stays until sent or closed, however long that takes
    drafting: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stack {
    // oldest first
    shown: Vec<Shown>,

    queued: VecDeque<Banner>,

    // the pointer is on a Banner
    hovered: bool,

    // a fullscreen window covers the focused output: quiet Banners wait, they are not dropped
    held: bool,

    // the Banners that left, any way, whose notification stays out of the history, for it to close:
    // id and version
    left: Vec<(u32, u64)>,
}

impl Stack {
    /*
     * a new notification, or a new version of one, which replaces it where it is and starts its
     * time again. DND drops one that is not loud, and the version it replaces. Whether it closes as
     * it leaves is the new version's rule, never the old one's
     */
    pub fn arrive(&mut self, banner: Banner, dnd: bool, now: Instant) {
        self.left.retain(|&(left, _)| left != banner.id);

        if dnd && !banner.loud() {
            self.shown.retain(|shown| shown.banner.id != banner.id);
            self.queued.retain(|queued| queued.id != banner.id);

            if !banner.history {
                self.left.push((banner.id, banner.version));
            }

            self.promote(now);
            return;
        }

        if let Some(shown) = self
            .shown
            .iter_mut()
            .find(|shown| shown.banner.id == banner.id)
        {
            // a replacement under a draft keeps it, unless the new version takes no reply or is
            // from another sender, whose text the draft must not be sent to
            shown.drafting &= banner.reply.is_some() && banner.toast.app == shown.banner.toast.app;
            shown.timer = if shown.drafting {
                Timer::Sticky
            } else {
                timer(&banner, self.hovered, now)
            };
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

    // fullscreen silence came on or off; quiet Banners wait, then show with a fresh time, and the
    // sound they skipped is not played. false when it already was
    pub fn set_held(&mut self, held: bool, now: Instant) -> bool {
        if self.held == held {
            return false;
        }

        self.held = held;

        if held {
            let mut kept = Vec::new();
            let mut quiet = VecDeque::new();

            for shown in self.shown.drain(..) {
                if waits(held, shown.banner.loud()) {
                    quiet.push_back(shown.banner);
                } else {
                    kept.push(shown);
                }
            }

            self.shown = kept;

            for banner in quiet.into_iter().rev() {
                self.queued.push_front(banner);
            }
        }

        self.promote(now);
        true
    }

    // its sender or the user closed it; false when it was not here
    pub fn close(&mut self, id: u32, now: Instant) -> bool {
        self.keep(|banner| banner.id != id, now)
    }

    // DND came on: only loud Banners stay; false when there was nothing to drop
    pub fn silence(&mut self, now: Instant) -> bool {
        self.keep(Banner::loud, now)
    }

    // anything DND would drop
    pub fn silences(&self) -> bool {
        self.shown.iter().any(|shown| !shown.banner.loud())
            || self.queued.iter().any(|queued| !queued.loud())
    }

    // the Banners that left whose notification stays out of the history, each once
    pub fn take_left(&mut self) -> Vec<(u32, u64)> {
        std::mem::take(&mut self.left)
    }

    pub fn has_left(&self) -> bool {
        !self.left.is_empty()
    }

    // only the Banners `stays` holds, shown or queued, noting each that leaves; false when none did
    fn keep(&mut self, stays: impl Fn(&Banner) -> bool, now: Instant) -> bool {
        let before = (self.shown.len(), self.queued.len());

        let leaving: Vec<(u32, u64)> = self
            .all()
            .filter(|banner| !stays(banner) && !banner.history)
            .map(|banner| (banner.id, banner.version))
            .collect();
        self.left.extend(leaving);

        self.shown.retain(|shown| stays(&shown.banner));
        self.queued.retain(&stays);

        let dropped = before != (self.shown.len(), self.queued.len());

        self.promote(now);
        dropped
    }

    fn all(&self) -> impl Iterator<Item = &Banner> {
        self.shown
            .iter()
            .map(|shown| &shown.banner)
            .chain(&self.queued)
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

    pub fn drafting(&self, id: u32) -> bool {
        self.shown
            .iter()
            .any(|shown| shown.banner.id == id && shown.drafting)
    }

    // a reply is being typed on the Banner or not any more (its field is empty), which holds its
    // time or starts it afresh; false when nothing changed
    pub fn draft(&mut self, id: u32, drafting: bool, now: Instant) -> bool {
        let hovered = self.hovered;

        let Some(shown) = self
            .shown
            .iter_mut()
            .find(|shown| shown.banner.id == id && shown.drafting != drafting)
        else {
            return false;
        };

        shown.drafting = drafting;
        shown.timer = if drafting {
            Timer::Sticky
        } else {
            timer(&shown.banner, hovered, now)
        };

        true
    }

    // takes down every Banner whose time ran out, and shows queued ones in their place
    pub fn expire(&mut self, now: Instant) {
        let over = |shown: &Shown| matches!(shown.timer, Timer::Running(until) if until <= now);

        for shown in self.shown.iter().filter(|shown| over(shown)) {
            if !shown.banner.history {
                self.left.push((shown.banner.id, shown.banner.version));
            }
        }

        self.shown.retain(|shown| !over(shown));

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
        while self.shown.len() < MOST {
            let Some(index) = self
                .queued
                .iter()
                .position(|banner| !waits(self.held, banner.loud()))
            else {
                break;
            };
            let Some(banner) = self.queued.remove(index) else {
                break;
            };
            let timer = timer(&banner, self.hovered, now);

            self.shown.push(Shown {
                banner,
                timer,
                drafting: false,
            });
        }

        // with nothing showing, nothing is under the pointer
        if self.shown.is_empty() {
            self.hovered = false;
        }
    }
}

// fullscreen silence already combined into `held`: a quiet Banner waits, a loud one shows
fn waits(held: bool, loud: bool) -> bool {
    crate::sources::notifications::policy::hold_banner(loud, held, true)
}

// a full time, held while the pointer is on the Banners
fn timer(banner: &Banner, hovered: bool, now: Instant) -> Timer {
    match banner.length() {
        None => Timer::Sticky,
        Some(length) if hovered => Timer::Paused(length),
        Some(length) => Timer::Running(now + length),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn banner(id: u32, urgency: Urgency) -> Banner {
        Banner {
            id,
            version: 0,
            toast: Toast::default(),
            urgency,
            default: false,
            actions: Vec::new(),
            reply: None,
            history: true,
            bypass_dnd: false,
            timeout: None,
        }
    }

    fn ids(stack: &Stack) -> Vec<u32> {
        let mut ids: Vec<u32> = stack.shown().map(|banner| banner.id).collect();
        ids.sort_unstable();
        ids
    }

    // a rule's bypass shows a Banner under DND as a Critical one does, and nothing else does
    #[test]
    fn dnd_keeps_only_critical_and_bypassing_banners() {
        let now = Instant::now();
        let mut stack = Stack::default();
        let bypassing = Banner {
            bypass_dnd: true,
            ..banner(3, Urgency::Low)
        };

        stack.arrive(banner(1, Urgency::Normal), false, now);
        stack.arrive(banner(2, Urgency::Critical), false, now);
        stack.arrive(bypassing.clone(), false, now);
        assert!(stack.silence(now));
        assert_eq!(ids(&stack), [2, 3]);
        assert!(!stack.silences());

        let mut held = Stack::default();
        held.arrive(banner(1, Urgency::Normal), false, now);
        held.arrive(
            Banner {
                bypass_dnd: true,
                ..banner(4, Urgency::Low)
            },
            false,
            now,
        );
        assert!(held.set_held(true, now));
        assert!(held.shown().all(|banner| banner.loud()));
        assert!(held.contains(1));
        held.arrive(banner(5, Urgency::Low), false, now);
        assert!(held.shown().all(|banner| banner.id != 5));
        assert!(held.set_held(false, now));
        assert!(held.shown().any(|banner| banner.id == 1));
        assert!(held.shown().any(|banner| banner.id == 5));

        let mut quiet = Stack::default();
        quiet.arrive(banner(1, Urgency::Normal), true, now);
        quiet.arrive(bypassing, true, now);
        assert_eq!(ids(&quiet), [3]);
    }

    // a Banner whose notification stays out of the history is noted however it leaves, once
    #[test]
    fn an_unkept_banner_is_noted_as_it_leaves() {
        let now = Instant::now();
        let unkept = |id, urgency| Banner {
            history: false,
            ..banner(id, urgency)
        };
        let mut stack = Stack::default();

        stack.arrive(unkept(1, Urgency::Low), false, now);
        stack.arrive(unkept(2, Urgency::Normal), false, now);
        stack.arrive(unkept(3, Urgency::Critical), false, now);
        stack.arrive(banner(4, Urgency::Low), false, now);
        assert!(!stack.has_left());

        // its timer
        stack.expire(now + LOW);
        assert_eq!(stack.take_left(), [(1, 0)]);

        // DND coming on
        stack.silence(now);
        assert_eq!(stack.take_left(), [(2, 0)]);

        // its X, or its sender closing it
        stack.close(3, now);
        assert_eq!(stack.take_left(), [(3, 0)]);

        // dropped as it arrives under DND
        stack.arrive(unkept(5, Urgency::Normal), true, now);
        assert_eq!(stack.take_left(), [(5, 0)]);
        assert!(!stack.has_left());
    }

    // a new version DND drops closes as its own rule says, never as the version it replaced
    #[test]
    fn a_replacement_under_dnd_follows_its_own_rule() {
        let now = Instant::now();
        let mut stack = Stack::default();
        let loud_unkept = Banner {
            bypass_dnd: true,
            history: false,
            ..banner(1, Urgency::Normal)
        };

        stack.arrive(loud_unkept.clone(), true, now);
        stack.arrive(banner(1, Urgency::Normal), true, now);
        assert!(!stack.contains(1));
        assert!(!stack.has_left());

        // and the other way, a kept version replaced by an unkept one closes
        stack.arrive(loud_unkept, true, now);
        stack.arrive(
            Banner {
                history: false,
                ..banner(1, Urgency::Normal)
            },
            true,
            now,
        );
        assert_eq!(stack.take_left(), [(1, 0)]);
    }

    // a rule's time stands for any urgency, a Critical one too
    #[test]
    fn a_rule_s_timeout_times_even_a_critical_banner() {
        let now = Instant::now();
        let mut stack = Stack::default();

        stack.arrive(banner(1, Urgency::Critical), false, now);
        stack.arrive(
            Banner {
                timeout: Some(Duration::from_secs(2)),
                ..banner(2, Urgency::Critical)
            },
            false,
            now,
        );

        assert_eq!(stack.deadline(), Some(now + Duration::from_secs(2)));
        stack.expire(now + Duration::from_secs(2));
        assert_eq!(ids(&stack), [1]);
    }

    // a reply being typed is never lost to the clock, a replacement included; once the field is
    // empty again the Banner has its time afresh
    #[test]
    fn a_banner_with_a_draft_stays_until_the_draft_ends() {
        let now = Instant::now();
        let later = now + Duration::from_secs(60);
        let mut stack = Stack::default();
        let replying = |id| Banner {
            reply: Some(String::new()),
            ..banner(id, Urgency::Normal)
        };

        stack.arrive(replying(1), false, now);
        assert!(stack.draft(1, true, now));
        assert!(!stack.draft(1, true, now));

        stack.expire(later);
        assert!(stack.contains(1));

        // a new version keeps the draft while it takes a reply, and lets go when it does not
        stack.arrive(replying(1), false, later);
        assert_eq!(stack.deadline(), None);
        stack.arrive(banner(1, Urgency::Normal), false, later);
        assert_eq!(stack.deadline(), Some(later + NORMAL));

        stack.arrive(replying(2), false, later);
        stack.draft(2, true, later);
        assert!(stack.draft(2, false, later));
        assert_eq!(stack.deadline(), Some(later + NORMAL));
    }
}
