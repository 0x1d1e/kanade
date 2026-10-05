use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use amane::{Keyboard, Service};

use super::activity::{Activity, Frame, Id, Interrupt, Lifetime, Scope};
use super::arbiter::{self, Arbiter};
use super::command::Command;
use super::fade::Crossfade;
use super::geometry::{self, REST, Shape};
use super::motion::{Mode, Spring};
use super::presentation::{Content, Input, Presentation, Presentations, Surface};

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
 * its own after this unless the pointer comes onto it (#48). Only keys the open Surface consumes
 * restart it (`attend`): someone typing into a window who missed the island would hold it forever
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
    // by monitor name, from the first input or focus on it
    islands: HashMap<String, Island>,

    /*
     * every island not in `islands`: unfocused while niri names the focused output, which it
     * touches, and never raised, so they all show one Frame and morph as one
     */
    untouched: Island,

    arbiter: Arbiter,

    presentations: Presentations,

    // from niri; none while unknown or without niri, which counts every monitor as focused
    focused_output: Option<String>,
}

#[derive(Clone, Default)]
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
    content: Crossfade<Content>,

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
            untouched: Island::default(),
            arbiter: Arbiter::default(),
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

            // Write cannot be made quiet from here, so only write when something falls due
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
     * everything niri tells the core, in one update reconciled once. Focus moves the
     * FocusedOutput Activities; the focused island is touched, so every untouched one is unfocused
     */
    pub fn set_niri(&mut self, focused_output: Option<String>, overview: bool, now: Instant) {
        if let Some(monitor) = &focused_output {
            self.island(monitor);
        }

        self.focused_output = focused_output;
        self.presentations.set_overview(overview);
        self.sync(now);
    }

    // what the Arbiter shows on this island at `now`
    pub fn frame(&self, monitor: &str, now: Instant) -> Frame {
        self.arbiter.frame(
            now,
            arbiter::Island {
                focused: self.focused(monitor),
                expanded: self.expanded(monitor),
            },
        )
    }

    pub fn dnd(&self) -> bool {
        self.arbiter.dnd()
    }

    // registered, live or not yet swept; a source checks this so withdrawing nothing never writes
    pub fn contains(&self, id: &Id) -> bool {
        self.arbiter.contains(id)
    }

    // the Surface open on any island; at most one island is Expanded
    pub fn surface(&self) -> Option<Surface> {
        self.presentations.expanded().map(|(_, surface)| surface)
    }

    // the monitor of the one Expanded island, if any
    pub fn expanded_on(&self) -> Option<&str> {
        self.presentations.expanded().map(|(monitor, _)| monitor)
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
    pub fn content(&self, monitor: &str, now: Instant) -> [Option<(Content, f32)>; 2] {
        let island = self.get(monitor);

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
        self.get(monitor).armed
    }

    pub fn inside(&self, monitor: &str) -> bool {
        self.get(monitor).inside
    }

    // the earliest moment some island or the Arbiter changes on its own
    fn deadline(&self) -> Option<Instant> {
        self.islands
            .values()
            .filter_map(|island| island.due.map(|due| due.at))
            .chain(self.arbiter.deadline())
            .min()
    }

    fn spring(&self, monitor: &str) -> Option<&Spring<3>> {
        self.get(monitor).shape.as_ref()
    }

    // right clicked to stay open after the pointer leaves
    pub fn pinned(&self, monitor: &str) -> bool {
        self.presentations.pinned(monitor)
    }

    // opened without a press, so it holds the keyboard until it collapses
    pub fn held(&self, monitor: &str) -> bool {
        self.get(monitor).held
    }

    // this opening of a Surface, see `Presentations::visit`
    pub fn visit(&self) -> u64 {
        self.presentations.visit()
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
     * a Critical Activity arriving collapses the open Surface it shows on (plan 5.1 rule 4), an
     * existing one escalated to Critical included; a repost of one already up does not, so a
     * Surface reopened over it stays
     */
    pub fn post(&mut self, activity: Activity, now: Instant) {
        let global = activity.scope() == Scope::Global;

        // a Transient the open Surface already shows would only wait behind it as a badge
        let absorbed = self
            .presentations
            .expanded()
            .is_some_and(|(monitor, surface)| {
                matches!(activity.lifetime(), Lifetime::Transient(_))
                    && (global || self.focused(monitor))
                    && surface.shows(&activity)
            });
        if absorbed {
            return;
        }

        let arrives = activity.interrupt() == Interrupt::Preempt
            && !self.arbiter.preempting(activity.id(), now);

        self.change(|arbiter| arbiter.post(activity, now));

        let open = self
            .presentations
            .expanded()
            .map(|(monitor, _)| monitor.to_owned());

        match open {
            Some(monitor) if arrives && (global || self.focused(&monitor)) => {
                self.input(&monitor, Input::Preempt, now);
            }
            _ => self.sync(now),
        }
    }

    pub fn withdraw(&mut self, id: &Id, now: Instant) {
        self.change(|arbiter| {
            arbiter.withdraw(id);
        });
        self.sync(now);
    }

    pub fn set_dnd(&mut self, dnd: bool, now: Instant) {
        self.arbiter.set_dnd(dnd);
        self.sync(now);
    }

    // changes the Arbiter, telling listen() when that moves its deadline
    fn change(&mut self, change: impl FnOnce(&mut Arbiter)) {
        let before = self.arbiter.deadline();

        change(&mut self.arbiter);

        if self.arbiter.deadline() != before {
            nudge();
        }
    }

    /*
     * what an IPC command does, decided on a read so a call that changes nothing never writes.
     * It goes to the focused output; without niri every monitor counts as focused, so it goes to
     * the open island, and with none open there is nowhere to open
     */
    pub fn resolve(&self, command: Command) -> Result<Option<Effect>, NoFocus> {
        let open = self.presentations.expanded();

        // an island open on another output is not the focused one's to close
        let focused = open.filter(|&(monitor, _)| self.focused(monitor));
        let collapse = || Ok(focused.map(|(monitor, _)| Effect::Collapse(monitor.to_owned())));

        let surface = match command {
            // Activities and DND reach the Arbiter whatever the islands show
            Command::Post(activity) => return Ok(Some(Effect::Post(activity))),
            Command::Withdraw(id) => {
                return Ok(self.arbiter.contains(&id).then_some(Effect::Withdraw(id)));
            }
            Command::ToggleDnd => return Ok(Some(Effect::Dnd(!self.dnd()))),

            // the timer's own source runs these, the island only shows what it posts
            Command::StartTimer(_) | Command::StopTimer => return Ok(None),

            // nothing is open or opens while the overview is
            _ if self.overview() => return Ok(None),

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

        // opening the Surface already showing still restarts the hold, which a pointer on it has none of
        if self.presentation(monitor) == Presentation::Expanded(surface) && self.inside(monitor) {
            return Ok(None);
        }

        Ok(Some(Effect::Open(monitor.to_owned(), surface)))
    }

    pub fn apply(&mut self, effect: Effect, now: Instant) {
        match effect {
            Effect::Open(monitor, surface) => self.open(&monitor, surface, now),
            Effect::Collapse(monitor) => self.input(&monitor, Input::Collapse, now),
            Effect::Post(activity) => self.post(activity, now),
            Effect::Withdraw(id) => self.withdraw(&id, now),
            Effect::Dnd(dnd) => self.set_dnd(dnd, now),
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

    /*
     * a key the open Surface consumed: someone is using it, so a held island the pointer is not on
     * waits the whole hold again. Keys it ignores never get here, so they still do not extend it
     */
    pub fn attend(&mut self, monitor: &str, now: Instant) {
        let island = self.island(monitor);

        if island.held
            && !island.inside
            && island.due.is_some_and(|due| due.input == Input::Collapse)
        {
            island.due = Some(Due {
                at: now + HOLD,
                input: Input::Collapse,
            });
            nudge();
        }
    }

    pub fn input(&mut self, monitor: &str, input: Input, now: Instant) {
        // touched before the Presentation is, so sync gives it its own Frame from now on
        let island = self.island(monitor);

        // input that decides something ends a pending Peek or grace; scrolling an open island the
        // pointer just left, which decides nothing yet, still lets it collapse
        if input.decides() && island.due.take().is_some() {
            nudge();
        }

        self.presentations.input(monitor, input);

        /*
         * a pinned island stays open with nobody on it, so it waits for nothing and gives back a
         * held keyboard: holding it unattended would starve every other window (#48)
         */
        if self.presentations.pinned(monitor) {
            let island = self.island(monitor);
            island.held = false;

            if island.due.take().is_some() {
                nudge();
            }
        }

        self.sync(now);
    }

    /*
     * every island follows its Frame and its Presentation, since a post reaches many islands and
     * opening one collapses any other. The primary of a Presentation is what shows: the
     * Transient over the primary Activity, or that one
     */
    fn sync(&mut self, now: Instant) {
        let showing = |frame: Frame| frame.transient.or(frame.primary);

        let untouched = showing(self.arbiter.frame(
            now,
            arbiter::Island {
                focused: self.focused_output.is_none(),
                expanded: false,
            },
        ));

        self.presentations.set_untouched(
            untouched
                .as_ref()
                .map(|activity| Surface::of(activity.kind())),
        );

        let shown: Vec<(String, Option<Activity>)> = self
            .islands
            .keys()
            .map(|monitor| (monitor.clone(), showing(self.frame(monitor, now))))
            .collect();

        for (monitor, shown) in &shown {
            let primary = shown.as_ref().map(|activity| Surface::of(activity.kind()));

            self.presentations.set_primary(monitor, primary);
        }

        let presentation = self.presentations.untouched();
        follow(
            &mut self.untouched,
            Content::new(presentation, untouched),
            now,
        );

        for (monitor, shown) in shown {
            let presentation = self.presentations.get(&monitor);

            if let Some(island) = self.islands.get_mut(&monitor) {
                follow(island, Content::new(presentation, shown), now);
            }
        }
    }

    fn get(&self, monitor: &str) -> &Island {
        self.islands.get(monitor).unwrap_or(&self.untouched)
    }

    pub fn set_armed(&mut self, monitor: &str, armed: bool) {
        self.island(monitor).armed = armed;
    }

    /*
     * the pointer entered or left the input region, which arms or disarms the keyboard. In, a
     * Compact island peeks after the hover delay; out, a Peek or an open Surface collapses after the
     * grace unless pinned; either edge cancels the other
     */
    pub fn hover(&mut self, monitor: &str, inside: bool, now: Instant) {
        let presentation = self.presentation(monitor);
        let pinned = self.pinned(monitor);
        let island = self.island(monitor);

        if island.inside == inside {
            return;
        }

        // on before any press, since OnDemand focuses only on one (#2)
        island.inside = inside;
        island.armed = inside;

        let due = match (inside, presentation) {
            (true, Presentation::Compact) => Some((HOVER_DELAY, Input::Hover)),
            (false, Presentation::Peek) if !pinned => Some((GRACE, Input::Unhover)),
            (false, Presentation::Expanded(_)) if !pinned => Some((GRACE, Input::Collapse)),
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

    // applies every input that fell due by now, and drops the Transients that expired
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

        // they were out of every Frame from their expiry on, the next sync shows that
        if self.arbiter.expire(now) {
            self.sync(now);
        }
    }

    // the first touch starts from where every untouched island is, mid-morph included
    fn island(&mut self, monitor: &str) -> &mut Island {
        self.islands
            .entry(monitor.to_owned())
            .or_insert_with(|| self.untouched.clone())
    }
}

// the island morphs to `content`, unless it is already headed there
fn follow(island: &mut Island, content: Content, now: Instant) {
    let presentation = content.presentation;
    let expanded = matches!(presentation, Presentation::Expanded(_));

    island.held &= expanded;

    if island.due.is_some_and(|due| !due.applies(presentation)) {
        island.due = None;
        nudge();
    }

    if content == *island.content.target() {
        return;
    }

    /*
     * content and shape start the leg together, the same shape too, so a new Activity in the same
     * form still crossfades; a level that moved starts none. An island that never changed has no
     * spring
     */
    let spring = island
        .shape
        .get_or_insert_with(|| Spring::new(REST.into(), MORPH));

    if island.content.to(content, spring.progress(now)) {
        spring.to(geometry::shape(presentation).into(), now);
    }
}

// an IPC command's change, from IslandService::resolve
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Open(String, Surface),
    Collapse(String),
    Post(Activity),
    Withdraw(Id),
    Dnd(bool),
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
    use crate::island::activity::{Detail, Device, Kind, Priority, Volume};

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

    #[test]
    fn a_consumed_key_restarts_the_hold() {
        let mut island = IslandService::new();
        let now = Instant::now();
        let later = now + ms(4000);

        island.open(MONITOR, Surface::Notifications, now);
        island.attend(MONITOR, later);
        assert_eq!(island.deadline(), Some(later + HOLD));

        island.expire(now + HOLD);
        assert!(island.expanded(MONITOR));
    }

    #[test]
    fn a_key_holds_nothing_the_pointer_opened_or_rests_on() {
        let mut island = IslandService::new();
        let now = Instant::now();

        island.input(MONITOR, Input::Open(Surface::Notifications), now);
        island.attend(MONITOR, now + ms(100));
        assert_eq!(island.deadline(), None);

        island.open(MONITOR, Surface::Notifications, now);
        island.hover(MONITOR, true, now + ms(100));
        island.attend(MONITOR, now + ms(200));
        assert_eq!(island.deadline(), None);

        // and nothing once it collapsed
        island.input(MONITOR, Input::Collapse, now + ms(300));
        island.attend(MONITOR, now + ms(400));
        assert_eq!(island.deadline(), None);
    }

    // the pointer is on the open island
    fn expanded(now: Instant) -> IslandService {
        let mut island = IslandService::new();

        island.hover(MONITOR, true, now);
        island.input(MONITOR, Input::Click, now);

        island
    }

    fn media() -> Activity {
        Activity::persistent(Id::new(Kind::Media, "spotify"), Priority::Media)
    }

    // the pointer is on a Compact island
    fn compact(now: Instant) -> IslandService {
        let mut island = IslandService::new();

        island.post(media(), now);
        island.hover(MONITOR, true, now);

        island
    }

    // the Presentations showing at `at` and how strongly
    fn presentations(island: &IslandService, at: Instant) -> Vec<(Presentation, f32)> {
        island
            .content(MONITOR, at)
            .into_iter()
            .flatten()
            .map(|(content, opacity)| (content.presentation, opacity))
            .collect()
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

        island.withdraw(media().id(), now + ms(60));
        assert_eq!(island.deadline(), None);

        // a Peek the pointer left rests at once, its grace has nothing to return to
        let mut island = compact(now);

        island.expire(now + HOVER_DELAY);
        island.hover(MONITOR, false, now + ms(500));
        island.withdraw(media().id(), now + ms(600));

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
    fn wheel_leaves_the_timers_and_the_motion_alone() {
        let now = Instant::now();
        let later = now + Duration::from_secs(1);

        let mut island = compact(now);

        island.input(MONITOR, Input::Wheel(3.0), now + ms(60));
        assert_eq!(island.deadline(), Some(now + HOVER_DELAY));

        let mut island = expanded(now);

        island.hover(MONITOR, false, later);
        island.input(MONITOR, Input::Wheel(3.0), later);

        assert_eq!(island.deadline(), Some(later + GRACE));
        assert!(island.settled(MONITOR, later));
    }

    #[test]
    fn a_pinned_surface_outlasts_the_pointer() {
        let now = Instant::now();
        let mut island = expanded(now);

        island.input(MONITOR, Input::RightClick, now);
        island.hover(MONITOR, false, now + ms(10));

        assert_eq!(island.deadline(), None);
        assert!(island.pinned(MONITOR));
        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Controls)
        );

        // unpinned, leaving collapses after the grace again
        let back = now + ms(20);
        island.hover(MONITOR, true, back);
        island.input(MONITOR, Input::RightClick, back);
        island.hover(MONITOR, false, back);

        assert_eq!(island.deadline(), Some(back + GRACE));
    }

    #[test]
    fn right_click_during_the_hover_delay_peeks_pinned_at_once() {
        let now = Instant::now();
        let mut island = compact(now);

        island.input(MONITOR, Input::RightClick, now + ms(60));
        assert_eq!(island.deadline(), None);
        assert_eq!(island.presentation(MONITOR), Presentation::Peek);

        island.hover(MONITOR, false, now + ms(80));
        island.expire(now + Duration::from_secs(10));

        assert_eq!(island.presentation(MONITOR), Presentation::Peek);

        // Escape ends it
        island.input(MONITOR, Input::Collapse, now + Duration::from_secs(10));
        assert_eq!(island.presentation(MONITOR), Presentation::Compact);
        assert!(!island.pinned(MONITOR));
    }

    // a pinned island stays open by itself, so a held keyboard would never come back to anyone
    #[test]
    fn pinning_a_held_island_gives_the_keyboard_back() {
        let now = Instant::now();
        let mut island = IslandService::new();

        island.open(MONITOR, Surface::Launcher, now);
        island.hover(MONITOR, true, now + ms(10));
        island.input(MONITOR, Input::RightClick, now + ms(20));

        assert!(!island.held(MONITOR));
        assert_eq!(island.keyboard(MONITOR), Keyboard::OnDemand);

        island.hover(MONITOR, false, now + ms(30));
        island.expire(now + HOLD + GRACE);

        assert_eq!(island.deadline(), None);
        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Launcher)
        );

        // unpinning does not bring the hold back
        island.input(MONITOR, Input::RightClick, now + HOLD + GRACE);
        assert!(!island.held(MONITOR));
    }

    // the old content fades out with the start of the morph, the new one in toward its end
    #[test]
    fn content_crossfades_with_the_morph() {
        let now = Instant::now();
        let mut island = compact(now);

        let settled = now + Duration::from_secs(1);
        assert_eq!(
            island.content(MONITOR, settled),
            [
                None,
                Some((Content::new(Presentation::Compact, Some(media())), 1.0))
            ]
        );

        island.input(MONITOR, Input::Click, settled);

        let media = Presentation::Expanded(Surface::Media);
        let shown = |at| presentations(&island, at);

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

    // Notifications and Launcher share one shape, so only the spring's in-place leg can time the fade
    #[test]
    fn same_shape_surface_switch_still_crossfades() {
        let now = Instant::now();
        let mut island = compact(now);

        island.input(MONITOR, Input::Click, now);
        island.input(MONITOR, Input::Open(Surface::Notifications), now);

        let notifications = Presentation::Expanded(Surface::Notifications);
        let launcher = Presentation::Expanded(Surface::Launcher);
        let switch = now + Duration::from_secs(1);

        assert_eq!(island.presentation(MONITOR), notifications);
        assert!(island.settled(MONITOR, switch));

        let shape = island.shape(MONITOR, switch);

        island.input(MONITOR, Input::Open(Surface::Launcher), switch);

        assert_eq!(island.presentation(MONITOR), launcher);
        assert_eq!(geometry::shape(notifications), geometry::shape(launcher));
        assert!(!island.settled(MONITOR, switch));

        let shown = |at| presentations(&island, at);

        assert_eq!(shown(switch), [(notifications, 1.0)]);

        /*
         * Notifications fades out to nothing, then Launcher fades in from nothing; never back, never
         * both. Nothing shows only at the instant of the handover, so each side of it is near empty
         */
        let mut last = (notifications, 1.0);
        let mut handover = None;
        let mut step = 0;

        while !island.settled(MONITOR, switch + ms(step)) {
            let at = switch + ms(step);

            assert_eq!(island.shape(MONITOR, at), shape);

            if let [now] = shown(at)[..] {
                match (last.0 == notifications, now.0 == notifications) {
                    (true, true) => assert!(now.1 <= last.1, "Notifications grew at {step} ms"),
                    (false, false) => assert!(now.1 >= last.1, "Launcher dipped at {step} ms"),
                    (true, false) => handover = Some((last.1, now.1)),
                    (false, true) => panic!("Notifications came back at {step} ms"),
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
        island.post(
            Activity::persistent(Id::new(Kind::Notification, "7"), Priority::Actionable),
            later,
        );

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
    fn open_switches_the_surface() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        run(&mut island, Command::Open(Surface::Media), now);

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
    fn opening_the_surface_showing_restarts_the_hold() {
        let now = Instant::now();
        let later = now + ms(4000);
        let mut island = focused_on(MONITOR, now);

        run(&mut island, Command::Open(Surface::Media), now);

        assert_eq!(
            run(&mut island, Command::Open(Surface::Media), later),
            Some(Effect::Open(MONITOR.to_owned(), Surface::Media))
        );
        assert_eq!(island.deadline(), Some(later + HOLD));

        island.expire(now + HOLD);
        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Media)
        );
    }

    // under the pointer there is no hold to restart, so a repeat changes nothing and never writes
    #[test]
    fn opening_the_surface_showing_under_the_pointer_is_skipped() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.hover(MONITOR, true, now);
        run(&mut island, Command::Open(Surface::Media), now);

        assert_eq!(island.resolve(Command::Open(Surface::Media)), Ok(None));
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
            assert_eq!(island.resolve(command.clone()), Ok(None), "{command:?}");
        }

        // the overview collapsed the island for good, so without niri nothing stands in for focus
        island.set_niri(None, false, now);
        assert_eq!(island.resolve(Command::Open(Surface::Media)), Err(NoFocus));
    }

    fn volume() -> Activity {
        Activity::transient(Id::new(Kind::Volume, "volume"), Priority::Osd, OSD)
    }

    fn battery() -> Activity {
        Activity::persistent(Id::new(Kind::Battery, "BAT0"), Priority::Critical)
    }

    const OSD: Duration = Duration::from_millis(1200);

    fn shown(island: &IslandService, monitor: &str, now: Instant) -> Option<Activity> {
        let frame = island.frame(monitor, now);

        frame.transient.or(frame.primary)
    }

    // the accept case of #20: shows, expires, and the persistent one returns with no re-post
    #[test]
    fn transient_over_a_persistent_shows_expires_and_returns() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.post(media(), now);
        island.post(volume(), now + ms(100));

        let expiry = now + ms(100) + OSD;

        assert_eq!(shown(&island, MONITOR, now + ms(100)), Some(volume()));
        assert_eq!(island.presentation(MONITOR), Presentation::Compact);
        assert_eq!(island.deadline(), Some(expiry));

        island.expire(expiry - ms(1));
        assert_eq!(
            island.get(MONITOR).content.target().activity,
            Some(volume())
        );

        island.expire(expiry);
        assert_eq!(shown(&island, MONITOR, expiry), Some(media()));
        assert_eq!(island.get(MONITOR).content.target().activity, Some(media()));
        assert_eq!(island.presentation(MONITOR), Presentation::Compact);
        assert_eq!(island.deadline(), None);
    }

    // Compact to Compact keeps the shape, so only the spring's in-place leg times the crossfade
    #[test]
    fn a_new_activity_in_the_same_form_crossfades() {
        let now = Instant::now();
        let later = now + Duration::from_secs(1);
        let mut island = focused_on(MONITOR, now);

        island.post(media(), now);
        assert!(island.settled(MONITOR, later));

        island.post(volume(), later);

        assert!(!island.settled(MONITOR, later + ms(1)));
        assert_eq!(island.shape(MONITOR, later + ms(50)), geometry::COMPACT);
        assert_eq!(
            island.content(MONITOR, later)[0]
                .as_ref()
                .map(|(content, _)| content.activity.clone()),
            Some(Some(media()))
        );

        // a repost of the same Activity changes nothing on screen
        let settled = later + Duration::from_secs(1);
        island.post(volume(), settled);
        assert!(island.settled(MONITOR, settled));
    }

    // a held volume key: each step redraws the bar where it stands, mid-morph or settled
    #[test]
    fn a_moved_level_redraws_in_place() {
        let level = |percent| {
            volume().with_detail(Detail::Volume(Volume {
                device: Device::Speaker,
                percent,
                muted: false,
            }))
        };
        let showing = |island: &IslandService, at| {
            island.content(MONITOR, at).map(|shown| {
                shown.map(|(content, opacity)| {
                    (content.activity.map(|a| a.detail().clone()), opacity)
                })
            })
        };

        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.post(media(), now);
        island.post(level(40), now + ms(500));

        // mid-morph, the step only changes what fades in
        let mid = now + ms(560);
        let before = showing(&island, mid);
        island.post(level(45), mid);
        let after = showing(&island, mid);

        assert_eq!(before[0], after[0]);
        assert_eq!(
            before[1].as_ref().map(|(_, o)| *o),
            after[1].as_ref().map(|(_, o)| *o)
        );
        assert_eq!(
            island.get(MONITOR).content.target().activity,
            Some(level(45))
        );

        let settled = now + Duration::from_secs(1);
        assert!(island.settled(MONITOR, settled));

        island.post(level(50), settled);
        assert!(island.settled(MONITOR, settled));
        assert_eq!(
            showing(&island, settled),
            [
                None,
                Some((
                    Some(Detail::Volume(Volume {
                        device: Device::Speaker,
                        percent: 50,
                        muted: false,
                    })),
                    1.0
                ))
            ]
        );
    }

    // the Media Surface sets the speaker volume, so its Transient never queues behind it
    #[test]
    fn the_open_surface_absorbs_what_it_shows() {
        let level = |device| {
            volume().with_detail(Detail::Volume(Volume {
                device,
                percent: 40,
                muted: false,
            }))
        };

        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);
        run(&mut island, Command::Open(Surface::Media), now);

        island.post(level(Device::Speaker), now + ms(100));
        assert!(!island.contains(level(Device::Speaker).id()));

        island.post(level(Device::Microphone), now + ms(200));
        assert!(island.contains(level(Device::Microphone).id()));
        assert_eq!(island.surface(), Some(Surface::Media));
    }

    #[test]
    fn transient_shows_only_on_the_focused_island() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.post(volume(), now);

        assert_eq!(island.presentation(MONITOR), Presentation::Compact);
        assert_eq!(island.presentation(OTHER), Presentation::Rest);

        // focus moves it, an untouched island included
        island.set_niri(Some(OTHER.to_owned()), false, now + ms(100));

        assert_eq!(island.presentation(MONITOR), Presentation::Rest);
        assert_eq!(island.presentation(OTHER), Presentation::Compact);
        assert_eq!(island.presentation("DP-1"), Presentation::Rest);
    }

    #[test]
    fn global_post_morphs_every_island_touched_or_not() {
        let now = Instant::now();
        let later = now + Duration::from_secs(1);
        let mut island = focused_on(MONITOR, now);

        island.post(media(), now);

        for monitor in [MONITOR, OTHER, "DP-1"] {
            assert_eq!(island.presentation(monitor), Presentation::Compact);
            assert!(!island.settled(monitor, now + ms(1)), "{monitor}");
            assert_eq!(island.shape(monitor, later), geometry::COMPACT, "{monitor}");
        }

        // touching one mid-morph keeps its motion
        let mid = now + ms(60);
        let shape = island.shape(OTHER, mid);

        island.hover(OTHER, true, mid);
        assert_eq!(island.shape(OTHER, mid), shape);

        island.withdraw(media().id(), later);

        for monitor in [MONITOR, OTHER, "DP-1"] {
            assert_eq!(
                island.presentation(monitor),
                Presentation::Rest,
                "{monitor}"
            );
        }
    }

    #[test]
    fn click_opens_the_surface_of_what_shows() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);
        let toast = Activity::transient(
            Id::new(Kind::Notification, "7"),
            Priority::Actionable,
            Duration::from_secs(5),
        );

        island.post(media(), now);
        island.post(toast, now);
        island.input(MONITOR, Input::Click, now);

        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Notifications)
        );
    }

    // plan 5.1 rule 4: a toast waits behind an open Surface and shows once it closes
    #[test]
    fn queued_transient_shows_after_the_surface_closes() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.post(media(), now);
        island.input(MONITOR, Input::Click, now);
        island.post(volume(), now + ms(100));

        assert!(island.expanded(MONITOR));
        assert_eq!(island.frame(MONITOR, now + ms(100)).queued, [volume()]);

        island.input(MONITOR, Input::Collapse, now + ms(200));

        assert_eq!(shown(&island, MONITOR, now + ms(200)), Some(volume()));
        assert_eq!(
            island.get(MONITOR).content.target().activity,
            Some(volume())
        );
    }

    #[test]
    fn critical_arriving_collapses_the_open_surface_once() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.post(media(), now);
        island.open(MONITOR, Surface::Media, now);
        island.post(battery(), now + ms(100));

        assert_eq!(island.presentation(MONITOR), Presentation::Compact);
        assert_eq!(island.keyboard(MONITOR), Keyboard::None);
        assert_eq!(shown(&island, MONITOR, now + ms(100)), Some(battery()));

        // reopened over it, a repost of the same Activity leaves it open
        island.input(MONITOR, Input::Click, now + ms(200));
        island.post(battery(), now + ms(300));

        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Controls)
        );
    }

    // the same Id escalated to Critical arrives, though it was registered all along
    #[test]
    fn ongoing_activity_becoming_critical_preempts_open_surface() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);
        let low = Activity::persistent(Id::new(Kind::Battery, "BAT0"), Priority::Ongoing);

        island.post(low, now);
        island.open(MONITOR, Surface::Controls, now + ms(100));
        island.post(battery(), now + ms(200));

        assert_eq!(island.presentation(MONITOR), Presentation::Compact);
        assert_eq!(island.keyboard(MONITOR), Keyboard::None);
        assert_eq!(shown(&island, MONITOR, now + ms(200)), Some(battery()));
    }

    // past its expiry it is gone, even before expire() sweeps it, so a repost arrives again
    #[test]
    fn expired_critical_reposted_preempts_again() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);
        let call = Activity::transient(Id::new(Kind::Privacy, "call"), Priority::Critical, OSD);

        island.post(call.clone(), now);

        let expiry = now + OSD;

        island.open(MONITOR, Surface::Controls, expiry);
        island.post(call.clone(), expiry);

        assert_eq!(island.presentation(MONITOR), Presentation::Compact);
        assert_eq!(shown(&island, MONITOR, expiry), Some(call));
    }

    #[test]
    fn critical_transient_preempts_only_the_focused_island() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);
        let call = Activity::transient(Id::new(Kind::Privacy, "call"), Priority::Critical, OSD);

        island.input(OTHER, Input::Click, now);
        island.post(call.clone(), now);
        assert!(island.expanded(OTHER));

        island.withdraw(call.id(), now);
        island.set_niri(Some(OTHER.to_owned()), false, now);
        island.post(call, now);
        assert!(!island.expanded(OTHER));
    }

    #[test]
    fn critical_never_preempts_under_the_overview() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.set_niri(Some(MONITOR.to_owned()), true, now);
        island.post(battery(), now);
        assert_eq!(island.presentation(MONITOR), Presentation::Rest);

        island.set_niri(Some(MONITOR.to_owned()), false, now);
        assert_eq!(island.presentation(MONITOR), Presentation::Compact);
    }

    #[test]
    fn debug_commands_post_withdraw_and_toggle_dnd() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        assert_eq!(
            island.resolve(Command::Withdraw(media().id().clone())),
            Ok(None)
        );

        run(&mut island, Command::Post(media()), now);
        assert_eq!(island.presentation(MONITOR), Presentation::Compact);

        run(&mut island, Command::Withdraw(media().id().clone()), now);
        assert_eq!(island.presentation(MONITOR), Presentation::Rest);

        assert_eq!(
            run(&mut island, Command::ToggleDnd, now),
            Some(Effect::Dnd(true))
        );
        assert!(island.dnd());
        assert_eq!(
            run(&mut island, Command::ToggleDnd, now),
            Some(Effect::Dnd(false))
        );
        assert!(!island.dnd());

        // the overview hides what they post, it does not refuse them
        island.set_niri(Some(MONITOR.to_owned()), true, now);
        assert_eq!(
            island.resolve(Command::Post(media())),
            Ok(Some(Effect::Post(media())))
        );
    }

    #[test]
    fn dnd_hides_a_toast_and_brings_it_back() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);
        let toast = Activity::transient(
            Id::new(Kind::Notification, "7"),
            Priority::Passive,
            Duration::from_secs(5),
        );

        island.post(toast.clone(), now);
        island.set_dnd(true, now + ms(100));
        assert_eq!(island.presentation(MONITOR), Presentation::Rest);

        island.set_dnd(false, now + ms(200));
        assert_eq!(shown(&island, MONITOR, now + ms(200)), Some(toast));
    }
}
