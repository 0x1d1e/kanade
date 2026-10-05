use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use amane::{Keyboard, Service};

use super::command::Command;
use super::fade::Crossfade;
use super::geometry::{self, REST, Shape};
use super::motion::{Mode, Spring};
use super::presentation::{Input, Presentation, Presentations, Surface};

// plan 5.2 starting values, expand and collapse take the same time
const MORPH: Mode = Mode::Spring {
    response: Duration::from_millis(180),
};

// plan 5.2: a pointer resting on a Compact island for 100-140 ms peeks
const HOVER_DELAY: Duration = Duration::from_millis(120);

// plan 5.2: pointer out collapses after 200-300 ms, back in before that keeps the island open
const GRACE: Duration = Duration::from_millis(250);

/*
 * an island opened without a press holds the keyboard until it collapses (#4), so it collapses on
 * its own after this unless the pointer comes onto it (#48). Keys do not extend it: someone typing
 * into a window who missed the island would hold it forever
 */
const HOLD: Duration = Duration::from_secs(5);

/*
 * a deadline change nudges listen(), which otherwise sleeps until the next deadline (#5);
 * one pending nudge is enough, since listen() reads every deadline again when it wakes
 */
static NUDGE: LazyLock<(SyncSender<()>, Mutex<Receiver<()>>)> = LazyLock::new(|| {
    let (sender, receiver) = mpsc::sync_channel(1);

    (sender, Mutex::new(receiver))
});

// the one Amane-facing piece of island/: owns the Arbiter and per-monitor Presentation
pub struct IslandService {
    // by monitor name, an island nobody touched yet is at rest
    islands: HashMap<String, Island>,

    presentations: Presentations,

    // from niri; none while unknown or without niri, which counts every monitor as focused
    focused_output: Option<String>,
}

#[derive(Default)]
struct Island {
    // the pointer is in the input region and has not pressed Escape, so a press can take focus
    armed: bool,

    // opened without a press (IPC, keybind), so OnDemand never got focus; held until collapse,
    // since niri drops the focus on any switch to OnDemand, even right after a press (#4)
    held: bool,

    // the pointer is in the input region
    inside: bool,

    // none until the first morph; width, height and radius as one spring group
    shape: Option<Spring<3>>,

    // the content, faded by the shape's progress so both change in one motion
    content: Crossfade,

    // what the pointer started: a Peek after the hover delay, or a collapse after the grace
    due: Option<Due>,
}

// an input the island gives itself at `at`, unless something else happens first
#[derive(Debug, Clone, Copy, PartialEq)]
struct Due {
    at: Instant,
    input: Input,
}

impl Due {
    // still meaningful for the island in this Presentation
    fn applies(self, presentation: Presentation) -> bool {
        matches!(
            (self.input, presentation),
            (Input::Hover, Presentation::Compact)
                | (Input::Unhover, Presentation::Peek)
                | (Input::Collapse, Presentation::Expanded(_))
        )
    }
}

impl Service for IslandService {
    fn new() -> Self {
        Self {
            islands: HashMap::new(),
            presentations: Presentations::default(),
            focused_output: None,
        }
    }

    /*
     * sleeps until the next deadline or a nudge, so an idle island never wakes (#5);
     * a nudge sent between the read and the wait is still queued, so the wait never misses it
     */
    fn listen() {
        let receiver = NUDGE.1.lock().unwrap_or_else(PoisonError::into_inner);

        loop {
            // no deadline waits forever, recv_timeout falls back to recv on overflow
            let wait = Self::read().deadline().map_or(Duration::MAX, |deadline| {
                deadline.saturating_duration_since(Instant::now())
            });

            if receiver.recv_timeout(wait) != Err(RecvTimeoutError::Timeout) {
                continue;
            }

            let now = Instant::now();

            // Write cannot be made quiet from here, so only write when something expires
            if Self::read()
                .deadline()
                .is_some_and(|deadline| deadline <= now)
            {
                Self::write().expire(now);
            }
        }
    }
}

impl IslandService {
    pub fn presentation(&self, monitor: &str) -> Presentation {
        self.presentations.get(monitor)
    }

    // plan 5.3: niri's overview is open, so every island rests and passes the pointer through
    pub fn overview(&self) -> bool {
        self.presentations.overview()
    }

    // where FocusedOutput Activities and IPC commands go; every monitor while niri does not say
    pub fn focused(&self, monitor: &str) -> bool {
        self.focused_output
            .as_deref()
            .is_none_or(|focused| focused == monitor)
    }

    /*
     * everything niri tells the core, in one update reconciled once; focus changes no Presentation
     * yet, but will through the Arbiter's scope filter (#19)
     */
    pub fn set_niri(&mut self, focused_output: Option<String>, overview: bool, now: Instant) {
        self.focused_output = focused_output;
        self.presentations.set_overview(overview);
        self.sync(now);
    }

    pub fn expanded(&self, monitor: &str) -> bool {
        matches!(self.presentation(monitor), Presentation::Expanded(_))
    }

    // read in the view at the frame's time
    pub fn shape(&self, monitor: &str, now: Instant) -> Shape {
        self.spring(monitor)
            .map_or(REST, |spring| Shape::from(spring.at(now)))
    }

    // what shows at `now` and how strongly, at most one of them visible
    pub fn content(&self, monitor: &str, now: Instant) -> [Option<(Presentation, f32)>; 2] {
        let Some(island) = self.islands.get(monitor) else {
            return Crossfade::default().shown(1.0);
        };

        let progress = island
            .shape
            .as_ref()
            .map_or(1.0, |spring| spring.progress(now));

        island.content.shown(progress)
    }

    // false while the view has to keep asking for frames
    pub fn settled(&self, monitor: &str, now: Instant) -> bool {
        self.spring(monitor)
            .is_none_or(|spring| spring.settled(now))
    }

    pub fn armed(&self, monitor: &str) -> bool {
        self.islands.get(monitor).is_some_and(|island| island.armed)
    }

    pub fn inside(&self, monitor: &str) -> bool {
        self.islands
            .get(monitor)
            .is_some_and(|island| island.inside)
    }

    // the earliest moment some island changes on its own
    fn deadline(&self) -> Option<Instant> {
        self.islands
            .values()
            .filter_map(|island| island.due.map(|due| due.at))
            .min()
    }

    fn spring(&self, monitor: &str) -> Option<&Spring<3>> {
        self.islands
            .get(monitor)
            .and_then(|island| island.shape.as_ref())
    }

    fn held(&self, monitor: &str) -> bool {
        self.islands.get(monitor).is_some_and(|island| island.held)
    }

    /*
     * OnDemand focuses only on a press, so it must be on before the press that expands (#2).
     * Exclusive would keep the keyboard from overlays opened later (niri gives it to the first
     * mapped exclusive surface), so only an island opened without a press holds it (#4)
     */
    pub fn keyboard(&self, monitor: &str) -> Keyboard {
        if self.held(monitor) {
            Keyboard::Exclusive
        } else if self.expanded(monitor) || self.armed(monitor) {
            Keyboard::OnDemand
        } else {
            Keyboard::None
        }
    }

    /*
     * stand-ins until the Arbiter decides each island's primary from posted Activities (#20);
     * the caller names the island and the Surface of its primary
     */
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "sources post once the Arbiter is wired in, #20")
    )]
    pub fn post(&mut self, monitor: &str, primary: Surface, now: Instant) {
        self.island(monitor);
        self.presentations.set_primary(monitor, Some(primary));
        self.sync(now);
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "sources withdraw once the Arbiter is wired in, #20"
        )
    )]
    pub fn withdraw(&mut self, monitor: &str, now: Instant) {
        self.island(monitor);
        self.presentations.set_primary(monitor, None);
        self.sync(now);
    }

    /*
     * what an IPC command does, decided on a read so a call that changes nothing never writes.
     * It goes to the focused output; without niri every monitor counts as focused, so it goes to
     * the open island, and with none open there is nowhere to open
     */
    pub fn resolve(&self, command: Command) -> Result<Option<Effect>, NoFocus> {
        // nothing is open or opens while the overview is
        if self.overview() {
            return Ok(None);
        }

        let open = self.presentations.expanded();

        // an island open on another output is not the focused one's to close
        let focused = open.filter(|&(monitor, _)| self.focused(monitor));
        let collapse = || Ok(focused.map(|(monitor, _)| Effect::Collapse(monitor.to_owned())));

        let surface = match command {
            Command::Collapse => return collapse(),

            Command::Toggle(surface) if focused.is_some_and(|(_, shown)| shown == surface) => {
                return collapse();
            }

            Command::Open(surface) | Command::Toggle(surface) => surface,
        };

        let monitor = self
            .focused_output
            .as_deref()
            .or(open.map(|(monitor, _)| monitor))
            .ok_or(NoFocus)?;

        if self.presentation(monitor) == Presentation::Expanded(surface) {
            return Ok(None);
        }

        Ok(Some(Effect::Open(monitor.to_owned(), surface)))
    }

    pub fn apply(&mut self, effect: Effect, now: Instant) {
        match effect {
            Effect::Open(monitor, surface) => self.open(&monitor, surface, now),
            Effect::Collapse(monitor) => self.input(&monitor, Input::Collapse, now),
        }
    }

    // expands without a press, so the island holds the keyboard to still get Escape, for a while
    pub fn open(&mut self, monitor: &str, surface: Surface, now: Instant) {
        self.input(monitor, Input::Open(surface), now);

        // nothing opens while the overview is, and a hold without a Surface would starve the keyboard
        let expanded = self.expanded(monitor);
        let island = self.island(monitor);
        island.held = expanded;

        // a pointer already on it collapses after the grace once it leaves instead
        if expanded && !island.inside {
            island.due = Some(Due {
                at: now + HOLD,
                input: Input::Collapse,
            });
            nudge();
        }
    }

    pub fn input(&mut self, monitor: &str, input: Input, now: Instant) {
        self.presentations.input(monitor, input);

        // input that decides something ends a pending Peek or grace; scrolling an open island the
        // pointer just left, which decides nothing yet, still lets it collapse
        if input.decides() && self.island(monitor).due.take().is_some() {
            nudge();
        }

        self.sync(now);
    }

    // every island follows its Presentation, since opening one collapses any other
    fn sync(&mut self, now: Instant) {
        for (monitor, island) in &mut self.islands {
            let presentation = self.presentations.get(monitor);
            let expanded = matches!(presentation, Presentation::Expanded(_));
            let target = geometry::shape(presentation);

            island.held &= expanded;

            if island.due.is_some_and(|due| !due.applies(presentation)) {
                island.due = None;
                nudge();
            }

            if presentation == island.content.target() {
                continue;
            }

            // content and shape start the leg together; an island that never changed has no spring
            let spring = island
                .shape
                .get_or_insert_with(|| Spring::new(REST.into(), MORPH));

            island.content.to(presentation, spring.progress(now));
            spring.to(target.into(), now);
        }
    }

    pub fn set_armed(&mut self, monitor: &str, armed: bool) {
        self.island(monitor).armed = armed;
    }

    /*
     * the pointer entered or left the input region, which arms or disarms the keyboard. In, a
     * Compact island peeks after the hover delay; out, a Peek or an open Surface collapses after the
     * grace; either edge cancels the other
     */
    pub fn hover(&mut self, monitor: &str, inside: bool, now: Instant) {
        let presentation = self.presentation(monitor);
        let island = self.island(monitor);

        if island.inside == inside {
            return;
        }

        // on before any press, since OnDemand focuses only on one (#2)
        island.inside = inside;
        island.armed = inside;

        let due = match (inside, presentation) {
            (true, Presentation::Compact) => Some((HOVER_DELAY, Input::Hover)),
            (false, Presentation::Peek) => Some((GRACE, Input::Unhover)),
            (false, Presentation::Expanded(_)) => Some((GRACE, Input::Collapse)),
            _ => None,
        };

        let due = due.map(|(delay, input)| Due {
            at: now + delay,
            input,
        });

        if island.due.is_some() || due.is_some() {
            island.due = due;
            nudge();
        }
    }

    // applies every input that fell due by now
    pub fn expire(&mut self, now: Instant) {
        let due: Vec<(String, Input)> = self
            .islands
            .iter_mut()
            .filter_map(|(monitor, island)| {
                // taken here, so the input below does not nudge listen() for its own deadline
                let due = island.due.take_if(|due| due.at <= now)?;

                Some((monitor.clone(), due.input))
            })
            .collect();

        for (monitor, input) in due {
            self.input(&monitor, input, now);
        }
    }

    fn island(&mut self, monitor: &str) -> &mut Island {
        self.islands.entry(monitor.to_owned()).or_default()
    }
}

// an IPC command's change to one island, from IslandService::resolve
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Open(String, Surface),
    Collapse(String),
}

// niri has not said which output is focused and no island is open to stand in for it
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoFocus;

impl std::fmt::Display for NoFocus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("island: no focused output known, is niri running?")
    }
}

// never blocks: a nudge already queued wakes listen() just the same
fn nudge() {
    let _ = NUDGE.0.try_send(());
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: &str = "eDP-1";

    #[test]
    fn untouched_island_takes_no_keyboard() {
        assert_eq!(IslandService::new().keyboard(MONITOR), Keyboard::None);
    }

    #[test]
    fn pointer_expanded_island_waits_for_the_press() {
        let mut island = IslandService::new();

        island.set_armed(MONITOR, true);
        assert_eq!(island.keyboard(MONITOR), Keyboard::OnDemand);

        island.input(MONITOR, Input::Click, Instant::now());
        assert_eq!(island.keyboard(MONITOR), Keyboard::OnDemand);
    }

    #[test]
    fn opened_island_holds_the_keyboard_through_pointer_input() {
        let mut island = IslandService::new();

        island.open(MONITOR, Surface::Controls, Instant::now());
        assert_eq!(island.keyboard(MONITOR), Keyboard::Exclusive);

        island.set_armed(MONITOR, true);
        island.input(MONITOR, Input::Click, Instant::now());
        assert_eq!(island.keyboard(MONITOR), Keyboard::Exclusive);
    }

    #[test]
    fn collapse_releases_a_held_keyboard() {
        let mut island = IslandService::new();

        island.open(MONITOR, Surface::Controls, Instant::now());
        island.input(MONITOR, Input::Collapse, Instant::now());
        assert_eq!(island.keyboard(MONITOR), Keyboard::None);

        // a later pointer expand does not bring the hold back
        island.input(MONITOR, Input::Click, Instant::now());
        assert_eq!(island.keyboard(MONITOR), Keyboard::OnDemand);
    }

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn ignored_held_island_collapses_after_the_hold() {
        let mut island = IslandService::new();
        let now = Instant::now();

        island.open(MONITOR, Surface::Controls, now);
        assert_eq!(island.deadline(), Some(now + HOLD));

        island.expire(now + HOLD - ms(1));
        assert_eq!(island.keyboard(MONITOR), Keyboard::Exclusive);

        island.expire(now + HOLD);
        assert!(!island.expanded(MONITOR));
        assert_eq!(island.keyboard(MONITOR), Keyboard::None);
        assert_eq!(island.deadline(), None);
    }

    #[test]
    fn escape_within_the_hold_ends_it() {
        let mut island = IslandService::new();
        let now = Instant::now();

        island.open(MONITOR, Surface::Controls, now);
        island.input(MONITOR, Input::Collapse, now + ms(100));

        assert_eq!(island.keyboard(MONITOR), Keyboard::None);
        assert_eq!(island.deadline(), None);
    }

    #[test]
    fn pointer_on_a_held_island_trades_the_hold_for_the_grace() {
        let mut island = IslandService::new();
        let now = Instant::now();

        island.open(MONITOR, Surface::Controls, now);
        island.hover(MONITOR, true, now + ms(100));
        assert_eq!(island.deadline(), None);

        // resting on it past the hold keeps it open
        island.expire(now + HOLD);
        assert!(island.expanded(MONITOR));

        let out = now + HOLD + ms(100);
        island.hover(MONITOR, false, out);
        assert_eq!(island.deadline(), Some(out + GRACE));
    }

    #[test]
    fn opening_under_the_pointer_sets_no_hold() {
        let mut island = IslandService::new();
        let now = Instant::now();

        island.hover(MONITOR, true, now);
        island.open(MONITOR, Surface::Controls, now);

        assert_eq!(island.keyboard(MONITOR), Keyboard::Exclusive);
        assert_eq!(island.deadline(), None);
    }

    #[test]
    fn switching_the_surface_restarts_the_hold() {
        let mut island = IslandService::new();
        let now = Instant::now();
        let later = now + ms(2000);

        island.open(MONITOR, Surface::Media, now);
        island.open(MONITOR, Surface::Controls, later);

        assert_eq!(island.deadline(), Some(later + HOLD));
    }

    // the pointer is on the open island
    fn expanded(now: Instant) -> IslandService {
        let mut island = IslandService::new();

        island.hover(MONITOR, true, now);
        island.input(MONITOR, Input::Click, now);

        island
    }

    // the pointer is on a Compact island
    fn compact(now: Instant) -> IslandService {
        let mut island = IslandService::new();

        island.post(MONITOR, Surface::Media, now);
        island.hover(MONITOR, true, now);

        island
    }

    #[test]
    fn hover_peeks_after_the_delay() {
        let now = Instant::now();
        let mut island = compact(now);

        assert_eq!(island.deadline(), Some(now + HOVER_DELAY));

        island.expire(now + HOVER_DELAY - ms(1));
        assert_eq!(island.presentation(MONITOR), Presentation::Compact);

        island.expire(now + HOVER_DELAY);
        assert_eq!(island.presentation(MONITOR), Presentation::Peek);
        assert_eq!(island.deadline(), None);
    }

    // a Peek grows under a still pointer, which must already have the keyboard for the press
    #[test]
    fn pointer_in_arms_the_keyboard_through_the_peek() {
        let now = Instant::now();
        let mut island = compact(now);

        assert_eq!(island.keyboard(MONITOR), Keyboard::OnDemand);

        island.expire(now + HOVER_DELAY);
        assert_eq!(island.keyboard(MONITOR), Keyboard::OnDemand);

        island.hover(MONITOR, false, now + ms(500));
        assert_eq!(island.keyboard(MONITOR), Keyboard::None);
    }

    #[test]
    fn pointer_out_before_the_delay_never_peeks() {
        let now = Instant::now();
        let mut island = compact(now);

        island.hover(MONITOR, false, now + ms(60));
        assert_eq!(island.deadline(), None);

        island.expire(now + HOVER_DELAY);
        assert_eq!(island.presentation(MONITOR), Presentation::Compact);
    }

    #[test]
    fn pointer_out_ends_a_peek_after_the_grace() {
        let now = Instant::now();
        let mut island = compact(now);

        island.expire(now + HOVER_DELAY);

        let out = now + ms(500);
        island.hover(MONITOR, false, out);
        assert_eq!(island.deadline(), Some(out + GRACE));

        island.expire(out + GRACE - ms(1));
        assert_eq!(island.presentation(MONITOR), Presentation::Peek);

        island.expire(out + GRACE);
        assert_eq!(island.presentation(MONITOR), Presentation::Compact);
    }

    #[test]
    fn pointer_back_in_keeps_the_peek() {
        let now = Instant::now();
        let mut island = compact(now);

        island.expire(now + HOVER_DELAY);
        island.hover(MONITOR, false, now + ms(500));
        island.hover(MONITOR, true, now + ms(600));

        // back on a Peek, there is nothing left to wait for
        assert_eq!(island.deadline(), None);

        island.expire(now + ms(2_000));
        assert_eq!(island.presentation(MONITOR), Presentation::Peek);
    }

    #[test]
    fn hover_does_nothing_without_a_primary() {
        let mut island = IslandService::new();

        island.hover(MONITOR, true, Instant::now());
        assert_eq!(island.deadline(), None);
    }

    #[test]
    fn click_during_the_delay_expands_and_cancels_it() {
        let now = Instant::now();
        let mut island = compact(now);

        island.input(MONITOR, Input::Click, now + ms(60));
        assert_eq!(island.deadline(), None);
        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Media)
        );
    }

    #[test]
    fn withdrawal_cancels_the_delay_and_the_grace() {
        let now = Instant::now();
        let mut island = compact(now);

        island.withdraw(MONITOR, now + ms(60));
        assert_eq!(island.deadline(), None);

        // a Peek the pointer left rests at once, its grace has nothing to return to
        let mut island = compact(now);

        island.expire(now + HOVER_DELAY);
        island.hover(MONITOR, false, now + ms(500));
        island.withdraw(MONITOR, now + ms(600));

        assert_eq!(island.presentation(MONITOR), Presentation::Rest);
        assert_eq!(island.deadline(), None);
    }

    #[test]
    fn pointer_out_collapses_when_the_grace_runs_out() {
        let now = Instant::now();
        let mut island = expanded(now);

        island.hover(MONITOR, false, now);

        assert!(!island.armed(MONITOR));
        assert_eq!(island.deadline(), Some(now + GRACE));

        island.expire(now + GRACE - ms(1));
        assert!(island.expanded(MONITOR));

        island.expire(now + GRACE);
        assert!(!island.expanded(MONITOR));
        assert_eq!(island.deadline(), None);
    }

    #[test]
    fn pointer_back_in_keeps_the_island_open() {
        let now = Instant::now();
        let mut island = expanded(now);

        island.hover(MONITOR, false, now);
        island.hover(MONITOR, true, now + ms(100));

        assert_eq!(island.deadline(), None);

        island.expire(now + GRACE);
        assert!(island.expanded(MONITOR));
    }

    #[test]
    fn repeated_leave_keeps_the_first_deadline() {
        let now = Instant::now();
        let mut island = expanded(now);

        island.hover(MONITOR, false, now);
        island.hover(MONITOR, false, now + ms(100));

        assert_eq!(island.deadline(), Some(now + GRACE));
    }

    #[test]
    fn leaving_a_resting_island_sets_no_deadline() {
        let mut island = IslandService::new();

        island.hover(MONITOR, true, Instant::now());
        island.hover(MONITOR, false, Instant::now());
        assert_eq!(island.deadline(), None);
    }

    #[test]
    fn explicit_collapse_cancels_the_grace() {
        let now = Instant::now();
        let mut island = expanded(now);

        island.hover(MONITOR, false, now);
        island.input(MONITOR, Input::Collapse, now);

        assert_eq!(island.deadline(), None);
    }

    #[test]
    fn collapsing_under_the_pointer_does_not_peek() {
        let now = Instant::now();
        let mut island = compact(now);

        island.input(MONITOR, Input::Click, now);
        island.input(MONITOR, Input::Collapse, now + ms(500));

        // Escape with the pointer still on the body; it has to leave and come back to peek
        assert_eq!(island.presentation(MONITOR), Presentation::Compact);
        assert_eq!(island.deadline(), None);
    }

    #[test]
    fn right_click_and_wheel_leave_the_timers_and_the_motion_alone() {
        let now = Instant::now();
        let later = now + Duration::from_secs(1);

        for input in [Input::RightClick, Input::Wheel(3.0)] {
            let mut island = compact(now);

            island.input(MONITOR, input, now + ms(60));
            assert_eq!(island.deadline(), Some(now + HOVER_DELAY));

            let mut island = expanded(now);

            island.hover(MONITOR, false, later);
            island.input(MONITOR, input, later);

            assert_eq!(island.deadline(), Some(later + GRACE));
            assert!(island.settled(MONITOR, later));
        }
    }

    // the old content fades out with the start of the morph, the new one in toward its end
    #[test]
    fn content_crossfades_with_the_morph() {
        let now = Instant::now();
        let mut island = compact(now);

        let settled = now + Duration::from_secs(1);
        assert_eq!(
            island.content(MONITOR, settled),
            [None, Some((Presentation::Compact, 1.0))]
        );

        island.input(MONITOR, Input::Click, settled);

        let media = Presentation::Expanded(Surface::Media);
        let shown = |at| -> Vec<_> { island.content(MONITOR, at).into_iter().flatten().collect() };

        assert_eq!(shown(settled), [(Presentation::Compact, 1.0)]);

        let mut seen_media = false;

        for step in 0..400 {
            let at = settled + ms(step);
            let content = shown(at);

            assert!(content.len() <= 1, "{content:?}");

            if let [(presentation, _)] = content[..] {
                assert!(
                    !seen_media || presentation == media,
                    "{content:?} at {step}"
                );
                seen_media |= presentation == media;
            }
        }

        assert_eq!(shown(settled + ms(1_000)), [(media, 1.0)]);
    }

    // Media and Launcher share one shape, so only the spring's in-place leg can time the fade
    #[test]
    fn same_shape_surface_switch_still_crossfades() {
        let now = Instant::now();
        let mut island = compact(now);

        island.input(MONITOR, Input::Click, now);

        let media = Presentation::Expanded(Surface::Media);
        let launcher = Presentation::Expanded(Surface::Launcher);
        let switch = now + Duration::from_secs(1);

        assert_eq!(island.presentation(MONITOR), media);
        assert!(island.settled(MONITOR, switch));

        let shape = island.shape(MONITOR, switch);

        island.input(MONITOR, Input::Open(Surface::Launcher), switch);

        assert_eq!(island.presentation(MONITOR), launcher);
        assert_eq!(geometry::shape(media), geometry::shape(launcher));
        assert!(!island.settled(MONITOR, switch));

        let shown = |at| -> Vec<_> { island.content(MONITOR, at).into_iter().flatten().collect() };

        assert_eq!(shown(switch), [(media, 1.0)]);

        /*
         * Media fades out to nothing, then Launcher fades in from nothing; never back, never both.
         * Nothing shows only at the instant of the handover, so each side of it is near empty
         */
        let mut last = (media, 1.0);
        let mut handover = None;
        let mut step = 0;

        while !island.settled(MONITOR, switch + ms(step)) {
            let at = switch + ms(step);

            assert_eq!(island.shape(MONITOR, at), shape);

            if let [now] = shown(at)[..] {
                match (last.0 == media, now.0 == media) {
                    (true, true) => assert!(now.1 <= last.1, "Media grew at {step} ms"),
                    (false, false) => assert!(now.1 >= last.1, "Launcher dipped at {step} ms"),
                    (true, false) => handover = Some((last.1, now.1)),
                    (false, true) => panic!("Media came back at {step} ms"),
                }

                last = now;
            } else {
                assert_eq!(shown(at), [], "both at {step} ms");
            }

            step += 1;

            assert!(step < 1_000, "never settled");
        }

        let (out, into) = handover.expect("never handed over to Launcher");

        assert!(out < 0.1 && into < 0.1, "handed over at {out} -> {into}");
        assert_eq!(shown(switch + ms(step)), [(launcher, 1.0)]);
    }

    // input that changes no Presentation starts no motion
    #[test]
    fn unchanged_presentation_stays_settled() {
        let now = Instant::now();
        let mut island = expanded(now);
        let later = now + Duration::from_secs(1);

        island.input(MONITOR, Input::Click, later);
        island.post(MONITOR, Surface::Notifications, later);

        assert!(island.settled(MONITOR, later));
    }

    #[test]
    fn every_monitor_is_focused_until_niri_says_which() {
        let now = Instant::now();
        let mut island = IslandService::new();

        assert!(island.focused(MONITOR) && island.focused("HDMI-A-1"));

        island.set_niri(Some(String::from("HDMI-A-1")), false, now);
        assert!(!island.focused(MONITOR) && island.focused("HDMI-A-1"));

        island.set_niri(None, false, now);
        assert!(island.focused(MONITOR) && island.focused("HDMI-A-1"));
    }

    // an open island under the pointer, with its grace running, morphs to rest and lets go of all
    #[test]
    fn overview_collapses_to_rest_and_drops_the_hold_and_the_timers() {
        let now = Instant::now();
        let later = now + Duration::from_secs(1);
        let mut island = compact(now);

        island.open(MONITOR, Surface::Controls, now);
        island.hover(MONITOR, false, now);
        assert_eq!(island.deadline(), Some(now + GRACE));

        island.set_niri(None, true, later);

        assert!(island.overview());
        assert_eq!(island.presentation(MONITOR), Presentation::Rest);
        assert_eq!(island.keyboard(MONITOR), Keyboard::None);
        assert_eq!(island.deadline(), None);
        assert!(!island.settled(MONITOR, later + ms(1)));
        assert_eq!(island.shape(MONITOR, later + Duration::from_secs(1)), REST);

        // IPC cannot open it meanwhile, nor take the keyboard
        island.open(MONITOR, Surface::Controls, later);
        assert_eq!(island.presentation(MONITOR), Presentation::Rest);
        assert_eq!(island.keyboard(MONITOR), Keyboard::None);

        // the primary comes back, the open Surface does not
        let closed = later + Duration::from_secs(1);
        island.set_niri(None, false, closed);

        assert_eq!(island.presentation(MONITOR), Presentation::Compact);
        assert!(!island.settled(MONITOR, closed + ms(1)));
        assert_eq!(
            island.shape(MONITOR, closed + Duration::from_secs(1)),
            geometry::COMPACT
        );
    }

    #[test]
    fn opening_another_island_collapses_this_one() {
        let mut island = IslandService::new();
        let now = Instant::now();

        island.open(MONITOR, Surface::Controls, now);
        island.hover(MONITOR, true, now);
        island.hover(MONITOR, false, now);
        let later = now + ms(100);
        island.input("HDMI-A-1", Input::Click, later);

        assert!(island.expanded("HDMI-A-1"));
        assert!(!island.expanded(MONITOR));

        // its hold and its grace go with it, and it morphs back to rest
        assert_eq!(island.keyboard(MONITOR), Keyboard::None);
        assert_eq!(island.deadline(), None);
        assert!(!island.settled(MONITOR, later + ms(1)));
        assert_eq!(island.shape(MONITOR, later + Duration::from_secs(1)), REST);
    }

    const OTHER: &str = "HDMI-A-1";

    // the island after `command`, applied as the IPC handler does
    fn run(island: &mut IslandService, command: Command, now: Instant) -> Option<Effect> {
        let effect = island.resolve(command).expect("a focused output");

        if let Some(effect) = effect.clone() {
            island.apply(effect, now);
        }

        effect
    }

    fn focused_on(monitor: &str, now: Instant) -> IslandService {
        let mut island = IslandService::new();

        island.set_niri(Some(monitor.to_owned()), false, now);
        island
    }

    #[test]
    fn open_goes_to_the_focused_output_and_holds_the_keyboard() {
        let now = Instant::now();
        let mut island = focused_on(OTHER, now);

        assert_eq!(
            run(&mut island, Command::Open(Surface::Media), now),
            Some(Effect::Open(OTHER.to_owned(), Surface::Media))
        );
        assert_eq!(
            island.presentation(OTHER),
            Presentation::Expanded(Surface::Media)
        );
        assert_eq!(island.presentation(MONITOR), Presentation::Rest);
        assert_eq!(island.keyboard(OTHER), Keyboard::Exclusive);
    }

    #[test]
    fn open_switches_the_surface_and_skips_the_one_showing() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        run(&mut island, Command::Open(Surface::Media), now);
        assert_eq!(run(&mut island, Command::Open(Surface::Media), now), None);

        assert_eq!(
            run(&mut island, Command::Open(Surface::Launcher), now),
            Some(Effect::Open(MONITOR.to_owned(), Surface::Launcher))
        );
        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Launcher)
        );
    }

    #[test]
    fn open_on_the_focused_output_moves_the_open_island_there() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        run(&mut island, Command::Open(Surface::Controls), now);
        island.set_niri(Some(OTHER.to_owned()), false, now);

        // the same Surface open elsewhere is not showing on the focused output
        run(&mut island, Command::Toggle(Surface::Controls), now);

        assert_eq!(
            island.presentation(OTHER),
            Presentation::Expanded(Surface::Controls)
        );
        assert!(!island.expanded(MONITOR));
        assert_eq!(island.keyboard(MONITOR), Keyboard::None);
    }

    #[test]
    fn toggle_opens_switches_and_closes() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        run(&mut island, Command::Toggle(Surface::Media), now);
        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Media)
        );

        run(&mut island, Command::Toggle(Surface::Controls), now);
        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Controls)
        );

        assert_eq!(
            run(&mut island, Command::Toggle(Surface::Controls), now),
            Some(Effect::Collapse(MONITOR.to_owned()))
        );
        assert_eq!(island.presentation(MONITOR), Presentation::Rest);
        assert_eq!(island.keyboard(MONITOR), Keyboard::None);
    }

    #[test]
    fn toggle_closes_a_pointer_opened_surface() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.input(MONITOR, Input::Click, now);
        run(&mut island, Command::Toggle(Surface::Controls), now);

        assert_eq!(island.presentation(MONITOR), Presentation::Rest);
    }

    #[test]
    fn collapse_closes_only_the_focused_island() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        assert_eq!(run(&mut island, Command::Collapse, now), None);

        island.input(OTHER, Input::Click, now);
        assert_eq!(run(&mut island, Command::Collapse, now), None);
        assert!(island.expanded(OTHER));

        island.set_niri(Some(OTHER.to_owned()), false, now);
        assert_eq!(
            run(&mut island, Command::Collapse, now),
            Some(Effect::Collapse(OTHER.to_owned()))
        );
        assert!(!island.expanded(OTHER));
    }

    #[test]
    fn without_niri_commands_go_to_the_open_island() {
        let now = Instant::now();
        let mut island = IslandService::new();

        assert_eq!(island.resolve(Command::Open(Surface::Media)), Err(NoFocus));
        assert_eq!(
            island.resolve(Command::Toggle(Surface::Media)),
            Err(NoFocus)
        );
        assert_eq!(island.resolve(Command::Collapse), Ok(None));

        island.input(OTHER, Input::Click, now);

        assert_eq!(
            run(&mut island, Command::Open(Surface::Media), now),
            Some(Effect::Open(OTHER.to_owned(), Surface::Media))
        );
        assert_eq!(
            run(&mut island, Command::Toggle(Surface::Media), now),
            Some(Effect::Collapse(OTHER.to_owned()))
        );
        assert_eq!(island.resolve(Command::Open(Surface::Media)), Err(NoFocus));

        island.input(OTHER, Input::Click, now);
        assert_eq!(
            run(&mut island, Command::Collapse, now),
            Some(Effect::Collapse(OTHER.to_owned()))
        );
    }

    #[test]
    fn overview_takes_no_command() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        run(&mut island, Command::Open(Surface::Media), now);
        island.set_niri(Some(MONITOR.to_owned()), true, now);

        for command in [
            Command::Open(Surface::Launcher),
            Command::Toggle(Surface::Media),
            Command::Collapse,
        ] {
            assert_eq!(island.resolve(command), Ok(None), "{command:?}");
        }

        // the overview collapsed the island for good, so without niri nothing stands in for focus
        island.set_niri(None, false, now);
        assert_eq!(island.resolve(Command::Open(Surface::Media)), Err(NoFocus));
    }
}
