//! The session (#156, docs/design.md Session): the Session Surface and `kanade session` lock,
//! sleep, restart, power off and log out. A lock is the lock Module's, and sleep is asked of logind
//! at once. A restart, power off or log out ends what is open, so it counts down `COUNTDOWN` first,
//! as a Critical Session Activity on every island with Cancel and Now; another asked meanwhile
//! takes its place and counts from the start.
//!
//! logind is asked over a zbus connection of its own, not `src/bus.rs`'s `Bus`, which answers a refusal
//! with nothing: a refusal, like polkit's or an inhibitor's, shows as a failed Session Activity.
//! It is asked not to have polkit ask for a password, which could be typed long after a Cancel, so
//! one that needs it is refused. One call at a time waits on a thread of its own for its answer,
//! never given up on, since a call given up on may still be carried out; until it answers, nothing
//! else is asked of logind here and no countdown starts. One that never answers may still be
//! carried out, so nothing more is asked until Kanade restarts.
//!
//! A countdown starts, cancels and ends where it is asked, so the Surface shows it at once; the
//! session thread ends one that ran out. While one counts down it writes `Seconds` each whole
//! second, so only the windows that drew what is left draw again; with none it sleeps until woken.

use std::io::{self, Write as _};
use std::panic;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::service::Service;
use zbus::Message;
use zbus::blocking::Connection;
use zbus::zvariant::ObjectPath;

use super::capture::SHOWN;
use crate::island::activity::{
    Action, Activity, Countdown, Detail, Ending, Id, Interrupt, Kind, Leave, Leaving, Lifetime,
    Priority, Scope,
};
use crate::island::presentation::Surface;
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

// the bus itself, as the sender of an error it answers in logind's place
const BUS: &str = "org.freedesktop.DBus";

/*
 * the bus's errors for a call it never delivered, beside those starting a logind that could not
 * start, its own or systemd's it passes on; any other of its own, like NoReply, may come after
 * logind got it
 */
const UNDELIVERED: [&str; 5] = [
    "org.freedesktop.DBus.Error.ServiceUnknown",
    "org.freedesktop.DBus.Error.NameHasNoOwner",
    "org.freedesktop.DBus.Error.AccessDenied",
    "org.freedesktop.DBus.Error.LimitsExceeded",
    // starting logind took too long, so the call waiting on it was dropped
    "org.freedesktop.DBus.Error.TimedOut",
];
const UNSPAWNED: &str = "org.freedesktop.DBus.Error.Spawn.";
const UNACTIVATED: &str = "org.freedesktop.systemd1.";

/*
 * errors saying an answer never came, which logind too may pass on from systemd, so a job it
 * started may still run
 */
const LOST: [&str; 12] = [
    "org.freedesktop.DBus.Error.NoReply",
    "org.freedesktop.DBus.Error.IOError",
    "org.freedesktop.DBus.Error.NoServer",
    "org.freedesktop.DBus.Error.NoNetwork",
    "org.freedesktop.DBus.Error.Timeout",
    "org.freedesktop.DBus.Error.TimedOut",
    "org.freedesktop.DBus.Error.Disconnected",
    "System.Error.ETIMEDOUT",
    "System.Error.ECONNRESET",
    "System.Error.ECONNABORTED",
    "System.Error.ENOTCONN",
    "System.Error.EPIPE",
];

// why a request is refused while logind has not answered the last
const BUSY: &str = "the last request may still happen: logind has not answered it yet";

// why one is refused once logind never answered one
const UNANSWERED: &str =
    "an earlier request may still happen: logind never answered it, so nothing more is asked";

// what shows for a request logind never answered
pub const NO_ANSWER: &str = "logind did not answer; nothing more is asked until Kanade restarts";

// wakes the session thread when what counts down changed; set before IPC can take a command
static WAKE: OnceLock<Sender<()>> = OnceLock::new();

/*
 * what counts down, kept across a restart of the thread: taken out before logind is asked, so a
 * panic in the call never asks again, and whether logind has answered. Changed where asked, the
 * thread reading it and ending it
 */
static PENDING: Mutex<Pending> = Mutex::new(Pending::new());

struct Pending {
    counting: Option<Counting>,

    // what logind was asked, beside a countdown, so neither starts while it has not answered
    asking: Asking,

    // the serial of the restart's, power off's or log out's refusal showing
    refused: Option<u64>,

    // the serial of the request logind never answered, showing apart, so no refusal replaces it
    unanswered: Option<u64>,

    // the serials given out since Kanade started, never given again
    issued: u64,
}

impl Pending {
    const fn new() -> Self {
        Pending {
            counting: None,
            asking: Asking::Idle,
            refused: None,
            unanswered: None,
            issued: 0,
        }
    }

    /*
     * a new countdown, and the one it took the place of; refused while logind has not answered,
     * since it would end while what logind was asked may still be carried out
     */
    fn start(
        &mut self,
        end: Ending,
        now: Instant,
    ) -> Result<(Counting, Option<Counting>), &'static str> {
        if let Some(why) = self.asking.refusal() {
            return Err(why);
        }
        self.issued += 1;

        let counting = Counting {
            end,
            countdown: Countdown::new(COUNTDOWN, now),
            serial: self.issued,
        };

        Ok((counting, self.counting.replace(counting)))
    }

    // logind about to be asked; refused while it has not answered the last
    fn ask(&mut self) -> Result<(), &'static str> {
        if let Some(why) = self.asking.refusal() {
            return Err(why);
        }
        self.asking = Asking::Waiting;

        Ok(())
    }

    // logind answered, or `never` will, so what was asked may still be carried out
    fn answered(&mut self, never: bool) {
        self.asking = if never {
            Asking::Unanswered
        } else {
            Asking::Idle
        };
    }

    /*
     * the countdown `serial` names, ended, and whether logind may be asked to carry it out: both
     * at once, so no countdown starts in between. One ending while logind has not answered is
     * refused, not tried again
     */
    fn end(&mut self, serial: u64) -> Option<(Counting, Result<(), &'static str>)> {
        let counting = self.take(serial)?;

        Some((counting, self.ask()))
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

    // the request logind never answered, and the one it took the place of
    fn unanswer(&mut self) -> (u64, Option<u64>) {
        self.issued += 1;

        (self.issued, self.unanswered.replace(self.issued))
    }

    // whether the refusal `serial` names showed, now dismissed: not once another took its place
    fn dismissed(&mut self, serial: u64) -> bool {
        let is = |shown: &mut u64| *shown == serial;

        self.refused.take_if(is).is_some() || self.unanswered.take_if(is).is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asking {
    Idle,
    Waiting,

    /*
     * no answer came, as when the bus gave up on it, the connection broke or the call panicked, so
     * what was asked may still be carried out
     */
    Unanswered,
}

impl Asking {
    // why logind is not asked now
    fn refusal(self) -> Option<&'static str> {
        match self {
            Asking::Idle => None,
            Asking::Waiting => Some(BUSY),
            Asking::Unanswered => Some(UNANSWERED),
        }
    }
}

// what went wrong asking logind
#[derive(Debug, PartialEq, Eq)]
enum Failure {
    // as logind or zbus said, so carried out by no one
    Refused(String),

    /*
     * the bus gave up on the answer, the connection broke, or zbus failed otherwise, as zbus
     * said: any of them may come once the call was sent
     */
    Unanswered(String),
}

/*
 * what logind is being asked, from just after `Pending::ask` or `Pending::end` let it, with
 * nothing between that panics, until it answers: a panic in between may leave it asked and
 * unanswered, so it may still be carried out, and says so. Made with no `pending()` guard held,
 * which its drop takes; a panic showing that is caught, since one escaping a drop while unwinding
 * aborts the shell
 */
struct Asked(Leave);

impl Drop for Asked {
    fn drop(&mut self) {
        if thread::panicking() {
            // latched first, so the latch never waits on showing it
            pending().answered(true);

            let leave = self.0;
            drop(panic::catch_unwind(|| {
                unanswered(leave, "the call panicked")
            }));
        }
    }
}

/*
 * logind answered, or never will, marked once dropped, so a panic showing its refusal still lets
 * the next ask
 */
struct Answered {
    never: bool,
}

impl Drop for Answered {
    fn drop(&mut self) {
        pending().answered(self.never);
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
            suspend()?;
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
        Ok(text) => Ok(text),
        Err(why) => {
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

/*
 * a new countdown, in place of any already counting; refused when no thread would end it, or while
 * logind has not answered the last request
 */
fn start(end: Ending, now: Instant) -> Result<(), String> {
    let started = if wake() {
        let mut pending = pending();

        pending
            .start(end, now)
            .map(|(counting, replaced)| (counting, replaced, pending.refused.take()))
            .map_err(String::from)
    } else {
        Err(String::from("the session thread is not running"))
    };

    let (counting, replaced, refused) = match started {
        Ok(started) => started,
        Err(why) => {
            failed(Leave::Ending(end), why.clone());
            return Err(why);
        }
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

/*
 * the countdown `serial` names, ended now, as when it ran out: asked of logind, then withdrawn,
 * even by a panic, so no countdown is left with a Cancel and Now that do nothing
 */
fn end_now(serial: u64) {
    let ended = pending().end(serial);
    let Some((counting, asked)) = ended else {
        return;
    };
    let _withdrawn = Withdrawn(serial);
    let leave = Leave::Ending(counting.end);

    let call = move || match counting.end {
        Ending::Restart => manager(&system()?, "Reboot"),
        Ending::PowerOff => manager(&system()?, "PowerOff"),
        Ending::LogOut => terminate(&system()?),
    };

    // refused, it shows on the island
    match asked {
        Ok(()) => drop(ask(Asked(leave), call)),
        Err(why) => failed(leave, String::from(why)),
    }
}

// a countdown ended, its Activity withdrawn once dropped
struct Withdrawn(u64);

impl Drop for Withdrawn {
    fn drop(&mut self) {
        IslandService::write().withdraw(&counting_id(self.0), Instant::now());
        wake();
    }
}

fn suspend() -> Result<(), String> {
    let asked = pending().ask();

    match asked {
        Ok(()) => ask(Asked(Leave::Sleep), || manager(&system()?, "Suspend")),
        Err(why) => {
            failed(Leave::Sleep, String::from(why));
            Err(String::from(why))
        }
    }
}

/*
 * asks logind on a thread of its own, once `Pending::ask` let it, a refusal showing on the island.
 * Never answered, it may still be carried out, so it shows until dismissed and nothing more is
 * asked until Kanade restarts
 */
fn ask(
    asked: Asked,
    call: impl FnOnce() -> Result<(), Failure> + Send + 'static,
) -> Result<(), String> {
    let leave = asked.0;
    let spawned = thread::Builder::new()
        .name(String::from("session-ask"))
        .spawn(move || {
            let answer = call();

            drop(asked);

            // shown before answered, so no countdown starts in between to hide it
            let _answered = Answered {
                never: matches!(answer, Err(Failure::Unanswered(_))),
            };

            match answer {
                Ok(()) => {}
                Err(Failure::Refused(why)) => failed(leave, why),
                Err(Failure::Unanswered(said)) => unanswered(leave, &said),
            }
        });

    match spawned {
        Ok(_) => Ok(()),

        // nothing was asked
        Err(error) => {
            let _answered = Answered { never: false };
            let why = format!("cannot start a thread: {error}");

            failed(leave, why.clone());
            Err(why)
        }
    }
}

// refused if there is none, since then nothing was asked
fn system() -> Result<Connection, Failure> {
    Connection::system().map_err(|error| Failure::Refused(said(&error)))
}

// not interactive, so polkit refuses at once rather than ask for a password
fn manager(bus: &Connection, method: &str) -> Result<(), Failure> {
    called(bus.call_method(Some(LOGIND), PATH, Some(MANAGER), method, &(false,)))
}

fn terminate(bus: &Connection) -> Result<(), Failure> {
    let session = session_path(lock::session())?;

    called(bus.call_method(Some(LOGIND), &session, Some(SESSION), "Terminate", &()))
}

// checked before the call, so a bad path is known never sent
fn session_path(session: &str) -> Result<ObjectPath<'_>, Failure> {
    if session.is_empty() {
        return Err(Failure::Refused(String::from("logind names no session")));
    }

    ObjectPath::try_from(session)
        .map_err(|error| Failure::Refused(format!("logind's session {session}: {error}")))
}

/*
 * logind's answer to a call. An error logind sent means it was not carried out, since it answers
 * once it decided, unless it says an answer it waited on never came; one the bus sent only if it
 * never delivered the call. Anything else may come after the call was sent, so what was asked may
 * still be
 */
fn called(answer: zbus::Result<Message>) -> Result<(), Failure> {
    match answer {
        Ok(_) => Ok(()),
        Err(error) if never_carried_out(&error) => Err(Failure::Refused(said(&error))),
        Err(error) => Err(Failure::Unanswered(said(&error))),
    }
}

fn never_carried_out(error: &zbus::Error) -> bool {
    let zbus::Error::MethodError(name, _, reply) = error else {
        return false;
    };
    let name = name.as_str();
    let header = reply.header();
    let from_bus = header.sender().is_some_and(|sender| sender.as_str() == BUS);

    if from_bus {
        UNDELIVERED.contains(&name) || name.starts_with(UNSPAWNED) || name.starts_with(UNACTIVATED)
    } else {
        !LOST.contains(&name)
    }
}

// logind's own words for a refusal, else zbus's
fn said(error: &zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(_, Some(description), _) => description.clone(),
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

/*
 * Critical on every island, since it ends what is open unless cancelled, and collapsing a Surface
 * that would hide it, but not the Session Surface showing it
 */
fn activity(counting: Counting) -> Activity {
    Activity::new(
        counting_id(counting.serial),
        Priority::Critical,
        Lifetime::Persistent,
        Scope::Global,
        Interrupt::Preempt,
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
 * island until dismissed. The Session Surface shows no refusal, so one open closes for it
 */
fn failed(leave: Leave, why: String) {
    drop(writeln!(io::stderr().lock(), "kanade: session: {why}"));

    let Leave::Ending(_) = leave else {
        let now = Instant::now();

        // the island before PENDING, as a view takes them
        let mut island = IslandService::write();
        let activity = Activity::new(
            failed_id(),
            priority(),
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

        island.post(activity, now);
        island.displace(Surface::Session, now);
        return;
    };

    refused(Pending::refuse, failing, |serial| Leaving::Failed {
        leave,
        why,
        serial,
    });
}

/*
 * a request logind never answered, sleep too, may still happen. So its Activity shows on every
 * island until dismissed, Critical so that only newer Critical Activities rank above it, and
 * collapses a Surface open when it comes. Nothing more is asked, so a countdown running is ended
 * and refused now
 */
fn unanswered(leave: Leave, said: &str) {
    drop(writeln!(
        io::stderr().lock(),
        "kanade: session: {NO_ANSWER}: {said}"
    ));

    refused(
        Pending::unanswer,
        || (Priority::Critical, Interrupt::Preempt),
        |serial| Leaving::Unanswered { leave, serial },
    );

    let counting = pending().counting.take();
    if let Some(counting) = counting {
        IslandService::write().withdraw(&counting_id(counting.serial), Instant::now());
        wake();
        failed(Leave::Ending(counting.end), String::from(UNANSWERED));
    }
}

// shown on every island until dismissed, in place of the last `slot` gave
fn refused(
    slot: fn(&mut Pending) -> (u64, Option<u64>),
    urgency: fn() -> (Priority, Interrupt),
    leaving: impl FnOnce(String) -> Leaving,
) {
    let now = Instant::now();

    // the island before PENDING, as a view takes them
    let mut island = IslandService::write();
    let (priority, interrupt) = urgency();

    /*
     * the island held from the serial to the post, so a refusal at the same moment cannot post
     * one already replaced
     */
    let (serial, replaced) = slot(&mut pending());

    let activity = Activity::new(
        refused_id(serial),
        priority,
        Lifetime::Persistent,
        Scope::Global,
        interrupt,
    )
    .expect("a Persistent that does not auto-expand is valid")
    .with_actions(vec![Action {
        key: String::from(DISMISS),
        label: String::from("Dismiss"),
    }])
    .with_detail(Detail::Session(leaving(serial.to_string())));

    if let Some(replaced) = replaced {
        island.withdraw(&refused_id(replaced), now);
    }
    island.post(activity, now);
    island.displace(Surface::Session, now);
}

// a restart's, power off's or log out's refusal, which interrupts nothing
fn failing() -> (Priority, Interrupt) {
    (priority(), Interrupt::None)
}

// a refusal's: a countdown is Critical, so one beside it would never show otherwise
fn priority() -> Priority {
    if pending().counting.is_some() {
        Priority::Critical
    } else {
        Priority::Actionable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::presentation::Presentation;

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
        let mut pending = Pending::new();

        let (first, replaced) = pending.start(Ending::Restart, now).unwrap();
        assert_eq!(replaced, None);

        let (second, replaced) = pending.start(Ending::PowerOff, now).unwrap();
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
        let mut pending = Pending::new();

        let (first, replaced) = pending.refuse();
        assert_eq!(replaced, None);

        let (second, replaced) = pending.refuse();
        assert_eq!(replaced, Some(first));

        assert!(!pending.dismissed(first));
        assert!(pending.dismissed(second));
        assert!(!pending.dismissed(second));
    }

    // what logind was asked may still be carried out until it answers, so nothing else starts
    #[test]
    fn nothing_starts_while_logind_has_not_answered() {
        let now = Instant::now();
        let mut pending = Pending::new();

        assert_eq!(pending.ask(), Ok(()));
        assert_eq!(pending.ask(), Err(BUSY));
        assert_eq!(pending.start(Ending::Restart, now), Err(BUSY));

        pending.answered(false);
        assert!(pending.start(Ending::Restart, now).is_ok());
        assert_eq!(pending.ask(), Ok(()));

        // never answered, nothing more is asked
        pending.answered(true);
        assert_eq!(pending.ask(), Err(UNANSWERED));
        assert_eq!(pending.start(Ending::PowerOff, now), Err(UNANSWERED));
    }

    // ended and asked at once: nothing starts in between, and one refused is ended all the same
    #[test]
    fn a_countdown_ends_and_is_asked_at_once() {
        let now = Instant::now();
        let mut pending = Pending::new();

        let (first, _) = pending.start(Ending::Restart, now).unwrap();
        assert_eq!(pending.end(first.serial), Some((first, Ok(()))));
        assert_eq!(pending.end(first.serial), None);
        assert_eq!(pending.start(Ending::PowerOff, now), Err(BUSY));
        assert_eq!(pending.ask(), Err(BUSY));

        pending.answered(false);
        let (second, _) = pending.start(Ending::PowerOff, now).unwrap();
        assert_eq!(pending.ask(), Ok(()));
        assert_eq!(pending.end(second.serial), Some((second, Err(BUSY))));
        assert_eq!(pending.counting, None);
    }

    // a countdown collapses any other open Surface, but not the Session Surface showing it
    #[test]
    fn a_countdown_shows_over_another_surface_but_keeps_its_own() {
        let now = Instant::now();
        let counting = |serial| Counting {
            end: Ending::Restart,
            countdown: Countdown::new(COUNTDOWN, now),
            serial,
        };
        let mut island = IslandService::new();
        island.set_niri(Some(String::from("eDP-1")), false, now);

        island.open("eDP-1", Surface::Launcher, now);
        island.post(activity(counting(1)), now);
        assert_eq!(island.presentation("eDP-1"), Presentation::Compact);

        island.open("eDP-1", Surface::Session, now);
        island.post(activity(counting(2)), now);
        assert_eq!(
            island.presentation("eDP-1"),
            Presentation::Expanded(Surface::Session)
        );
    }

    /*
     * a logind on a socket pair, with no bus between, answering the first call with the error
     * `answer` names, from `sender` if given, as the bus would; or hanging up on it
     */
    fn logind(answer: Option<(Option<&'static str>, &'static str, &'static str)>) -> Connection {
        heard(answer).0
    }

    // the same, and the call it heard
    fn heard(
        answer: Option<(Option<&'static str>, &'static str, &'static str)>,
    ) -> (Connection, Receiver<Message>) {
        let (ours, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let (listening, listens) = mpsc::channel();
        let (hearing, hears) = mpsc::channel();

        thread::spawn(move || {
            let logind = zbus::blocking::connection::Builder::async_io_unix_stream(theirs)
                .server(zbus::Guid::generate())
                .unwrap()
                .p2p()
                .build()
                .unwrap();

            // listening before the call is sent, since zbus drops a message no one listens for
            let messages = zbus::blocking::MessageIterator::from(&logind);
            listening.send(()).unwrap();

            let call = messages
                .map_while(Result::ok)
                .find(|message| message.header().member().is_some())
                .unwrap();

            if let Some((sender, name, said)) = answer {
                let mut error = Message::error(&call.header(), name).unwrap();

                if let Some(sender) = sender {
                    error = error.sender(sender).unwrap();
                }
                logind.send(&error.build(&(said,)).unwrap()).unwrap();
            }
            drop(hearing.send(call));
        });

        let bus = zbus::blocking::connection::Builder::async_io_unix_stream(ours)
            .p2p()
            .build()
            .unwrap();
        listens.recv().unwrap();

        (bus, hears)
    }

    // never letting polkit ask for a password, by the argument and the message alike
    #[test]
    fn logind_is_asked_not_interactively() {
        let (bus, hears) = heard(Some((None, "org.freedesktop.login1.X", "said")));
        drop(manager(&bus, "Reboot"));
        let call = hears.recv().unwrap();

        assert_eq!(call.body().deserialize::<(bool,)>().unwrap(), (false,));
        assert!(
            !call
                .primary_header()
                .flags()
                .contains(zbus::message::Flags::AllowInteractiveAuth)
        );
    }

    // as zbus gives them: an error logind sent, or the bus for a call never delivered, is refused
    #[test]
    fn a_refusal_is_told_from_no_answer_over_a_connection() {
        let answer = |sender, name| {
            let bus = logind(Some((sender, name, "said")));
            manager(&bus, "Reboot")
        };
        let refused = Err(Failure::Refused(String::from("said")));
        let unanswered = |answer| matches!(answer, Err(Failure::Unanswered(_)));

        // logind's own, whatever it names, but one passing on an answer it never got
        for sender in [None, Some(":1.5")] {
            for name in [
                "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired",
                "org.freedesktop.login1.BlockedByInhibitorLock",
                "System.Error.EBUSY",
            ] {
                assert_eq!(answer(sender, name), refused);
            }
            for name in [
                "org.freedesktop.DBus.Error.NoReply",
                "org.freedesktop.DBus.Error.IOError",
                "org.freedesktop.DBus.Error.NoServer",
                "org.freedesktop.DBus.Error.NoNetwork",
                "org.freedesktop.DBus.Error.Timeout",
                "org.freedesktop.DBus.Error.TimedOut",
                "org.freedesktop.DBus.Error.Disconnected",
                "System.Error.ETIMEDOUT",
                "System.Error.ECONNRESET",
                "System.Error.ECONNABORTED",
                "System.Error.ENOTCONN",
                "System.Error.EPIPE",
            ] {
                assert!(unanswered(answer(sender, name)));
            }
        }

        // the bus's, for a call never delivered, or one that may have been
        for name in [
            "org.freedesktop.DBus.Error.ServiceUnknown",
            "org.freedesktop.DBus.Error.NameHasNoOwner",
            "org.freedesktop.DBus.Error.AccessDenied",
            "org.freedesktop.DBus.Error.LimitsExceeded",
            "org.freedesktop.DBus.Error.TimedOut",
            "org.freedesktop.DBus.Error.Spawn.ChildExited",
            "org.freedesktop.systemd1.NoSuchUnit",
        ] {
            assert_eq!(answer(Some("org.freedesktop.DBus"), name), refused);
        }
        for name in [
            "org.freedesktop.DBus.Error.NoReply",
            "org.freedesktop.DBus.Error.Timeout",
        ] {
            assert!(unanswered(answer(Some("org.freedesktop.DBus"), name)));
        }

        // a hang-up
        assert!(unanswered(manager(&logind(None), "Reboot")));
    }

    /*
     * a sleep asked as `suspend` asks it, from no request before, once logind answered or never
     * will and that showed: what logind was asked, and the Activity saying it never answered
     */
    fn asked(call: impl FnOnce() -> Result<(), Failure> + Send + 'static) -> (Asking, Option<u64>) {
        let (before, counted, refused) = {
            let mut pending = pending();
            pending.asking = Asking::Idle;
            pending.ask().unwrap();
            (
                pending.unanswered,
                pending.counting.is_some(),
                pending.refused,
            )
        };
        ask(Asked(Leave::Sleep), call).unwrap();

        /*
         * a panic latches before it shows no answer and refuses a countdown, so only these say
         * the ask thread is done
         */
        let since = Instant::now();
        loop {
            let pending = pending();
            let settled = match pending.asking {
                Asking::Idle => true,
                Asking::Waiting => false,
                Asking::Unanswered => {
                    pending.unanswered != before && (!counted || pending.refused != refused)
                }
            };
            if settled {
                return (pending.asking, pending.unanswered);
            }
            drop(pending);

            assert!(since.elapsed() < Duration::from_secs(5), "never settled");
            thread::sleep(Duration::from_millis(5));
        }
    }

    /*
     * through the shell's own PENDING and island: a refusal lets the next request, while no answer
     * or a panic stops every one after and shows until dismissed. The only test asking of PENDING,
     * resetting it and what it set on the island after
     */
    #[test]
    fn only_an_answer_lets_the_next_request() {
        let refusing = logind(Some((
            None,
            "org.freedesktop.login1.BlockedByInhibitorLock",
            "said",
        )));
        assert_eq!(
            asked(move || manager(&refusing, "Suspend")),
            (Asking::Idle, None)
        );
        assert_eq!(pending().ask(), Ok(()));
        pending().answered(false);

        // a countdown, as the session thread would end it
        let (wake, woken) = mpsc::channel();
        let _ = WAKE.set(wake);
        assert!(request(Leave::Ending(Ending::Restart)).is_ok());
        let counted = pending().counting.unwrap().serial;

        // the Activity of no answer collapses a Surface open when it came
        let now = Instant::now();
        IslandService::write().set_niri(Some(String::from("eDP-1")), false, now);
        IslandService::write().open("eDP-1", Surface::Launcher, now);
        assert_eq!(IslandService::read().surface(), Some(Surface::Launcher));

        let hanging_up = logind(None);
        let (asking, shown) = asked(move || manager(&hanging_up, "Suspend"));
        assert_eq!(asking, Asking::Unanswered);
        assert!(shown.is_some());
        assert_eq!(IslandService::read().surface(), None);

        // the countdown, which could only be refused, is at once
        assert_eq!(pending().counting, None);
        assert!(pending().refused.is_some());
        let now = Instant::now();
        let frame = IslandService::read().frame("eDP-1", now);
        assert!(
            !frame
                .primary
                .iter()
                .chain(&frame.satellites)
                .any(|activity| *activity.id() == counting_id(counted))
        );

        // nothing more is asked, and no countdown starts; each refusal closes the Session Surface
        assert_eq!(pending().ask(), Err(UNANSWERED));
        for leave in [Leave::Sleep, Leave::Ending(Ending::Restart)] {
            IslandService::write().open("eDP-1", Surface::Session, Instant::now());
            assert_eq!(IslandService::read().surface(), Some(Surface::Session));

            assert_eq!(request(leave), Err(String::from(UNANSWERED)));
            assert_eq!(IslandService::read().surface(), None);
        }
        assert_eq!(pending().counting, None);

        // still on the island once a newer Critical takes the primary
        let now = Instant::now();
        let mut island = IslandService::write();
        island.post(
            Activity::new(
                Id::new(Kind::Battery, "low"),
                Priority::Critical,
                Lifetime::Persistent,
                Scope::Global,
                Interrupt::None,
            )
            .unwrap(),
            now,
        );
        let frame = island.frame("eDP-1", now + Duration::from_secs(60));
        let shown_id = refused_id(shown.unwrap());
        assert!(
            frame
                .primary
                .iter()
                .chain(&frame.satellites)
                .any(|activity| *activity.id() == shown_id)
        );
        drop(island);

        // no refusal after takes its place
        let (refused, _) = pending().refuse();
        assert_eq!(pending().unanswered, shown);
        assert!(pending().dismissed(refused));
        assert_eq!(pending().unanswered, shown);

        // a panic is no answer too, ending a countdown and closing the Session Surface
        pending().asking = Asking::Idle;
        assert!(request(Leave::Ending(Ending::PowerOff)).is_ok());
        assert!(pending().counting.is_some());
        IslandService::write().open("eDP-1", Surface::Session, Instant::now());
        assert_eq!(IslandService::read().surface(), Some(Surface::Session));

        let (asking, panicked) = asked(|| panic!("a call that fails halfway"));
        assert_eq!(asking, Asking::Unanswered);
        assert_ne!(panicked, shown);
        assert_eq!(IslandService::read().surface(), None);
        assert_eq!(pending().counting, None);
        assert!(pending().refused.is_some());

        // left as no other test would find them, the serials never given again
        drop(woken);
        let issued = pending().issued;
        *pending() = Pending {
            issued,
            ..Pending::new()
        };

        let now = Instant::now();
        let mut island = IslandService::write();
        for serial in 1..=issued {
            island.withdraw(&refused_id(serial), now);
            island.withdraw(&counting_id(serial), now);
        }
        island.withdraw(&failed_id(), now);
        island.withdraw(&Id::new(Kind::Battery, "low"), now);
        island.set_niri(None, false, now);
    }

    // a session path logind could not take is refused, never sent and so never latching
    #[test]
    fn a_bad_session_path_is_refused_before_the_call() {
        assert!(session_path("/org/freedesktop/login1/session/_32").is_ok());
        assert_eq!(
            session_path(""),
            Err(Failure::Refused(String::from("logind names no session")))
        );
        assert!(matches!(
            session_path("session 2"),
            Err(Failure::Refused(_))
        ));
    }
}
