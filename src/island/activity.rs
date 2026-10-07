//! Activities and the Frame the Arbiter makes of them (CONTEXT.md, plan 5.1). Pure data.
//!
//! Lifetime, Priority, Scope and Interrupt are set apart and none follows from another (ADR 0009).
//! `Activity::new` refuses only what cannot be carried out, never an unusual combination.

#![expect(dead_code, reason = "the Arbiter and the sources use these, #17-#33")]

use super::fade::InPlace;
use super::presentation::Surface;
use std::time::{Duration, Instant};

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
    Timer,
    Screenshot,
    Recording,
    Caffeine,
}

impl Kind {
    pub const ALL: [Kind; 12] = [
        Kind::Media,
        Kind::Notification,
        Kind::Volume,
        Kind::Brightness,
        Kind::Workspace,
        Kind::Battery,
        Kind::Network,
        Kind::Bluetooth,
        Kind::Timer,
        Kind::Screenshot,
        Kind::Recording,
        Kind::Caffeine,
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
            Kind::Timer => "timer",
            Kind::Screenshot => "screenshot",
            Kind::Recording => "recording",
            Kind::Caffeine => "caffeine",
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

    // a workspace switch
    Osd,

    // timer, low battery
    Ongoing,

    // a notification with actions
    Actionable,

    // critical battery, call
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

// whether and how an Activity pushes in front of what the island shows
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interrupt {
    // takes its place by priority and interrupts nothing
    None,

    // displaces a Surface the user opened (plan 5.1 rule 4)
    Preempt,

    // opens the Activity's own Surface for this long, then the islands show what they did before
    AutoExpand(Duration),
}

// a policy that cannot be carried out, refused by `Activity::new`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidActivity {
    // its Kind has no Surface of its own to expand into
    AutoExpandWithoutSurface,

    // expired as it was posted
    ZeroLifetime,
}

impl std::fmt::Display for InvalidActivity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str(match self {
            InvalidActivity::AutoExpandWithoutSurface => {
                "auto-expand needs a Kind with a Surface of its own"
            }
            InvalidActivity::ZeroLifetime => "a Transient lifetime must be longer than zero",
        })
    }
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

    pub fn kind(&self) -> Kind {
        self.kind
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
 * Detail replaces the Activity, so its small form crossfades to the new one unless it stays in
 * place (`InPlace for Content`)
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

    Notification(Toast),

    Timer(Countdown),

    Screenshot(Shot),

    Recording(Clip),

    Caffeine(Awake),
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
            Detail::Notification(_) => Some(Kind::Notification),
            Detail::Timer(_) => Some(Kind::Timer),
            Detail::Screenshot(_) => Some(Kind::Screenshot),
            Detail::Recording(_) => Some(Kind::Recording),
            Detail::Caffeine(_) => Some(Kind::Caffeine),
        }
    }

    /*
     * a level that moves rather than a new thing to show, so a repost of its Activity redraws it
     * where it stands: a held volume key slides one bar, a draining battery ticks its number, a
     * workspace switch moves the pager's mark. A new track stays in place too, but dissolves
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

// the same song played or paused redraws its mark where it stands; another one dissolves in
impl InPlace for Track {
    fn in_place(&self, next: &Track) -> bool {
        self.title == next.title && self.artist == next.artist && self.art == next.art
    }
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

// a notification as its toast shows it, plain text
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Toast {
    // the sender, like "Firefox"; may be empty
    pub app: String,

    // the one-line title, the app's name when the sender gave none
    pub summary: String,

    // markup and line breaks taken out; may be empty
    pub body: String,

    // a local file, like a sender's avatar
    pub image: Option<String>,
}

// a screenshot niri saved
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shot {
    // absolute
    pub path: String,

    // its path is on the clipboard, by the Activity's action
    pub copied: bool,
}

// a screen recording Kanade started, at its file's absolute path
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clip {
    // still recording the output named
    Recording { path: String, output: String },

    // finished; `copied` once its path is on the clipboard, by the Activity's action
    Saved { path: String, copied: bool },

    // ended without saving anything, for why
    Failed { path: String, why: String },
}

impl Clip {
    pub fn path(&self) -> &str {
        match self {
            Clip::Recording { path, .. } | Clip::Saved { path, .. } | Clip::Failed { path, .. } => {
                path
            }
        }
    }
}

// caffeine keeping the session from going idle
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Awake {
    /*
     * on, for `length` when one was asked for, else until turned off; `serial` names this time it
     * was turned on, so a stale Turn off never turns off a newer one
     */
    On {
        serial: String,
        length: Option<Duration>,
    },

    // ended without being turned off, for why
    Failed {
        why: String,
    },
}

// how the machine reaches the network, as NetworkManager's primary connection says
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Uplink {
    Wired,

    // the network's name
    Wifi(String),

    // a VPN, a tethered phone and the rest, by the connection's own name
    Other(String),
}

/*
 * a timer running out at `ends`, started for `length`. What it reads follows from the time it is
 * drawn at, so the Activity never changes while it counts down
 */
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Countdown {
    pub ends: Instant,
    pub length: Duration,

    // paused then, so what is left stays what it was then
    pub paused: Option<Instant>,
}

impl Countdown {
    pub fn new(length: Duration, now: Instant) -> Countdown {
        Countdown {
            ends: now + length,
            length,
            paused: None,
        }
    }

    pub fn left(&self, now: Instant) -> Duration {
        self.ends
            .saturating_duration_since(self.paused.unwrap_or(now))
    }

    // what is left stops going down; a paused one stays as it is
    pub fn pause(self, now: Instant) -> Countdown {
        Countdown {
            paused: self.paused.or(Some(now)),
            ..self
        }
    }

    // counts down again from what was left; a running one stays as it is
    pub fn resume(self, now: Instant) -> Countdown {
        match self.paused {
            Some(paused) => Countdown {
                ends: self.ends + now.saturating_duration_since(paused),
                paused: None,
                ..self
            },
            None => self,
        }
    }

    // the moment it runs out, none while paused
    pub fn running_out(&self) -> Option<Instant> {
        self.paused.is_none().then_some(self.ends)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    id: Id,
    priority: Priority,
    lifetime: Lifetime,
    scope: Scope,
    interrupt: Interrupt,
    actions: Vec<Action>,
    detail: Detail,
}

impl Activity {
    /*
     * every policy field set by the source that posts it. Unusual combinations stand, like a
     * Persistent FocusedOutput or a Critical that interrupts nothing (ADR 0009)
     */
    pub fn new(
        id: Id,
        priority: Priority,
        lifetime: Lifetime,
        scope: Scope,
        interrupt: Interrupt,
    ) -> Result<Activity, InvalidActivity> {
        if lifetime == Lifetime::Transient(Duration::ZERO) {
            return Err(InvalidActivity::ZeroLifetime);
        }

        if matches!(interrupt, Interrupt::AutoExpand(_)) && Surface::own(id.kind).is_none() {
            return Err(InvalidActivity::AutoExpandWithoutSurface);
        }

        Ok(Activity {
            id,
            priority,
            lifetime,
            scope,
            interrupt,
            actions: Vec::new(),
            detail: Detail::None,
        })
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

    pub fn scope(&self) -> Scope {
        self.scope
    }

    pub fn interrupt(&self) -> Interrupt {
        self.interrupt
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
}

// the two policies most tests need, so each reads as what it tests
#[cfg(test)]
pub mod fixture {
    use std::time::Duration;

    use super::{Activity, Id, Interrupt, Lifetime, Priority, Scope};

    // on every island, competing for the primary, interrupting nothing
    pub fn persistent(id: Id, priority: Priority) -> Activity {
        Activity::new(
            id,
            priority,
            Lifetime::Persistent,
            Scope::Global,
            Interrupt::None,
        )
        .expect("a Persistent that does not auto-expand is valid")
    }

    // on the focused island, competing for the primary, for `duration`
    pub fn shown(id: Id, priority: Priority, duration: Duration) -> Activity {
        Activity::new(
            id,
            priority,
            Lifetime::Transient(duration),
            Scope::FocusedOutput,
            Interrupt::None,
        )
        .expect("a shown Activity lasts a while")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OSD: Duration = Duration::from_millis(1200);

    #[test]
    fn a_track_played_or_paused_is_in_place_another_is_not() {
        let track = Track {
            title: "a".into(),
            artist: "x".into(),
            art: Some("/a.png".into()),
            playing: true,
        };
        let paused = Track {
            playing: false,
            ..track.clone()
        };

        assert!(track.in_place(&paused));

        for next in [
            Track {
                title: "b".into(),
                ..track.clone()
            },
            Track {
                artist: "y".into(),
                ..track.clone()
            },
            Track {
                art: None,
                ..track.clone()
            },
        ] {
            assert!(!track.in_place(&next), "{next:?}");
        }
    }

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

    fn new(
        kind: Kind,
        priority: Priority,
        lifetime: Lifetime,
        interrupt: Interrupt,
    ) -> Result<Activity, InvalidActivity> {
        Activity::new(
            Id::new(kind, "key"),
            priority,
            lifetime,
            Scope::Global,
            interrupt,
        )
    }

    #[test]
    fn auto_expand_needs_a_surface_of_its_own() {
        for kind in Kind::ALL {
            let activity = new(
                kind,
                Priority::Ongoing,
                Lifetime::Persistent,
                Interrupt::AutoExpand(OSD),
            );

            if matches!(kind, Kind::Media | Kind::Notification) {
                assert!(activity.is_ok(), "{kind:?}");
            } else {
                assert_eq!(
                    activity,
                    Err(InvalidActivity::AutoExpandWithoutSurface),
                    "{kind:?}"
                );
            }
        }
    }

    #[test]
    fn a_transient_lifetime_is_never_zero() {
        for interrupt in [Interrupt::None, Interrupt::Preempt] {
            assert_eq!(
                new(
                    Kind::Volume,
                    Priority::Osd,
                    Lifetime::Transient(Duration::ZERO),
                    interrupt
                ),
                Err(InvalidActivity::ZeroLifetime)
            );
        }

        assert!(
            new(
                Kind::Volume,
                Priority::Osd,
                Lifetime::Transient(Duration::from_nanos(1)),
                Interrupt::None
            )
            .is_ok()
        );
    }

    // no field derives another, so each is what the source set, however unusual (ADR 0009)
    #[test]
    fn unusual_combinations_stand() {
        let accepted = [
            (
                Priority::Media,
                Lifetime::Persistent,
                Scope::FocusedOutput,
                Interrupt::None,
            ),
            (
                Priority::Osd,
                Lifetime::Transient(OSD),
                Scope::Global,
                Interrupt::None,
            ),
            (
                Priority::Passive,
                Lifetime::Persistent,
                Scope::Global,
                Interrupt::Preempt,
            ),
            (
                Priority::Critical,
                Lifetime::Persistent,
                Scope::Global,
                Interrupt::None,
            ),
            (
                Priority::Passive,
                Lifetime::Transient(OSD),
                Scope::FocusedOutput,
                Interrupt::AutoExpand(OSD),
            ),
        ];

        for (priority, lifetime, scope, interrupt) in accepted {
            let activity = Activity::new(
                Id::new(Kind::Notification, "7"),
                priority,
                lifetime,
                scope,
                interrupt,
            )
            .unwrap_or_else(|invalid| {
                panic!("{priority:?} {lifetime:?} {scope:?} {interrupt:?}: {invalid}")
            });

            assert_eq!(activity.priority(), priority);
            assert_eq!(activity.lifetime(), lifetime);
            assert_eq!(activity.scope(), scope);
            assert_eq!(activity.interrupt(), interrupt);
        }
    }

    #[test]
    fn kind_comes_from_the_id() {
        let toast = fixture::shown(Id::new(Kind::Notification, "7"), Priority::Passive, OSD)
            .with_actions(vec![Action {
                key: String::from("reply"),
                label: String::from("Reply"),
            }]);

        assert_eq!(toast.kind(), Kind::Notification);
        assert_eq!(toast.actions().len(), 1);
    }

    #[test]
    fn detail_belongs_to_its_kind() {
        let media = fixture::persistent(Id::new(Kind::Media, "mpv"), Priority::Media);
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
        assert!(!Detail::Notification(Toast::default()).is_level());
        assert!(!Detail::None.is_level());
    }

    #[test]
    #[should_panic(expected = "does not describe")]
    fn detail_of_another_kind_is_refused() {
        let _ = fixture::persistent(Id::new(Kind::Battery, "BAT0"), Priority::Critical)
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

        assert_eq!(Kind::parse("Timer"), None);
        assert_eq!(Priority::parse(""), None);
    }

    #[test]
    fn a_paused_countdown_keeps_what_was_left() {
        let start = Instant::now();
        let second = Duration::from_secs(1);
        let timer = Countdown::new(60 * second, start);

        assert_eq!(timer.left(start + 10 * second), 50 * second);
        assert_eq!(timer.running_out(), Some(start + 60 * second));

        let paused = timer.pause(start + 10 * second);

        assert_eq!(paused.left(start + 100 * second), 50 * second);
        assert_eq!(paused.running_out(), None);
        assert_eq!(paused.pause(start + 20 * second), paused);

        let resumed = paused.resume(start + 100 * second);

        assert_eq!(resumed.left(start + 110 * second), 40 * second);
        assert_eq!(resumed.running_out(), Some(start + 150 * second));
        assert_eq!(resumed.resume(start + 120 * second), resumed);
        assert_eq!(resumed.length, 60 * second);
    }
}
