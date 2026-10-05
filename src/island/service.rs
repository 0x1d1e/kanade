use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use amane::{Keyboard, Service};

use super::geometry::{EXPANDED, REST, Shape};
use super::motion::{Mode, Spring};
use super::presentation::{Input, Presentation, Presentations, Surface};

// plan 5.2 starting values, expand and collapse take the same time
const MORPH: Mode = Mode::Spring {
    response: Duration::from_millis(180),
};

// plan 5.2: pointer out collapses after 200-300 ms, back in before that keeps the island open
const GRACE: Duration = Duration::from_millis(250);

// no Arbiter yet (#20), so no island has a primary Activity
const PRIMARY: Option<Surface> = None;

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
}

#[derive(Default)]
struct Island {
    // the pointer is on the body, so a press there can take keyboard focus
    armed: bool,

    // opened without a press (IPC, keybind), so OnDemand never got focus; held until collapse,
    // since niri drops the focus on any switch to OnDemand, even right after a press (#4)
    held: bool,

    // none until the first morph; width, height and radius as one spring group
    shape: Option<Spring<3>>,

    // the pointer left the expanded body, it collapses then unless it comes back
    collapse_at: Option<Instant>,
}

impl Service for IslandService {
    fn new() -> Self {
        Self {
            islands: HashMap::new(),
            presentations: Presentations::default(),
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
        self.presentations.get(monitor, PRIMARY)
    }

    pub fn expanded(&self, monitor: &str) -> bool {
        matches!(self.presentation(monitor), Presentation::Expanded(_))
    }

    // read in the view at the frame's time
    pub fn shape(&self, monitor: &str, now: Instant) -> Shape {
        self.spring(monitor)
            .map_or(REST, |spring| Shape::from(spring.at(now)))
    }

    // false while the view has to keep asking for frames
    pub fn settled(&self, monitor: &str, now: Instant) -> bool {
        self.spring(monitor)
            .is_none_or(|spring| spring.settled(now))
    }

    pub fn armed(&self, monitor: &str) -> bool {
        self.islands.get(monitor).is_some_and(|island| island.armed)
    }

    // the pointer is out and the grace is running
    pub fn leaving(&self, monitor: &str) -> bool {
        self.islands
            .get(monitor)
            .is_some_and(|island| island.collapse_at.is_some())
    }

    // the earliest moment some island changes on its own
    fn deadline(&self) -> Option<Instant> {
        self.islands
            .values()
            .filter_map(|island| island.collapse_at)
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

    // expands without a press, so the island holds the keyboard to still get Escape
    pub fn open(&mut self, monitor: &str, now: Instant) {
        self.input(monitor, Input::Open(Surface::Controls), now);
        self.island(monitor).held = true;
    }

    pub fn input(&mut self, monitor: &str, input: Input, now: Instant) {
        self.presentations.input(monitor, input, PRIMARY);

        // explicit input decides right away, a pending grace no longer applies
        if self.island(monitor).collapse_at.take().is_some() {
            nudge();
        }

        self.sync(now);
    }

    // every island follows its Presentation, since opening one collapses any other
    fn sync(&mut self, now: Instant) {
        for (monitor, island) in &mut self.islands {
            let expanded = matches!(
                self.presentations.get(monitor, PRIMARY),
                Presentation::Expanded(_)
            );

            island.held &= expanded;

            if !expanded && island.collapse_at.take().is_some() {
                nudge();
            }

            // an island that never morphed stays at rest without a spring
            if island.shape.is_some() || expanded {
                island
                    .shape
                    .get_or_insert_with(|| Spring::new(REST.into(), MORPH))
                    .to(if expanded { EXPANDED } else { REST }.into(), now);
            }
        }
    }

    pub fn set_armed(&mut self, monitor: &str, armed: bool) {
        self.island(monitor).armed = armed;
    }

    // disarms right away, collapses only after the grace
    pub fn leave(&mut self, monitor: &str, now: Instant) {
        let expanded = self.expanded(monitor);
        let island = self.island(monitor);

        island.armed = false;

        if expanded && island.collapse_at.is_none() {
            island.collapse_at = Some(now + GRACE);
            nudge();
        }
    }

    // back in before the grace ran out
    pub fn enter(&mut self, monitor: &str) {
        if self.island(monitor).collapse_at.take().is_some() {
            nudge();
        }
    }

    // collapses every island whose grace ran out by now
    pub fn expire(&mut self, now: Instant) {
        let due: Vec<String> = self
            .islands
            .iter_mut()
            .filter(|(_, island)| island.collapse_at.is_some_and(|at| at <= now))
            .map(|(monitor, island)| {
                // cleared here, so the collapse below does not nudge listen() for its own deadline
                island.collapse_at = None;
                monitor.clone()
            })
            .collect();

        for monitor in due {
            self.input(&monitor, Input::Collapse, now);
        }
    }

    fn island(&mut self, monitor: &str) -> &mut Island {
        self.islands.entry(monitor.to_owned()).or_default()
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

        island.open(MONITOR, Instant::now());
        assert_eq!(island.keyboard(MONITOR), Keyboard::Exclusive);

        island.set_armed(MONITOR, true);
        island.input(MONITOR, Input::Click, Instant::now());
        assert_eq!(island.keyboard(MONITOR), Keyboard::Exclusive);
    }

    #[test]
    fn collapse_releases_a_held_keyboard() {
        let mut island = IslandService::new();

        island.open(MONITOR, Instant::now());
        island.input(MONITOR, Input::Collapse, Instant::now());
        assert_eq!(island.keyboard(MONITOR), Keyboard::None);

        // a later pointer expand does not bring the hold back
        island.input(MONITOR, Input::Click, Instant::now());
        assert_eq!(island.keyboard(MONITOR), Keyboard::OnDemand);
    }

    #[test]
    fn pointer_out_collapses_when_the_grace_runs_out() {
        let mut island = IslandService::new();
        let now = Instant::now();

        island.set_armed(MONITOR, true);
        island.input(MONITOR, Input::Click, Instant::now());
        island.leave(MONITOR, now);

        assert!(!island.armed(MONITOR));
        assert_eq!(island.deadline(), Some(now + GRACE));

        island.expire(now + GRACE - Duration::from_millis(1));
        assert!(island.expanded(MONITOR));

        island.expire(now + GRACE);
        assert!(!island.expanded(MONITOR));
        assert_eq!(island.deadline(), None);
    }

    #[test]
    fn pointer_back_in_keeps_the_island_open() {
        let mut island = IslandService::new();
        let now = Instant::now();

        island.input(MONITOR, Input::Click, Instant::now());
        island.leave(MONITOR, now);
        island.enter(MONITOR);

        assert_eq!(island.deadline(), None);

        island.expire(now + GRACE);
        assert!(island.expanded(MONITOR));
    }

    #[test]
    fn repeated_leave_keeps_the_first_deadline() {
        let mut island = IslandService::new();
        let now = Instant::now();

        island.input(MONITOR, Input::Click, Instant::now());
        island.leave(MONITOR, now);
        island.leave(MONITOR, now + Duration::from_millis(100));

        assert_eq!(island.deadline(), Some(now + GRACE));
    }

    #[test]
    fn leaving_a_collapsed_island_sets_no_deadline() {
        let mut island = IslandService::new();

        island.leave(MONITOR, Instant::now());
        assert_eq!(island.deadline(), None);
    }

    #[test]
    fn explicit_collapse_cancels_the_grace() {
        let mut island = IslandService::new();

        island.input(MONITOR, Input::Click, Instant::now());
        island.leave(MONITOR, Instant::now());
        island.input(MONITOR, Input::Collapse, Instant::now());

        assert_eq!(island.deadline(), None);
    }

    #[test]
    fn opening_another_island_collapses_this_one() {
        let mut island = IslandService::new();
        let now = Instant::now();

        island.open(MONITOR, now);
        island.leave(MONITOR, now);
        let later = now + Duration::from_millis(100);
        island.input("HDMI-A-1", Input::Click, later);

        assert!(island.expanded("HDMI-A-1"));
        assert!(!island.expanded(MONITOR));

        // its hold and its grace go with it, and it morphs back to rest
        assert_eq!(island.keyboard(MONITOR), Keyboard::None);
        assert_eq!(island.deadline(), None);
        assert!(!island.settled(MONITOR, later + Duration::from_millis(1)));
        assert_eq!(island.shape(MONITOR, later + Duration::from_secs(1)), REST);
    }
}
