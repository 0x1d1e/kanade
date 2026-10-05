//! Per-island Presentation (plan 4, 5.2). Pure: primary changes and input in, Presentation out.
//!
//! Each island keeps its primary and what the user holds it in: a Peek or an open Surface.
//! Rest or Compact follows from the primary alone.

use std::collections::HashMap;

// full interactive content of an Expanded island
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "opened once Media Activities exist")
    )]
    Media,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "opened once Notifications exist")
    )]
    Notifications,
    Controls,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "opened by IPC verbs that do not exist yet")
    )]
    Launcher,
}

// Expanded always carries a Surface, there is no surface-less expanded form
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presentation {
    Rest,
    Compact,
    Peek,
    Expanded(Surface),
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
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the Arbiter decides preemption, #19")
    )]
    Preempt,
}

impl Input {
    // false for input no Presentation reacts to yet, so it neither ends a pending Peek or grace
    // nor needs writing at all
    pub fn decides(self) -> bool {
        !matches!(self, Input::RightClick | Input::Wheel(_))
    }
}

// what the user holds an island in, beyond what its primary Activity gives it
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    Peek,
    Expanded(Surface),
}

#[derive(Debug, Default)]
struct Island {
    // the Surface of the island's primary Activity, none without one
    primary: Option<Surface>,

    // a Peek only ever exists with a primary, set_primary ends it on withdrawal
    held: Option<Held>,
}

// every island's Presentation by monitor; an island nobody touched rests
#[derive(Debug, Default)]
pub struct Presentations {
    islands: HashMap<String, Island>,

    // niri's overview is open, every island rests and takes no input (plan 5.3)
    overview: bool,
}

impl Presentations {
    pub fn get(&self, monitor: &str) -> Presentation {
        let Some(island) = self.islands.get(monitor).filter(|_| !self.overview) else {
            return Presentation::Rest;
        };

        match (island.held, island.primary) {
            (Some(Held::Expanded(surface)), _) => Presentation::Expanded(surface),
            (Some(Held::Peek), _) => Presentation::Peek,
            (None, Some(_)) => Presentation::Compact,
            (None, None) => Presentation::Rest,
        }
    }

    /*
     * Rest --Activity posted--> Compact and back on withdrawal. A withdrawal also ends a Peek,
     * which showed that primary, so a later post starts at Compact again. An open Surface stays
     */
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the Arbiter provides the primary, #20")
    )]
    pub fn set_primary(&mut self, monitor: &str, primary: Option<Surface>) {
        let island = self.island(monitor);

        island.primary = primary;

        if primary.is_none() && island.held == Some(Held::Peek) {
            island.held = None;
        }
    }

    /*
     * opening collapses every open Surface and Peek for good; primaries stay, so Compact comes back
     * on close (preemption never destroys)
     */
    pub fn set_overview(&mut self, open: bool) {
        self.overview = open;

        if open {
            for island in self.islands.values_mut() {
                island.held = None;
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
            | (Input::Unhover, Presentation::Peek) => island.held = None,

            // only an island with a primary has a larger small form to peek into
            (Input::Hover, Presentation::Compact) => island.held = Some(Held::Peek),

            (Input::RightClick | Input::Wheel(_), _) => {}

            _ => {}
        }
    }

    // at most one island is Expanded, opening one collapses any other
    fn expand(&mut self, monitor: &str, surface: Surface) {
        for island in self.islands.values_mut() {
            if matches!(island.held, Some(Held::Expanded(_))) {
                island.held = None;
            }
        }

        self.island(monitor).held = Some(Held::Expanded(surface));
    }

    fn island(&mut self, monitor: &str) -> &mut Island {
        self.islands.entry(monitor.to_owned()).or_default()
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
