//! Screen capture (plan 5.3, 7): niri only says a cast exists, not why, so every cast is one
//! ScreenCast Activity, never a "Recording" or "Sharing". It shows from the first cast until the
//! last one stops, and a lost niri stream takes it away, since nobody can say any more.
//!
//! niri.rs follows niri's EventStream and says whether anything casts; this decides what the island
//! makes of it.

use super::niri::{Change, Seen};
use crate::island::activity::{Activity, Id, Kind, Priority};

// only the first cast starting or the last one stopping changes what the island shows
pub fn change(before: &Seen, now: &Seen) -> Option<Change> {
    match (before.casting, now.casting) {
        (false, true) => Some(Change::Post(activity())),
        (true, false) => Some(Change::Withdraw(id())),
        _ => None,
    }
}

// Ongoing, so it becomes a Satellite beside a higher primary, and the primary over Media
fn activity() -> Activity {
    Activity::persistent(id(), Priority::Ongoing)
}

// one, however many casts there are
fn id() -> Id {
    Id::new(Kind::ScreenCast, "cast")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::activity::{Interrupt, Lifetime};

    fn casting(casting: bool) -> Seen {
        Seen {
            casting,
            ..Seen::default()
        }
    }

    #[test]
    fn the_first_cast_posts_one_persistent_screen_cast() {
        let Some(Change::Post(activity)) = change(&casting(false), &casting(true)) else {
            panic!("no post");
        };

        assert_eq!(activity.id(), &id());
        assert_eq!(activity.lifetime(), Lifetime::Persistent);
        assert_eq!(activity.priority(), Priority::Ongoing);
        assert_eq!(activity.interrupt(), Interrupt::Never);
    }

    #[test]
    fn the_last_cast_stopping_withdraws_it() {
        assert_eq!(
            change(&casting(true), &casting(false)),
            Some(Change::Withdraw(id()))
        );
    }

    // another cast, or one that changes, is still the same capture
    #[test]
    fn casting_on_changes_nothing() {
        assert_eq!(change(&casting(true), &casting(true)), None);
        assert_eq!(change(&casting(false), &casting(false)), None);
    }
}
