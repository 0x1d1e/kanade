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

impl Kind {
    pub const ALL: [Kind; 11] = [
        Kind::Media,
        Kind::Notification,
        Kind::Volume,
        Kind::Brightness,
        Kind::Workspace,
        Kind::Battery,
        Kind::Network,
        Kind::Bluetooth,
        Kind::ScreenCast,
        Kind::Timer,
        Kind::Privacy,
    ];

    // as IPC names it
    pub fn name(self) -> &'static str {
        match self {
            Kind::Media => "media",
            Kind::Notification => "notification",
            Kind::Volume => "volume",
            Kind::Brightness => "brightness",
            Kind::Workspace => "workspace",
            Kind::Battery => "battery",
            Kind::Network => "network",
            Kind::Bluetooth => "bluetooth",
            Kind::ScreenCast => "screen-cast",
            Kind::Timer => "timer",
            Kind::Privacy => "privacy",
        }
    }

    pub fn parse(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

// lowest first, so Critical is the greatest
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    Passive,
    Media,

    // volume, brightness, workspace
    Osd,

    // screen cast, timer, low battery
    Ongoing,

    // a notification with actions
    Actionable,

    // critical battery, privacy, call
    Critical,
}

impl Priority {
    pub const ALL: [Priority; 6] = [
        Priority::Passive,
        Priority::Media,
        Priority::Osd,
        Priority::Ongoing,
        Priority::Actionable,
        Priority::Critical,
    ];

    // as IPC names it
    pub fn name(self) -> &'static str {
        match self {
            Priority::Passive => "passive",
            Priority::Media => "media",
            Priority::Osd => "osd",
            Priority::Ongoing => "ongoing",
            Priority::Actionable => "actionable",
            Priority::Critical => "critical",
        }
    }

    pub fn parse(name: &str) -> Option<Priority> {
        Priority::ALL
            .into_iter()
            .find(|priority| priority.name() == name)
    }
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
 * Kind-scoped, so keys of different Kinds never collide: notification 7 and timer 7
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

/*
 * what the source knows that the island draws, typed per Kind. Not identity: a repost with new
 * Detail replaces the Activity, so its small form crossfades to the new one
 */
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Detail {
    // drawn as its Kind and key, as fake Activities are
    #[default]
    None,

    Media(Track),

    Volume(Volume),

    // 0 to 100
    Brightness(u8),

    Battery(Charge),

    Workspace(Workspace),
}

impl Detail {
    // the one Kind it can describe, none for any
    fn kind(&self) -> Option<Kind> {
        match self {
            Detail::None => None,
            Detail::Media(_) => Some(Kind::Media),
            Detail::Volume(_) => Some(Kind::Volume),
            Detail::Brightness(_) => Some(Kind::Brightness),
            Detail::Battery(_) => Some(Kind::Battery),
            Detail::Workspace(_) => Some(Kind::Workspace),
        }
    }

    /*
     * a level that moves rather than a new thing to show, so a repost of its Activity redraws it
     * where it stands: a held volume key slides one bar, a draining battery ticks its number, a
     * workspace switch moves the pager's mark, where a new track crossfades
     */
    pub fn is_level(&self) -> bool {
        matches!(
            self,
            Detail::Volume(_) | Detail::Brightness(_) | Detail::Battery(_) | Detail::Workspace(_)
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Track {
    pub title: String,

    // empty when the player names none
    pub artist: String,

    // a local file, the only art Amane can draw
    pub art: Option<String>,

    pub playing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Volume {
    pub device: Device,

    // 0 to 100
    pub percent: u8,

    pub muted: bool,
}

// the default sound device a Volume is for
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    Speaker,
    Microphone,
}

// a battery running low, from where it shows until the charger goes in
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Charge {
    // 0 to 100
    pub percent: u8,

    // low enough to preempt, drawn red rather than amber
    pub critical: bool,
}

// the workspace focused on its output, as niri numbers it there
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    // 1 based, among the workspaces on its output
    pub index: u32,

    // how many workspaces its output has, niri's empty one at the end included
    pub count: u32,

    // none unless named in niri's config
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    id: Id,
    priority: Priority,
    lifetime: Lifetime,
    actions: Vec<Action>,
    detail: Detail,
}

impl Activity {
    pub fn persistent(id: Id, priority: Priority) -> Activity {
        Activity {
            id,
            priority,
            lifetime: Lifetime::Persistent,
            actions: Vec::new(),
            detail: Detail::None,
        }
    }

    pub fn transient(id: Id, priority: Priority, duration: Duration) -> Activity {
        Activity {
            id,
            priority,
            lifetime: Lifetime::Transient(duration),
            actions: Vec::new(),
            detail: Detail::None,
        }
    }

    pub fn with_actions(mut self, actions: Vec<Action>) -> Activity {
        self.actions = actions;
        self
    }

    // a Detail of another Kind is a source's bug, caught here rather than drawn wrong
    pub fn with_detail(mut self, detail: Detail) -> Activity {
        assert!(
            detail.kind().is_none_or(|kind| kind == self.kind()),
            "{detail:?} does not describe a {:?} Activity",
            self.kind()
        );

        self.detail = detail;
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

    pub fn detail(&self) -> &Detail {
        &self.detail
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

// what the Arbiter decides to show on one island
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frame {
    pub primary: Option<Activity>,

    // Ongoing and Critical Persistent Activities beside the primary, highest first, bounded
    pub satellites: Vec<Activity>,

    // the Satellites past the bound, shown only as a count
    pub overflow: usize,

    pub transient: Option<Activity>,

    // Transients kept off an open Surface, highest first, shown as a badge
    pub queued: Vec<Activity>,
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

    #[test]
    fn detail_belongs_to_its_kind() {
        let media = Activity::persistent(Id::new(Kind::Media, "mpv"), Priority::Media);
        let track = Detail::Media(Track {
            title: String::from("Song"),
            ..Track::default()
        });

        assert_eq!(media.detail(), &Detail::None);
        assert_eq!(media.clone().with_detail(track.clone()).detail(), &track);
        assert_ne!(media.clone().with_detail(track.clone()), media);
    }

    #[test]
    fn only_levels_move_in_place() {
        let volume = Detail::Volume(Volume {
            device: Device::Speaker,
            percent: 40,
            muted: false,
        });

        assert!(volume.is_level());
        assert!(Detail::Brightness(70).is_level());
        assert!(
            Detail::Battery(Charge {
                percent: 15,
                critical: false
            })
            .is_level()
        );
        assert!(
            Detail::Workspace(Workspace {
                index: 2,
                count: 3,
                name: None
            })
            .is_level()
        );
        assert!(!Detail::Media(Track::default()).is_level());
        assert!(!Detail::None.is_level());
    }

    #[test]
    #[should_panic(expected = "does not describe")]
    fn detail_of_another_kind_is_refused() {
        let _ = Activity::persistent(Id::new(Kind::Battery, "BAT0"), Priority::Critical)
            .with_detail(Detail::Media(Track::default()));
    }

    #[test]
    fn names_parse_back() {
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.name()), Some(kind));
        }

        for priority in Priority::ALL {
            assert_eq!(Priority::parse(priority.name()), Some(priority));
        }

        assert_eq!(Kind::parse("ScreenCast"), None);
        assert_eq!(Priority::parse(""), None);
    }
}
