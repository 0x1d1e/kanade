//! Per-island Presentation (CONTEXT.md, plan 5.2). Pure: the Activities shown and input in,
//! Presentation out.
//!
//! Each island keeps its primary and top Satellite and what the user raised it to: a Peek of one
//! of them or an open Surface, pinned or not. Rest, Compact or Split follows from the Frame alone.

use std::collections::HashMap;

use super::activity::{Activity, Detail, Id, Kind};
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
    #[cfg(test)]
    pub const ALL: [Surface; 4] = [
        Surface::Media,
        Surface::Notifications,
        Surface::Controls,
        Surface::Launcher,
    ];

    // the Surface a Kind is also, the only one an Activity may open itself (`Interrupt::AutoExpand`)
    pub fn own(kind: Kind) -> Option<Surface> {
        match kind {
            Kind::Media => Some(Surface::Media),
            Kind::Notification => Some(Surface::Notifications),
            _ => None,
        }
    }
}

/*
 * Expanded always carries a Surface, there is no surface-less expanded form. Which Activity a
 * Peek shows is `Presentations::peeked`, so a Presentation stays a plain value
 */
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Presentation {
    #[default]
    Rest,
    Compact,

    // Compact with the top Satellite beside the primary in one body, while the Frame has one
    Split,

    Peek,
    Expanded(Surface),
}

// a part of a Split body: the primary leading, the top Satellite trailing (ADR 0010)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Segment {
    #[default]
    Primary,
    Satellite,
}

/*
 * what the body shows: the Presentation, and in a small form the Activity it is the form of. Split
 * also shows the top Satellite in its trailing segment
 */
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Content {
    pub presentation: Presentation,
    pub activity: Option<Activity>,
    pub satellite: Option<Activity>,
}

impl Content {
    /*
     * an open Surface shows itself, not an Activity, and Rest shows the time; a Peek shows the
     * Activity it holds, whichever of the two it is
     */
    pub fn new(
        presentation: Presentation,
        primary: Option<Activity>,
        satellite: Option<Activity>,
        peeked: Option<&Id>,
    ) -> Self {
        let (activity, satellite) = match presentation {
            Presentation::Compact => (primary, None),
            Presentation::Split => (primary, satellite),
            Presentation::Peek => {
                let peeked = [primary, satellite]
                    .into_iter()
                    .flatten()
                    .find(|activity| Some(activity.id()) == peeked);

                (peeked, None)
            }
            Presentation::Rest | Presentation::Expanded(_) => (None, None),
        };

        Self {
            presentation,
            activity,
            satellite,
        }
    }

    // a Split whose primary and top Satellite traded places, which slides them instead of fading
    pub fn swapped(&self, next: &Content) -> bool {
        let id = |activity: &Option<Activity>| activity.as_ref().map(Activity::id).cloned();

        self.presentation == Presentation::Split
            && next.presentation == Presentation::Split
            && id(&self.activity) == id(&next.satellite)
            && id(&self.satellite) == id(&next.activity)
            && id(&self.activity) != id(&self.satellite)
    }
}

/*
 * the same Activity in the same form with its level moved redraws where it stands, and a new track
 * dissolves where it stands (`Dissolve`); a level that first appears is new content, so it
 * crossfades in. A Split's two segments trading places slide there (`Content::swapped`)
 */
impl InPlace for Content {
    fn in_place(&self, next: &Content) -> bool {
        let moved = |shown: &Option<Activity>, next: &Option<Activity>| match (shown, next) {
            (Some(shown), Some(next)) => {
                shown.id() == next.id()
                    && match (shown.detail(), next.detail()) {
                        (Detail::Media(_), Detail::Media(_)) => true,
                        (shown, next) => shown.is_level() && next.is_level(),
                    }
            }
            (shown, next) => shown == next,
        };

        self.presentation == next.presentation
            && (moved(&self.activity, &next.activity) && moved(&self.satellite, &next.satellite)
                || self.swapped(next))
    }
}

// Wheel carries a scroll delta, so no Eq
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Input {
    // left click on the body, on the segment under the pointer
    Click(Segment),

    /*
     * pins or unpins a Peek or an open Surface, peeking the segment pinned from Compact or Split
     * and opening Controls pinned from Rest
     */
    RightClick(Segment),

    // routed but meaning nothing yet: the wheel belongs to the Surface under it (#27); positive
    // scrolls down
    Wheel(f32),

    // IPC or keybind asking for a Surface
    Open(Surface),

    // Escape, pointer out after the grace, IPC collapse
    Collapse,

    // the pointer stayed on the island for the hover delay, on this segment; then off it for the grace
    Hover(Segment),
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

    // the user's own choice, which takes a pending AutoExpand over; the island's timers and a
    // Preempt are policy, not choice
    pub fn claims(self) -> bool {
        self.decides() && !matches!(self, Input::Hover(_) | Input::Unhover | Input::Preempt)
    }
}

// what the user raised an island to, beyond what its Frame gives it
#[derive(Debug, Clone, PartialEq, Eq)]
enum Raised {
    // one Activity by identity, the primary or the top Satellite
    Peek(Id),

    Expanded(Surface),
}

#[derive(Debug, Clone, Default)]
struct Island {
    // the Frame's primary and top Satellite; a Satellite only ever with a primary
    primary: Option<Id>,
    satellite: Option<Id>,

    // a Peek only ever of one of the two, `set_shown` ends it once its Activity is neither
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

    // every island no input reached yet: one Frame for all of them, never raised
    untouched: Island,

    // niri's overview is open, every island rests and takes no input (plan 5.3)
    overview: bool,

    // how many times a Surface opened, so state kept for one opening ends with it
    visits: u64,

    // the Surfaces that never open, since their Module is off
    withheld: Vec<Surface>,
}

impl Presentations {
    // once at start, from the Modules that are off; every Surface may open until then
    pub fn withhold(&mut self, surfaces: &[Surface]) {
        self.withheld = surfaces.to_vec();
    }

    pub fn offers(&self, surface: Surface) -> bool {
        !self.withheld.contains(&surface)
    }

    /*
     * what a click on an island showing this Activity opens, or with none what one at Rest opens:
     * its own Surface, else Controls, where a Kind without one has its control. None while that is
     * withheld too
     */
    fn clicked(&self, id: Option<&Id>) -> Option<Surface> {
        id.and_then(|id| Surface::own(id.kind()))
            .into_iter()
            .chain([Surface::Controls])
            .find(|&surface| self.offers(surface))
    }

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

    // the Activity a Peek on this island shows, none without a Peek
    pub fn peeked(&self, monitor: &str) -> Option<&Id> {
        if self.overview {
            return None;
        }

        match &self.islands.get(monitor).unwrap_or(&self.untouched).raised {
            Some(Raised::Peek(id)) => Some(id),
            _ => None,
        }
    }

    // what every untouched island shows
    pub fn untouched(&self) -> Presentation {
        self.of(&self.untouched)
    }

    fn of(&self, island: &Island) -> Presentation {
        if self.overview {
            return Presentation::Rest;
        }

        match (&island.raised, &island.primary, &island.satellite) {
            (Some(Raised::Expanded(surface)), ..) => Presentation::Expanded(*surface),
            (Some(Raised::Peek(_)), ..) => Presentation::Peek,
            (None, Some(_), Some(_)) => Presentation::Split,
            (None, Some(_), None) => Presentation::Compact,
            (None, None, _) => Presentation::Rest,
        }
    }

    /*
     * Rest --Activity posted--> Compact --Satellite--> Split, and back as they go. A Peek lasts
     * while its Activity is still the primary or the top Satellite, so a later post starts at
     * Compact or Split again. An open Surface stays
     */
    pub fn set_shown(&mut self, monitor: &str, primary: Option<Id>, satellite: Option<Id>) {
        self.island(monitor).set_shown(primary, satellite);
    }

    // for every island not touched yet, which then starts from it
    pub fn set_untouched(&mut self, primary: Option<Id>, satellite: Option<Id>) {
        self.untouched.set_shown(primary, satellite);
    }

    /*
     * opening collapses every open Surface and Peek for good; the Frame stays, so Compact or Split
     * comes back on close (preemption never destroys)
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
            (Input::Click(_), Presentation::Expanded(_)) => {}

            // the Surface of the Activity under the pointer; Rest has none and no remembered last Surface, so Controls
            (Input::Click(segment), _) => {
                let under = island.under(segment).cloned();

                if let Some(surface) = self.clicked(under.as_ref()) {
                    self.expand(monitor, surface);
                }
            }

            (Input::Open(surface), _) => self.expand(monitor, surface),

            (Input::Collapse | Input::Preempt, Presentation::Expanded(_)) => island.raise(None),

            // a pinned Peek waits for Escape or another right click, not for the pointer
            (Input::Collapse, Presentation::Peek) if island.pinned => island.raise(None),
            (Input::Unhover, Presentation::Peek) if !island.pinned => island.raise(None),

            // only an island with a primary has a larger small form to peek into
            (Input::Hover(segment), Presentation::Compact | Presentation::Split) => {
                island.peek(segment);
            }

            // right click raises to what the pointer would and pins it: a Peek, or at Rest what a click opens
            (Input::RightClick(segment), Presentation::Compact | Presentation::Split) => {
                island.peek(segment);
                island.pinned = true;
            }
            (Input::RightClick(_), Presentation::Rest) => {
                if let Some(surface) = self.clicked(None) {
                    self.expand(monitor, surface);
                    self.island(monitor).pinned = true;
                }
            }
            (Input::RightClick(_), Presentation::Peek | Presentation::Expanded(_)) => {
                island.pinned = !island.pinned;
            }

            _ => {}
        }
    }

    // at most one island is Expanded, opening one collapses any other; a withheld Surface opens nowhere
    fn expand(&mut self, monitor: &str, surface: Surface) {
        if !self.offers(surface) {
            return;
        }

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
        if self.overview
            || !self.offers(surface)
            || self.get(monitor) == Presentation::Expanded(surface)
        {
            return None;
        }

        self.island(monitor);

        let prior = self
            .islands
            .iter()
            .filter(|&(name, island)| {
                name == monitor || matches!(island.raised, Some(Raised::Expanded(_)))
            })
            .map(|(name, island)| (name.clone(), island.raised.clone(), island.pinned))
            .collect();

        self.expand(monitor, surface);

        Some(Prior(prior))
    }

    /*
     * puts back what `auto_expand` changed, a Surface reopening as a new visit. A Peek whose
     * Activity no longer shows on the island stays ended, as `set_shown` would have ended it
     */
    pub fn restore(&mut self, prior: Prior) {
        if self.overview {
            return;
        }

        for (monitor, raised, pinned) in prior.0 {
            let island = self.island(&monitor);
            let raised = raised.filter(|raised| match raised {
                Raised::Peek(id) => island.shows(id),
                Raised::Expanded(_) => true,
            });
            let expanded = matches!(raised, Some(Raised::Expanded(_)));

            island.pinned = pinned && raised.is_some();
            island.raised = raised;

            if expanded {
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
    fn set_shown(&mut self, primary: Option<Id>, satellite: Option<Id>) {
        self.primary = primary;
        self.satellite = satellite.filter(|_| self.primary.is_some());

        if let Some(Raised::Peek(id)) = &self.raised
            && !self.shows(id)
        {
            self.raise(None);
        }
    }

    // the Activity is the primary or the top Satellite, so a Peek of it still has it to show
    fn shows(&self, id: &Id) -> bool {
        self.primary.as_ref() == Some(id) || self.satellite.as_ref() == Some(id)
    }

    /*
     * the Activity a pointer on `segment` acts on: the peeked one on a Peek, the segment's on a
     * Split, the primary on Compact; none at Rest or on a Surface
     */
    fn under(&self, segment: Segment) -> Option<&Id> {
        match (&self.raised, segment) {
            (Some(Raised::Peek(id)), _) => Some(id),
            (Some(Raised::Expanded(_)), _) => None,
            (None, Segment::Satellite) if self.satellite.is_some() => self.satellite.as_ref(),
            (None, _) => self.primary.as_ref(),
        }
    }

    // peeks the Activity under `segment`, if there is one
    fn peek(&mut self, segment: Segment) {
        if let Some(id) = self.under(segment).cloned() {
            self.raise(Some(Raised::Peek(id)));
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

    use Presentation::{Compact, Expanded, Peek, Rest, Split};
    use Segment::{Primary, Satellite};
    use Surface::{Controls, Launcher, Media, Notifications};

    // the one Activity of this Kind the tests show
    fn id(kind: Kind) -> Id {
        Id::new(kind, kind.name())
    }

    const MONITOR: &str = "eDP-1";
    const OTHER: &str = "HDMI-A-1";

    // the island on MONITOR after these inputs, with `primary` throughout
    fn after(inputs: &[Input], primary: Option<Kind>) -> Presentation {
        let mut presentations = Presentations::default();

        presentations.set_shown(MONITOR, primary.map(id), None);

        for &input in inputs {
            presentations.input(MONITOR, input);
        }

        presentations.get(MONITOR)
    }

    #[test]
    fn every_opening_is_a_new_visit() {
        let mut presentations = Presentations::default();
        let first = presentations.visit();

        presentations.input(MONITOR, Input::Open(Notifications));
        let open = presentations.visit();
        assert_ne!(open, first);

        // a click inside keeps the visit, a Surface replacing it starts another
        presentations.input(MONITOR, Input::Click(Primary));
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
        assert_eq!(after(&[], Some(Kind::Media)), Compact);
    }

    #[test]
    fn activity_posted_and_withdrawn() {
        let mut presentations = Presentations::default();

        presentations.set_shown(MONITOR, Some(id(Kind::Media)), None);
        assert_eq!(presentations.get(MONITOR), Compact);

        presentations.set_shown(MONITOR, None, None);
        assert_eq!(presentations.get(MONITOR), Rest);
    }

    #[test]
    fn rest_click_opens_controls() {
        assert_eq!(after(&[Input::Click(Primary)], None), Expanded(Controls));
    }

    #[test]
    fn compact_or_peek_click_opens_the_primarys_surface() {
        assert_eq!(
            after(&[Input::Click(Primary)], Some(Kind::Media)),
            Expanded(Media)
        );

        assert_eq!(
            after(
                &[Input::Hover(Primary), Input::Click(Primary)],
                Some(Kind::Notification)
            ),
            Expanded(Notifications)
        );
    }

    #[test]
    fn open_shows_the_requested_surface_from_anywhere() {
        assert_eq!(after(&[Input::Open(Launcher)], None), Expanded(Launcher));
        assert_eq!(
            after(&[Input::Open(Launcher)], Some(Kind::Media)),
            Expanded(Launcher)
        );

        assert_eq!(
            after(
                &[Input::Hover(Primary), Input::Open(Launcher)],
                Some(Kind::Media)
            ),
            Expanded(Launcher)
        );

        // an open island switches Surface without collapsing first
        assert_eq!(
            after(
                &[Input::Click(Primary), Input::Open(Launcher)],
                Some(Kind::Media)
            ),
            Expanded(Launcher)
        );
    }

    #[test]
    fn click_inside_an_open_surface_keeps_it() {
        assert_eq!(
            after(
                &[Input::Open(Launcher), Input::Click(Primary)],
                Some(Kind::Media)
            ),
            Expanded(Launcher)
        );
    }

    #[test]
    fn collapse_returns_to_compact_or_rest() {
        assert_eq!(after(&[Input::Click(Primary), Input::Collapse], None), Rest);
        assert_eq!(
            after(&[Input::Click(Primary), Input::Collapse], Some(Kind::Media)),
            Compact
        );
    }

    #[test]
    fn collapse_and_preempt_leave_small_forms_alone() {
        assert_eq!(after(&[Input::Collapse], None), Rest);
        assert_eq!(after(&[Input::Preempt], Some(Kind::Media)), Compact);
        assert_eq!(
            after(&[Input::Hover(Primary), Input::Collapse], Some(Kind::Media)),
            Peek
        );
        assert_eq!(
            after(&[Input::Hover(Primary), Input::Preempt], Some(Kind::Media)),
            Peek
        );
    }

    #[test]
    fn preempt_collapses_an_open_surface_to_compact() {
        assert_eq!(
            after(&[Input::Click(Primary), Input::Preempt], Some(Kind::Media)),
            Compact
        );
    }

    #[test]
    fn hover_peeks_only_from_compact() {
        assert_eq!(after(&[Input::Hover(Primary)], Some(Kind::Media)), Peek);
        assert_eq!(
            after(&[Input::Hover(Primary), Input::Unhover], Some(Kind::Media)),
            Compact
        );

        // nothing to peek into at Rest, and an open Surface stays open
        assert_eq!(after(&[Input::Hover(Primary)], None), Rest);
        assert_eq!(
            after(
                &[Input::Click(Primary), Input::Hover(Primary)],
                Some(Kind::Media)
            ),
            Expanded(Media)
        );
        assert_eq!(
            after(&[Input::Click(Primary), Input::Unhover], Some(Kind::Media)),
            Expanded(Media)
        );
    }

    #[test]
    fn wheel_changes_nothing_yet() {
        for setup in [&[][..], &[Input::Hover(Primary)], &[Input::Click(Primary)]] {
            for primary in [None, Some(Kind::Media)] {
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

        presentations.set_shown(MONITOR, Some(id(Kind::Media)), None);

        for &input in inputs {
            presentations.input(MONITOR, input);
        }

        (presentations.get(MONITOR), presentations.pinned(MONITOR))
    }

    // Rest has no Peek, so it raises to what a click opens, pinned like any Surface (ADR 0010)
    #[test]
    fn right_click_at_rest_opens_controls_pinned() {
        let mut presentations = Presentations::default();
        let visit = presentations.visit();

        presentations.input(MONITOR, Input::RightClick(Primary));

        assert_eq!(presentations.get(MONITOR), Expanded(Controls));
        assert!(presentations.pinned(MONITOR));
        assert_ne!(presentations.visit(), visit);

        // and its pin ends as any Surface's
        presentations.input(MONITOR, Input::RightClick(Primary));
        assert_eq!(presentations.get(MONITOR), Expanded(Controls));
        assert!(!presentations.pinned(MONITOR));

        presentations.input(MONITOR, Input::RightClick(Primary));
        presentations.input(MONITOR, Input::Collapse);
        assert_eq!(presentations.get(MONITOR), Rest);
        assert!(!presentations.pinned(MONITOR));

        // it collapses another island's Surface like any opening
        presentations.input(OTHER, Input::Open(Launcher));
        presentations.input(MONITOR, Input::RightClick(Primary));
        assert_eq!(presentations.expanded(), Some((MONITOR, Controls)));
    }

    #[test]
    fn right_click_peeks_pinned_from_compact() {
        assert_eq!(pinned_after(&[Input::RightClick(Primary)]), (Peek, true));
        assert_eq!(
            pinned_after(&[Input::Hover(Primary), Input::RightClick(Primary)]),
            (Peek, true)
        );
    }

    #[test]
    fn right_click_pins_and_unpins_an_open_surface() {
        assert_eq!(
            pinned_after(&[Input::Click(Primary), Input::RightClick(Primary)]),
            (Expanded(Media), true)
        );
        assert_eq!(
            pinned_after(&[
                Input::Click(Primary),
                Input::RightClick(Primary),
                Input::RightClick(Primary)
            ]),
            (Expanded(Media), false)
        );
        assert_eq!(
            pinned_after(&[Input::RightClick(Primary), Input::RightClick(Primary)]),
            (Peek, false)
        );
    }

    #[test]
    fn a_pinned_peek_outlasts_the_pointer_until_escape() {
        assert_eq!(
            pinned_after(&[Input::RightClick(Primary), Input::Unhover]),
            (Peek, true)
        );
        assert_eq!(
            pinned_after(&[Input::RightClick(Primary), Input::Collapse]),
            (Compact, false)
        );

        // unpinned, it is an ordinary Peek again
        assert_eq!(
            pinned_after(&[
                Input::RightClick(Primary),
                Input::RightClick(Primary),
                Input::Unhover
            ]),
            (Compact, false)
        );
    }

    // no remembered state: whatever ends or replaces the pinned Peek or Surface ends the pin
    #[test]
    fn a_pin_ends_with_what_it_kept_open() {
        assert_eq!(
            pinned_after(&[Input::RightClick(Primary), Input::Click(Primary)]),
            (Expanded(Media), false)
        );
        assert_eq!(
            pinned_after(&[
                Input::Click(Primary),
                Input::RightClick(Primary),
                Input::Open(Launcher)
            ]),
            (Expanded(Launcher), false)
        );

        for end in [Input::Collapse, Input::Preempt] {
            let inputs = [Input::Click(Primary), Input::RightClick(Primary), end];

            assert_eq!(pinned_after(&inputs), (Compact, false), "{end:?}");
            assert_eq!(
                pinned_after(&[&inputs[..], &[Input::Click(Primary)]].concat()),
                (Expanded(Media), false),
                "{end:?}"
            );
        }

        // a pinned Peek stays through a Critical, which only displaces a Surface
        assert_eq!(
            pinned_after(&[Input::RightClick(Primary), Input::Preempt]),
            (Peek, true)
        );
    }

    #[test]
    fn withdrawal_and_other_islands_end_a_pin() {
        let mut presentations = Presentations::default();

        presentations.set_shown(MONITOR, Some(id(Kind::Media)), None);
        presentations.input(MONITOR, Input::RightClick(Primary));
        presentations.set_shown(MONITOR, None, None);

        assert_eq!(presentations.get(MONITOR), Rest);
        assert!(!presentations.pinned(MONITOR));

        presentations.set_shown(MONITOR, Some(id(Kind::Media)), None);
        assert_eq!(presentations.get(MONITOR), Compact);

        // opening another island collapses a pinned one too
        presentations.input(MONITOR, Input::Click(Primary));
        presentations.input(MONITOR, Input::RightClick(Primary));
        presentations.input(OTHER, Input::Open(Launcher));

        assert_eq!(presentations.get(MONITOR), Compact);
        assert!(!presentations.pinned(MONITOR));
    }

    #[test]
    fn overview_ends_a_pin() {
        let mut presentations = Presentations::default();

        presentations.set_shown(MONITOR, Some(id(Kind::Media)), None);
        presentations.input(MONITOR, Input::RightClick(Primary));
        presentations.set_overview(true);

        assert!(!presentations.pinned(MONITOR));

        // and right click does not pin under it
        presentations.input(MONITOR, Input::RightClick(Primary));
        presentations.set_overview(false);

        assert_eq!(presentations.get(MONITOR), Compact);
        assert!(!presentations.pinned(MONITOR));
    }

    #[test]
    fn withdrawn_primary_ends_a_peek_for_good() {
        let mut presentations = Presentations::default();

        presentations.set_shown(MONITOR, Some(id(Kind::Media)), None);
        presentations.input(MONITOR, Input::Hover(Primary));
        assert_eq!(presentations.get(MONITOR), Peek);

        presentations.set_shown(MONITOR, None, None);
        assert_eq!(presentations.get(MONITOR), Rest);

        // Rest --Activity posted--> Compact, whichever Activity it is
        for primary in [Kind::Media, Kind::Notification] {
            presentations.set_shown(MONITOR, Some(id(primary)), None);
            assert_eq!(presentations.get(MONITOR), Compact);
        }
    }

    // a Peek holds its Activity, not a slot: another primary in its place ends it
    #[test]
    fn replaced_primary_ends_a_peek() {
        let mut presentations = Presentations::default();

        presentations.set_shown(MONITOR, Some(id(Kind::Media)), None);
        presentations.input(MONITOR, Input::Hover(Primary));
        presentations.set_shown(MONITOR, Some(id(Kind::Notification)), None);

        assert_eq!(presentations.get(MONITOR), Compact);
        assert_eq!(presentations.peeked(MONITOR), None);
    }

    #[test]
    fn open_surface_outlives_its_primary() {
        let mut presentations = Presentations::default();

        presentations.set_shown(MONITOR, Some(id(Kind::Media)), None);
        presentations.input(MONITOR, Input::Click(Primary));
        presentations.set_shown(MONITOR, None, None);

        assert_eq!(presentations.get(MONITOR), Expanded(Media));

        presentations.input(MONITOR, Input::Collapse);
        assert_eq!(presentations.get(MONITOR), Rest);
    }

    #[test]
    fn no_remembered_last_surface() {
        assert_eq!(
            after(
                &[
                    Input::Open(Launcher),
                    Input::Collapse,
                    Input::Click(Primary)
                ],
                None
            ),
            Expanded(Controls)
        );
    }

    #[test]
    fn overview_rests_every_island_until_it_closes() {
        let mut presentations = Presentations::default();

        presentations.set_shown(MONITOR, Some(id(Kind::Media)), None);
        presentations.input(MONITOR, Input::Click(Primary));
        presentations.set_shown(OTHER, Some(id(Kind::Media)), None);
        presentations.input(OTHER, Input::Hover(Primary));

        presentations.set_overview(true);

        assert_eq!(presentations.get(MONITOR), Rest);
        assert_eq!(presentations.get(OTHER), Rest);
        assert_eq!(presentations.get("DP-1"), Rest);

        // nothing opens, peeks or comes back while it is open
        for input in [
            Input::Click(Primary),
            Input::Open(Launcher),
            Input::Hover(Primary),
        ] {
            presentations.input(MONITOR, input);
            assert_eq!(presentations.get(MONITOR), Rest, "{input:?}");
        }

        // a primary posted meanwhile counts once it closes
        presentations.set_shown("DP-1", Some(id(Kind::Notification)), None);
        presentations.set_overview(false);

        assert_eq!(presentations.get(MONITOR), Compact);
        assert_eq!(presentations.get(OTHER), Compact);
        assert_eq!(presentations.get("DP-1"), Compact);

        presentations.input(MONITOR, Input::Click(Primary));
        assert_eq!(presentations.get(MONITOR), Expanded(Media));
    }

    #[test]
    fn closed_overview_changes_nothing() {
        let mut presentations = Presentations::default();

        presentations.set_shown(MONITOR, Some(id(Kind::Media)), None);
        presentations.input(MONITOR, Input::Hover(Primary));
        presentations.set_overview(false);

        assert_eq!(presentations.get(MONITOR), Peek);
    }

    #[test]
    fn untouched_islands_share_one_primary_until_touched() {
        let mut presentations = Presentations::default();

        presentations.set_untouched(Some(id(Kind::Media)), None);
        assert_eq!(presentations.get(MONITOR), Compact);
        assert_eq!(presentations.untouched(), Compact);

        // a touched island starts from it, then goes its own way
        presentations.input(MONITOR, Input::Hover(Primary));
        presentations.set_untouched(None, None);

        assert_eq!(presentations.get(MONITOR), Peek);
        assert_eq!(presentations.get(OTHER), Rest);

        presentations.input(OTHER, Input::Click(Primary));
        assert_eq!(presentations.get(OTHER), Expanded(Controls));
    }

    fn activity(kind: Kind) -> Activity {
        use crate::island::activity::Priority;

        fixture::persistent(id(kind), Priority::Ongoing)
    }

    #[test]
    fn content_names_the_activities_only_in_a_small_form() {
        let (media, timer) = (activity(Kind::Media), activity(Kind::Timer));
        let content = |presentation, peeked: Option<Id>| {
            Content::new(
                presentation,
                Some(media.clone()),
                Some(timer.clone()),
                peeked.as_ref(),
            )
        };

        let compact = content(Compact, None);
        assert_eq!(compact.activity.as_ref(), Some(&media));
        assert_eq!(compact.satellite, None);

        let split = content(Split, None);
        assert_eq!(split.activity.as_ref(), Some(&media));
        assert_eq!(split.satellite.as_ref(), Some(&timer));

        // a Peek shows the one it holds, alone
        for shown in [&media, &timer] {
            let peek = content(Peek, Some(shown.id().clone()));
            assert_eq!(peek.activity.as_ref(), Some(shown));
            assert_eq!(peek.satellite, None);
        }

        for presentation in [Rest, Expanded(Media)] {
            let content = content(presentation, Some(media.id().clone()));
            assert_eq!((content.activity, content.satellite), (None, None));
        }
    }

    // a primary change that trades the two segments slides them, anything else in them crossfades
    #[test]
    fn a_split_slides_only_when_its_segments_trade_places() {
        let (media, timer, battery) = (
            activity(Kind::Media),
            activity(Kind::Timer),
            activity(Kind::Battery),
        );
        let split = |primary: &Activity, satellite: &Activity| {
            Content::new(Split, Some(primary.clone()), Some(satellite.clone()), None)
        };

        let shown = split(&media, &timer);
        let swapped = split(&timer, &media);

        assert!(shown.swapped(&swapped) && shown.in_place(&swapped));
        assert!(swapped.swapped(&shown));

        assert!(!shown.swapped(&shown));
        assert!(!shown.in_place(&split(&media, &battery)));
        assert!(!shown.in_place(&split(&battery, &media)));

        let compact = Content::new(Compact, Some(media.clone()), None, None);
        assert!(!shown.in_place(&compact) && !compact.in_place(&shown));
    }

    #[test]
    fn only_a_shown_level_moves_in_place() {
        use crate::island::activity::{Detail, Device, Id, Priority, Volume};

        let content = |detail| {
            let speaker = fixture::persistent(Id::new(Kind::Volume, "speaker"), Priority::Osd)
                .with_detail(detail);

            Content::new(Compact, Some(speaker), None, None)
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
        assert!(!level(40).in_place(&Content {
            presentation: Peek,
            ..level(45)
        }));
    }

    #[test]
    fn a_kind_opens_its_own_surface_or_controls() {
        let presentations = Presentations::default();
        let clicked = |kind| presentations.clicked(Some(&id(kind)));

        assert_eq!(clicked(Kind::Media), Some(Media));
        assert_eq!(clicked(Kind::Notification), Some(Notifications));
        assert_eq!(clicked(Kind::Volume), Some(Controls));
        assert_eq!(clicked(Kind::Timer), Some(Controls));
        assert_eq!(presentations.clicked(None), Some(Controls));
    }

    // its Module is off: a Kind whose own Surface is withheld has its control in Controls, as one without
    #[test]
    fn a_withheld_surface_never_opens() {
        let mut presentations = Presentations::default();
        presentations.withhold(&[Notifications, Controls]);

        assert_eq!(presentations.clicked(Some(&id(Kind::Notification))), None);
        assert_eq!(presentations.clicked(None), None);

        presentations.withhold(&[Notifications]);
        assert_eq!(
            presentations.clicked(Some(&id(Kind::Notification))),
            Some(Controls)
        );
        assert_eq!(presentations.clicked(Some(&id(Kind::Media))), Some(Media));

        presentations.withhold(&[Controls, Launcher]);

        for input in [
            Input::Click(Primary),
            Input::RightClick(Primary),
            Input::Open(Controls),
            Input::Open(Launcher),
        ] {
            presentations.input(MONITOR, input);
            assert_eq!(presentations.get(MONITOR), Rest, "{input:?}");
            assert!(!presentations.pinned(MONITOR), "{input:?}");
        }

        assert_eq!(presentations.auto_expand(MONITOR, Controls), None);

        presentations.withhold(&[Notifications]);
        assert_eq!(presentations.auto_expand(MONITOR, Notifications), None);
        assert_eq!(presentations.get(MONITOR), Rest);

        // what is offered still opens
        presentations.input(MONITOR, Input::Open(Controls));
        assert_eq!(presentations.get(MONITOR), Expanded(Controls));
    }

    #[test]
    fn expanded_names_the_open_island() {
        let mut presentations = Presentations::default();

        presentations.set_shown(MONITOR, Some(id(Kind::Media)), None);
        assert_eq!(presentations.expanded(), None);

        presentations.input(OTHER, Input::Open(Launcher));
        assert_eq!(presentations.expanded(), Some((OTHER, Launcher)));

        presentations.input(MONITOR, Input::Click(Primary));
        assert_eq!(presentations.expanded(), Some((MONITOR, Media)));

        presentations.set_overview(true);
        assert_eq!(presentations.expanded(), None);
    }

    #[test]
    fn at_most_one_island_is_expanded() {
        let mut presentations = Presentations::default();

        presentations.set_shown(MONITOR, Some(id(Kind::Media)), None);
        presentations.set_shown(OTHER, Some(id(Kind::Media)), None);

        presentations.input(OTHER, Input::Open(Launcher));
        presentations.input(MONITOR, Input::Click(Primary));

        assert_eq!(presentations.get(MONITOR), Expanded(Media));
        assert_eq!(presentations.get(OTHER), Compact);

        // a peek elsewhere is not an expansion and stays
        presentations.input(OTHER, Input::Hover(Primary));
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

    // media primary and a timer Satellite on MONITOR, after these inputs
    fn split_after(inputs: &[Input]) -> Presentations {
        let mut presentations = Presentations::default();

        presentations.set_shown(MONITOR, Some(id(Kind::Media)), Some(id(Kind::Timer)));

        for &input in inputs {
            presentations.input(MONITOR, input);
        }

        presentations
    }

    #[test]
    fn split_follows_the_frame() {
        let mut presentations = split_after(&[]);
        assert_eq!(presentations.get(MONITOR), Split);

        presentations.set_shown(MONITOR, Some(id(Kind::Media)), None);
        assert_eq!(presentations.get(MONITOR), Compact);

        presentations.set_shown(MONITOR, Some(id(Kind::Timer)), Some(id(Kind::Media)));
        assert_eq!(presentations.get(MONITOR), Split);

        presentations.set_shown(MONITOR, None, None);
        assert_eq!(presentations.get(MONITOR), Rest);

        // a Satellite is only ever beside a primary
        presentations.set_shown(MONITOR, None, Some(id(Kind::Timer)));
        assert_eq!(presentations.get(MONITOR), Rest);

        presentations.set_untouched(Some(id(Kind::Media)), Some(id(Kind::Timer)));
        assert_eq!(presentations.untouched(), Split);
        assert_eq!(presentations.get(OTHER), Split);
    }

    #[test]
    fn hover_peeks_the_segment_under_the_pointer() {
        for (segment, kind) in [(Primary, Kind::Media), (Satellite, Kind::Timer)] {
            let presentations = split_after(&[Input::Hover(segment)]);

            assert_eq!(presentations.get(MONITOR), Peek, "{segment:?}");
            assert_eq!(
                presentations.peeked(MONITOR),
                Some(&id(kind)),
                "{segment:?}"
            );
        }

        // Compact has one segment, the primary, wherever the pointer is
        let mut presentations = Presentations::default();
        presentations.set_shown(MONITOR, Some(id(Kind::Media)), None);
        presentations.input(MONITOR, Input::Hover(Satellite));

        assert_eq!(presentations.peeked(MONITOR), Some(&id(Kind::Media)));
    }

    #[test]
    fn click_opens_the_surface_of_the_segment_or_the_peek() {
        let opened = |inputs: &[Input]| split_after(inputs).get(MONITOR);

        assert_eq!(opened(&[Input::Click(Primary)]), Expanded(Media));
        assert_eq!(opened(&[Input::Click(Satellite)]), Expanded(Controls));

        // on a Peek, the peeked Activity's wherever the pointer is
        assert_eq!(
            opened(&[Input::Hover(Satellite), Input::Click(Primary)]),
            Expanded(Controls)
        );
        assert_eq!(
            opened(&[Input::Hover(Primary), Input::Click(Satellite)]),
            Expanded(Media)
        );

        // a Satellite with a Surface of its own opens it
        let mut presentations = Presentations::default();
        presentations.set_shown(MONITOR, Some(id(Kind::Timer)), Some(id(Kind::Media)));
        presentations.input(MONITOR, Input::Click(Satellite));

        assert_eq!(presentations.get(MONITOR), Expanded(Media));
    }

    #[test]
    fn right_click_peeks_the_segment_pinned() {
        let presentations = split_after(&[Input::RightClick(Satellite)]);

        assert_eq!(presentations.get(MONITOR), Peek);
        assert_eq!(presentations.peeked(MONITOR), Some(&id(Kind::Timer)));
        assert!(presentations.pinned(MONITOR));

        // on the Peek it toggles the pin and keeps what it shows
        let presentations =
            split_after(&[Input::RightClick(Satellite), Input::RightClick(Primary)]);

        assert_eq!(presentations.peeked(MONITOR), Some(&id(Kind::Timer)));
        assert!(!presentations.pinned(MONITOR));
    }

    #[test]
    fn collapse_returns_to_split() {
        for inputs in [
            &[Input::Click(Satellite), Input::Collapse][..],
            &[Input::Hover(Satellite), Input::Unhover],
            &[Input::RightClick(Satellite), Input::Collapse],
        ] {
            assert_eq!(split_after(inputs).get(MONITOR), Split, "{inputs:?}");
        }
    }

    #[test]
    fn a_peek_lasts_while_its_activity_shows() {
        // the peeked Satellite becomes the primary, and the primary it peeked becomes a Satellite
        for (segment, kind) in [(Satellite, Kind::Timer), (Primary, Kind::Media)] {
            let mut presentations = split_after(&[Input::RightClick(segment)]);

            presentations.set_shown(MONITOR, Some(id(Kind::Timer)), Some(id(Kind::Media)));

            assert_eq!(presentations.get(MONITOR), Peek, "{segment:?}");
            assert_eq!(
                presentations.peeked(MONITOR),
                Some(&id(kind)),
                "{segment:?}"
            );
            assert!(presentations.pinned(MONITOR), "{segment:?}");
        }

        // one that leaves both ends it, pinned or not
        for inputs in [Input::Hover(Satellite), Input::RightClick(Satellite)] {
            let mut presentations = split_after(&[inputs]);

            presentations.set_shown(MONITOR, Some(id(Kind::Media)), Some(id(Kind::Battery)));

            assert_eq!(presentations.get(MONITOR), Split, "{inputs:?}");
            assert_eq!(presentations.peeked(MONITOR), None, "{inputs:?}");
            assert!(!presentations.pinned(MONITOR), "{inputs:?}");
        }
    }

    #[test]
    fn restore_brings_a_peek_back_only_while_its_activity_shows() {
        for (satellite, back) in [(Kind::Timer, Peek), (Kind::Battery, Split)] {
            let mut presentations = split_after(&[Input::RightClick(Satellite)]);
            let prior = presentations.auto_expand(MONITOR, Notifications).unwrap();

            presentations.set_shown(MONITOR, Some(id(Kind::Media)), Some(id(satellite)));
            presentations.restore(prior);

            assert_eq!(presentations.get(MONITOR), back, "{satellite:?}");
            assert_eq!(presentations.pinned(MONITOR), back == Peek, "{satellite:?}");
        }
    }
}
