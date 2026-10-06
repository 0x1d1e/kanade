use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use amane::{Keyboard, Service};

use super::activity::{Activity, Detail, Frame, Id, Interrupt, Kind, Scope, Track};
use super::arbiter::{self, Arbiter};
use super::command::Command;
use super::fade::{Crossfade, Dissolve};
use super::geometry::{self, REST, Shape};
use super::motion::{Mode, Spring};
use super::presentation::{Content, Input, Presentation, Presentations, Prior, Surface};
use super::satellites::{Mark, Satellites};

// plan 5.2: how long a morph takes, by what it changes, see `response`
const EXPAND: Duration = Duration::from_millis(180);
const SURFACE_CHANGE: Duration = Duration::from_millis(220);
const COLLAPSE: Duration = Duration::from_millis(180);

// plan 5.2: a pointer resting on a Compact island for 100-140 ms peeks
const HOVER_DELAY: Duration = Duration::from_millis(120);

// plan 5.2: pointer out collapses after 200-300 ms, back in before that keeps the island open
const GRACE: Duration = Duration::from_millis(250);

// the island's motion and pointer timings (#39); the defaults are the constants above
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timings {
    pub motion: Mode,
    pub expand: Duration,
    pub surface_change: Duration,
    pub collapse: Duration,
    pub hover: Duration,
    pub grace: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            motion: Mode::Spring,
            expand: EXPAND,
            surface_change: SURFACE_CHANGE,
            collapse: COLLAPSE,
            hover: HOVER_DELAY,
            grace: GRACE,
        }
    }
}

impl Timings {
    // a new track dissolves in about as long as a Surface takes to replace another
    pub fn track_change(&self) -> Duration {
        self.surface_change
    }
}

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

    // from `retime`, else the defaults
    timings: Timings,

    // an AutoExpand's Surface open, until it gives the islands back or the user takes them
    auto: Option<Auto>,
}

// what an `Interrupt::AutoExpand` opened, and what it puts back at `until`
#[derive(Debug)]
struct Auto {
    // the island it opened on
    monitor: String,

    until: Instant,

    prior: Prior,

    // the islands that held the keyboard before, so hold it again once restored
    held: Vec<String>,
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

    // the track the content shows, while it is a Media Activity's, dissolving to each new one
    track: Option<Dissolve<Track>>,

    // beside the body, coming out from under it and tucking back
    satellites: Satellites,

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
            timings: Timings::default(),
            auto: None,
        }
    }

    /*
     * sleeps until the next deadline or a nudge, so an idle island never wakes (#5);
     * a nudge sent between the read and the wait is still queued, so the wait never misses it.
     * A panic here listens again after 5 s: Amane restarts every Service's listen (#95)
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

    // for motion outside the island, like the Media Surface's own dissolve
    pub fn timings(&self) -> Timings {
        self.timings
    }

    // the config's timings, at start and on each reload (#102); a motion under way keeps its own
    pub fn retime(&mut self, timings: Timings) {
        self.timings = timings;
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

        // the overview ends every open Surface, so there is nothing to give back once it closes
        if overview && self.auto.take().is_some() {
            nudge();
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

    /*
     * the track the content shows dissolving to the current one; the view checks it is the one it
     * draws, since a Crossfade fading a Media Activity out has already moved on from it
     */
    pub fn track(&self, monitor: &str) -> Option<&Dissolve<Track>> {
        self.get(monitor).track.as_ref()
    }

    // false while the view has to keep asking for frames
    pub fn settled(&self, monitor: &str, now: Instant) -> bool {
        let island = self.get(monitor);

        island
            .shape
            .as_ref()
            .is_none_or(|spring| spring.settled(now))
            && island.track.as_ref().is_none_or(|track| track.settled(now))
            && island.satellites.settled(now)
    }

    pub fn satellites(&self, monitor: &str) -> &Satellites {
        &self.get(monitor).satellites
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
            .chain(self.auto.as_ref().map(|auto| auto.until))
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
     * a Preempt Activity arriving collapses the open Surface it shows on (plan 5.1 rule 4), an
     * existing one turned Preempt included; a repost of one already up does not, so a Surface
     * reopened over it stays. A pending AutoExpand gives the islands back first. An AutoExpand one
     * arriving opens its own Surface, as `auto_expand`
     */
    pub fn post(&mut self, activity: Activity, now: Instant) {
        let global = activity.scope() == Scope::Global;
        let id = activity.id().clone();
        let kind = activity.kind();

        // a transient the open Surface already shows would only wait behind it as a badge
        if activity.interrupt() == Interrupt::Transient && self.absorbs()(&activity) {
            return;
        }

        let up = self.arbiter.interrupt(&id, now);
        let arrives = activity.interrupt() == Interrupt::Preempt && up != Some(Interrupt::Preempt);
        let expands = match activity.interrupt() {
            Interrupt::AutoExpand(duration) if !matches!(up, Some(Interrupt::AutoExpand(_))) => {
                Some(duration)
            }
            _ => None,
        };

        self.change(|arbiter| arbiter.post(activity, now));

        // DND may have dropped it, and what never arrived interrupts nothing
        let live = self.arbiter.contains(&id);
        let arrives = arrives && live;
        let expands = expands.filter(|_| live);

        // the islands go back first, so it preempts what it would have without the AutoExpand
        if arrives {
            self.restore(now);
        }

        let open = self
            .presentations
            .expanded()
            .map(|(monitor, _)| monitor.to_owned());

        match open {
            Some(monitor) if arrives && (global || self.focused(&monitor)) => {
                self.give(&monitor, Input::Preempt, now);
            }
            _ => self.sync(now),
        }

        if let Some(duration) = expands {
            self.auto_expand(kind, duration, now);
        }
    }

    /*
     * opens the Kind's own Surface on the focused island for `duration`, then gives every island
     * it changed back what it showed. Another arriving meanwhile keeps the first's prior and
     * takes its own duration; nothing opens while the overview is or with no island to open on
     */
    fn auto_expand(&mut self, kind: Kind, duration: Duration, now: Instant) {
        let Some(surface) = Surface::own(kind) else {
            return;
        };
        let Some(monitor) = self
            .focused_output
            .clone()
            .or_else(|| self.expanded_on().map(str::to_owned))
        else {
            return;
        };

        self.island(&monitor);

        let Some(prior) = self.presentations.auto_expand(&monitor, surface) else {
            return;
        };

        let (prior, mut held) = match self.auto.take() {
            Some(auto) => (auto.prior.and(prior), auto.held),
            None => (prior, Vec::new()),
        };

        // nobody opened it, so it waits for nothing and holds no keyboard until it gives back
        for name in prior.monitors() {
            let island = self.island(name);

            if island.held && !held.iter().any(|other| other == name) {
                held.push(name.to_owned());
            }
            island.held = false;
            island.due = None;
        }

        self.auto = Some(Auto {
            monitor,
            until: now + duration,
            prior,
            held,
        });
        nudge();

        self.absorb();
        self.sync(now);
    }

    /*
     * the AutoExpand's time ran out: every island it changed shows what it did before, and waits
     * as it would have, holding the keyboard again or collapsing after the grace with nobody on it
     */
    fn restore(&mut self, now: Instant) {
        let Some(auto) = self.auto.take() else {
            return;
        };
        let monitors: Vec<String> = auto.prior.monitors().map(str::to_owned).collect();

        self.presentations.restore(auto.prior);

        for monitor in monitors {
            let presentation = self.presentation(&monitor);
            let pinned = self.pinned(&monitor);
            let grace = self.timings.grace;
            let was_held = auto.held.contains(&monitor);
            let island = self.island(&monitor);

            let expanded = matches!(presentation, Presentation::Expanded(_));
            island.held = was_held && expanded;

            island.due = match presentation {
                _ if island.inside || pinned => None,
                Presentation::Peek => Some((grace, Input::Unhover)),
                Presentation::Expanded(_) if island.held => Some((HOLD, Input::Collapse)),
                Presentation::Expanded(_) => Some((grace, Input::Collapse)),
                _ => None,
            }
            .map(|(delay, input)| Due {
                at: now + delay,
                input,
            });
        }

        nudge();
        self.absorb();
        self.sync(now);
    }

    /*
     * the user acted on the islands, so their choice owns them: a pending AutoExpand gives nothing
     * back, and its Surface waits like one the user opened, collapsing after the grace with nobody on it
     */
    pub fn claim(&mut self, now: Instant) {
        let Some(auto) = self.auto.take() else {
            return;
        };
        let expanded = self.expanded(&auto.monitor) && !self.pinned(&auto.monitor);
        let grace = self.timings.grace;
        let island = self.island(&auto.monitor);

        if expanded && !island.inside && island.due.is_none() {
            island.due = Some(Due {
                at: now + grace,
                input: Input::Collapse,
            });
        }

        nudge();
    }

    // an AutoExpand's Surface is open until it gives the islands back, so a click claims it
    pub fn auto(&self) -> bool {
        self.auto.is_some()
    }

    // what the open Surface already shows, on the island each Activity reaches
    fn absorbs(&self) -> impl Fn(&Activity) -> bool + use<> {
        let open = self
            .presentations
            .expanded()
            .map(|(monitor, surface)| (self.focused(monitor), surface));

        move |activity| {
            open.is_some_and(|(focused, surface)| {
                (focused || activity.scope() == Scope::Global) && surface.shows(activity)
            })
        }
    }

    // drops the Transients up that a Surface just opened shows, as `post` drops those posted after
    fn absorb(&mut self) {
        let absorbs = self.absorbs();

        self.change(|arbiter| {
            arbiter.absorb(absorbs);
        });
    }

    pub fn withdraw(&mut self, id: &Id, now: Instant) {
        self.change(|arbiter| {
            arbiter.withdraw(id, now);
        });
        self.sync(now);
    }

    pub fn set_dnd(&mut self, dnd: bool, now: Instant) {
        self.change(|arbiter| arbiter.set_dnd(dnd, now));
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
            Command::SetDnd(dnd) => return Ok((dnd != self.dnd()).then_some(Effect::Dnd(dnd))),

            // nothing is open or opens while the overview is
            _ if self.overview() => return Ok(None),

            Command::Collapse => return collapse(),

            Command::Close(surface) | Command::Toggle(surface)
                if focused.is_some_and(|(_, shown)| shown == surface) =>
            {
                return collapse();
            }

            // shows another Surface, or none
            Command::Close(_) => return Ok(None),

            Command::Open(surface) | Command::Toggle(surface) => surface,
        };

        let monitor = self
            .focused_output
            .as_deref()
            .or(open.map(|(monitor, _)| monitor))
            .ok_or(NoFocus)?;

        // opening the Surface already showing still restarts the hold, which a pointer on it has none of
        if self.presentation(monitor) == Presentation::Expanded(surface)
            && self.inside(monitor)
            && self.auto.is_none()
        {
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
        self.claim(now);

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

    // the user's input, or an IPC command, which takes a pending AutoExpand over
    pub fn input(&mut self, monitor: &str, input: Input, now: Instant) {
        if input.claims() {
            self.claim(now);
        }

        self.give(monitor, input, now);
    }

    // an input to the Presentation, the user's or one the island gives itself
    fn give(&mut self, monitor: &str, input: Input, now: Instant) {
        // touched before the Presentation is, so sync gives it its own Frame from now on
        let island = self.island(monitor);

        // input that decides something ends a pending Peek or grace; scrolling an open island the
        // pointer just left, which decides nothing yet, still lets it collapse
        if input.decides() && island.due.take().is_some() {
            nudge();
        }

        self.presentations.input(monitor, input);
        self.absorb();

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
        let showing = |frame: Frame| (Mark::of(&frame), frame.transient.or(frame.primary));

        let (marks, untouched) = showing(self.arbiter.frame(
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

        let shown: Vec<_> = self
            .islands
            .keys()
            .map(|monitor| {
                let (marks, shown) = showing(self.frame(monitor, now));

                (monitor.clone(), marks, shown)
            })
            .collect();

        for (monitor, _, shown) in &shown {
            let primary = shown.as_ref().map(|activity| Surface::of(activity.kind()));

            self.presentations.set_primary(monitor, primary);
        }

        let presentation = self.presentations.untouched();
        follow(
            &mut self.untouched,
            Content::new(presentation, untouched),
            marks,
            self.timings,
            now,
        );

        for (monitor, marks, shown) in shown {
            let presentation = self.presentations.get(&monitor);

            if let Some(island) = self.islands.get_mut(&monitor) {
                let content = Content::new(presentation, shown);

                follow(island, content, marks, self.timings, now);
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

        // nobody opened an AutoExpand's Surface, so the pointer leaving it is not a pointer leaving
        let pinned = self.pinned(monitor)
            || self
                .auto
                .as_ref()
                .is_some_and(|auto| auto.monitor == monitor);
        let Timings { hover, grace, .. } = self.timings;
        let island = self.island(monitor);

        if island.inside == inside {
            return;
        }

        // on before any press, since OnDemand focuses only on one (#2)
        island.inside = inside;
        island.armed = inside;

        let due = match (inside, presentation) {
            (true, Presentation::Compact) => Some((hover, Input::Hover)),
            (false, Presentation::Peek) if !pinned => Some((grace, Input::Unhover)),
            (false, Presentation::Expanded(_)) if !pinned => Some((grace, Input::Collapse)),
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
            self.give(&monitor, input, now);
        }

        // the pointer still on its Surface keeps it open, as on one the user opened
        let ended = self.auto.as_ref().filter(|auto| auto.until <= now);

        match ended.map(|auto| self.inside(&auto.monitor)) {
            Some(true) => self.claim(now),
            Some(false) => self.restore(now),
            None => {}
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

// the island morphs to `content` and its Satellites to `marks`, unless already headed there
fn follow(island: &mut Island, content: Content, marks: Vec<Mark>, timings: Timings, now: Instant) {
    let presentation = content.presentation;
    let expanded = matches!(presentation, Presentation::Expanded(_));
    let motion = timings.motion;

    island
        .satellites
        .follow(marks, timings.expand, timings.collapse, motion, now);

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
        .get_or_insert_with(|| Spring::new(REST.into(), motion));

    let from = island.content.target().presentation;
    let track = match content.activity.as_ref().map(Activity::detail) {
        Some(Detail::Media(track)) => Some(track.clone()),
        _ => None,
    };

    if island.content.to(content, spring.progress(now)) {
        spring.to(
            geometry::shape(presentation).into(),
            response(from, presentation, timings),
            now,
        );

        // new content fades in whole, its track with it
        island.track = track.map(|track| Dissolve::new(track, motion));
    } else if let (Some(dissolve), Some(track)) = (&mut island.track, track) {
        dissolve.to(track, timings.track_change(), now);
    }
}

/*
 * a Surface replacing another changes the most at once, so it takes longest; down to a smaller
 * form collapses, anything else expands, a new Activity in the same form included
 */
fn response(from: Presentation, to: Presentation, timings: Timings) -> Duration {
    let rank = |presentation| match presentation {
        Presentation::Rest => 0,
        Presentation::Compact => 1,
        Presentation::Peek => 2,
        Presentation::Expanded(_) => 3,
    };

    match (from, to) {
        (Presentation::Expanded(from), Presentation::Expanded(to)) if from != to => {
            timings.surface_change
        }
        _ if rank(to) < rank(from) => timings.collapse,
        _ => timings.expand,
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
    use crate::island::activity::fixture;
    use crate::island::activity::{Detail, Device, Lifetime, Priority, Toast, Track, Volume};

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
        fixture::persistent(Id::new(Kind::Media, "spotify"), Priority::Media)
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

    #[test]
    fn morph_takes_the_time_of_what_it_changes() {
        use Presentation::{Compact, Expanded, Peek, Rest};

        let media = Expanded(Surface::Media);
        let controls = Expanded(Surface::Controls);
        let response = |from, to| response(from, to, Timings::default());

        assert_eq!(response(Rest, Compact), EXPAND);
        assert_eq!(response(Compact, Peek), EXPAND);
        assert_eq!(response(Peek, media), EXPAND);
        assert_eq!(response(Rest, controls), EXPAND);
        assert_eq!(response(media, controls), SURFACE_CHANGE);
        assert_eq!(response(media, Compact), COLLAPSE);
        assert_eq!(response(Peek, Compact), COLLAPSE);
        assert_eq!(response(Compact, Rest), COLLAPSE);

        // a new Activity in the same form
        assert_eq!(response(Compact, Compact), EXPAND);
    }

    #[test]
    fn surface_change_is_still_moving_when_an_expand_is_there() {
        let now = Instant::now();
        let mut island = IslandService::new();

        island.open(MONITOR, Surface::Controls, now);
        let expanded = island.shape(MONITOR, now + EXPAND);

        island.open(MONITOR, Surface::Media, now + ms(1_000));
        let changed = island.shape(MONITOR, now + ms(1_000) + EXPAND);

        // both 95% of their way by their own response, the slower one not yet
        let share = |from: Shape, at: Shape, to: Shape| {
            (at.height - from.height) / (to.height - from.height)
        };

        let expand = share(REST, expanded, geometry::CONTROLS);
        let change = share(geometry::CONTROLS, changed, geometry::MEDIA);

        assert!((expand - 0.95).abs() < 0.01, "{expand}");
        assert!(change < 0.93, "{change}");
    }

    // #39: a configured timing replaces its default everywhere it applies
    #[test]
    fn configured_timings_time_the_island() {
        let now = Instant::now();
        let mut island = IslandService::new();
        island.timings = Timings {
            expand: ms(400),
            hover: ms(300),
            grace: ms(600),
            ..Timings::default()
        };

        island.post(media(), now);
        island.hover(MONITOR, true, now);
        assert_eq!(island.deadline(), Some(now + ms(300)));

        island.expire(now + ms(300));
        assert_eq!(island.presentation(MONITOR), Presentation::Peek);

        island.hover(MONITOR, false, now + ms(300));
        assert_eq!(island.deadline(), Some(now + ms(900)));

        island.open(MONITOR, Surface::Controls, now + ms(1_000));
        assert!(!island.settled(MONITOR, now + ms(1_000) + EXPAND));
    }

    // geometry is there at once, the content still crossfades and asks for frames for 80 ms
    #[test]
    fn reduced_motion_snaps_the_shape_and_fades_the_content() {
        let now = Instant::now();
        let mut island = IslandService::new();
        island.timings.motion = Mode::Reduced;

        island.open(MONITOR, Surface::Controls, now);

        assert_eq!(island.shape(MONITOR, now), geometry::CONTROLS);
        assert!(!island.settled(MONITOR, now + ms(40)));
        assert!(island.settled(MONITOR, now + ms(80)));

        let content = |at| -> Vec<_> {
            island
                .content(MONITOR, at)
                .into_iter()
                .flatten()
                .map(|(content, _)| content.presentation)
                .collect()
        };

        assert_eq!(content(now + ms(10)), [Presentation::Rest]);
        assert_eq!(
            content(now + ms(70)),
            [Presentation::Expanded(Surface::Controls)]
        );
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
            fixture::persistent(Id::new(Kind::Notification, "7"), Priority::Actionable),
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
    fn close_collapses_only_the_surface_it_names() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        assert_eq!(island.resolve(Command::Close(Surface::Launcher)), Ok(None));

        run(&mut island, Command::Open(Surface::Controls), now);
        assert_eq!(island.resolve(Command::Close(Surface::Launcher)), Ok(None));

        assert_eq!(
            run(&mut island, Command::Close(Surface::Controls), now),
            Some(Effect::Collapse(MONITOR.to_owned()))
        );
        assert_eq!(island.presentation(MONITOR), Presentation::Rest);

        // open on another output, it is not the focused island's to close
        island.input(OTHER, Input::Click, now);
        assert_eq!(island.resolve(Command::Close(Surface::Controls)), Ok(None));
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
            Command::Close(Surface::Media),
            Command::Collapse,
        ] {
            assert_eq!(island.resolve(command.clone()), Ok(None), "{command:?}");
        }

        // the overview collapsed the island for good, so without niri nothing stands in for focus
        island.set_niri(None, false, now);
        assert_eq!(island.resolve(Command::Open(Surface::Media)), Err(NoFocus));
    }

    fn volume() -> Activity {
        fixture::shown(Id::new(Kind::Volume, "volume"), Priority::Osd, OSD)
    }

    fn battery() -> Activity {
        Activity::new(
            Id::new(Kind::Battery, "BAT0"),
            Priority::Critical,
            Lifetime::Persistent,
            Scope::Global,
            Interrupt::Preempt,
        )
        .unwrap()
    }

    // brief, on the focused island, preempting
    fn call() -> Activity {
        Activity::new(
            Id::new(Kind::Notification, "call"),
            Priority::Critical,
            Lifetime::Transient(OSD),
            Scope::FocusedOutput,
            Interrupt::Preempt,
        )
        .unwrap()
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

    // #37: another song in the same Media Activity dissolves in place, no crossfade leg, no morph
    #[test]
    fn a_new_track_dissolves_where_it_stands() {
        let song = |title: &str, playing| {
            media().with_detail(Detail::Media(Track {
                title: title.into(),
                artist: "Artist".into(),
                art: None,
                playing,
            }))
        };
        let shown = |island: &IslandService, at| {
            island
                .content(MONITOR, at)
                .map(|shown| shown.map(|(content, opacity)| (content.activity, opacity)))
        };

        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);
        island.post(song("a", true), now);

        let later = now + Duration::from_secs(1);
        assert!(island.settled(MONITOR, later));

        island.post(song("b", true), later);

        assert_eq!(
            shown(&island, later),
            [None, Some((Some(song("b", true)), 1.0))]
        );
        assert_eq!(island.shape(MONITOR, later + ms(50)), geometry::COMPACT);

        let dissolve = island.track(MONITOR).unwrap();
        assert_eq!(dissolve.target().title, "b");
        assert_eq!(dissolve.from(later).map(|t| t.title.as_str()), Some("a"));
        assert!(!island.settled(MONITOR, later + ms(1)));

        // played or paused, it redraws where it stands
        let settled = later + Duration::from_secs(1);
        assert!(island.settled(MONITOR, settled));

        island.post(song("b", false), settled);
        assert!(island.settled(MONITOR, settled));
        assert!(!island.track(MONITOR).unwrap().target().playing);
    }

    // a Satellite coming out keeps the island asking for frames until it is in place
    #[test]
    fn a_satellite_coming_out_is_not_settled() {
        let timer = |key| fixture::persistent(Id::new(Kind::Timer, key), Priority::Ongoing);

        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);
        island.post(timer("a"), now);

        let later = now + Duration::from_secs(1);
        assert!(island.settled(MONITOR, later));

        island.post(timer("b"), later);
        assert_eq!(island.frame(MONITOR, later).satellites.len(), 1);
        assert!(!island.settled(MONITOR, later + ms(1)));
        assert!(island.settled(MONITOR, later + Duration::from_secs(1)));
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
        let toast = fixture::shown(
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

    // listen() wakes when the primary's dwell ends, and the island moves on to the newer one
    #[test]
    fn the_island_follows_the_primary_once_its_dwell_ends() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);
        let history = fixture::persistent(Id::new(Kind::Notification, "7"), Priority::Media);

        island.post(media(), now);
        island.post(history, now + ms(100));
        assert_eq!(island.deadline(), Some(now + arbiter::DWELL));

        island.input(MONITOR, Input::Click, now + ms(200));
        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Media)
        );
        island.input(MONITOR, Input::Collapse, now + ms(300));

        island.expire(now + arbiter::DWELL);
        island.input(MONITOR, Input::Click, now + arbiter::DWELL);
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

    // a toast up when its Surface opens is in the list, not also a badge
    #[test]
    fn opening_a_surface_drops_the_transients_it_shows() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);
        let toast = fixture::shown(
            Id::new(Kind::Notification, "7"),
            Priority::Actionable,
            Duration::from_secs(5),
        )
        .with_detail(Detail::Notification(Toast::default()));

        island.post(media(), now);
        island.post(toast.clone(), now);
        island.input(MONITOR, Input::Click, now);
        island.post(volume(), now);

        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Notifications)
        );
        assert_eq!(island.frame(MONITOR, now).queued, [volume()]);

        // nor does it return once the Surface closes
        island.input(MONITOR, Input::Collapse, now + ms(100));

        assert_eq!(shown(&island, MONITOR, now + ms(100)), Some(volume()));
        assert!(!island.arbiter.contains(toast.id()));
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
        let low = fixture::persistent(Id::new(Kind::Battery, "BAT0"), Priority::Ongoing);

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
        let call = call();

        island.post(call.clone(), now);

        let expiry = now + OSD;

        island.open(MONITOR, Surface::Controls, expiry);
        island.post(call.clone(), expiry);

        assert_eq!(island.presentation(MONITOR), Presentation::Compact);
        assert_eq!(shown(&island, MONITOR, expiry), Some(call));
    }

    #[test]
    fn a_focused_output_preempt_preempts_only_the_focused_island() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);
        let call = call();

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

        // setting it as it is writes nothing
        assert_eq!(island.resolve(Command::SetDnd(false)), Ok(None));
        assert_eq!(
            run(&mut island, Command::SetDnd(true), now),
            Some(Effect::Dnd(true))
        );
        assert_eq!(island.resolve(Command::SetDnd(true)), Ok(None));
        assert_eq!(
            run(&mut island, Command::SetDnd(false), now),
            Some(Effect::Dnd(false))
        );

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
        let toast = fixture::shown(
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

    const AUTO: Duration = Duration::from_secs(3);

    // a Notification that opens its own Surface for AUTO
    fn arriving(key: &str, priority: Priority, lifetime: Lifetime) -> Activity {
        Activity::new(
            Id::new(Kind::Notification, key),
            priority,
            lifetime,
            Scope::Global,
            Interrupt::AutoExpand(AUTO),
        )
        .unwrap()
    }

    fn auto(key: &str) -> Activity {
        arriving(key, Priority::Passive, Lifetime::Persistent)
    }

    #[test]
    fn auto_expand_opens_its_surface_then_restores() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.post(media(), now);
        island.post(auto("7"), now);

        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Notifications)
        );

        // nobody opened it, so it holds no keyboard and nothing but its own time closes it
        assert_eq!(island.keyboard(MONITOR), Keyboard::OnDemand);
        assert_eq!(island.deadline(), Some(now + AUTO));

        island.expire(now + AUTO - ms(1));
        assert!(island.expanded(MONITOR));

        island.expire(now + AUTO);
        assert_eq!(island.presentation(MONITOR), Presentation::Compact);
        assert_eq!(island.deadline(), None);
    }

    #[test]
    fn auto_expand_gives_back_a_surface_the_user_had_open() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.open(MONITOR, Surface::Controls, now);
        island.post(auto("7"), now + ms(100));
        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Notifications)
        );
        assert!(!island.held(MONITOR));

        let until = now + ms(100) + AUTO;
        island.expire(until);

        // as it was: held, and collapsing after a whole hold again
        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Controls)
        );
        assert!(island.held(MONITOR));
        assert_eq!(island.deadline(), Some(until + HOLD));
    }

    #[test]
    fn auto_expand_gives_back_the_island_it_collapsed() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.hover(OTHER, true, now);
        island.input(OTHER, Input::Click, now);
        island.input(OTHER, Input::RightClick, now);
        island.post(auto("7"), now);

        // the Notification is Global, so the collapsed island shows it
        assert_eq!(island.presentation(OTHER), Presentation::Compact);
        assert!(island.expanded(MONITOR));

        island.expire(now + AUTO);

        assert_eq!(
            island.presentation(OTHER),
            Presentation::Expanded(Surface::Controls)
        );
        assert!(island.pinned(OTHER));
        assert_eq!(island.presentation(MONITOR), Presentation::Compact);
    }

    #[test]
    fn auto_expand_gives_back_a_peek_only_with_its_primary() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.post(media(), now);
        island.input(MONITOR, Input::RightClick, now);
        assert_eq!(island.presentation(MONITOR), Presentation::Peek);

        island.post(auto("7"), now);
        island.expire(now + AUTO);
        assert_eq!(island.presentation(MONITOR), Presentation::Peek);
        assert!(island.pinned(MONITOR));

        island.post(auto("8"), now + AUTO);
        island.withdraw(media().id(), now + AUTO);
        island.withdraw(auto("7").id(), now + AUTO);
        island.withdraw(auto("8").id(), now + AUTO);
        island.expire(now + AUTO * 2);
        assert_eq!(island.presentation(MONITOR), Presentation::Rest);
    }

    // the pointer coming and going is no choice, so the Surface still goes back
    #[test]
    fn hover_alone_keeps_the_restore() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.post(media(), now);
        island.post(auto("7"), now);
        island.hover(MONITOR, true, now + ms(100));
        island.hover(MONITOR, false, now + ms(200));

        assert_eq!(island.deadline(), Some(now + AUTO));

        island.expire(now + AUTO);
        assert_eq!(island.presentation(MONITOR), Presentation::Compact);
    }

    // the pointer still on it at its end keeps it open, then it waits like one the user opened
    #[test]
    fn the_pointer_on_it_at_the_deadline_keeps_it_open() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.post(media(), now);
        island.post(auto("7"), now);
        island.hover(MONITOR, true, now + ms(100));

        island.expire(now + AUTO);
        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Notifications)
        );
        assert!(!island.auto());
        assert_eq!(island.deadline(), None);

        let out = now + AUTO + ms(500);
        island.hover(MONITOR, false, out);
        assert_eq!(island.deadline(), Some(out + GRACE));

        island.expire(out + GRACE);
        assert_eq!(island.presentation(MONITOR), Presentation::Compact);
    }

    // a Preempt is policy, not choice: the islands go back, then it preempts as it would have
    #[test]
    fn a_preempt_meanwhile_preempts_what_was_there() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.open(MONITOR, Surface::Controls, now);
        island.post(auto("7"), now);
        island.post(battery(), now + ms(100));

        assert!(!island.auto());
        assert!(!island.expanded(MONITOR));
        assert!(!island.held(MONITOR));
        assert_eq!(shown(&island, MONITOR, now + ms(100)), Some(battery()));

        island.expire(now + AUTO);
        assert!(!island.expanded(MONITOR));
    }

    // one on the focused island gives back an island it does not reach, which then stays
    #[test]
    fn a_focused_output_preempt_meanwhile_gives_back_the_other_island() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.hover(OTHER, true, now);
        island.input(OTHER, Input::Click, now);
        island.input(OTHER, Input::RightClick, now);
        island.post(auto("7"), now);
        island.post(call(), now + ms(100));

        assert!(!island.auto());
        assert!(!island.expanded(MONITOR));
        assert_eq!(
            island.presentation(OTHER),
            Presentation::Expanded(Surface::Controls)
        );
        assert!(island.pinned(OTHER));

        island.expire(now + AUTO);
        assert_eq!(
            island.presentation(OTHER),
            Presentation::Expanded(Surface::Controls)
        );
    }

    // a toast DND silences, though it preempts
    fn silenced() -> Activity {
        Activity::new(
            Id::new(Kind::Notification, "silenced"),
            Priority::Passive,
            Lifetime::Transient(Duration::from_secs(5)),
            Scope::FocusedOutput,
            Interrupt::Preempt,
        )
        .unwrap()
    }

    // DND drops it, so nothing arrived and nothing is preempted
    #[test]
    fn a_silenced_preempt_preempts_nothing() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.set_dnd(true, now);
        island.open(MONITOR, Surface::Controls, now);
        island.post(silenced(), now + ms(100));

        assert!(!island.contains(silenced().id()));
        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Controls)
        );
        assert!(island.held(MONITOR));
        assert_eq!(island.deadline(), Some(now + HOLD));
    }

    // nor does it disturb a pending restore
    #[test]
    fn a_silenced_preempt_keeps_a_pending_restore() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.open(MONITOR, Surface::Controls, now);
        island.post(auto("7"), now);
        island.set_dnd(true, now + ms(50));
        island.post(silenced(), now + ms(100));

        assert!(island.auto());
        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Notifications)
        );

        island.expire(now + AUTO);
        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Controls)
        );
    }

    type Act = fn(&mut IslandService, Instant);

    // any explicit action takes the islands over: nothing is given back
    #[test]
    fn a_user_action_meanwhile_keeps_their_choice() {
        let now = Instant::now();
        let later = now + ms(100);
        let actions: [(&str, Act); 6] = [
            ("click", |island, now| {
                island.input(MONITOR, Input::Click, now)
            }),
            ("pin", |island, now| {
                island.input(MONITOR, Input::RightClick, now)
            }),
            ("collapse", |island, now| {
                island.input(MONITOR, Input::Collapse, now)
            }),
            ("open", |island, now| {
                island.open(MONITOR, Surface::Launcher, now)
            }),
            ("key", |island, now| island.attend(MONITOR, now)),
            ("press", |island, now| island.claim(now)),
        ];

        for (action, act) in actions {
            let mut island = focused_on(MONITOR, now);

            island.post(media(), now);
            island.post(auto("7"), now);
            island.hover(MONITOR, true, now);
            act(&mut island, later);

            let chosen = island.presentation(MONITOR);
            assert!(!island.auto(), "{action}");

            // under the pointer, nothing closes what the user chose
            island.expire(now + AUTO);
            assert_eq!(island.presentation(MONITOR), chosen, "{action}");
        }
    }

    // claimed with the pointer away, it waits like one the user opened and left
    #[test]
    fn a_claimed_surface_collapses_after_the_grace() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.post(media(), now);
        island.post(auto("7"), now);
        island.claim(now + ms(100));

        assert_eq!(island.deadline(), Some(now + ms(100) + GRACE));

        island.expire(now + ms(100) + GRACE);
        assert_eq!(island.presentation(MONITOR), Presentation::Compact);
    }

    // a repost is no new arrival, so a Surface the user closed stays closed
    #[test]
    fn a_repost_does_not_expand_again() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.post(auto("7"), now);
        island.input(MONITOR, Input::Collapse, now + ms(100));
        island.post(auto("7"), now + ms(200));

        assert!(!island.expanded(MONITOR));
    }

    // a second gives back what was there before the first, at its own end
    #[test]
    fn a_second_auto_expand_keeps_the_first_prior() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.open(MONITOR, Surface::Controls, now);
        island.post(auto("7"), now);

        let media = Activity::new(
            Id::new(Kind::Media, "spotify"),
            Priority::Media,
            Lifetime::Persistent,
            Scope::Global,
            Interrupt::AutoExpand(AUTO),
        )
        .unwrap();
        island.post(media, now + ms(1000));
        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Media)
        );

        island.expire(now + AUTO);
        assert!(island.expanded(MONITOR));

        island.expire(now + ms(1000) + AUTO);
        assert_eq!(
            island.presentation(MONITOR),
            Presentation::Expanded(Surface::Controls)
        );
    }

    #[test]
    fn no_auto_expand_under_the_overview_or_dnd() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.set_niri(Some(MONITOR.to_owned()), true, now);
        island.post(auto("7"), now);
        island.set_niri(Some(MONITOR.to_owned()), false, now);
        assert!(!island.expanded(MONITOR));
        assert_eq!(island.deadline(), None);

        // DND drops the toast, so nothing arrived to open anything
        island.set_dnd(true, now);
        island.post(
            arriving("8", Priority::Passive, Lifetime::Transient(OSD)),
            now,
        );
        assert!(!island.expanded(MONITOR));
    }

    // the overview opening ends the Surface for good, so nothing comes back after it
    #[test]
    fn the_overview_ends_a_pending_restore() {
        let now = Instant::now();
        let mut island = focused_on(MONITOR, now);

        island.open(MONITOR, Surface::Controls, now);
        island.post(auto("7"), now);
        island.set_niri(Some(MONITOR.to_owned()), true, now + ms(100));
        island.set_niri(Some(MONITOR.to_owned()), false, now + ms(200));
        island.expire(now + AUTO);

        assert!(!island.expanded(MONITOR));
    }
}
