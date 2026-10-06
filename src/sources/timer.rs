//! The timer (plan 5.1, 7): `island timer start <duration>` counts down one Persistent Ongoing Timer
//! Activity, `island timer stop` takes it away, and running out takes it away and sends a
//! notification, which toasts and stays in the history like any other. Starting it again restarts it.
//!
//! The Activity says when the timer runs out, and the view reads it at the time it draws, so nothing
//! reposts it each second. A window that draws a reading asks to be drawn again when that reading
//! changes, and one that stops drawing it stops asking: a hidden timer draws no frames. Clocks and
//! Satellite readings change at their own pace, so each redraws only the windows that drew one.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use amane::{Argument, Bus, Service};

use crate::island::activity::{Activity, Countdown, Detail, Id, Kind, Priority};
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
        self.redraw.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

enum Event {
    Start(Duration),
    Stop,

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

// likewise for Satellite readings, which change far less often than a clock beside them
pub struct Shorts;

impl Service for Shorts {
    fn new() -> Self {
        Shorts
    }

    fn listen() {}
}

pub fn start(length: Duration) {
    send(Event::Start(length));
}

pub fn stop() {
    send(Event::Stop);
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

    thread::spawn(move || follow(&events));
}

// runs for good, asleep until IPC, the end of the timer, or a reading changes
fn follow(events: &Receiver<Event>) {
    let mut running: Option<Countdown> = None;

    supervise::run("timer", || {
        loop {
            let now = Instant::now();

            if let Some(countdown) = running.take_if(|countdown| countdown.ends <= now) {
                forget();
                IslandService::write().withdraw(&id(), now);
                notify(&countdown);
            }

            for form in [&CLOCK, &SHORT] {
                if form.lock().take_if(|redraw| *redraw <= now).is_some() {
                    (form.invalidate)();
                }
            }

            let deadline = running
                .map(|countdown| countdown.ends)
                .into_iter()
                .chain([&CLOCK, &SHORT].into_iter().filter_map(|form| *form.lock()))
                .min();

            let event = match deadline {
                Some(deadline) => events.recv_timeout(deadline.saturating_duration_since(now)),
                None => events.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };

            match event {
                Ok(Event::Start(length)) => {
                    let now = Instant::now();
                    let countdown = Countdown {
                        ends: now + length,
                        length,
                    };

                    running = Some(countdown);
                    forget();
                    IslandService::write().post(activity(countdown), now);
                }
                Ok(Event::Stop) => {
                    // stopping no timer changes nothing, so it writes nothing
                    if running.take().is_some() {
                        forget();
                        IslandService::write().withdraw(&id(), Instant::now());
                    }
                }
                Ok(Event::Wake) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
}

// Ongoing, so it becomes a Satellite beside a higher primary, and the primary over Media
fn activity(countdown: Countdown) -> Activity {
    Activity::persistent(id(), Priority::Ongoing).with_detail(Detail::Timer(countdown))
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

    if let Some(at) = next.and_then(|next| countdown.ends.checked_sub(Duration::from_secs(next))) {
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
    let left = countdown.ends.saturating_duration_since(now);

    left.as_secs() + u64::from(left.subsec_nanos() > 0)
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
    use crate::island::activity::{Interrupt, Lifetime};

    fn countdown(length: u64) -> Countdown {
        Countdown {
            ends: Instant::now() + Duration::from_secs(length),
            length: Duration::from_secs(length),
        }
    }

    #[test]
    fn the_timer_is_one_persistent_ongoing_activity() {
        let timer = activity(countdown(60));

        assert_eq!(timer.id(), &id());
        assert_eq!(timer.priority(), Priority::Ongoing);
        assert_eq!(timer.lifetime(), Lifetime::Persistent);
        assert_eq!(timer.interrupt(), Interrupt::Never);
    }

    #[test]
    fn what_is_left_rounds_up_to_whole_seconds() {
        let timer = countdown(300);
        let at = |left: u64| timer.ends - Duration::from_millis(left);

        assert_eq!(left(&timer, timer.ends - timer.length), 300);
        assert_eq!(left(&timer, at(299_001)), 300);
        assert_eq!(left(&timer, at(299_000)), 299);
        assert_eq!(left(&timer, at(1)), 1);
        assert_eq!(left(&timer, timer.ends), 0);
        assert_eq!(left(&timer, timer.ends + Duration::from_secs(5)), 0);
    }

    #[test]
    fn the_clock_reads_like_a_stopwatch_and_changes_every_second() {
        assert_eq!(clock_at(1500), (String::from("25:00"), Some(1499)));
        assert_eq!(clock_at(59), (String::from("0:59"), Some(58)));
        assert_eq!(clock_at(1), (String::from("0:01"), Some(0)));
        assert_eq!(clock_at(0), (String::from("0:00"), None));
        assert_eq!(clock_at(3600), (String::from("1:00:00"), Some(3599)));
        assert_eq!(clock_at(5405), (String::from("1:30:05"), Some(5404)));
    }

    // the redraw falls exactly where the text changes, never sooner
    #[test]
    fn a_satellite_reads_the_largest_unit_and_changes_only_with_it() {
        for seconds in 0..=24 * 3600 {
            let (text, next) = short_at(seconds);

            assert!(text.len() <= 3, "{text}");

            match next {
                Some(next) => {
                    assert!(next < seconds);
                    assert_eq!(short_at(next + 1).0, text, "{seconds}");
                    assert_ne!(short_at(next).0, text, "{seconds}");
                }
                None => assert_eq!(seconds, 0),
            }
        }

        assert_eq!(short_at(1500).0, "25m");
        assert_eq!(short_at(1441).0, "25m");
        assert_eq!(short_at(1440).0, "24m");
        assert_eq!(short_at(60).0, "1m");
        assert_eq!(short_at(59).0, "59s");
        assert_eq!(short_at(5940).0, "99m");
        assert_eq!(short_at(5941).0, "2h");
        assert_eq!(short_at(86_400).0, "24h");
    }
}
