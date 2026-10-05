//! Activities and the Frame the Arbiter makes of them (CONTEXT.md, plan 5.1). Pure data.
//!
//! Scope and Interrupt follow from Lifetime and Priority, so an Activity cannot carry a
//! combination the plan rules out, such as a Global Transient or a Passive that preempts.

#![expect(dead_code, reason = "the Arbiter and the sources use these, #17-#33")]

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Media,
    Notification,
    Volume,
    Brightness,
    Workspace,
    Battery,
    Network,
    Bluetooth,
    ScreenCast,
    Timer,
    Privacy,
}

// lowest first, so Critical is the greatest
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    Passive,
    Media,

    // volume, brightness, workspace
    Osd,

    // screen cast, timer
    Ongoing,

    // a notification with actions
    Actionable,

    // low battery, privacy, call
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifetime {
    // until withdrawn
    Persistent,

    // expires on its own, counted from the latest post
    Transient(Duration),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    FocusedOutput,
}

// how far an Activity may push in front of what the island shows; each level includes the ones before
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Interrupt {
    // takes its place by priority, never over the primary
    Never,

    // shows over the primary for its Lifetime, then the primary returns
    Transient,

    // may also displace a Surface the user opened (plan 5.1 rule 4)
    Preempt,
}

/*
 * Kind-scoped, so a source picks keys without knowing the others': notification 7 and timer 7
 * stay two Activities. Posting the same Id again replaces the Activity
 */
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Id {
    kind: Kind,
    key: String,
}

impl Id {
    pub fn new(kind: Kind, key: impl Into<String>) -> Id {
        Id {
            kind,
            key: key.into(),
        }
    }

    pub fn key(&self) -> &str {
        &self.key
    }
}

// the source that posted the Activity runs it by key
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    id: Id,
    priority: Priority,
    lifetime: Lifetime,
    actions: Vec<Action>,
}

impl Activity {
    pub fn persistent(id: Id, priority: Priority) -> Activity {
        Activity {
            id,
            priority,
            lifetime: Lifetime::Persistent,
            actions: Vec::new(),
        }
    }

    pub fn transient(id: Id, priority: Priority, duration: Duration) -> Activity {
        Activity {
            id,
            priority,
            lifetime: Lifetime::Transient(duration),
            actions: Vec::new(),
        }
    }

    pub fn with_actions(mut self, actions: Vec<Action>) -> Activity {
        self.actions = actions;
        self
    }

    pub fn id(&self) -> &Id {
        &self.id
    }

    pub fn kind(&self) -> Kind {
        self.id.kind
    }

    pub fn priority(&self) -> Priority {
        self.priority
    }

    pub fn lifetime(&self) -> Lifetime {
        self.lifetime
    }

    pub fn actions(&self) -> &[Action] {
        &self.actions
    }

    // Transients are for the output the user is looking at
    pub fn scope(&self) -> Scope {
        match self.lifetime {
            Lifetime::Persistent => Scope::Global,
            Lifetime::Transient(_) => Scope::FocusedOutput,
        }
    }

    // only Critical preempts, so a toast never steals a Surface the user is using
    pub fn interrupt(&self) -> Interrupt {
        match (self.priority, self.lifetime) {
            (Priority::Critical, _) => Interrupt::Preempt,
            (_, Lifetime::Transient(_)) => Interrupt::Transient,
            (_, Lifetime::Persistent) => Interrupt::Never,
        }
    }
}

// what the Arbiter decides to show, before it is filtered per island by Scope
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frame {
    pub primary: Option<Activity>,

    // Persistent Activities beside the primary, bounded (#18)
    pub satellites: Vec<Activity>,

    pub transient: Option<Activity>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const OSD: Duration = Duration::from_millis(1200);

    #[test]
    fn priority_orders_critical_first() {
        let mut priorities = vec![
            Priority::Media,
            Priority::Critical,
            Priority::Passive,
            Priority::Actionable,
            Priority::Osd,
            Priority::Ongoing,
        ];
        priorities.sort_by(|a, b| b.cmp(a));

        assert_eq!(
            priorities,
            [
                Priority::Critical,
                Priority::Actionable,
                Priority::Ongoing,
                Priority::Osd,
                Priority::Media,
                Priority::Passive,
            ]
        );
    }

    #[test]
    fn identity_is_kind_scoped() {
        assert_ne!(Id::new(Kind::Notification, "7"), Id::new(Kind::Timer, "7"));
        assert_eq!(
            Id::new(Kind::Notification, "7"),
            Id::new(Kind::Notification, String::from("7"))
        );
    }

    #[test]
    fn transients_are_for_the_focused_output() {
        let volume = Activity::transient(Id::new(Kind::Volume, "volume"), Priority::Osd, OSD);
        let media = Activity::persistent(Id::new(Kind::Media, "spotify"), Priority::Media);

        assert_eq!(volume.scope(), Scope::FocusedOutput);
        assert_eq!(media.scope(), Scope::Global);
    }

    #[test]
    fn only_critical_preempts() {
        let battery = Activity::persistent(Id::new(Kind::Battery, "BAT0"), Priority::Critical);
        let call = Activity::transient(Id::new(Kind::Privacy, "call"), Priority::Critical, OSD);
        let toast = Activity::transient(
            Id::new(Kind::Notification, "7"),
            Priority::Actionable,
            Duration::from_secs(5),
        );
        let cast = Activity::persistent(Id::new(Kind::ScreenCast, "cast"), Priority::Ongoing);

        assert_eq!(battery.interrupt(), Interrupt::Preempt);
        assert_eq!(call.interrupt(), Interrupt::Preempt);
        assert_eq!(toast.interrupt(), Interrupt::Transient);
        assert_eq!(cast.interrupt(), Interrupt::Never);

        // a Critical Transient also shows over the primary
        assert!(call.interrupt() >= Interrupt::Transient);
    }

    #[test]
    fn kind_comes_from_the_id() {
        let toast = Activity::transient(Id::new(Kind::Notification, "7"), Priority::Passive, OSD)
            .with_actions(vec![Action {
                key: String::from("reply"),
                label: String::from("Reply"),
            }]);

        assert_eq!(toast.kind(), Kind::Notification);
        assert_eq!(toast.actions().len(), 1);
    }
}
