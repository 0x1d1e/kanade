//! The Arbiter (CONTEXT.md, plan 5.1 rules 1, 2, 7). Pure: posts and now in, Frame out.
//!
//! Persistent Activities compete for the primary, Transient ones for the transient that shows
//! over it. Each slot takes the highest Priority, tie by the newest post. Expired Activities
//! stay registered until `expire`, but never reach a Frame, so the primary returns on its own.

#![cfg_attr(
    not(test),
    expect(dead_code, reason = "IslandService wires the Arbiter in, #20")
)]

use std::collections::HashMap;
use std::time::Instant;

use super::activity::{Activity, Frame, Id, Lifetime};

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

    // the next expiry after now, when the Frame changes on its own
    pub fn deadline(&self, now: Instant) -> Option<Instant> {
        self.activities
            .values()
            .filter(|entry| entry.live(now))
            .filter_map(Entry::expiry)
            .min()
    }

    pub fn frame(&self, now: Instant) -> Frame {
        let top = |transient: bool| {
            self.activities
                .values()
                .filter(|entry| entry.live(now) && entry.expiry().is_some() == transient)
                .max_by_key(|entry| (entry.activity.priority(), entry.post))
                .map(|entry| entry.activity.clone())
        };

        Frame {
            primary: top(false),
            satellites: Vec::new(),
            transient: top(true),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::island::activity::{Kind, Priority};

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
}
