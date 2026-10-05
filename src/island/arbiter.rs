//! The Arbiter (CONTEXT.md, plan 5.1 rules 1, 2, 3, 7). Pure: posts and now in, Frame out.
//!
//! Persistent Activities compete for the primary, Transient ones for the transient that shows
//! over it. Each slot takes the highest Priority, tie by the newest post. The Ongoing and Critical
//! Persistent ones that lose the primary become Satellites. Expired Activities
//! stay registered until `expire`, but never reach a Frame, so the primary returns on its own.

#![cfg_attr(
    not(test),
    expect(dead_code, reason = "IslandService wires the Arbiter in, #20")
)]

use std::cmp::Reverse;
use std::collections::HashMap;
use std::time::Instant;

use super::activity::{Activity, Frame, Id, Lifetime, Priority};

// beside the primary at once; the rest only count (plan 5.1 rule 3)
pub const SATELLITES: usize = 2;

#[derive(Debug, Default)]
pub struct Arbiter {
    activities: HashMap<Id, Entry>,

    // counts posts, so the newest wins a tie even at the same instant
    posts: u64,
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
    // a post with a known Id replaces that Activity and restarts its Lifetime
    pub fn post(&mut self, activity: Activity, now: Instant) {
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

    // whether it was registered
    pub fn withdraw(&mut self, id: &Id) -> bool {
        self.activities.remove(id).is_some()
    }

    // drops what expired by now; whether anything did
    pub fn expire(&mut self, now: Instant) -> bool {
        let before = self.activities.len();

        self.activities.retain(|_, entry| entry.live(now));

        self.activities.len() != before
    }

    // the next expiry after now, of any live Transient, hidden or not; the Frame may not change then
    pub fn deadline(&self, now: Instant) -> Option<Instant> {
        self.activities
            .values()
            .filter(|entry| entry.live(now))
            .filter_map(Entry::expiry)
            .min()
    }

    pub fn frame(&self, now: Instant) -> Frame {
        // highest first, so the primary leads and Satellites keep the same order
        let ranked = |transient: bool| {
            let mut entries: Vec<&Entry> = self
                .activities
                .values()
                .filter(|entry| entry.live(now) && entry.expiry().is_some() == transient)
                .collect();
            entries.sort_by_key(|entry| Reverse((entry.activity.priority(), entry.post)));
            entries.into_iter().map(|entry| entry.activity.clone())
        };

        let mut persistent = ranked(false);
        let primary = persistent.next();
        let mut satellites: Vec<Activity> = persistent
            .filter(|activity| {
                matches!(activity.priority(), Priority::Ongoing | Priority::Critical)
            })
            .collect();
        let overflow = satellites.len().saturating_sub(SATELLITES);
        satellites.truncate(SATELLITES);

        Frame {
            primary,
            satellites,
            overflow,
            transient: ranked(true).next(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::island::activity::Kind;

    const OSD: Duration = Duration::from_millis(1200);

    fn ms(milliseconds: u64) -> Duration {
        Duration::from_millis(milliseconds)
    }

    fn media(key: &str) -> Activity {
        Activity::persistent(Id::new(Kind::Media, key), Priority::Media)
    }

    fn cast() -> Activity {
        Activity::persistent(Id::new(Kind::ScreenCast, "cast"), Priority::Ongoing)
    }

    fn volume() -> Activity {
        Activity::transient(Id::new(Kind::Volume, "volume"), Priority::Osd, OSD)
    }

    fn primary(arbiter: &Arbiter, now: Instant) -> Option<Id> {
        arbiter
            .frame(now)
            .primary
            .map(|activity| activity.id().clone())
    }

    fn transient(arbiter: &Arbiter, now: Instant) -> Option<Id> {
        arbiter
            .frame(now)
            .transient
            .map(|activity| activity.id().clone())
    }

    #[test]
    fn nothing_posted_is_an_empty_frame() {
        let arbiter = Arbiter::default();

        assert_eq!(arbiter.frame(Instant::now()), Frame::default());
        assert_eq!(arbiter.deadline(Instant::now()), None);
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
        arbiter.post(media("mpv"), t0 + ms(10));
        assert_eq!(
            primary(&arbiter, t0 + ms(10)),
            Some(media("mpv").id().clone())
        );

        // same instant: the later post
        arbiter.post(media("firefox"), t0 + ms(10));
        assert_eq!(
            primary(&arbiter, t0 + ms(10)),
            Some(media("firefox").id().clone())
        );

        // a repost is a new post
        arbiter.post(media("spotify"), t0 + ms(20));
        assert_eq!(
            primary(&arbiter, t0 + ms(20)),
            Some(media("spotify").id().clone())
        );
    }

    #[test]
    fn same_id_replaces() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();
        let id = Id::new(Kind::Battery, "BAT0");

        arbiter.post(Activity::persistent(id.clone(), Priority::Passive), t0);
        arbiter.post(Activity::persistent(id.clone(), Priority::Critical), t0);

        let frame = arbiter.frame(t0);

        assert_eq!(
            frame.primary.map(|activity| activity.priority()),
            Some(Priority::Critical)
        );
        assert!(arbiter.withdraw(&id));
        assert_eq!(arbiter.frame(t0), Frame::default());
        assert!(!arbiter.withdraw(&id));
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
        assert_eq!(arbiter.deadline(t0 + ms(2000)), Some(t0 + ms(2200)));
        assert_eq!(transient(&arbiter, t0 + ms(2200)), None);
    }

    #[test]
    fn transient_shows_over_the_primary_then_the_primary_returns() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.post(media("spotify"), t0);
        arbiter.post(volume(), t0 + ms(100));

        let over = arbiter.frame(t0 + ms(500));
        assert_eq!(
            over.transient.map(|activity| activity.kind()),
            Some(Kind::Volume)
        );
        assert_eq!(over.primary, Some(media("spotify")));

        // no repost: the primary was never displaced, only covered
        let after = arbiter.frame(t0 + ms(1300));
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
        let toast = Activity::transient(
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
        assert_eq!(
            arbiter.deadline(expiry - Duration::from_nanos(1)),
            Some(expiry)
        );
        assert_eq!(arbiter.deadline(expiry), None);
    }

    #[test]
    fn deadline_is_the_next_expiry() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();
        let toast = Activity::transient(
            Id::new(Kind::Notification, "7"),
            Priority::Passive,
            Duration::from_secs(5),
        );

        arbiter.post(media("spotify"), t0);
        assert_eq!(arbiter.deadline(t0), None);

        arbiter.post(toast, t0);
        arbiter.post(volume(), t0);
        assert_eq!(arbiter.deadline(t0), Some(t0 + OSD));
        assert_eq!(arbiter.deadline(t0 + OSD), Some(t0 + ms(5000)));
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
        Activity::persistent(Id::new(Kind::Timer, key), Priority::Ongoing)
    }

    fn satellites(arbiter: &Arbiter, now: Instant) -> (Vec<Id>, usize) {
        let frame = arbiter.frame(now);
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
        let battery = Activity::persistent(Id::new(Kind::Battery, "BAT0"), Priority::Critical);
        let wifi = Activity::persistent(Id::new(Kind::Network, "wlan0"), Priority::Passive);

        arbiter.post(battery.clone(), t0);
        arbiter.post(cast(), t0);
        arbiter.post(media("spotify"), t0);
        arbiter.post(wifi, t0);

        // the primary is not repeated, Media and Passive never become Satellites
        assert_eq!(primary(&arbiter, t0), Some(battery.id().clone()));
        assert_eq!(satellites(&arbiter, t0), (vec![cast().id().clone()], 0));
    }

    #[test]
    fn transients_are_never_satellites() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();
        let call = Activity::transient(Id::new(Kind::Privacy, "call"), Priority::Critical, OSD);

        arbiter.post(media("spotify"), t0);
        arbiter.post(call, t0);

        assert_eq!(satellites(&arbiter, t0), (vec![], 0));
    }

    #[test]
    fn satellites_cap_and_count_the_overflow() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();

        arbiter.post(media("spotify"), t0);
        for key in ["a", "b", "c", "d"] {
            arbiter.post(ongoing(key), t0);
        }

        // "d" is the newest, so the primary; three left for two places
        let (shown, overflow) = satellites(&arbiter, t0);
        assert_eq!(shown.len(), SATELLITES);
        assert_eq!(overflow, 1);

        assert!(arbiter.withdraw(ongoing("a").id()));
        assert_eq!(satellites(&arbiter, t0).1, 0);
    }

    #[test]
    fn satellites_are_highest_first_then_newest() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();
        let battery = Activity::persistent(Id::new(Kind::Battery, "BAT0"), Priority::Critical);
        let privacy = Activity::persistent(Id::new(Kind::Privacy, "mic"), Priority::Critical);

        arbiter.post(privacy.clone(), t0);
        arbiter.post(ongoing("old"), t0);
        arbiter.post(ongoing("new"), t0 + ms(10));
        arbiter.post(battery.clone(), t0 + ms(20));

        // battery is the newest Critical, so the primary
        assert_eq!(primary(&arbiter, t0 + ms(20)), Some(battery.id().clone()));
        assert_eq!(
            satellites(&arbiter, t0 + ms(20)),
            (vec![privacy.id().clone(), ongoing("new").id().clone()], 1)
        );
    }

    #[test]
    fn a_satellite_is_promoted_when_the_primary_is_withdrawn() {
        let t0 = Instant::now();
        let mut arbiter = Arbiter::default();
        let battery = Activity::persistent(Id::new(Kind::Battery, "BAT0"), Priority::Critical);

        arbiter.post(battery.clone(), t0);
        arbiter.post(cast(), t0);
        arbiter.post(ongoing("timer"), t0 + ms(10));

        assert!(arbiter.withdraw(battery.id()));

        assert_eq!(primary(&arbiter, t0), Some(ongoing("timer").id().clone()));
        assert_eq!(satellites(&arbiter, t0), (vec![cast().id().clone()], 0));
    }
}
