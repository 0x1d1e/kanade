//! Per-island Presentation (CONTEXT.md, plan 5.2). Pure: primary changes and input in, Presentation out.
//!
//! Each island keeps its primary and what the user raised it to: a Peek or an open Surface.
//! Rest or Compact follows from the primary alone.

use std::collections::HashMap;

use super::activity::{Activity, Kind};
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
        match kind {
            Kind::Media => Surface::Media,
            Kind::Notification => Surface::Notifications,
            _ => Surface::Controls,
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
    // an open Surface shows itself, not the Activity, and Rest shows nothing
    pub fn new(presentation: Presentation, shown: Option<Activity>) -> Self {
        let small = matches!(presentation, Presentation::Compact | Presentation::Peek);

        Self {
            presentation,
            activity: shown.filter(|_| small),
        }
    }
}

// the same Activity in the same form with its level moved redraws where it stands; a level that
// first appears is new content, so it crossfades in
impl InPlace for Content {
    fn in_place(&self, next: &Content) -> bool {
        let moved = match (&self.activity, &next.activity) {
            (Some(shown), Some(next)) => {
                shown.id() == next.id() && shown.detail().is_level() && next.detail().is_level()
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

    /*
     * routed but meaning nothing yet: right click is context and pin (#31), the wheel belongs to
     * the Surface under it, Media volume first (#27); positive scrolls down
     */
    RightClick,
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
        !matches!(self, Input::RightClick | Input::Wheel(_))
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
}

// every island's Presentation by monitor
#[derive(Debug, Default)]
pub struct Presentations {
    islands: HashMap<String, Island>,

    // every island no input reached yet: one primary for all of them, never raised
    untouched: Island,

    // niri's overview is open, every island rests and takes no input (plan 5.3)
    overview: bool,
}

impl Presentations {
    pub fn get(&self, monitor: &str) -> Presentation {
        self.of(self.islands.get(monitor).unwrap_or(&self.untouched))
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
                island.raised = None;
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

            (Input::Collapse | Input::Preempt, Presentation::Expanded(_))
            | (Input::Unhover, Presentation::Peek) => island.raised = None,

            // only an island with a primary has a larger small form to peek into
            (Input::Hover, Presentation::Compact) => island.raised = Some(Raised::Peek),

            (Input::RightClick | Input::Wheel(_), _) => {}

            _ => {}
        }
    }

    // at most one island is Expanded, opening one collapses any other
    fn expand(&mut self, monitor: &str, surface: Surface) {
        for island in self.islands.values_mut() {
            if matches!(island.raised, Some(Raised::Expanded(_))) {
                island.raised = None;
            }
        }

        self.island(monitor).raised = Some(Raised::Expanded(surface));
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
            self.raised = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn right_click_and_wheel_change_nothing_yet() {
        for setup in [&[][..], &[Input::Hover], &[Input::Click]] {
            for primary in [None, Some(Media)] {
                let before = after(setup, primary);

                for input in [Input::RightClick, Input::Wheel(1.0), Input::Wheel(-1.0)] {
                    let inputs = [setup, &[input]].concat();

                    assert_eq!(after(&inputs, primary), before, "{inputs:?} {primary:?}");
                }
            }
        }
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

        let media = Activity::persistent(Id::new(Kind::Media, "spotify"), Priority::Media);

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
            let speaker = Activity::persistent(Id::new(Kind::Volume, "speaker"), Priority::Osd)
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
}
