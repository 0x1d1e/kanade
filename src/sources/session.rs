//! The session (#156, docs/design.md Session): the Session Surface and `kanade session` lock,
//! sleep, restart, power off and log out. A lock is the lock Module's, and sleep is asked of logind
//! at once. A restart, power off or log out ends what is open, so it counts down `COUNTDOWN` first,
//! as a Critical Session Activity on every island with Cancel and Now; another asked meanwhile
//! takes its place and counts from the start.
//!
//! logind is asked over a zbus connection of its own, not Amane's `Bus`, which answers a refusal
//! with nothing: a refusal, like polkit's or an inhibitor's, shows as a failed Session Activity.
//! One call at a time waits on a thread of its own, up to `ANSWER` for a polkit agent to ask for a
//! password, so neither the draw thread nor a countdown waits on it.
//!
//! A countdown starts, cancels and ends where it is asked, so the Surface shows it at once; the
//! session thread ends one that ran out. While one counts down it writes `Seconds` each whole
//! second, so only the windows that drew what is left draw again; with none it sleeps until woken.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use amane::Service;
use zbus::blocking::{Connection, connection};

use super::capture::SHOWN;
use crate::island::activity::{
    Action, Activity, Countdown, Detail, Ending, Id, Interrupt, Kind, Leave, Leaving, Lifetime,
    Priority, Scope,
};
use crate::island::service::IslandService;
use crate::{lock, modules, supervise};

// how long a restart, power off or log out waits to be cancelled
pub const COUNTDOWN: Duration = Duration::from_secs(60);

// the Activity's actions while one counts down
pub const CANCEL: &str = "cancel";
pub const NOW: &str = "now";

// the action of a refused restart, power off or log out
pub const DISMISS: &str = "dismiss";

const LOGIND: &str = "org.freedesktop.login1";
const PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const SESSION: &str = "org.freedesktop.login1.Session";

/*
 * how long logind may take to answer, a polkit agent asking for a password included, so a call
 * never answered does not hold back every later one
 */
const ANSWER: Duration = Duration::from_secs(300);

// a logind call waits for its answer, so a second is refused rather than piled up behind it
static ASKING: AtomicBool = AtomicBool::new(false);

// wakes the session thread when what counts down changed; set before IPC can take a command
static WAKE: OnceLock<Sender<()>> = OnceLock::new();

/*
 * what counts down, kept across a restart of the thread: taken out before logind is asked, so a
 * panic in the call never asks again. Changed where asked, the thread reading it and ending it
 */
static PENDING: Mutex<Pending> = Mutex::new(Pending {
    counting: None,
    refused: None,
    issued: 0,
});

struct Pending {
    counting: Option<Counting>,

    // the serial of the restart's, power off's or log out's refusal showing
    refused: Option<u64>,

    // the serials given out since Kanade started, never given again
    issued: u64,
}

impl Pending {
    // a new countdown, and the one it took the place of
    fn start(&mut self, end: Ending, now: Instant) -> (Counting, Option<Counting>) {
        self.issued += 1;

        let counting = Counting {
            end,
            countdown: Countdown::new(COUNTDOWN, now),
            serial: self.issued,
        };

        (counting, self.counting.replace(counting))
    }

    // the countdown `serial` names, once: none after another took its place or it ended
    fn take(&mut self, serial: u64) -> Option<Counting> {
        self.counting.take_if(|counting| counting.serial == serial)
    }

    // a new refusal, and the one it took the place of
    fn refuse(&mut self) -> (u64, Option<u64>) {
        self.issued += 1;

        (self.issued, self.refused.replace(self.issued))
    }

    // whether the refusal `serial` names showed, now dismissed: not once another took its place
    fn dismissed(&mut self, serial: u64) -> bool {
        self.refused.take_if(|refused| *refused == serial).is_some()
    }
}

// a restart, power off or log out counting down
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Counting {
    pub end: Ending,
    pub countdown: Countdown,

    // which one, so a press meant for one since replaced does nothing
    pub serial: u64,
}

/*
 * written each whole second while one counts down, so only the windows that drew what is left
 * draw again; it holds nothing, since that follows from the time a window draws at
 */
pub struct Seconds;

impl Service for Seconds {
    fn new() -> Self {
        Seconds
    }

    fn listen() {}
}

// called before IPC runs, so no command finds the thread missing
pub fn spawn() {
    let (wake, woken) = mpsc::channel();
    let _ = WAKE.set(wake);

    supervise::spawn("session", move || follow(&woken));
}

// false once the session thread is gone, as when it could not start
fn wake() -> bool {
    WAKE.get().is_some_and(|wake| wake.send(()).is_ok())
}

/*
 * does what `leave` asks and says so, or why it did not; logind's refusal shows on the island
 * later. Call it from the draw thread, which the lock screen lives on
 */
pub fn request(leave: Leave) -> Result<String, String> {
    match leave {
        Leave::Lock => lock(),
        Leave::Sleep => {
            suspend();
            Ok(String::from("suspending; a refusal shows on the island"))
        }
        Leave::Ending(end) => {
            start(end, Instant::now())?;
            Ok(format!(
                "{} in {} s; cancel on the island or in `kanade session menu`",
                doing(end).to_lowercase(),
                COUNTDOWN.as_secs()
            ))
        }
    }
}

// the lock Module's, as `kanade lock` asks it; a failure shows on the island too
fn lock() -> Result<String, String> {
    let started = if modules::on("lock") {
        lock::start()
    } else {
        Err(String::from("module lock is off"))
    };

    match started {
        Ok(lock::Started::Requested(text)) => Ok(text),
        Ok(lock::Started::Unknown(why)) | Err(why) => {
            failed(Leave::Lock, why.clone());
            Err(why)
        }
    }
}

/*
 * what counts down, set before its Activity is posted and cleared before it is withdrawn, so a
 * view that redraws on the island's change sees it
 */
pub fn counting() -> Option<Counting> {
    pending().counting
}

/*
 * cancels the countdown `serial` names, or ends it now, as its Activity's actions do, or dismisses
 * a refusal
 */
pub fn act(key: &str, serial: &str) {
    let Ok(serial) = serial.parse() else {
        return;
    };

    match key {
        CANCEL => cancel(serial),
        NOW => end_now(serial),
        DISMISS => dismiss(serial),
        _ => {}
    }
}

// whole seconds left, rounded up, so it reads 1 through its last second; subscribes to `Seconds`
pub fn left(countdown: &Countdown, now: Instant) -> u64 {
    drop(Seconds::read());

    seconds(countdown, now)
}

fn seconds(countdown: &Countdown, now: Instant) -> u64 {
    let left = countdown.left(now);

    left.as_secs() + u64::from(left.subsec_nanos() > 0)
}

// what a countdown says it is doing, like "Restarting"
pub fn doing(end: Ending) -> &'static str {
    match end {
        Ending::Restart => "Restarting",
        Ending::PowerOff => "Powering off",
        Ending::LogOut => "Logging out",
    }
}

// what ends a countdown at once, like "Restart now"
pub fn at_once(end: Ending) -> &'static str {
    match end {
        Ending::Restart => "Restart now",
        Ending::PowerOff => "Power off now",
        Ending::LogOut => "Log out now",
    }
}

// runs for good, asleep until woken, or each whole second while one counts down
fn follow(woken: &Receiver<()>) {
    supervise::run("session", || {
        loop {
            let now = Instant::now();
            let counting = pending().counting;

            let woke = match counting {
                Some(counting) if seconds(&counting.countdown, now) == 0 => {
                    end_now(counting.serial);
                    continue;
                }
                Some(counting) => {
                    let tick = next_tick(&counting.countdown, now);

                    woken.recv_timeout(tick.saturating_duration_since(now))
                }
                None => woken.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };

            match woke {
                Ok(()) => {}

                // the last second over, it ends rather than reading 0
                Err(RecvTimeoutError::Timeout)
                    if counting.is_some_and(|counting| {
                        seconds(&counting.countdown, Instant::now()) > 0
                    }) =>
                {
                    drop(Seconds::write());
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
}

fn pending() -> std::sync::MutexGuard<'static, Pending> {
    PENDING.lock().unwrap_or_else(PoisonError::into_inner)
}

// the moment what is left next reads a second less
fn next_tick(countdown: &Countdown, now: Instant) -> Instant {
    let left = seconds(countdown, now);

    countdown.ends - Duration::from_secs(left - 1)
}

// a new countdown, in place of any already counting; refused when no thread would end it
fn start(end: Ending, now: Instant) -> Result<(), String> {
    if !wake() {
        let why = String::from("the session thread is not running");

        failed(Leave::Ending(end), why.clone());
        return Err(why);
    }

    let (counting, replaced, refused) = {
        let mut pending = pending();
        let (counting, replaced) = pending.start(end, now);

        (counting, replaced, pending.refused.take())
    };

    // a refusal before is over
    let mut island = IslandService::write();

    if let Some(replaced) = replaced {
        island.withdraw(&counting_id(replaced.serial), now);
    }
    if let Some(refused) = refused {
        island.withdraw(&refused_id(refused), now);
    }
    island.withdraw(&failed_id(), now);
    island.post(activity(counting), now);
    drop(island);

    wake();
    Ok(())
}

fn dismiss(serial: u64) {
    if pending().dismissed(serial) {
        IslandService::write().withdraw(&refused_id(serial), Instant::now());
    }
}

fn cancel(serial: u64) {
    if pending().take(serial).is_some() {
        IslandService::write().withdraw(&counting_id(serial), Instant::now());
        wake();
    }
}

// the countdown `serial` names, ended now, as when it ran out: withdrawn, then asked of logind
fn end_now(serial: u64) {
    let Some(counting) = pending().take(serial) else {
        return;
    };

    IslandService::write().withdraw(&counting_id(serial), Instant::now());
    wake();

    ask(Leave::Ending(counting.end), move |bus| match counting.end {
        Ending::Restart => manager(bus, "Reboot"),
        Ending::PowerOff => manager(bus, "PowerOff"),
        Ending::LogOut => terminate(bus),
    });
}

fn suspend() {
    ask(Leave::Sleep, |bus| manager(bus, "Suspend"));
}

// asks logind on a thread of its own, a refusal showing on the island
fn ask(leave: Leave, call: impl FnOnce(&Connection) -> Result<(), String> + Send + 'static) {
    if ASKING.swap(true, Ordering::AcqRel) {
        failed(
            leave,
            String::from("logind is still answering the last request"),
        );
        return;
    }

    let spawned = thread::Builder::new()
        .name(String::from("session-ask"))
        .spawn(move || {
            let asked = connection::Builder::system()
                .and_then(|bus| bus.method_timeout(ANSWER).build())
                .map_err(|error| said(&error))
                .and_then(|bus| call(&bus));

            ASKING.store(false, Ordering::Release);

            if let Err(why) = asked {
                failed(leave, why);
            }
        });

    if let Err(error) = spawned {
        ASKING.store(false, Ordering::Release);
        failed(leave, format!("cannot start a thread: {error}"));
    }
}

// interactive, so a polkit agent may ask for a password
fn manager(bus: &Connection, method: &str) -> Result<(), String> {
    bus.call_method(Some(LOGIND), PATH, Some(MANAGER), method, &(true,))
        .map(drop)
        .map_err(|error| said(&error))
}

fn terminate(bus: &Connection) -> Result<(), String> {
    let session = lock::session();

    if session.is_empty() {
        return Err(String::from("logind names no session"));
    }

    bus.call_method(Some(LOGIND), session, Some(SESSION), "Terminate", &())
        .map(drop)
        .map_err(|error| said(&error))
}

/*
 * logind's own words for a refusal, else zbus's; a call given up on may still be carried out, as
 * once a password is typed
 */
fn said(error: &zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(_, Some(description), _) => description.clone(),
        zbus::Error::InputOutput(error) if error.kind() == io::ErrorKind::TimedOut => format!(
            "logind did not answer in {} min; it may still happen",
            ANSWER.as_secs() / 60
        ),
        error => error.to_string(),
    }
}

/*
 * each countdown its own, so withdrawing one that ended never withdraws the one that took its
 * place meanwhile
 */
fn counting_id(serial: u64) -> Id {
    Id::new(Kind::Session, format!("countdown {serial}"))
}

// a lock's or sleep's refusal, beside a countdown, so one refused once a countdown started shows
fn failed_id() -> Id {
    Id::new(Kind::Session, "failed")
}

// a restart's, power off's or log out's, beside a lock's or sleep's, so neither hides the other
fn refused_id(serial: u64) -> Id {
    Id::new(Kind::Session, format!("refused {serial}"))
}

// Critical on every island, since it ends what is open unless cancelled
fn activity(counting: Counting) -> Activity {
    Activity::new(
        counting_id(counting.serial),
        Priority::Critical,
        Lifetime::Persistent,
        Scope::Global,
        Interrupt::None,
    )
    .expect("a Persistent that does not auto-expand is valid")
    .with_actions(vec![
        Action {
            key: String::from(CANCEL),
            label: String::from("Cancel"),
        },
        Action {
            key: String::from(NOW),
            label: String::from(at_once(counting.end)),
        },
    ])
    .with_detail(Detail::Session(Leaving::Counting {
        end: counting.end,
        countdown: counting.countdown,
        serial: counting.serial.to_string(),
    }))
}

/*
 * a lock or sleep refused as a failed recording shows, on the output the user is looking at. A
 * restart, power off or log out may have counted down unattended, so its refusal stays on every
 * island until dismissed
 */
fn failed(leave: Leave, why: String) {
    eprintln!("kanade: session: {why}");

    let now = Instant::now();

    let Leave::Ending(_) = leave else {
        let activity = Activity::new(
            failed_id(),
            Priority::Actionable,
            Lifetime::Transient(SHOWN),
            Scope::FocusedOutput,
            Interrupt::None,
        )
        .expect("a Transient that does not auto-expand is valid")
        .with_detail(Detail::Session(Leaving::Failed {
            leave,
            why,
            serial: String::new(),
        }));

        IslandService::write().post(activity, now);
        return;
    };

    /*
     * the island held from the serial to the post, so a refusal at the same moment cannot post
     * one already replaced; the island before the session state, as a view takes them
     */
    let mut island = IslandService::write();
    let (serial, replaced) = pending().refuse();

    let activity = Activity::new(
        refused_id(serial),
        Priority::Actionable,
        Lifetime::Persistent,
        Scope::Global,
        Interrupt::None,
    )
    .expect("a Persistent that does not auto-expand is valid")
    .with_actions(vec![Action {
        key: String::from(DISMISS),
        label: String::from("Dismiss"),
    }])
    .with_detail(Detail::Session(Leaving::Failed {
        leave,
        why,
        serial: serial.to_string(),
    }));

    if let Some(replaced) = replaced {
        island.withdraw(&refused_id(replaced), now);
    }
    island.post(activity, now);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_left_ticks_on_whole_seconds() {
        let start = Instant::now();
        let countdown = Countdown::new(COUNTDOWN, start);
        let at = |left: u64| countdown.ends - Duration::from_millis(left);

        assert_eq!(seconds(&countdown, start), 60);
        assert_eq!(next_tick(&countdown, start), at(59_000));
        assert_eq!(seconds(&countdown, at(59_000)), 59);
        assert_eq!(next_tick(&countdown, at(59_000)), at(58_000));
        assert_eq!(next_tick(&countdown, at(500)), countdown.ends);
        assert_eq!(seconds(&countdown, countdown.ends), 0);
    }

    #[test]
    fn a_countdown_ends_once_and_a_stale_press_does_nothing() {
        let now = Instant::now();
        let mut pending = Pending {
            counting: None,
            refused: None,
            issued: 0,
        };

        let (first, replaced) = pending.start(Ending::Restart, now);
        assert_eq!(replaced, None);

        let (second, replaced) = pending.start(Ending::PowerOff, now);
        assert_eq!(replaced, Some(first));

        // a Cancel or Now meant for the one replaced
        assert_eq!(pending.take(first.serial), None);
        assert_eq!(pending.counting, Some(second));

        // a cancel and the countdown running out, only the first of them ending it
        assert_eq!(pending.take(second.serial), Some(second));
        assert_eq!(pending.take(second.serial), None);
    }

    #[test]
    fn a_dismiss_meant_for_a_refusal_replaced_does_nothing() {
        let mut pending = Pending {
            counting: None,
            refused: None,
            issued: 0,
        };

        let (first, replaced) = pending.refuse();
        assert_eq!(replaced, None);

        let (second, replaced) = pending.refuse();
        assert_eq!(replaced, Some(first));

        assert!(!pending.dismissed(first));
        assert!(pending.dismissed(second));
        assert!(!pending.dismissed(second));
    }
}
