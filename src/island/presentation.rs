//! Per-island Presentation (plan 4, 5.2). Pure: input and the island's primary in, Presentation out.
//!
//! Only what the user did is stored: peeking or an open Surface. Rest or Compact follows from
//! whether the island has a primary Activity, so posting or withdrawing one needs no transition
//! of its own and cannot leave a stale level behind.

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    // left click on the body
    Click,

    // IPC or keybind asking for a Surface
    Open(Surface),

    // Escape, pointer out after the grace, IPC collapse
    Collapse,

    // the pointer rested on the body for the hover delay, and left it again
    #[cfg_attr(not(test), expect(dead_code, reason = "hover delay is #10"))]
    Hover,
    #[cfg_attr(not(test), expect(dead_code, reason = "hover delay is #10"))]
    Unhover,

    // a Critical Activity displaces an open Surface (plan 5.1 rule 4)
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the Arbiter decides preemption, #19")
    )]
    Preempt,
}

// what the user holds an island in, beyond what its primary Activity gives it
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    Peek,
    Expanded(Surface),
}

// every island's Presentation by monitor; an island nobody touched holds nothing
#[derive(Debug, Default)]
pub struct Presentations {
    held: HashMap<String, Held>,
}

impl Presentations {
    // `primary` is the Surface of this island's primary Activity, none without one
    pub fn get(&self, monitor: &str, primary: Option<Surface>) -> Presentation {
        match (self.held.get(monitor), primary) {
            (Some(&Held::Expanded(surface)), _) => Presentation::Expanded(surface),
            (Some(Held::Peek), Some(_)) => Presentation::Peek,
            (_, Some(_)) => Presentation::Compact,
            (_, None) => Presentation::Rest,
        }
    }

    pub fn input(&mut self, monitor: &str, input: Input, primary: Option<Surface>) {
        let now = self.get(monitor, primary);

        match (input, now) {
            // a click inside an open Surface belongs to the Surface
            (Input::Click, Presentation::Expanded(_)) => {}

            // the primary's Surface; Rest has no primary and no remembered last Surface, so Controls
            (Input::Click, _) => self.expand(monitor, primary.unwrap_or(Surface::Controls)),

            (Input::Open(surface), _) => self.expand(monitor, surface),

            (Input::Collapse | Input::Preempt, Presentation::Expanded(_)) => {
                self.held.remove(monitor);
            }

            // only an island with a primary has a larger small form to peek into
            (Input::Hover, Presentation::Compact) => {
                self.held.insert(monitor.to_owned(), Held::Peek);
            }

            (Input::Unhover, _) if self.held.get(monitor) == Some(&Held::Peek) => {
                self.held.remove(monitor);
            }

            _ => {}
        }
    }

    // at most one island is Expanded, opening one collapses any other
    fn expand(&mut self, monitor: &str, surface: Surface) {
        self.held
            .retain(|other, held| other == monitor || !matches!(held, Held::Expanded(_)));

        self.held
            .insert(monitor.to_owned(), Held::Expanded(surface));
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

        for &input in inputs {
            presentations.input(MONITOR, input, primary);
        }

        presentations.get(MONITOR, primary)
    }

    #[test]
    fn untouched_island_rests_or_shows_its_primary() {
        assert_eq!(after(&[], None), Rest);
        assert_eq!(after(&[], Some(Media)), Compact);
    }

    #[test]
    fn activity_posted_and_withdrawn() {
        let presentations = Presentations::default();

        // Rest --Activity posted--> Compact, and back once it is withdrawn
        assert_eq!(presentations.get(MONITOR, None), Rest);
        assert_eq!(presentations.get(MONITOR, Some(Media)), Compact);
        assert_eq!(presentations.get(MONITOR, None), Rest);
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
    fn withdrawn_primary_ends_a_peek() {
        let mut presentations = Presentations::default();

        presentations.input(MONITOR, Input::Hover, Some(Media));
        assert_eq!(presentations.get(MONITOR, None), Rest);
    }

    #[test]
    fn open_surface_outlives_its_primary() {
        let mut presentations = Presentations::default();

        presentations.input(MONITOR, Input::Click, Some(Media));
        assert_eq!(presentations.get(MONITOR, None), Expanded(Media));
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
    fn at_most_one_island_is_expanded() {
        let mut presentations = Presentations::default();

        presentations.input(OTHER, Input::Open(Launcher), None);
        presentations.input(MONITOR, Input::Click, Some(Media));

        assert_eq!(presentations.get(MONITOR, Some(Media)), Expanded(Media));
        assert_eq!(presentations.get(OTHER, Some(Media)), Compact);

        // a peek elsewhere is not an expansion and stays
        presentations.input(OTHER, Input::Hover, Some(Media));
        presentations.input(MONITOR, Input::Open(Controls), Some(Media));

        assert_eq!(presentations.get(OTHER, Some(Media)), Peek);
    }
}
