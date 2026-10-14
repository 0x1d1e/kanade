//! The timer: `kanade timer start <duration>` counts down one Persistent Ongoing Timer
//! Activity, `pause` and `resume` stop and restart its countdown, `cancel` takes it away, and
//! running out takes it away and sends a notification, which toasts and stays in the history like
//! any other. Starting it again restarts it.
//!
//! The Activity says when the timer runs out, and the view reads it at the time it draws, so nothing
//! reposts it each second. A window that draws a reading asks to be drawn again when that reading
//! changes, and one that stops drawing it stops asking: a hidden timer draws no frames. Clocks and
//! Satellite readings change at their own pace, so each redraws only the windows that drew one.
//!
//! `kanade timer pomodoro` runs the same countdown through a pomodoro's phases: a focus, then a
//! break, a long one after the last round's focus, then round 1 again, until cancelled. Each phase
//! the timer moves on to by itself takes attention for ANNOUNCE: the Activity turns Actionable and
//! Preempt, so it shows over Media and collapses an open Surface, then goes back to Ongoing. A
//! Transient beside it would only compete for the primary, and an open Surface would hide the
//! start of a break the user asked to be told of. A skip is the user's own doing, so takes none.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use crate::bus::{Argument, Bus};
use kanade_runtime::{MutexExt, service::Service};

use crate::config;
use crate::island::activity::{
    Action, Activity, Countdown, Detail, Id, Interrupt, Kind, Lifetime, Phase, Priority, Round,
    Scope,
};
use crate::island::service::IslandService;
use crate::supervise;

// the notification daemon, Kanade's own unless another one has the name
const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

// where IPC and the readings reach the thread, set before IPC can take a command
static EVENTS: OnceLock<Sender<Event>> = OnceLock::new();

static CLOCK: Form = Form {
    redraw: Mutex::new(None),
    at: clock_at,
    subscribe: || drop(Clocks::read()),
    invalidate: || drop(Clocks::write()),
};

static SHORT: Form = Form {
    redraw: Mutex::new(None),
    at: short_at,
    subscribe: || drop(Shorts::read()),
    invalidate: || drop(Shorts::write()),
};

// one way to read what is left, with the windows that drew it
struct Form {
    // the earliest moment a drawn reading changes, until the windows are drawn again for it
    redraw: Mutex<Option<Instant>>,

    // the text for the seconds left, with the seconds left at which it next reads otherwise
    at: fn(u64) -> (String, Option<u64>),

    // subscribes the window drawing a reading to its redraw
    subscribe: fn(),

    // draws the windows that drew a reading again
    invalidate: fn(),
}

impl Form {
    fn lock(&self) -> MutexGuard<'_, Option<Instant>> {
        self.redraw.locked()
    }
}

// how long a phase the pomodoro moved on to by itself takes attention
const ANNOUNCE: Duration = Duration::from_secs(6);

// the Skip pill's Action key, before the serial of the phase it skips
const SKIP: &str = "skip";

// the Pause or Resume pill's Action key, and the Stop pill's
const PAUSE: &str = "pause";
const RESUME: &str = "resume";
const STOP: &str = "stop";

// what `kanade timer` asks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Start(Duration),
    Pomodoro(Lengths),
    Pause,
    Resume,
    Cancel,

    // the pomodoro's next phase at once; with a serial, only while that phase runs
    Skip(Option<u64>),
}

// what `kanade timer pomodoro` was told; the rest is `timer.pomodoro.*`
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Lengths {
    pub work: Option<Duration>,
    pub short: Option<Duration>,
    pub long: Option<Duration>,
    pub rounds: Option<u32>,
}

// the most rounds `kanade timer pomodoro` takes, as `timer.pomodoro.rounds`
pub const ROUNDS: u32 = config::Pomodoro::ROUNDS.range.most;

impl Lengths {
    fn plan(self, defaults: &config::Pomodoro) -> Plan {
        let minutes = |minutes: u32| Duration::from_secs(u64::from(minutes) * 60);

        Plan {
            work: self.work.unwrap_or(minutes(defaults.work)),
            short: self.short.unwrap_or(minutes(defaults.short)),
            long: self.long.unwrap_or(minutes(defaults.long)),
            rounds: self.rounds.unwrap_or(defaults.rounds).max(1),
        }
    }
}

// how long each of a pomodoro's phases lasts, and its rounds
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Plan {
    work: Duration,
    short: Duration,
    long: Duration,
    rounds: u32,
}

impl Plan {
    fn length(&self, phase: Phase) -> Duration {
        match phase {
            Phase::Focus => self.work,
            Phase::Break => self.short,
            Phase::LongBreak => self.long,
        }
    }
}

// a pomodoro's first phase
fn first(rounds: u32) -> Round {
    Round {
        phase: Phase::Focus,
        round: 1,
        rounds,
    }
}

// the phase after `round`'s: a break after each focus, the long one after the last, then round 1
fn next(round: Round) -> Round {
    let (phase, number) = match round.phase {
        Phase::Focus if round.round >= round.rounds => (Phase::LongBreak, round.round),
        Phase::Focus => (Phase::Break, round.round),
        Phase::Break => (Phase::Focus, round.round + 1),
        Phase::LongBreak => (Phase::Focus, 1),
    };

    Round {
        phase,
        round: number,
        ..round
    }
}

// what counts down: a plain timer, or a pomodoro's phase
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Running {
    countdown: Countdown,
    pomodoro: Option<Pomodoro>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pomodoro {
    plan: Plan,
    round: Round,

    // names the phase, so a Skip pill drawn for one never skips the next
    serial: u64,

    // taking attention until then, the phase begun by itself
    announce: Option<Instant>,
}

impl Running {
    fn with(self, countdown: Countdown) -> Running {
        Running { countdown, ..self }
    }
}

enum Event {
    Do(Request),

    // a reading asked for an earlier redraw than the thread waits for
    Wake,
}

/*
 * written when a drawn clock changes, so only the windows that drew one draw again; it holds
 * nothing, since the reading follows from the time a window draws at
 */
pub struct Clocks;

impl Service for Clocks {
    fn new() -> Self {
        Clocks
    }

    fn listen() {}
}

/*
 * the timer as the island shows it, for the Timer Surface to draw: its countdown and the pomodoro's
 * round, none while none runs. Set before its Activity is posted or withdrawn, so a view that
 * redraws on the island's change sees it
 */
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Standing(Option<(Countdown, Option<Round>)>);

impl Service for Standing {
    fn new() -> Self {
        Standing::default()
    }

    fn listen() {}
}

pub fn standing() -> Option<(Countdown, Option<Round>)> {
    Standing::read().0
}

// likewise for Satellite readings, which change far less often than a clock beside them
pub struct Shorts;

impl Service for Shorts {
    fn new() -> Self {
        Shorts
    }

    fn listen() {}
}

pub fn request(request: Request) {
    send(Event::Do(request));
}

fn send(event: Event) {
    if let Some(events) = EVENTS.get() {
        let _ = events.send(event);
    }
}

// called before IPC runs, so no command finds the thread missing
pub fn spawn() {
    let (send, events) = mpsc::channel();
    let _ = EVENTS.set(send);

    supervise::spawn("timer", move || follow(&events));
}

/*
 * the timer as asked and as the island shows it, kept across a restart: the island is brought to
 * what was asked before anything else, and what it shows is noted only once it does, so a panic
 * in between leaves nothing stale and loses no start or stop
 */
#[derive(Debug, Default)]
struct Timer {
    running: Option<Running>,
    shown: Option<Running>,

    // ran out, its notification not sent yet; a pomodoro never does
    ended: Option<Countdown>,

    // the last pomodoro phase's serial, counting across pomodoros
    serial: u64,
}

impl Timer {
    /*
     * a pause or resume without a timer does nothing; one that ran out by `now` ends first, so a
     * request just past its end, before the thread woke for it, still sends its notification
     */
    fn apply(&mut self, request: Request, defaults: &config::Pomodoro, now: Instant) {
        self.expire(now);

        self.running = match request {
            Request::Start(length) => Some(Running {
                countdown: Countdown::new(length, now),
                pomodoro: None,
            }),
            Request::Pomodoro(lengths) => {
                let plan = lengths.plan(defaults);

                Some(Running {
                    countdown: Countdown::new(plan.work, now),
                    pomodoro: Some(Pomodoro {
                        plan,
                        round: first(plan.rounds),
                        serial: self.next_serial(),
                        announce: None,
                    }),
                })
            }
            Request::Pause => self
                .running
                .map(|running| running.with(running.countdown.pause(now))),
            Request::Resume => self
                .running
                .map(|running| running.with(running.countdown.resume(now))),
            Request::Cancel => None,
            Request::Skip(serial) => self.running.map(|running| match running.pomodoro {
                Some(pomodoro) if serial.is_none_or(|serial| serial == pomodoro.serial) => {
                    self.advance(running, pomodoro, now, None)
                }
                _ => running,
            }),
        };
    }

    /*
     * a timer that ran out by `now` ends; a pomodoro moves on from when its phase ran out, through
     * any others that ran out since, like over a suspend, the last taking attention from `now`
     */
    fn expire(&mut self, now: Instant) {
        let Some(mut running) = self.running else {
            return;
        };

        while let Some(ends) = running.countdown.running_out().filter(|&ends| ends <= now) {
            let Some(pomodoro) = running.pomodoro else {
                self.running = None;
                self.ended = Some(running.countdown);
                return;
            };

            running = self.advance(running, pomodoro, ends, Some(now + ANNOUNCE));
        }

        if let Some(pomodoro) = &mut running.pomodoro
            && pomodoro.announce.is_some_and(|until| until <= now)
        {
            pomodoro.announce = None;
        }

        self.running = Some(running);
    }

    // the pomodoro's next phase from `at`, paused if it was
    fn advance(
        &mut self,
        running: Running,
        pomodoro: Pomodoro,
        at: Instant,
        announce: Option<Instant>,
    ) -> Running {
        let round = next(pomodoro.round);
        let countdown = Countdown::new(pomodoro.plan.length(round.phase), at);

        Running {
            countdown: match running.countdown.paused {
                Some(_) => countdown.pause(at),
                None => countdown,
            },
            pomodoro: Some(Pomodoro {
                round,
                serial: self.next_serial(),
                announce,
                ..pomodoro
            }),
        }
    }

    fn next_serial(&mut self) -> u64 {
        self.serial += 1;
        self.serial
    }

    // the next moment it changes by itself: it runs out, or stops taking attention
    fn deadline(&self) -> Option<Instant> {
        let running = self.running?;
        let announce = running.pomodoro.and_then(|pomodoro| pomodoro.announce);

        running
            .countdown
            .running_out()
            .into_iter()
            .chain(announce)
            .min()
    }

    // shows `running` on the island, none withdrawing it; one already shown writes nothing
    fn sync(&mut self, show: impl FnOnce(Option<Running>)) {
        if self.shown != self.running {
            show(self.running);
            self.shown = self.running;
        }
    }

    // at most once, so a notification that panics is not sent again every restart
    fn notify(&mut self, notify: impl FnOnce(&Countdown)) {
        if let Some(countdown) = self.ended.take() {
            notify(&countdown);
        }
    }
}

// runs for good, asleep until IPC, the end of the timer, or a reading changes
fn follow(events: &Receiver<Event>) {
    let mut timer = Timer::default();

    supervise::run("timer", || {
        loop {
            let now = Instant::now();

            timer.expire(now);
            timer.sync(show);
            timer.notify(notify);

            for form in [&CLOCK, &SHORT] {
                if form.lock().take_if(|redraw| *redraw <= now).is_some() {
                    (form.invalidate)();
                }
            }

            let deadline = timer
                .deadline()
                .into_iter()
                .chain([&CLOCK, &SHORT].into_iter().filter_map(|form| *form.lock()))
                .min();

            let event = match deadline {
                Some(deadline) => events.recv_timeout(deadline.saturating_duration_since(now)),
                None => events.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };

            match event {
                Ok(Event::Do(request)) => {
                    timer.apply(request, &config::get().pomodoro, Instant::now());
                }
                Ok(Event::Wake) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
}

fn show(running: Option<Running>) {
    let now = Instant::now();

    forget();

    Standing::write().0 = running.map(|running| {
        (
            running.countdown,
            running.pomodoro.map(|pomodoro| pomodoro.round),
        )
    });

    match running {
        Some(running) => IslandService::write().post(activity(running), now),
        None => IslandService::write().withdraw(&id(), now),
    }
}

/*
 * Ongoing, so it becomes a Satellite beside a higher primary, and the primary over Media; a phase
 * begun by itself is Actionable and Preempt while it takes attention
 */
fn activity(running: Running) -> Activity {
    let pomodoro = running.pomodoro;
    let (priority, interrupt) = match pomodoro.and_then(|pomodoro| pomodoro.announce) {
        Some(_) => (Priority::Actionable, Interrupt::Preempt),
        None => (Priority::Ongoing, Interrupt::None),
    };
    let skip = pomodoro.map(|pomodoro| Action {
        key: format!("{SKIP} {}", pomodoro.serial),
        label: String::from("Skip"),
    });
    let (pause, label) = match running.countdown.paused {
        Some(_) => (RESUME, "Resume"),
        None => (PAUSE, "Pause"),
    };
    let actions = skip.into_iter().chain([
        Action {
            key: String::from(pause),
            label: String::from(label),
        },
        Action {
            key: String::from(STOP),
            label: String::from("Stop"),
        },
    ]);

    Activity::new(
        id(),
        priority,
        Lifetime::Persistent,
        Scope::Global,
        interrupt,
    )
    .expect("a Persistent that does not auto-expand is valid")
    .with_actions(actions.collect())
    .with_detail(Detail::Timer(
        running.countdown,
        pomodoro.map(|pomodoro| pomodoro.round),
    ))
}

// an Action pill pressed: Skip moves the pomodoro on, if the phase it was drawn for still runs
pub fn act(key: &str, _subject: &str) {
    match key {
        PAUSE => return request(Request::Pause),
        RESUME => return request(Request::Resume),
        STOP => return request(Request::Cancel),
        _ => {}
    }

    if let Some(serial) = key
        .strip_prefix(SKIP)
        .and_then(|serial| serial.trim().parse().ok())
    {
        request(Request::Skip(Some(serial)));
    }
}

// what the phase is, like Focus or Long break
pub fn phase(phase: Phase) -> &'static str {
    match phase {
        Phase::Focus => "Focus",
        Phase::Break => "Break",
        Phase::LongBreak => "Long break",
    }
}

// one timer, so a start while one runs replaces it
fn id() -> Id {
    Id::new(Kind::Timer, "timer")
}

// a failed call, with no daemon at all, leaves only the withdrawal to say it ran out
fn notify(countdown: &Countdown) {
    Bus::session().call(
        NOTIFICATIONS,
        PATH,
        NOTIFICATIONS,
        "Notify",
        &[
            Argument::from("Kanade"),
            Argument::Unsigned(0),
            Argument::from(""),
            Argument::from("Time's up"),
            Argument::from(format!("{} timer", length(countdown))),
            Argument::TextList(Vec::new()),
            Argument::Map(BTreeMap::new()),
            Argument::Int(-1),
        ],
    );
}

// how long the timer was started for, like 25:00
pub fn length(countdown: &Countdown) -> String {
    clock_at(countdown.length.as_secs()).0
}

// what is left as a clock, like 4:59 or 1:04:59, asking for a redraw when it next changes
pub fn clock(countdown: &Countdown, now: Instant) -> String {
    read(&CLOCK, countdown, now)
}

// what is left in a Satellite's few characters, like 45s, 25m or 3h, asking for a redraw likewise
pub fn short(countdown: &Countdown, now: Instant) -> String {
    read(&SHORT, countdown, now)
}

/*
 * drops the redraws the old timer's readings asked for; the island change that follows draws their
 * windows anyway, and they ask again for what they then read
 */
fn forget() {
    for form in [&CLOCK, &SHORT] {
        *form.lock() = None;
    }
}

fn read(form: &Form, countdown: &Countdown, now: Instant) -> String {
    (form.subscribe)();

    let (text, next) = (form.at)(left(countdown, now));

    // a paused one reads the same until it runs again, which reposts it
    let ends = countdown.running_out();

    if let Some(at) = next.and_then(|next| ends?.checked_sub(Duration::from_secs(next))) {
        redraw_at(form, at);
    }

    text
}

// only an earlier redraw wakes the thread, the one it waits for covers any later one
fn redraw_at(form: &Form, at: Instant) {
    let mut redraw = form.lock();

    if redraw.is_some_and(|redraw| redraw <= at) {
        return;
    }

    *redraw = Some(at);
    drop(redraw);

    send(Event::Wake);
}

// whole seconds left, rounded up, so it reads 0:01 through its last second and 0:00 once run out
fn left(countdown: &Countdown, now: Instant) -> u64 {
    let left = countdown.left(now);

    left.as_secs() + u64::from(left.subsec_nanos() > 0)
}

// the longest timer, so a Satellite's few characters always fit it
const LONGEST: Duration = Duration::from_secs(24 * 60 * 60);

// hours, minutes and seconds, each at most once and in that order, like 1h30m; never zero
pub fn duration(text: &str) -> Option<Duration> {
    const UNITS: [(char, u64); 3] = [('h', 3600), ('m', 60), ('s', 1)];

    let mut units = UNITS.iter();
    let mut rest = text;
    let mut seconds: u64 = 0;

    while !rest.is_empty() {
        let digits = rest.find(|c: char| !c.is_ascii_digit())?;
        let number: u64 = rest[..digits].parse().ok()?;
        let unit = rest[digits..].chars().next()?;
        let &(_, scale) = units.find(|&&(name, _)| name == unit)?;

        seconds = seconds.checked_add(number.checked_mul(scale)?)?;
        rest = &rest[digits + unit.len_utf8()..];
    }

    Some(Duration::from_secs(seconds)).filter(|length| !length.is_zero() && *length <= LONGEST)
}

// `kanade timer pomodoro`'s focus, break, long break and rounds, in that order, each optional
pub fn lengths(words: &[&str]) -> Option<Lengths> {
    // none when not given, refused when not a length
    let length = |at: usize| match words.get(at) {
        Some(word) => duration(word).map(Some),
        None => Some(None),
    };
    let rounds = match words.get(3) {
        Some(word) => Some(
            word.parse::<u32>()
                .ok()
                .filter(|rounds| (1..=ROUNDS).contains(rounds))?,
        ),
        None => None,
    };

    (words.len() <= 4).then_some(())?;

    Some(Lengths {
        work: length(0)?,
        short: length(1)?,
        long: length(2)?,
        rounds,
    })
}

/*
 * each form reads `seconds` left, with the seconds left at which it next reads otherwise, none once
 * the timer ran out
 */
fn clock_at(seconds: u64) -> (String, Option<u64>) {
    let (h, m, s) = (seconds / 3600, seconds / 60 % 60, seconds % 60);

    let text = if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    };

    (text, seconds.checked_sub(1))
}

// seconds in the last minute, then minutes up to 99, then hours, each rounded up like the clock
fn short_at(seconds: u64) -> (String, Option<u64>) {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 3600;

    // past this many minutes, two digits no longer say it
    const MINUTES: u64 = 99 * MINUTE;

    if seconds < MINUTE {
        return (format!("{seconds}s"), seconds.checked_sub(1));
    }

    if seconds <= MINUTES {
        let minutes = seconds.div_ceil(MINUTE);

        return (
            format!("{minutes}m"),
            Some(((minutes - 1) * MINUTE).max(MINUTE - 1)),
        );
    }

    let hours = seconds.div_ceil(HOUR);

    (format!("{hours}h"), Some(((hours - 1) * HOUR).max(MINUTES)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    fn round(phase: Phase, round: u32) -> Round {
        Round {
            phase,
            round,
            rounds: 3,
        }
    }

    fn pomodoro(timer: &mut Timer, now: Instant) {
        let lengths = Lengths {
            work: Some(25 * MINUTE),
            short: Some(5 * MINUTE),
            long: Some(15 * MINUTE),
            rounds: Some(3),
        };

        timer.apply(
            Request::Pomodoro(lengths),
            &config::Pomodoro::default(),
            now,
        );
    }

    fn shown(timer: &Timer) -> (Round, Duration, bool) {
        let running = timer.running.expect("a pomodoro never ends by itself");
        let pomodoro = running.pomodoro.expect("a pomodoro");

        (
            pomodoro.round,
            running.countdown.length,
            pomodoro.announce.is_some(),
        )
    }

    #[test]
    fn a_pomodoro_breaks_after_each_focus_and_long_after_the_last_round() {
        let mut at = first(3);
        let mut seen = vec![at];

        for _ in 0..7 {
            at = next(at);
            seen.push(at);
        }

        assert_eq!(
            seen,
            [
                round(Phase::Focus, 1),
                round(Phase::Break, 1),
                round(Phase::Focus, 2),
                round(Phase::Break, 2),
                round(Phase::Focus, 3),
                round(Phase::LongBreak, 3),
                round(Phase::Focus, 1),
                round(Phase::Break, 1),
            ]
        );

        // one round is a focus, then the long break
        assert_eq!(next(first(1)).phase, Phase::LongBreak);
        assert_eq!(next(next(first(1))), first(1));
    }

    #[test]
    fn phases_move_on_from_when_they_ran_out_and_take_attention_a_while() {
        let start = Instant::now();
        let mut timer = Timer::default();

        pomodoro(&mut timer, start);
        assert_eq!(shown(&timer), (round(Phase::Focus, 1), 25 * MINUTE, false));

        // woken late, the break still began when the focus ran out
        let late = start + 25 * MINUTE + Duration::from_secs(2);
        timer.expire(late);
        assert_eq!(shown(&timer), (round(Phase::Break, 1), 5 * MINUTE, true));
        assert_eq!(timer.running.unwrap().countdown.ends, start + 30 * MINUTE);
        assert_eq!(timer.deadline(), Some(late + ANNOUNCE));

        timer.expire(late + ANNOUNCE);
        assert_eq!(shown(&timer), (round(Phase::Break, 1), 5 * MINUTE, false));

        // a suspend over several phases lands on the one running now
        timer.expire(start + 25 * MINUTE + 5 * MINUTE + 25 * MINUTE + MINUTE);
        assert_eq!(shown(&timer), (round(Phase::Break, 2), 5 * MINUTE, true));
        assert_eq!(timer.ended, None);
    }

    #[test]
    fn a_skip_moves_on_only_from_the_phase_it_was_for() {
        let start = Instant::now();
        let mut timer = Timer::default();
        let defaults = config::Pomodoro::default();

        pomodoro(&mut timer, start);
        let focus = timer.running.unwrap().pomodoro.unwrap().serial;

        timer.apply(Request::Skip(Some(focus)), &defaults, start + MINUTE);
        assert_eq!(shown(&timer), (round(Phase::Break, 1), 5 * MINUTE, false));

        // a second press of the same pill does not skip the break too
        timer.apply(Request::Skip(Some(focus)), &defaults, start + MINUTE);
        assert_eq!(shown(&timer).0, round(Phase::Break, 1));

        // a paused one stays paused in the next phase
        timer.apply(Request::Pause, &defaults, start + 2 * MINUTE);
        timer.apply(Request::Skip(None), &defaults, start + 3 * MINUTE);
        let running = timer.running.unwrap();
        assert_eq!(shown(&timer).0, round(Phase::Focus, 2));
        assert_eq!(running.countdown.left(start + 60 * MINUTE), 25 * MINUTE);

        // a plain timer has no phase to skip, and still ends
        timer.apply(Request::Start(MINUTE), &defaults, start);
        timer.apply(Request::Skip(None), &defaults, start);
        assert_eq!(timer.running.unwrap().countdown.length, MINUTE);
        timer.expire(start + MINUTE);
        assert_eq!(timer.running, None);
        assert!(timer.ended.is_some());
    }

    #[test]
    fn pomodoro_lengths_are_given_in_order_the_rest_from_the_config() {
        let defaults = config::Pomodoro::default();
        let plan = |words: &[&str]| lengths(words).map(|lengths| lengths.plan(&defaults));

        assert_eq!(
            plan(&[]),
            Some(Plan {
                work: 25 * MINUTE,
                short: 5 * MINUTE,
                long: 15 * MINUTE,
                rounds: 4,
            })
        );
        assert_eq!(
            plan(&["50m", "10m"]),
            Some(Plan {
                work: 50 * MINUTE,
                short: 10 * MINUTE,
                long: 15 * MINUTE,
                rounds: 4,
            })
        );
        assert_eq!(
            plan(&["25m", "5m", "20m", "2"]).map(|plan| plan.rounds),
            Some(2)
        );

        for refused in [
            &["25"][..],
            &["25m", "0m"],
            &["25m", "5m", "15m", "0"],
            &["25m", "5m", "15m", "13"],
            &["25m", "5m", "15m", "4", "x"],
        ] {
            assert_eq!(lengths(refused), None, "{refused:?}");
        }
    }
}
