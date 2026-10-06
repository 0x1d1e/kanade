//! Per-island Presentation (CONTEXT.md, plan 5.2). Pure: primary changes and input in, Presentation out.
//!
//! Each island keeps its primary and what the user raised it to: a Peek or an open Surface, pinned
//! or not. Rest or Compact follows from the primary alone.

use std::collections::HashMap;

use super::activity::{Activity, Connection, Detail, Device, Kind, Priority, Uplink, Volume};
use super::fade::InPlace;

// full interactive content of an Expanded island
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Media,
    Notifications,
    Controls,
    Launcher,
}

impl Surface {
    pub const ALL: [Surface; 4] = [
        Surface::Media,
        Surface::Notifications,
        Surface::Controls,
        Surface::Launcher,
    ];

    // as IPC names it
    pub fn name(self) -> &'static str {
        match self {
            Surface::Media => "media",
            Surface::Notifications => "notifications",
            Surface::Controls => "controls",
            Surface::Launcher => "launcher",
        }
    }

    pub fn parse(name: &str) -> Option<Surface> {
        Surface::ALL
            .into_iter()
            .find(|surface| surface.name() == name)
    }

    // what a click on an island showing this Kind opens; a Kind without a Surface of its own has its control there
    pub fn of(kind: Kind) -> Surface {
        Surface::own(kind).unwrap_or(Surface::Controls)
    }

    // the Surface a Kind is also, the only one an Activity may open itself (`Interrupt::AutoExpand`)
    pub fn own(kind: Kind) -> Option<Surface> {
        match kind {
            Kind::Media => Some(Surface::Media),
            Kind::Notification => Some(Surface::Notifications),
            _ => None,
        }
    }

    /*
     * whether this Surface already shows what the Activity says, like the speaker volume the Media
     * Surface sets: a Transient saying it again would only wait as a badge
     */
    pub fn shows(self, activity: &Activity) -> bool {
        match (self, activity.detail()) {
            (
                Surface::Media,
                Detail::Volume(Volume {
                    device: Device::Speaker,
                    ..
                }),
            ) => true,

            // its history lists every notification; a Critical one still preempts (plan 5.1 rule 4)
            (Surface::Notifications, Detail::Notification(_)) => {
                activity.priority() != Priority::Critical
            }

            // both levels, the microphone's mute, the Wi-Fi network and the Bluetooth devices
            (
                Surface::Controls,
                Detail::Volume(_)
                | Detail::Brightness(_)
                | Detail::Bluetooth(_)
                | Detail::Network(Connection {
                    uplink: Uplink::Wifi(_),
                    ..
                }),
            ) => true,

            _ => false,
        }
    }
}

// Expanded always carries a Surface, there is no surface-less expanded form
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Presentation {
    #[default]
    Rest,
    Compact,
    Peek,
    Expanded(Surface),
}

// what the body shows: the Presentation, and in a small form the Activity it is the form of
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Content {
    pub presentation: Presentation,
    pub activity: Option<Activity>,
}

impl Content {
    // an open Surface shows itself, not the Activity, and Rest shows the time, not an Activity
    pub fn new(presentation: Presentation, shown: Option<Activity>) -> Self {
        let small = matches!(presentation, Presentation::Compact | Presentation::Peek);

        Self {
            presentation,
            activity: shown.filter(|_| small),
        }
    }
}

/*
 * the same Activity in the same form with its level moved redraws where it stands, and a new track
 * dissolves where it stands (`Dissolve`); a level that first appears is new content, so it
 * crossfades in
 */
impl InPlace for Content {
    fn in_place(&self, next: &Content) -> bool {
        let moved = match (&self.activity, &next.activity) {
            (Some(shown), Some(next)) => {
                shown.id() == next.id()
                    && match (shown.detail(), next.detail()) {
                        (Detail::Media(_), Detail::Media(_)) => true,
                        (shown, next) => shown.is_level() && next.is_level(),
                    }
            }
            (shown, next) => shown == next,
        };

        self.presentation == next.presentation && moved
    }
}

// Wheel carries a scroll delta, so no Eq
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Input {
    // left click on the body
    Click,

    // pins or unpins a Peek or an open Surface, peeking pinned from Compact
    RightClick,

    // routed but meaning nothing yet: the wheel belongs to the Surface under it (#27); positive
    // scrolls down
    Wheel(f32),

    // IPC or keybind asking for a Surface
    Open(Surface),

    // Escape, pointer out after the grace, IPC collapse
    Collapse,

    // the pointer stayed on the island for the hover delay, then off it for the grace
    Hover,
    Unhover,

    // a Critical Activity displaces an open Surface (plan 5.1 rule 4)
    Preempt,
}

impl Input {
    // false for input no Presentation reacts to yet, so it neither ends a pending Peek or grace
    // nor needs writing at all
    pub fn decides(self) -> bool {
        !matches!(self, Input::Wheel(_))
    }
}

// what the user raised an island to, beyond what its primary Activity gives it
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Raised {
    Peek,
    Expanded(Surface),
}

#[derive(Debug, Clone, Default)]
struct Island {
    // the Surface of the island's primary Activity, none without one
    primary: Option<Surface>,

    // a Peek only ever exists with a primary, set_primary ends it on withdrawal
    raised: Option<Raised>,

    /*
     * the pointer leaving does not end what the user raised it to. Only ever with `raised`, and
     * any change to that ends it, so nothing pinned outlives the Peek or Surface it kept open
     */
    pinned: bool,
}

/*
 * what an `Interrupt::AutoExpand` changed, so `Presentations::restore` puts it back: the island it
 * opened on and the one it collapsed, as they were before the first of the AutoExpands pending
 */
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prior(Vec<(String, Option<Raised>, bool)>);

impl Prior {
    // the islands either changed, each as `self` found it, the earlier
    pub fn and(mut self, later: Prior) -> Prior {
        for island in later.0 {
            if !self.0.iter().any(|(monitor, ..)| *monitor == island.0) {
                self.0.push(island);
            }
        }

        self
    }

    // the islands it puts back
    pub fn monitors(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(monitor, ..)| monitor.as_str())
    }
}

// every island's Presentation by monitor
#[derive(Debug, Default)]
pub struct Presentations {
    islands: HashMap<String, Island>,

    // every island no input reached yet: one primary for all of them, never raised
    untouched: Island,

    // niri's overview is open, every island rests and takes no input (plan 5.3)
    overview: bool,

    // how many times a Surface opened, so state kept for one opening ends with it
    visits: u64,
}

impl Presentations {
    pub fn get(&self, monitor: &str) -> Presentation {
        self.of(self.islands.get(monitor).unwrap_or(&self.untouched))
    }

    pub fn pinned(&self, monitor: &str) -> bool {
        !self.overview
            && self
                .islands
                .get(monitor)
                .is_some_and(|island| island.pinned)
    }

    // what every untouched island shows
    pub fn untouched(&self) -> Presentation {
        self.of(&self.untouched)
    }

    fn of(&self, island: &Island) -> Presentation {
        if self.overview {
            return Presentation::Rest;
        }

        match (island.raised, island.primary) {
            (Some(Raised::Expanded(surface)), _) => Presentation::Expanded(surface),
            (Some(Raised::Peek), _) => Presentation::Peek,
            (None, Some(_)) => Presentation::Compact,
            (None, None) => Presentation::Rest,
        }
    }

    /*
     * Rest --Activity posted--> Compact and back on withdrawal. A withdrawal also ends a Peek,
     * which showed that primary, so a later post starts at Compact again. An open Surface stays
     */
    pub fn set_primary(&mut self, monitor: &str, primary: Option<Surface>) {
        self.island(monitor).set_primary(primary);
    }

    // for every island not touched yet, which then starts from it
    pub fn set_untouched(&mut self, primary: Option<Surface>) {
        self.untouched.set_primary(primary);
    }

    /*
     * opening collapses every open Surface and Peek for good; primaries stay, so Compact comes back
     * on close (preemption never destroys)
     */
    pub fn set_overview(&mut self, open: bool) {
        self.overview = open;

        if open {
            for island in self.islands.values_mut() {
                island.raise(None);
            }
        }
    }

    pub fn overview(&self) -> bool {
        self.overview
    }

    pub fn input(&mut self, monitor: &str, input: Input) {
        if self.overview {
            return;
        }

        let now = self.get(monitor);
        let island = self.island(monitor);

        match (input, now) {
            // a click inside an open Surface belongs to the Surface
            (Input::Click, Presentation::Expanded(_)) => {}

            // the primary's Surface; Rest has no primary and no remembered last Surface, so Controls
            (Input::Click, _) => {
                let surface = island.primary.unwrap_or(Surface::Controls);
                self.expand(monitor, surface);
            }

            (Input::Open(surface), _) => self.expand(monitor, surface),

            (Input::Collapse | Input::Preempt, Presentation::Expanded(_)) => island.raise(None),

            // a pinned Peek waits for Escape or another right click, not for the pointer
            (Input::Collapse, Presentation::Peek) if island.pinned => island.raise(None),
            (Input::Unhover, Presentation::Peek) if !island.pinned => island.raise(None),

            // only an island with a primary has a larger small form to peek into
            (Input::Hover, Presentation::Compact) => island.raise(Some(Raised::Peek)),

            // Rest has nothing to keep open, and a click there already opens Controls
            (Input::RightClick, Presentation::Compact) => {
                island.raise(Some(Raised::Peek));
                island.pinned = true;
            }
            (Input::RightClick, Presentation::Peek | Presentation::Expanded(_)) => {
                island.pinned = !island.pinned;
            }

            _ => {}
        }
    }

    // at most one island is Expanded, opening one collapses any other
    fn expand(&mut self, monitor: &str, surface: Surface) {
        for island in self.islands.values_mut() {
            if matches!(island.raised, Some(Raised::Expanded(_))) {
                island.raise(None);
            }
        }

        self.island(monitor).raise(Some(Raised::Expanded(surface)));
        self.visits += 1;
    }

    /*
     * opens `surface` without the user (`Interrupt::AutoExpand`), returning what it changed;
     * nothing while the overview is open or the island already shows it
     */
    pub fn auto_expand(&mut self, monitor: &str, surface: Surface) -> Option<Prior> {
        if self.overview || self.get(monitor) == Presentation::Expanded(surface) {
            return None;
        }

        self.island(monitor);

        let prior = self
            .islands
            .iter()
            .filter(|&(name, island)| {
                name == monitor || matches!(island.raised, Some(Raised::Expanded(_)))
            })
            .map(|(name, island)| (name.clone(), island.raised, island.pinned))
            .collect();

        self.expand(monitor, surface);

        Some(Prior(prior))
    }

    /*
     * puts back what `auto_expand` changed, a Surface reopening as a new visit. A Peek whose
     * primary was withdrawn meanwhile stays ended, as `set_primary` would have ended it
     */
    pub fn restore(&mut self, prior: Prior) {
        if self.overview {
            return;
        }

        for (monitor, raised, pinned) in prior.0 {
            let island = self.island(&monitor);
            let raised =
                raised.filter(|&raised| raised != Raised::Peek || island.primary.is_some());

            island.raised = raised;
            island.pinned = pinned && raised.is_some();

            if matches!(raised, Some(Raised::Expanded(_))) {
                self.visits += 1;
            }
        }
    }

    /*
     * this opening of a Surface, new each time one opens or another replaces it, so a Surface's
     * focus and scroll last one visit without anything resetting them on close
     */
    pub fn visit(&self) -> u64 {
        self.visits
    }

    // the one Expanded island and its Surface, if any
    pub fn expanded(&self) -> Option<(&str, Surface)> {
        self.islands
            .keys()
            .find_map(|monitor| match self.get(monitor) {
                Presentation::Expanded(surface) => Some((monitor.as_str(), surface)),
                _ => None,
            })
    }

    // the first touch starts from what every untouched island shows
    fn island(&mut self, monitor: &str) -> &mut Island {
        self.islands
            .entry(monitor.to_owned())
            .or_insert_with(|| self.untouched.clone())
    }
}

impl Island {
    fn set_primary(&mut self, primary: Option<Surface>) {
        self.primary = primary;

        if primary.is_none() && self.raised == Some(Raised::Peek) {
            self.raise(None);
        }
    }

    // a pin keeps one Peek or one Surface open, so a new one, or none, starts unpinned
    fn raise(&mut self, raised: Option<Raised>) {
        self.raised = raised;
        self.pinned = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::activity::fixture;

    use Presentation::{Compact, Expanded, Peek, Rest};
    use Surface::{Controls, Launcher, Media, Notifications};

    const MONITOR: &str = "eDP-1";
    const OTHER: &str = "HDMI-A-1";

    // the island on MONITOR after these inputs, with `primary` throughout
    fn after(inputs: &[Input], primary: Option<Surface>) -> Presentation {
        let mut presentations = Presentations::default();

        presentations.set_primary(MONITOR, primary);

        for &input in inputs {
            presentations.input(MONITOR, input);
        }

        presentations.get(MONITOR)
    }

    #[test]
    fn media_and_controls_show_the_speaker_volume() {
        use super::super::activity::{Id, Priority};

        let level = |device| {
            fixture::shown(
                Id::new(Kind::Volume, "volume"),
                Priority::Osd,
                std::time::Duration::from_secs(1),
            )
            .with_detail(Detail::Volume(Volume {
                device,
                percent: 40,
                muted: false,
            }))
        };

        assert!(Media.shows(&level(Device::Speaker)));
        assert!(!Media.shows(&level(Device::Microphone)));
        assert!(Controls.shows(&level(Device::Speaker)));
        assert!(Controls.shows(&level(Device::Microphone)));
        assert!(!Notifications.shows(&level(Device::Speaker)));
    }

    #[test]
    fn controls_shows_wifi_and_bluetooth_but_not_other_uplinks() {
        use super::super::activity::{Id, Peer, Priority};

        let transient = |kind, detail| {
            fixture::shown(
                Id::new(kind, "key"),
                Priority::Passive,
                std::time::Duration::from_secs(2),
            )
            .with_detail(detail)
        };
        let network = |uplink| {
            transient(
                Kind::Network,
                Detail::Network(Connection {
                    uplink,
                    connected: true,
                }),
            )
        };

        assert!(Controls.shows(&network(Uplink::Wifi("home".into()))));
        assert!(!Controls.shows(&network(Uplink::Wired)));
        assert!(!Controls.shows(&network(Uplink::Other("vpn".into()))));
        assert!(Controls.shows(&transient(
            Kind::Bluetooth,
            Detail::Bluetooth(Peer {
                path: "/org/bluez/hci0/dev_buds".into(),
                name: "buds".into(),
                connected: true,
                battery: None,
            })
        )));
        assert!(Controls.shows(&transient(Kind::Brightness, Detail::Brightness(40))));
        assert!(!Media.shows(&transient(Kind::Brightness, Detail::Brightness(40))));
    }

    #[test]
    fn the_notifications_surface_shows_every_toast_but_a_critical_one() {
        use super::super::activity::{Id, Toast};

        let toast = |priority| {
            fixture::shown(
                Id::new(Kind::Notification, "7"),
                priority,
                std::time::Duration::from_secs(5),
            )
            .with_detail(Detail::Notification(Toast::default()))
        };

        assert!(Notifications.shows(&toast(Priority::Passive)));
        assert!(Notifications.shows(&toast(Priority::Actionable)));
        assert!(!Notifications.shows(&toast(Priority::Critical)));
        assert!(!Media.shows(&toast(Priority::Passive)));
    }

    #[test]
    fn every_opening_is_a_new_visit() {
        let mut presentations = Presentations::default();
        let first = presentations.visit();

        presentations.input(MONITOR, Input::Open(Notifications));
        let open = presentations.visit();
        assert_ne!(open, first);

        // a click inside keeps the visit, a Surface replacing it starts another
        presentations.input(MONITOR, Input::Click);
        assert_eq!(presentations.visit(), open);

        presentations.input(MONITOR, Input::Open(Launcher));
        assert_ne!(presentations.visit(), open);
        let launcher = presentations.visit();

        presentations.input(MONITOR, Input::Collapse);
        presentations.input(MONITOR, Input::Open(Notifications));
        assert_ne!(presentations.visit(), launcher);
    }

    #[test]
    fn untouched_island_rests_or_shows_its_primary() {
        assert_eq!(after(&[], None), Rest);
        assert_eq!(after(&[], Some(Media)), Compact);
    }

    #[test]
    fn activity_posted_and_withdrawn() {
        let mut presentations = Presentations::default();

        presentations.set_primary(MONITOR, Some(Media));
        assert_eq!(presentations.get(MONITOR), Compact);

        presentations.set_primary(MONITOR, None);
        assert_eq!(presentations.get(MONITOR), Rest);
    }

    #[test]
    fn rest_click_opens_controls() {
        assert_eq!(after(&[Input::Click], None), Expanded(Controls));
    }

    #[test]
    fn compact_or_peek_click_opens_the_primarys_surface() {
        assert_eq!(after(&[Input::Click], Some(Media)), Expanded(Media));

        assert_eq!(
            after(&[Input::Hover, Input::Click], Some(Notifications)),
            Expanded(Notifications)
        );
    }

    #[test]
    fn open_shows_the_requested_surface_from_anywhere() {
        assert_eq!(after(&[Input::Open(Launcher)], None), Expanded(Launcher));
        assert_eq!(
            after(&[Input::Open(Launcher)], Some(Media)),
            Expanded(Launcher)
        );

        assert_eq!(
            after(&[Input::Hover, Input::Open(Launcher)], Some(Media)),
            Expanded(Launcher)
        );

        // an open island switches Surface without collapsing first
        assert_eq!(
            after(&[Input::Click, Input::Open(Launcher)], Some(Media)),
            Expanded(Launcher)
        );
    }

    #[test]
    fn click_inside_an_open_surface_keeps_it() {
        assert_eq!(
            after(&[Input::Open(Launcher), Input::Click], Some(Media)),
            Expanded(Launcher)
        );
    }

    #[test]
    fn collapse_returns_to_compact_or_rest() {
        assert_eq!(after(&[Input::Click, Input::Collapse], None), Rest);
        assert_eq!(
            after(&[Input::Click, Input::Collapse], Some(Media)),
            Compact
        );
    }

    #[test]
    fn collapse_and_preempt_leave_small_forms_alone() {
        assert_eq!(after(&[Input::Collapse], None), Rest);
        assert_eq!(after(&[Input::Preempt], Some(Media)), Compact);
        assert_eq!(after(&[Input::Hover, Input::Collapse], Some(Media)), Peek);
        assert_eq!(after(&[Input::Hover, Input::Preempt], Some(Media)), Peek);
    }

    #[test]
    fn preempt_collapses_an_open_surface_to_compact() {
        assert_eq!(after(&[Input::Click, Input::Preempt], Some(Media)), Compact);
    }

    #[test]
    fn hover_peeks_only_from_compact() {
        assert_eq!(after(&[Input::Hover], Some(Media)), Peek);
        assert_eq!(after(&[Input::Hover, Input::Unhover], Some(Media)), Compact);

        // nothing to peek into at Rest, and an open Surface stays open
        assert_eq!(after(&[Input::Hover], None), Rest);
        assert_eq!(
            after(&[Input::Click, Input::Hover], Some(Media)),
            Expanded(Media)
        );
        assert_eq!(
            after(&[Input::Click, Input::Unhover], Some(Media)),
            Expanded(Media)
        );
    }

    #[test]
    fn wheel_changes_nothing_yet() {
        for setup in [&[][..], &[Input::Hover], &[Input::Click]] {
            for primary in [None, Some(Media)] {
                let before = after(setup, primary);

                for input in [Input::Wheel(1.0), Input::Wheel(-1.0)] {
                    let inputs = [setup, &[input]].concat();

                    assert_eq!(after(&inputs, primary), before, "{inputs:?} {primary:?}");
                }
            }
        }
    }

    // the island on MONITOR after these inputs with a Media primary, and whether it is pinned
    fn pinned_after(inputs: &[Input]) -> (Presentation, bool) {
        let mut presentations = Presentations::default();

        presentations.set_primary(MONITOR, Some(Media));

        for &input in inputs {
            presentations.input(MONITOR, input);
        }

        (presentations.get(MONITOR), presentations.pinned(MONITOR))
    }

    #[test]
    fn right_click_at_rest_does_nothing() {
        let mut presentations = Presentations::default();

        presentations.input(MONITOR, Input::RightClick);

        assert_eq!(presentations.get(MONITOR), Rest);
        assert!(!presentations.pinned(MONITOR));
    }

    #[test]
    fn right_click_peeks_pinned_from_compact() {
        assert_eq!(pinned_after(&[Input::RightClick]), (Peek, true));
        assert_eq!(
            pinned_after(&[Input::Hover, Input::RightClick]),
            (Peek, true)
        );
    }

    #[test]
    fn right_click_pins_and_unpins_an_open_surface() {
        assert_eq!(
            pinned_after(&[Input::Click, Input::RightClick]),
            (Expanded(Media), true)
        );
        assert_eq!(
            pinned_after(&[Input::Click, Input::RightClick, Input::RightClick]),
            (Expanded(Media), false)
        );
        assert_eq!(
            pinned_after(&[Input::RightClick, Input::RightClick]),
            (Peek, false)
        );
    }

    #[test]
    fn a_pinned_peek_outlasts_the_pointer_until_escape() {
        assert_eq!(
            pinned_after(&[Input::RightClick, Input::Unhover]),
            (Peek, true)
        );
        assert_eq!(
            pinned_after(&[Input::RightClick, Input::Collapse]),
            (Compact, false)
        );

        // unpinned, it is an ordinary Peek again
        assert_eq!(
            pinned_after(&[Input::RightClick, Input::RightClick, Input::Unhover]),
            (Compact, false)
        );
    }

    // no remembered state: whatever ends or replaces the pinned Peek or Surface ends the pin
    #[test]
    fn a_pin_ends_with_what_it_kept_open() {
        assert_eq!(
            pinned_after(&[Input::RightClick, Input::Click]),
            (Expanded(Media), false)
        );
        assert_eq!(
            pinned_after(&[Input::Click, Input::RightClick, Input::Open(Launcher)]),
            (Expanded(Launcher), false)
        );

        for end in [Input::Collapse, Input::Preempt] {
            let inputs = [Input::Click, Input::RightClick, end];

            assert_eq!(pinned_after(&inputs), (Compact, false), "{end:?}");
            assert_eq!(
                pinned_after(&[&inputs[..], &[Input::Click]].concat()),
                (Expanded(Media), false),
                "{end:?}"
            );
        }

        // a pinned Peek stays through a Critical, which only displaces a Surface
        assert_eq!(
            pinned_after(&[Input::RightClick, Input::Preempt]),
            (Peek, true)
        );
    }

    #[test]
    fn withdrawal_and_other_islands_end_a_pin() {
        let mut presentations = Presentations::default();

        presentations.set_primary(MONITOR, Some(Media));
        presentations.input(MONITOR, Input::RightClick);
        presentations.set_primary(MONITOR, None);

        assert_eq!(presentations.get(MONITOR), Rest);
        assert!(!presentations.pinned(MONITOR));

        presentations.set_primary(MONITOR, Some(Media));
        assert_eq!(presentations.get(MONITOR), Compact);

        // opening another island collapses a pinned one too
        presentations.input(MONITOR, Input::Click);
        presentations.input(MONITOR, Input::RightClick);
        presentations.input(OTHER, Input::Open(Launcher));

        assert_eq!(presentations.get(MONITOR), Compact);
        assert!(!presentations.pinned(MONITOR));
    }

    #[test]
    fn overview_ends_a_pin() {
        let mut presentations = Presentations::default();

        presentations.set_primary(MONITOR, Some(Media));
        presentations.input(MONITOR, Input::RightClick);
        presentations.set_overview(true);

        assert!(!presentations.pinned(MONITOR));

        // and right click does not pin under it
        presentations.input(MONITOR, Input::RightClick);
        presentations.set_overview(false);

        assert_eq!(presentations.get(MONITOR), Compact);
        assert!(!presentations.pinned(MONITOR));
    }

    #[test]
    fn withdrawn_primary_ends_a_peek_for_good() {
        let mut presentations = Presentations::default();

        presentations.set_primary(MONITOR, Some(Media));
        presentations.input(MONITOR, Input::Hover);
        assert_eq!(presentations.get(MONITOR), Peek);

        presentations.set_primary(MONITOR, None);
        assert_eq!(presentations.get(MONITOR), Rest);

        // Rest --Activity posted--> Compact, whichever Activity it is
        for primary in [Media, Notifications] {
            presentations.set_primary(MONITOR, Some(primary));
            assert_eq!(presentations.get(MONITOR), Compact);
        }
    }

    #[test]
    fn replaced_primary_keeps_a_peek() {
        let mut presentations = Presentations::default();

        presentations.set_primary(MONITOR, Some(Media));
        presentations.input(MONITOR, Input::Hover);
        presentations.set_primary(MONITOR, Some(Notifications));

        assert_eq!(presentations.get(MONITOR), Peek);

        presentations.input(MONITOR, Input::Click);
        assert_eq!(presentations.get(MONITOR), Expanded(Notifications));
    }

    #[test]
    fn open_surface_outlives_its_primary() {
        let mut presentations = Presentations::default();

        presentations.set_primary(MONITOR, Some(Media));
        presentations.input(MONITOR, Input::Click);
        presentations.set_primary(MONITOR, None);

        assert_eq!(presentations.get(MONITOR), Expanded(Media));

        presentations.input(MONITOR, Input::Collapse);
        assert_eq!(presentations.get(MONITOR), Rest);
    }

    #[test]
    fn no_remembered_last_surface() {
        assert_eq!(
            after(
                &[Input::Open(Launcher), Input::Collapse, Input::Click],
                None
            ),
            Expanded(Controls)
        );
    }

    #[test]
    fn overview_rests_every_island_until_it_closes() {
        let mut presentations = Presentations::default();

        presentations.set_primary(MONITOR, Some(Media));
        presentations.input(MONITOR, Input::Click);
        presentations.set_primary(OTHER, Some(Media));
        presentations.input(OTHER, Input::Hover);

        presentations.set_overview(true);

        assert_eq!(presentations.get(MONITOR), Rest);
        assert_eq!(presentations.get(OTHER), Rest);
        assert_eq!(presentations.get("DP-1"), Rest);

        // nothing opens, peeks or comes back while it is open
        for input in [Input::Click, Input::Open(Launcher), Input::Hover] {
            presentations.input(MONITOR, input);
            assert_eq!(presentations.get(MONITOR), Rest, "{input:?}");
        }

        // a primary posted meanwhile counts once it closes
        presentations.set_primary("DP-1", Some(Notifications));
        presentations.set_overview(false);

        assert_eq!(presentations.get(MONITOR), Compact);
        assert_eq!(presentations.get(OTHER), Compact);
        assert_eq!(presentations.get("DP-1"), Compact);

        presentations.input(MONITOR, Input::Click);
        assert_eq!(presentations.get(MONITOR), Expanded(Media));
    }

    #[test]
    fn closed_overview_changes_nothing() {
        let mut presentations = Presentations::default();

        presentations.set_primary(MONITOR, Some(Media));
        presentations.input(MONITOR, Input::Hover);
        presentations.set_overview(false);

        assert_eq!(presentations.get(MONITOR), Peek);
    }

    #[test]
    fn untouched_islands_share_one_primary_until_touched() {
        let mut presentations = Presentations::default();

        presentations.set_untouched(Some(Media));
        assert_eq!(presentations.get(MONITOR), Compact);
        assert_eq!(presentations.untouched(), Compact);

        // a touched island starts from it, then goes its own way
        presentations.input(MONITOR, Input::Hover);
        presentations.set_untouched(None);

        assert_eq!(presentations.get(MONITOR), Peek);
        assert_eq!(presentations.get(OTHER), Rest);

        presentations.input(OTHER, Input::Click);
        assert_eq!(presentations.get(OTHER), Expanded(Controls));
    }

    #[test]
    fn content_names_the_activity_only_in_a_small_form() {
        use crate::island::activity::{Id, Priority};

        let media = fixture::persistent(Id::new(Kind::Media, "spotify"), Priority::Media);

        for presentation in [Compact, Peek] {
            let content = Content::new(presentation, Some(media.clone()));
            assert_eq!(content.activity.as_ref(), Some(&media));
        }

        for presentation in [Rest, Expanded(Media)] {
            assert_eq!(
                Content::new(presentation, Some(media.clone())).activity,
                None
            );
        }
    }

    #[test]
    fn only_a_shown_level_moves_in_place() {
        use crate::island::activity::{Detail, Device, Id, Priority, Volume};

        let content = |detail| {
            let speaker = fixture::persistent(Id::new(Kind::Volume, "speaker"), Priority::Osd)
                .with_detail(detail);

            Content::new(Compact, Some(speaker))
        };

        let level = |percent| {
            content(Detail::Volume(Volume {
                device: Device::Speaker,
                percent,
                muted: false,
            }))
        };

        assert!(level(40).in_place(&level(45)));
        assert!(!content(Detail::None).in_place(&level(45)));
        assert!(!level(40).in_place(&content(Detail::None)));
        assert!(!level(40).in_place(&Content::new(Peek, level(45).activity)));
    }

    #[test]
    fn a_kind_opens_its_own_surface_or_controls() {
        assert_eq!(Surface::of(Kind::Media), Media);
        assert_eq!(Surface::of(Kind::Notification), Notifications);
        assert_eq!(Surface::of(Kind::Volume), Controls);
        assert_eq!(Surface::of(Kind::ScreenCast), Controls);
    }

    #[test]
    fn surface_names_round_trip() {
        for surface in Surface::ALL {
            assert_eq!(Surface::parse(surface.name()), Some(surface));
        }

        assert_eq!(Surface::parse("Media"), None);
        assert_eq!(Surface::parse(""), None);
    }

    #[test]
    fn expanded_names_the_open_island() {
        let mut presentations = Presentations::default();

        presentations.set_primary(MONITOR, Some(Media));
        assert_eq!(presentations.expanded(), None);

        presentations.input(OTHER, Input::Open(Launcher));
        assert_eq!(presentations.expanded(), Some((OTHER, Launcher)));

        presentations.input(MONITOR, Input::Click);
        assert_eq!(presentations.expanded(), Some((MONITOR, Media)));

        presentations.set_overview(true);
        assert_eq!(presentations.expanded(), None);
    }

    #[test]
    fn at_most_one_island_is_expanded() {
        let mut presentations = Presentations::default();

        presentations.set_primary(MONITOR, Some(Media));
        presentations.set_primary(OTHER, Some(Media));

        presentations.input(OTHER, Input::Open(Launcher));
        presentations.input(MONITOR, Input::Click);

        assert_eq!(presentations.get(MONITOR), Expanded(Media));
        assert_eq!(presentations.get(OTHER), Compact);

        // a peek elsewhere is not an expansion and stays
        presentations.input(OTHER, Input::Hover);
        presentations.input(MONITOR, Input::Open(Controls));

        assert_eq!(presentations.get(OTHER), Peek);
    }

    #[test]
    fn auto_expand_is_nothing_where_the_surface_already_shows() {
        let mut presentations = Presentations::default();

        presentations.input(MONITOR, Input::Open(Notifications));
        let visit = presentations.visit();

        assert_eq!(presentations.auto_expand(MONITOR, Notifications), None);
        assert_eq!(presentations.visit(), visit);

        presentations.set_overview(true);
        assert_eq!(presentations.auto_expand(MONITOR, Media), None);
    }

    #[test]
    fn restore_reopens_as_a_new_visit() {
        let mut presentations = Presentations::default();

        presentations.input(OTHER, Input::Open(Controls));
        let prior = presentations.auto_expand(MONITOR, Media).unwrap();
        let visit = presentations.visit();

        assert_eq!(presentations.get(OTHER), Rest);

        presentations.restore(prior);
        assert_eq!(presentations.get(OTHER), Expanded(Controls));
        assert_eq!(presentations.get(MONITOR), Rest);
        assert_eq!(presentations.visit(), visit + 1);
        assert_eq!(presentations.expanded(), Some((OTHER, Controls)));
    }
}
