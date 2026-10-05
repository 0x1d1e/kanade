use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use amane::{Keyboard, Service};

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

    // expands without a press, so the island holds the keyboard to still get Escape
    pub fn open(&mut self, monitor: &str, now: Instant) {
        self.input(monitor, Input::Open(Surface::Controls), now);
        self.island(monitor).held = true;
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

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
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
    fn opening_another_island_collapses_this_one() {
        let mut island = IslandService::new();
        let now = Instant::now();

        island.open(MONITOR, now);
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
}
