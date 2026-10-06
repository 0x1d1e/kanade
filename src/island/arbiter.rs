//! The Arbiter (CONTEXT.md, plan 5.1 rules 1-5, 7). Pure: posts and now in, Frame out.
//!
//! `Interrupt::Transient` Activities compete for the transient that shows over the primary, the
//! rest for the primary. Each slot takes the highest Priority, tie by the newest post. The Ongoing
//! and Critical Persistent ones that lose the primary become Satellites. Expired Activities
//! stay registered until `expire`, but never reach a Frame, so the primary returns on its own.
//!
//! A new primary dwells: for `DWELL` a newer Activity of its Priority waits behind it. Higher
//! Priority, Preempt, AutoExpand, withdraw and expiry replace it at once. Which primary shows since
//! when is the one state the Arbiter keeps besides the posts, settled at the `now` of each change.
//!
//! Each island gets its own Frame. Scope and an open Surface (rule 4) only hide Activities from
//! it, never withdraw one; dropping the transients an open Surface already shows is `absorb`. DND (rule 5) hides the toasts already up and drops those posted during it.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::activity::{Activity, Frame, Id, Interrupt, Kind, Lifetime, Priority, Scope};

// beside the primary at once; the rest only count (plan 5.1 rule 3)
pub const SATELLITES: usize = 2;

// how long a new primary stays before a newer one of its Priority replaces it
pub const DWELL: Duration = Duration::from_millis(1500);

#[derive(Debug, Default)]
pub struct Arbiter {
    activities: HashMap<Id, Entry>,

    // counts posts, so the newest wins a tie even at the same instant
    posts: u64,

    dnd: bool,

    // the primary of the islands off the focused output, then of the focused one's
    shown: [Option<Shown>; 2],
}

// the island a Frame is for
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Island {
    // on the focused output, the only one FocusedOutput Activities show on
    pub focused: bool,

    // showing a Surface the user opened
    pub expanded: bool,
}

// a primary, and when it became one
#[derive(Debug)]
struct Shown {
    id: Id,
    since: Instant,

    // when it gives way to the newer one of its Priority it keeps back, if any
    until: Option<Instant>,
}

#[derive(Debug)]
struct Entry {
    activity: Activity,
    posted: Instant,
    post: u64,
}

impl Entry {
    fn expiry(&self) -> Option<Instant> {
        match self.activity.lifetime() {
            Lifetime::Persistent => None,
            Lifetime::Transient(duration) => Some(self.posted + duration),
        }
    }

    // a Transient is gone at its expiry, not after it
    fn live(&self, now: Instant) -> bool {
        self.expiry().is_none_or(|expiry| now < expiry)
    }
}

impl Arbiter {
    /*
     * a post with a known Id replaces that Activity and restarts its Lifetime. A toast DND
     * silences still replaces, then is dropped, so turning DND off shows neither it nor the one
     * it replaced
     */
    pub fn post(&mut self, activity: Activity, now: Instant) {
        if self.silenced(&activity) {
            self.activities.remove(activity.id());
        } else {
            self.posts += 1;

            self.activities.insert(
                activity.id().clone(),
                Entry {
                    activity,
                    posted: now,
                    post: self.posts,
                },
            );
        }

        self.settle(now);
    }

    pub fn contains(&self, id: &Id) -> bool {
        self.activities.contains_key(id)
    }

    /*
     * how `id` interrupts, if up at `now`: a repost with the same Interrupt is no new arrival,
     * while one that changes it, or a repost of an expired one not yet swept, is
     */
    pub fn interrupt(&self, id: &Id, now: Instant) -> Option<Interrupt> {
        self.activities
            .get(id)
            .filter(|entry| entry.live(now))
            .map(|entry| entry.activity.interrupt())
    }

    // whether it was registered
    pub fn withdraw(&mut self, id: &Id, now: Instant) -> bool {
        let withdrawn = self.activities.remove(id).is_some();

        self.settle(now);
        withdrawn
    }

    // drops the transients `drop` picks, an open Surface showing them already; whether any were
    pub fn absorb(&mut self, drop: impl Fn(&Activity) -> bool) -> bool {
        let before = self.activities.len();

        self.activities.retain(|_, entry| {
            !(entry.activity.interrupt() == Interrupt::Transient && drop(&entry.activity))
        });

        self.activities.len() != before
    }

    // drops what expired by now and ends the dwells over; whether anything did
    pub fn expire(&mut self, now: Instant) -> bool {
        let before = self.activities.len();

        self.activities.retain(|_, entry| entry.live(now));

        let expired = self.activities.len() != before;

        self.settle(now) || expired
    }

    /*
     * when `expire` next has something to do: the earliest expiry of any registered Transient,
     * hidden or not, so the Frame may not change then, or end of a dwell keeping one back. Past
     * once due and waiting for `expire`
     */
    pub fn deadline(&self) -> Option<Instant> {
        self.activities
            .values()
            .filter_map(Entry::expiry)
            .chain(self.shown.iter().flatten().filter_map(|shown| shown.until))
            .min()
    }

    // DND silences Notification toasts, not the Notifications, and never a Critical one (rule 5)
    pub fn set_dnd(&mut self, dnd: bool, now: Instant) {
        self.dnd = dnd;
        self.settle(now);
    }

    pub fn dnd(&self) -> bool {
        self.dnd
    }

    pub fn frame(&self, now: Instant, island: Island) -> Frame {
        let ranked = self.ranked(now, island.focused, false);
        let primary = self.primary(&ranked, island.focused, now);
        let mut satellites: Vec<Activity> = ranked
            .iter()
            .enumerate()
            .filter(|&(index, _)| Some(index) != primary)
            .map(|(_, entry)| entry.activity.clone())
            .filter(|activity| {
                activity.lifetime() == Lifetime::Persistent
                    && matches!(activity.priority(), Priority::Ongoing | Priority::Critical)
            })
            .collect();
        let overflow = satellites.len().saturating_sub(SATELLITES);
        satellites.truncate(SATELLITES);

        /*
         * over a Surface the user opened, transients wait as a badge and show for what is left of
         * their Lifetime once it closes (rule 4); only a Preempt one closes it, on arrival
         */
        let mut transients = self
            .ranked(now, island.focused, true)
            .into_iter()
            .map(|entry| entry.activity.clone());
        let (transient, queued) = if island.expanded {
            (None, transients.collect())
        } else {
            (transients.next(), Vec::new())
        };

        Frame {
            primary: primary.map(|index| ranked[index].activity.clone()),
            satellites,
            overflow,
            transient,
            queued,
        }
    }

    // the live Activities for the transient slot or the primary on an island, highest first
    fn ranked(&self, now: Instant, focused: bool, transient: bool) -> Vec<&Entry> {
        let mut entries: Vec<&Entry> = self
            .activities
            .values()
            .filter(|entry| entry.live(now))
            .filter(|entry| (entry.activity.interrupt() == Interrupt::Transient) == transient)
            .filter(|entry| focused || entry.activity.scope() == Scope::Global)
            .filter(|entry| !self.silenced(&entry.activity))
            .collect();
        entries.sort_by_key(|entry| Reverse((entry.activity.priority(), entry.post)));
        entries
    }

    /*
     * where in `ranked` the primary is: the top, unless the one shown became the primary less
     * than DWELL ago and the top only ties it without interrupting
     */
    fn primary(&self, ranked: &[&Entry], focused: bool, now: Instant) -> Option<usize> {
        let top = &ranked.first()?.activity;
        let dwells = self.shown[usize::from(focused)]
            .as_ref()
            .filter(|shown| now < shown.since + DWELL)
            .and_then(|shown| {
                ranked
                    .iter()
                    .position(|entry| entry.activity.id() == &shown.id)
            })
            .filter(|&index| {
                ranked[index].activity.priority() == top.priority()
                    && !matches!(
                        top.interrupt(),
                        Interrupt::Preempt | Interrupt::AutoExpand(_)
                    )
            });

        Some(dwells.unwrap_or(0))
    }

    // records the primary each island shows at `now`; whether one changed
    fn settle(&mut self, now: Instant) -> bool {
        let mut changed = false;

        for focused in [false, true] {
            let ranked = self.ranked(now, focused, false);
            let before = self.shown[usize::from(focused)].as_ref();
            let shown = self.primary(&ranked, focused, now).map(|index| {
                let id = ranked[index].activity.id().clone();
                let since = before
                    .filter(|before| before.id == id)
                    .map_or(now, |before| before.since);

                Shown {
                    id,
                    since,
                    until: (index != 0).then_some(since + DWELL),
                }
            });

            changed |= shown.as_ref().map(|shown| &shown.id) != before.map(|before| &before.id);
            self.shown[usize::from(focused)] = shown;
        }

        changed
    }

    fn silenced(&self, activity: &Activity) -> bool {
        self.dnd
            && activity.kind() == Kind::Notification
            && matches!(activity.lifetime(), Lifetime::Transient(_))
            && activity.priority() != Priority::Critical
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::island::activity::fixture::{persistent, shown};

    // the focused island at rest, where everything shows
    const FOCUSED: Island = Island {
        focused: true,
        expanded: false,
    };

    const OSD: Duration = Duration::from_millis(1200);

    fn ms(milliseconds: u64) -> Duration {
        Duration::from_millis(milliseconds)
    }

    fn media(key: &str) -> Activity {
        persistent(Id::new(Kind::Media, key), Priority::Media)
    }

    fn cast() -> Activity {
        persistent(Id::new(Kind::ScreenCast, "cast"), Priority::Ongoing)
    }

    fn volume() -> Activity {
        shown(Id::new(Kind::Volume, "volume"), Priority::Osd, OSD)
    }

    fn primary(arbiter: &Arbiter, now: Instant) -> Option<Id> {
        arbiter
            .frame(now, FOCUSED)
            .primary
            .map(|activity| activity.id().clone())
    }

    fn transient(arbiter: &Arbiter, now: Instant) -> Option<Id> {
        arbiter
            .frame(now, FOCUSED)
            .transient
            .map(|activity| activity.id().clone())
    }

    #[test]
    fn nothing_posted_is_an_empty_frame() {
        let arbiter = Arbiter::default();

        assert_eq!(arbiter.frame(Instant::now(), FOCUSED), Frame::default());
        assert_eq!(arbiter.deadline(), None);
    }

    #[test]
    fn highest_priority_is_primary() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.post(cast(), t0);
        arbiter.post(media("spotify"), t0 + ms(10));

        assert_eq!(primary(&arbiter, t0 + ms(10)), Some(cast().id().clone()));
    }

    #[test]
    fn tie_goes_to_the_newest() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.post(media("spotify"), t0);
        arbiter.post(media("mpv"), t0 + DWELL);
        assert_eq!(
            primary(&arbiter, t0 + DWELL),
            Some(media("mpv").id().clone())
        );

        // same instant, kept back by mpv's dwell: the later post
        let then = t0 + DWELL + ms(10);
        arbiter.post(media("firefox"), then);
        arbiter.post(media("vlc"), then);
        arbiter.expire(t0 + DWELL * 2);
        assert_eq!(
            primary(&arbiter, t0 + DWELL * 2),
            Some(media("vlc").id().clone())
        );

        // a repost is a new post
        arbiter.post(media("spotify"), t0 + DWELL * 3);
        assert_eq!(
            primary(&arbiter, t0 + DWELL * 3),
            Some(media("spotify").id().clone())
        );
    }

    #[test]
    fn an_equal_priority_waits_out_the_dwell() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.post(media("spotify"), t0);
        arbiter.post(media("mpv"), t0 + ms(500));

        let spotify = Some(media("spotify").id().clone());
        assert_eq!(primary(&arbiter, t0 + ms(500)), spotify);
        assert_eq!(primary(&arbiter, t0 + DWELL - ms(1)), spotify);
        assert_eq!(arbiter.deadline(), Some(t0 + DWELL));

        // listen() wakes at the deadline, and expire() moves the primary on
        assert!(arbiter.expire(t0 + DWELL));
        assert_eq!(
            primary(&arbiter, t0 + DWELL),
            Some(media("mpv").id().clone())
        );
        assert_eq!(arbiter.deadline(), None);
        assert!(!arbiter.expire(t0 + DWELL * 2));
    }

    #[test]
    fn an_update_in_place_keeps_the_primary_and_its_dwell() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.post(media("spotify"), t0);
        arbiter.post(media("mpv"), t0 + ms(500));
        arbiter.post(media("spotify"), t0 + ms(1000));

        // the update neither loses the primary nor restarts its dwell
        assert_eq!(
            primary(&arbiter, t0 + ms(1000)),
            Some(media("spotify").id().clone())
        );
        assert_eq!(arbiter.deadline(), None);

        arbiter.post(media("mpv"), t0 + ms(1200));
        assert_eq!(arbiter.deadline(), Some(t0 + DWELL));
    }

    #[test]
    fn interruptions_bypass_the_dwell() {
        let t0 = Instant::now();
        let now = t0 + ms(100);
        let fresh = || {
            let mut arbiter = Arbiter::default();
            arbiter.post(media("spotify"), t0);
            arbiter
        };
        let media_with = |key: &str, lifetime, interrupt| {
            Activity::new(
                Id::new(Kind::Media, key),
                Priority::Media,
                lifetime,
                Scope::Global,
                interrupt,
            )
            .unwrap()
        };

        // higher Priority
        let mut arbiter = fresh();
        arbiter.post(cast(), now);
        assert_eq!(primary(&arbiter, now), Some(cast().id().clone()));

        // Preempt and AutoExpand, at an equal Priority
        for interrupt in [Interrupt::Preempt, Interrupt::AutoExpand(OSD)] {
            let mut arbiter = fresh();
            let interrupting = media_with("mpv", Lifetime::Persistent, interrupt);
            arbiter.post(interrupting.clone(), now);
            assert_eq!(primary(&arbiter, now), Some(interrupting.id().clone()));
        }

        // withdraw
        let mut arbiter = fresh();
        arbiter.post(media("mpv"), now);
        arbiter.post(media("vlc"), now);
        assert!(arbiter.withdraw(media("spotify").id(), now));
        assert_eq!(primary(&arbiter, now), Some(media("vlc").id().clone()));

        // expiry, before the dwell ends
        let mut arbiter = Arbiter::default();
        let brief = media_with("brief", Lifetime::Transient(OSD), Interrupt::None);
        arbiter.post(brief, t0);
        arbiter.post(media("mpv"), now);
        assert_eq!(arbiter.deadline(), Some(t0 + OSD));
        assert_eq!(primary(&arbiter, t0 + OSD), Some(media("mpv").id().clone()));
        assert!(arbiter.expire(t0 + OSD));
        assert_eq!(arbiter.deadline(), None);
    }

    #[test]
    fn a_new_primary_dwells_after_a_bypass() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.post(cast(), t0);
        arbiter.post(media("spotify"), t0);

        // the cast leaving makes spotify the primary now, so its dwell starts now
        arbiter.withdraw(cast().id(), t0 + DWELL * 2);
        arbiter.post(media("mpv"), t0 + DWELL * 2 + ms(100));
        assert_eq!(
            primary(&arbiter, t0 + DWELL * 2 + ms(100)),
            Some(media("spotify").id().clone())
        );
        assert_eq!(arbiter.deadline(), Some(t0 + DWELL * 3));
    }

    // FocusedOutput Activities show on the focused island only, so its primary dwells on its own
    #[test]
    fn each_island_dwells_on_its_own_primary() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();
        let other = Island {
            focused: false,
            expanded: false,
        };
        let local = Activity::new(
            Id::new(Kind::Media, "local"),
            Priority::Media,
            Lifetime::Persistent,
            Scope::FocusedOutput,
            Interrupt::None,
        )
        .unwrap();

        arbiter.post(media("spotify"), t0);
        arbiter.post(local.clone(), t0 + DWELL);
        arbiter.post(media("mpv"), t0 + DWELL + ms(100));

        let now = t0 + DWELL + ms(100);
        assert_eq!(primary(&arbiter, now), Some(local.id().clone()));
        assert_eq!(arbiter.frame(now, other).primary, Some(media("mpv")));
    }

    #[test]
    fn same_id_replaces() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();
        let id = Id::new(Kind::Battery, "BAT0");

        arbiter.post(persistent(id.clone(), Priority::Passive), t0);
        arbiter.post(persistent(id.clone(), Priority::Critical), t0);

        let frame = arbiter.frame(t0, FOCUSED);

        assert_eq!(
            frame.primary.map(|activity| activity.priority()),
            Some(Priority::Critical)
        );
        assert!(arbiter.withdraw(&id, t0));
        assert_eq!(arbiter.frame(t0, FOCUSED), Frame::default());
        assert!(!arbiter.withdraw(&id, t0));
    }

    #[test]
    fn replacement_extends_the_lifetime() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.post(volume(), t0);
        arbiter.post(volume(), t0 + ms(1000));

        // past the first post's expiry, inside the second's
        assert_eq!(
            transient(&arbiter, t0 + ms(2000)),
            Some(volume().id().clone())
        );
        assert_eq!(arbiter.deadline(), Some(t0 + ms(2200)));
        assert_eq!(transient(&arbiter, t0 + ms(2200)), None);
    }

    #[test]
    fn transient_shows_over_the_primary_then_the_primary_returns() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.post(media("spotify"), t0);
        arbiter.post(volume(), t0 + ms(100));

        let over = arbiter.frame(t0 + ms(500), FOCUSED);
        assert_eq!(
            over.transient.map(|activity| activity.kind()),
            Some(Kind::Volume)
        );
        assert_eq!(over.primary, Some(media("spotify")));

        // no repost: the primary was never displaced, only covered
        let after = arbiter.frame(t0 + ms(1300), FOCUSED);
        assert_eq!(after.transient, None);
        assert_eq!(after.primary, Some(media("spotify")));
    }

    #[test]
    fn a_transient_never_becomes_the_primary() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.post(volume(), t0);

        assert_eq!(primary(&arbiter, t0), None);
        assert_eq!(transient(&arbiter, t0), Some(volume().id().clone()));
    }

    #[test]
    fn transients_compete_by_priority() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();
        let toast = shown(
            Id::new(Kind::Notification, "7"),
            Priority::Actionable,
            Duration::from_secs(5),
        );

        arbiter.post(toast.clone(), t0);
        arbiter.post(volume(), t0 + ms(100));
        assert_eq!(transient(&arbiter, t0 + ms(100)), Some(toast.id().clone()));

        // once the toast is gone, the volume still has its own time left
        arbiter.post(volume(), t0 + ms(4500));
        assert_eq!(
            transient(&arbiter, t0 + ms(5000)),
            Some(volume().id().clone())
        );
    }

    #[test]
    fn expiry_boundary() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.post(volume(), t0);

        let expiry = t0 + OSD;
        assert!(transient(&arbiter, expiry - Duration::from_nanos(1)).is_some());
        assert_eq!(transient(&arbiter, expiry), None);
        assert_eq!(arbiter.deadline(), Some(expiry));

        // still due until expire drops it, so a late wake still expires it
        assert!(arbiter.expire(expiry));
        assert_eq!(arbiter.deadline(), None);
    }

    #[test]
    fn deadline_is_the_next_expiry() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();
        let toast = shown(
            Id::new(Kind::Notification, "7"),
            Priority::Passive,
            Duration::from_secs(5),
        );

        arbiter.post(media("spotify"), t0);
        assert_eq!(arbiter.deadline(), None);

        arbiter.post(toast, t0);
        arbiter.post(volume(), t0);
        assert_eq!(arbiter.deadline(), Some(t0 + OSD));

        arbiter.expire(t0 + OSD);
        assert_eq!(arbiter.deadline(), Some(t0 + ms(5000)));
    }

    #[test]
    fn expire_drops_only_what_expired() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.post(media("spotify"), t0);
        arbiter.post(volume(), t0);

        assert!(!arbiter.expire(t0 + ms(1199)));
        assert!(arbiter.expire(t0 + OSD));
        assert!(!arbiter.expire(t0 + ms(5000)));

        // gone for good: an earlier now cannot bring it back
        assert_eq!(transient(&arbiter, t0), None);
        assert_eq!(primary(&arbiter, t0), Some(media("spotify").id().clone()));
    }

    fn ongoing(key: &str) -> Activity {
        persistent(Id::new(Kind::Timer, key), Priority::Ongoing)
    }

    fn satellites(arbiter: &Arbiter, now: Instant) -> (Vec<Id>, usize) {
        let frame = arbiter.frame(now, FOCUSED);
        let ids = frame
            .satellites
            .iter()
            .map(|activity| activity.id().clone())
            .collect();

        (ids, frame.overflow)
    }

    #[test]
    fn ongoing_and_critical_beside_the_primary_become_satellites() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();
        let battery = persistent(Id::new(Kind::Battery, "BAT0"), Priority::Critical);
        let wifi = persistent(Id::new(Kind::Network, "wlan0"), Priority::Passive);

        arbiter.post(battery.clone(), t0);
        arbiter.post(cast(), t0);
        arbiter.post(media("spotify"), t0);
        arbiter.post(wifi, t0);

        // the primary is not repeated, Media and Passive never become Satellites
        assert_eq!(primary(&arbiter, t0), Some(battery.id().clone()));
        assert_eq!(satellites(&arbiter, t0), (vec![cast().id().clone()], 0));
    }

    #[test]
    fn only_persistent_activities_are_satellites() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();
        let battery = persistent(Id::new(Kind::Battery, "BAT0"), Priority::Critical);

        let alarm = Activity::new(
            Id::new(Kind::Timer, "alarm"),
            Priority::Critical,
            Lifetime::Transient(Duration::from_secs(5)),
            Scope::Global,
            Interrupt::None,
        )
        .unwrap();

        arbiter.post(alarm, t0);
        arbiter.post(battery.clone(), t0 + DWELL);

        // the alarm loses the primary to the newer Critical, and has a Lifetime, so goes nowhere
        assert_eq!(primary(&arbiter, t0 + DWELL), Some(battery.id().clone()));
        assert_eq!(satellites(&arbiter, t0 + DWELL), (vec![], 0));
    }

    // only `Interrupt::Transient` picks the transient slot, whatever the Lifetime
    #[test]
    fn the_interrupt_picks_the_slot() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();
        let brief = Activity::new(
            Id::new(Kind::Network, "wlan0"),
            Priority::Passive,
            Lifetime::Transient(OSD),
            Scope::Global,
            Interrupt::None,
        )
        .unwrap();
        let lasting = Activity::new(
            Id::new(Kind::Volume, "volume"),
            Priority::Osd,
            Lifetime::Persistent,
            Scope::FocusedOutput,
            Interrupt::Transient,
        )
        .unwrap();

        arbiter.post(brief.clone(), t0);
        arbiter.post(lasting.clone(), t0);

        let frame = arbiter.frame(t0, FOCUSED);
        assert_eq!(frame.primary, Some(brief));
        assert_eq!(frame.transient, Some(lasting.clone()));

        // the brief one still expires, the lasting one does not
        let later = arbiter.frame(t0 + OSD, FOCUSED);
        assert_eq!(later.primary, None);
        assert_eq!(later.transient, Some(lasting));
    }

    #[test]
    fn satellites_cap_and_count_the_overflow() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.post(media("spotify"), t0);
        for key in ["a", "b", "c", "d"] {
            arbiter.post(ongoing(key), t0);
        }

        // "a" outranked the media, so is the primary dwelling; three left for two places
        let (shown, overflow) = satellites(&arbiter, t0);
        assert_eq!(shown.len(), SATELLITES);
        assert_eq!(overflow, 1);

        assert!(arbiter.withdraw(ongoing("a").id(), t0));
        assert_eq!(satellites(&arbiter, t0).1, 0);
    }

    #[test]
    fn satellites_are_highest_first_then_newest() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();
        let battery = persistent(Id::new(Kind::Battery, "BAT0"), Priority::Critical);
        let privacy = persistent(Id::new(Kind::Privacy, "mic"), Priority::Critical);

        arbiter.post(privacy.clone(), t0);
        arbiter.post(ongoing("old"), t0);
        arbiter.post(ongoing("new"), t0 + ms(10));
        arbiter.post(battery.clone(), t0 + DWELL);

        // battery is the newest Critical, past privacy's dwell, so the primary
        assert_eq!(primary(&arbiter, t0 + DWELL), Some(battery.id().clone()));
        assert_eq!(
            satellites(&arbiter, t0 + DWELL),
            (vec![privacy.id().clone(), ongoing("new").id().clone()], 1)
        );
    }

    #[test]
    fn a_satellite_is_promoted_when_the_primary_is_withdrawn() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();
        let battery = persistent(Id::new(Kind::Battery, "BAT0"), Priority::Critical);

        arbiter.post(battery.clone(), t0);
        arbiter.post(cast(), t0);
        arbiter.post(ongoing("timer"), t0 + ms(10));

        assert!(arbiter.withdraw(battery.id(), t0));

        assert_eq!(primary(&arbiter, t0), Some(ongoing("timer").id().clone()));
        assert_eq!(satellites(&arbiter, t0), (vec![cast().id().clone()], 0));
    }

    fn toast(key: &str, priority: Priority) -> Activity {
        shown(
            Id::new(Kind::Notification, key),
            priority,
            Duration::from_secs(5),
        )
    }

    // Critical, but brief and preempting
    fn call() -> Activity {
        Activity::new(
            Id::new(Kind::Privacy, "call"),
            Priority::Critical,
            Lifetime::Transient(OSD),
            Scope::FocusedOutput,
            Interrupt::Preempt,
        )
        .unwrap()
    }

    const EXPANDED: Island = Island {
        focused: true,
        expanded: true,
    };

    #[test]
    fn transients_show_only_on_the_focused_island() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();
        let other = Island {
            focused: false,
            expanded: false,
        };

        arbiter.post(media("spotify"), t0);
        arbiter.post(cast(), t0);
        arbiter.post(volume(), t0);

        let focused = arbiter.frame(t0, FOCUSED);
        let unfocused = arbiter.frame(t0, other);

        assert_eq!(focused.transient, Some(volume()));
        assert_eq!(unfocused.transient, None);

        // Global ones show everywhere
        assert_eq!(unfocused.primary, focused.primary);
        assert_eq!(unfocused.satellites, focused.satellites);
    }

    #[test]
    fn an_open_surface_keeps_transients_as_a_badge() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.post(media("spotify"), t0);
        arbiter.post(volume(), t0);
        arbiter.post(toast("7", Priority::Actionable), t0);

        let open = arbiter.frame(t0, EXPANDED);
        assert_eq!(open.transient, None);
        assert_eq!(open.queued, [toast("7", Priority::Actionable), volume()]);
        assert_eq!(open.primary, Some(media("spotify")));

        // closed before the toast expired: it shows for the rest of its Lifetime
        let closed = arbiter.frame(t0 + ms(3000), FOCUSED);
        assert_eq!(closed.transient, Some(toast("7", Priority::Actionable)));
        assert_eq!(closed.queued, []);
    }

    // a Preempt Activity closes the Surface on arrival (`IslandService`), so competes for the primary
    #[test]
    fn a_preempt_activity_competes_for_the_primary() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.post(media("spotify"), t0);
        arbiter.post(toast("7", Priority::Actionable), t0);
        arbiter.post(call(), t0);

        let open = arbiter.frame(t0, EXPANDED);
        assert_eq!(open.primary, Some(call()));
        assert_eq!(open.transient, None);
        assert_eq!(open.queued, [toast("7", Priority::Actionable)]);

        // FocusedOutput still hides it elsewhere
        let other = Island {
            focused: false,
            expanded: false,
        };
        assert_eq!(arbiter.frame(t0, other).primary, Some(media("spotify")));
    }

    #[test]
    fn interrupt_is_that_of_a_live_activity() {
        let now = Instant::now();
        let mut arbiter = Arbiter::default();
        let battery = Id::new(Kind::Battery, "BAT0");

        assert_eq!(arbiter.interrupt(&battery, now), None);

        arbiter.post(persistent(battery.clone(), Priority::Ongoing), now);
        assert_eq!(arbiter.interrupt(&battery, now), Some(Interrupt::None));

        // a low Priority may preempt, a Critical need not
        let preempting = Activity::new(
            battery.clone(),
            Priority::Passive,
            Lifetime::Persistent,
            Scope::Global,
            Interrupt::Preempt,
        )
        .unwrap();
        arbiter.post(preempting, now);
        assert_eq!(arbiter.interrupt(&battery, now), Some(Interrupt::Preempt));

        // registered until expire() sweeps it, but no longer up
        arbiter.post(call(), now);
        assert_eq!(
            arbiter.interrupt(call().id(), now + OSD - ms(1)),
            Some(Interrupt::Preempt)
        );
        assert_eq!(arbiter.interrupt(call().id(), now + OSD), None);
    }

    // every policy `Activity::new` accepts, on every kind of island, makes a Frame
    #[test]
    fn the_arbiter_takes_any_valid_activity() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();
        let lifetimes = [Lifetime::Persistent, Lifetime::Transient(OSD)];
        let scopes = [Scope::Global, Scope::FocusedOutput];
        let interrupts = [
            Interrupt::None,
            Interrupt::Transient,
            Interrupt::Preempt,
            Interrupt::AutoExpand(OSD),
        ];
        let mut posted = 0;

        for (n, kind) in Kind::ALL.into_iter().enumerate() {
            for priority in Priority::ALL {
                for lifetime in lifetimes {
                    for scope in scopes {
                        for interrupt in interrupts {
                            let key =
                                format!("{n}-{priority:?}-{lifetime:?}-{scope:?}-{interrupt:?}");
                            let id = Id::new(kind, &key);

                            if let Ok(activity) =
                                Activity::new(id, priority, lifetime, scope, interrupt)
                            {
                                arbiter.post(activity, t0);
                                posted += 1;
                            }
                        }
                    }
                }
            }
        }

        assert!(posted > 0);
        for dnd in [false, true] {
            arbiter.set_dnd(dnd, t0);
            for focused in [false, true] {
                for expanded in [false, true] {
                    let island = Island { focused, expanded };
                    let frame = arbiter.frame(t0, island);
                    assert!(frame.primary.is_some());
                    assert!(frame.satellites.len() <= SATELLITES);
                    arbiter.frame(t0 + OSD, island);
                }
            }
        }
        arbiter.absorb(|activity| activity.kind() == Kind::Notification);
        assert!(arbiter.expire(t0 + OSD));
        assert!(arbiter.frame(t0 + OSD, FOCUSED).primary.is_some());
    }

    #[test]
    fn a_queued_transient_still_expires() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.post(volume(), t0);

        assert_eq!(arbiter.frame(t0, EXPANDED).queued, [volume()]);
        assert_eq!(arbiter.frame(t0 + OSD, EXPANDED).queued, []);
        assert_eq!(arbiter.frame(t0 + OSD, FOCUSED).transient, None);
    }

    #[test]
    fn dnd_hides_notification_toasts_only() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();
        let history = persistent(Id::new(Kind::Notification, "7"), Priority::Passive);

        arbiter.set_dnd(true, t0);
        arbiter.post(history.clone(), t0);
        arbiter.post(toast("8", Priority::Actionable), t0);

        // the toast is gone from every slot, the Notification and other Transients stay
        let frame = arbiter.frame(t0, FOCUSED);
        assert_eq!(frame.primary, Some(history));
        assert_eq!(frame.transient, None);
        assert_eq!(arbiter.frame(t0, EXPANDED).queued, []);

        arbiter.post(volume(), t0);
        assert_eq!(arbiter.frame(t0, FOCUSED).transient, Some(volume()));
    }

    #[test]
    fn dnd_never_hides_critical() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.set_dnd(true, t0);
        arbiter.post(toast("7", Priority::Critical), t0);

        assert_eq!(
            arbiter.frame(t0, FOCUSED).transient,
            Some(toast("7", Priority::Critical))
        );
    }

    #[test]
    fn dnd_drops_toasts_posted_during_it() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.set_dnd(true, t0);
        arbiter.post(toast("7", Priority::Passive), t0);
        arbiter.set_dnd(false, t0);

        assert!(!arbiter.dnd());
        assert_eq!(arbiter.frame(t0 + ms(1000), FOCUSED).transient, None);
        assert_eq!(arbiter.deadline(), None);
    }

    #[test]
    fn a_toast_reposted_during_dnd_replaces_the_one_already_up() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.post(toast("7", Priority::Passive), t0);
        arbiter.set_dnd(true, t0);
        arbiter.post(toast("7", Priority::Passive), t0 + ms(1000));
        arbiter.set_dnd(false, t0);

        // inside the first post's Lifetime, which the repost ended
        assert_eq!(arbiter.frame(t0 + ms(2000), FOCUSED).transient, None);
        assert_eq!(arbiter.deadline(), None);
    }

    #[test]
    fn dnd_hides_a_toast_already_up_without_withdrawing_it() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.post(toast("7", Priority::Passive), t0);

        arbiter.set_dnd(true, t0);
        assert_eq!(arbiter.frame(t0, FOCUSED).transient, None);

        // it was up before DND, so it comes back for the rest of its Lifetime
        arbiter.set_dnd(false, t0);
        assert_eq!(
            arbiter.frame(t0 + ms(1000), FOCUSED).transient,
            Some(toast("7", Priority::Passive))
        );
    }
}
