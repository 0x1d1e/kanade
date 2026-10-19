//! The lock screen (ADRs 0018, 0025): `kanade-lock` holds ext-session-lock over its own
//! Wayland connection and draws it, as soon as niri sizes each lock surface; the shell hands it
//! what each output's shows (`scene`) and checks the password typed with PAM. Every monitor shows
//! the time, the date, who is signed in and a password field, and nothing else: no notification,
//! no Activity. The password goes from the field to PAM and nowhere else; the field is emptied as
//! it is sent, and it is never logged or drawn.
//!
//! `kanade lock` exits 0 only once niri says the lock it asked for holds, with no password
//! typed since being checked or accepted. logind's `LockedHint` may still say the last lock right
//! after an unlock, so it is only for a crash.
//!
//! A crash while locked leaves niri locked on its red screen; the user unit (`UNIT`) restarts the
//! shell, which finds logind's `LockedHint` true and locks again, so niri swaps the dead lock for
//! this one.

use std::env;
use std::panic::{self, AssertUnwindSafe};
use std::process;
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::bus::{Argument, Bus, Value};
use kanade_lock::{Locker, News};
use kanade_runtime::{MutexExt, service::Service};

use crate::cli::Reply;
use crate::clock::Note;
use crate::config::{self, Config};
use crate::hooks::{self, Event};
use crate::later::{LATER, Later};
use crate::sources::{recording, sleep};

mod keyboard;
mod password;
mod scene;
mod who;

pub use self::keyboard::heard as keyboard;
use self::password::{PAM, PAM_DIRS, submit};
pub use self::password::{pam, pam_file, service, service_of, valid};
use self::scene::decode;
pub use self::scene::scene;
use self::who::look_up;

// the systemd user unit that runs the shell, which `dist/` holds and docs/wiki/Installation.md installs
pub const UNIT: &str = "kanade.service";

const LOGIND: &str = "org.freedesktop.login1";

// the session of whoever asks; from a user unit, which has none of its own, the graphical one
const AUTO: &str = "/org/freedesktop/login1/session/auto";

/*
 * the requests of `kanade lock` and of sleep (`hold`), numbered from 1 in this process;
 * each waits until `confirmed` reaches its own while the lock still holds. A request, a password
 * typed and what niri says of the lock are each taken under it, so they never interleave
 */
#[derive(Clone)]
struct Requests {
    newest: u64,

    // the newest request niri said it holds the lock for, with no unlock asked since
    confirmed: u64,

    // niri holds the lock, as `kanade-lock` last said
    locked: bool,

    // a password was accepted, and the lock asked to end
    unlocking: bool,

    // the lock of the newest request ended: unlocked, or ended by niri
    ended: bool,

    // niri refused the newest request's lock, as while another locker holds the session
    denied: bool,

    // from logind's `PrepareForSleep(true)` until `(false)`: no password goes to PAM
    sleeping: bool,

    // the sleeps begun, so a wake found late opens only the gate of the sleep it was asked about
    sleeps: u64,

    // a password was typed while `sleeping`, or its check dropped, so the lock screen says why
    refused: bool,

    // the check of a password typed, numbered `check`: one sleep dropped is never heard of
    checking: bool,
    check: u64,

    // PAM runs a check, even one dropped: one at a time, so none races a dropped one's answer
    pam: bool,

    // the passwords PAM refused, each one more shaking the field
    wrong: u64,

    // the last password checked was refused
    failed: bool,

    // a lock asked for dropped the check under way: its password has to be typed again
    dropped: bool,

    // the sleep logind is being asked whether it ended (`awake`), one at a time
    asking: Option<u64>,

    // what PAM last said during the check under way, or the error it ended on (ADR 0065)
    note: Option<Note>,

    // until when input unlocks without a password (ADR 0066): set only for the lock the idle
    // Module asked for when none held, and gone with any later request, sleep, unlock or the end
    grace: Option<Duration>,
}

impl Requests {
    const fn new() -> Self {
        Self {
            newest: 0,
            confirmed: 0,
            locked: false,
            unlocking: false,
            ended: false,
            denied: false,
            sleeping: false,
            sleeps: 0,
            refused: false,
            checking: false,
            check: 0,
            pam: false,
            wrong: 0,
            failed: false,
            dropped: false,
            asking: None,
            note: None,
            grace: None,
        }
    }

    // shuts the gate, and drops a check under way: its password has to be typed again
    fn sleep(&mut self) {
        if !self.sleeping {
            self.sleeping = true;
            self.sleeps += 1;
        }

        self.note = None;
        self.grace = None;

        if std::mem::take(&mut self.checking) {
            self.refused = true;
        }
    }

    // `sleep` woke, if it is still the one sleeping; whether a password waited on it
    fn woke(&mut self, sleep: u64) -> bool {
        if self.sleeps != sleep || !self.sleeping {
            return false;
        }

        self.sleeping = false;
        std::mem::take(&mut self.refused)
    }

    // the sleep to ask logind about, unless it is being asked already
    fn ask(&mut self) -> Option<u64> {
        if self.asking == Some(self.sleeps) {
            return None;
        }

        self.asking = Some(self.sleeps);
        self.asking
    }

    // logind answered about `sleep`, or could not be asked
    fn asked(&mut self, sleep: u64) {
        if self.asking == Some(sleep) {
            self.asking = None;
        }
    }

    /*
     * a check of a password typed begins, which `checked` ends by its number; none while one is
     * under way or accepted, or while the machine is on its way to sleep, when it is `refused`
     */
    fn check(&mut self) -> Option<u64> {
        if self.checking || self.unlocking {
            return None;
        }
        if self.sleeping {
            self.refused = true;
            return None;
        }
        if self.pam {
            return None;
        }

        self.check += 1;
        self.checking = true;
        self.pam = true;
        self.failed = false;
        self.dropped = false;
        self.note = None;
        Some(self.check)
    }

    // what PAM said during the check `check`; whether it is shown, which it is not once dropped
    fn noted(&mut self, check: u64, note: Note) -> bool {
        if !self.checking || self.check != check {
            return false;
        }

        self.note = Some(note);
        true
    }

    // whether the check `check` ended stands, so an accepted one unlocks; not once dropped
    fn checked(&mut self, check: u64, accepted: bool) -> bool {
        self.pam = false;
        if !self.checking || self.check != check {
            return false;
        }

        self.checking = false;
        self.failed = !accepted;

        // a prompt PAM showed is over; an error it ended on stays to be read
        if accepted || matches!(self.note, Some(Note::Info(_))) {
            self.note = None;
        }

        if accepted {
            self.unlocking = true;
            self.confirmed = 0;
            self.grace = None;
        } else {
            self.wrong += 1;
        }
        true
    }

    /*
     * whether a lock now may be asked for with a grace: none is held, on its way, ended but for
     * its unlock, or in the way of a sleep
     */
    fn graceable(&self) -> bool {
        !self.locked && !self.sleeping && self.holding() == Holding::Unlocked
    }

    // input unlocks the lock of request `asked`, once niri holds it, until `until`
    fn grant(&mut self, asked: u64, until: Duration) {
        if self.newest == asked && self.confirmed == asked && self.holding() == Holding::Held {
            self.grace = Some(until);
        }
    }

    /*
     * input at `now` on the lock screen: whether it unlocks, as an accepted password does. Decided
     * here, at `now` as the caller reads it, so input handled late finds the grace gone. Not
     * while a password is being checked, sleep shuts the gate, or an unlock is under way
     */
    fn touch(&mut self, now: Duration) -> bool {
        let Some(until) = self.grace else {
            return false;
        };

        if now >= until {
            self.grace = None;
            return false;
        }

        if !self.locked
            || self.sleeping
            || self.checking
            || self.pam
            || self.unlocking
            || self.holding() != Holding::Held
            || self.confirmed != self.newest
        {
            return false;
        }

        self.grace = None;
        self.unlocking = true;
        self.confirmed = 0;
        true
    }

    /*
     * A new request, confirmed once `kanade-lock` says niri holds a lock for it, even one held
     * already, as it says so again when asked. The lock wins: an unlock asked while niri still
     * locks is undone, one it holds ends and the session locks again, and a password being
     * checked has to be typed again
     */
    fn request(&mut self) -> u64 {
        self.newest += 1;
        self.unlocking = false;
        self.ended = false;
        self.denied = false;
        self.dropped |= std::mem::take(&mut self.checking);
        self.confirmed = 0;
        self.note = None;
        self.grace = None;
        self.newest
    }

    /*
     * what `kanade-lock` said of the lock. It takes asks in the order sent, under the requests, so
     * news of an older request's lock comes before that of the newest
     */
    fn heard(&mut self, news: News) {
        match news {
            News::Locked(request) => {
                self.locked = true;

                // an unlock asked since the newest request ends this lock right away
                if request == self.newest && !self.unlocking {
                    self.confirmed = request;
                    self.ended = false;
                    self.denied = false;
                }
            }
            News::Unlocked(request) => {
                self.locked = false;
                self.confirmed = 0;
                self.grace = None;

                // else a request came after the unlock, and its lock is on its way
                if request == self.newest {
                    self.unlocking = false;
                    self.ended = true;
                }
            }
            // an older request's lock: the newest one's is on its way
            News::Finished(request) if request != self.newest => {
                self.locked = false;
                self.confirmed = 0;
                self.grace = None;
            }
            News::Finished(_) | News::Gone => {
                let was = std::mem::take(&mut self.locked);
                self.unlocking = false;
                self.confirmed = 0;
                self.grace = None;

                if was {
                    self.ended = true;
                } else {
                    self.denied = true;
                }
            }
        }
    }

    fn status(&self) -> String {
        let holding = match self.holding() {
            Holding::Held => format!("confirmed #{}", self.confirmed),
            Holding::Checking => String::from("checking a password"),
            Holding::Unlocked => String::from("unlocked"),
            Holding::Denied => String::from("denied"),
        };

        format!("requested #{}\n{holding}", self.newest)
    }

    // where the request `number` stands
    fn settled(&self, number: u64) -> Settled {
        match self.holding() {
            Holding::Checking => Settled::Waiting,
            Holding::Unlocked => Settled::Unlocked,
            // only the newest is denied; an older one's lock is gone with it
            Holding::Denied if self.newest == number => Settled::Denied,
            Holding::Denied => Settled::Unlocked,
            // a later request confirmed is a lock after this one too
            Holding::Held if self.confirmed >= number => Settled::Locked,
            Holding::Held => Settled::Waiting,
        }
    }

    // what `hold`, having asked `asked`, does next
    fn next(&self, asked: Option<u64>) -> Next {
        match (asked, self.holding()) {
            (Some(asked), Holding::Held) if self.confirmed >= asked => Next::Done,
            (Some(_), Holding::Denied) => Next::Denied,
            (None, _) | (_, Holding::Unlocked) => Next::Ask,
            (Some(_), Holding::Held | Holding::Checking) => Next::Wait,
        }
    }

    // whether the lock of the newest request holds or is on its way; none asked yet is unlocked
    fn holding(&self) -> Holding {
        if self.checking {
            Holding::Checking
        } else if self.denied {
            Holding::Denied
        } else if self.newest == 0 || self.unlocking || self.ended {
            Holding::Unlocked
        } else {
            Holding::Held
        }
    }

    // what the lock screen says under the field
    fn said(&self) -> Said {
        let status = if self.checking {
            Status::Checking
        } else if self.sleeping && self.refused {
            Status::Sleeping
        } else if self.pam {
            // a dropped check, which what is typed waits on
            Status::Checking
        } else if self.dropped {
            Status::Again
        } else if self.failed {
            Status::Wrong
        } else {
            Status::None
        };

        Said {
            status,
            note: self.note.clone(),
            wrong: self.wrong,
            // until niri unlocks: the surface shows the time while it waits for that
            locked: self.locked || matches!(self.holding(), Holding::Held | Holding::Checking),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holding {
    // held, or on its way
    Held,

    // a password is being checked, which may unlock
    Checking,

    // a password was accepted since the newest request, or niri ended the lock
    Unlocked,

    // niri refused the lock
    Denied,
}

// where `hold` stands
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    // niri holds the lock asked for, which still holds
    Done,

    // no request stands, or an unlock ended it: ask
    Ask,

    // asked: wait on niri
    Wait,

    // niri refused it
    Denied,
}

static REQUESTS: Mutex<Requests> = Mutex::new(Requests::new());

// told each time niri says something of the lock, which may confirm a request or end it
static HEARD: Condvar = Condvar::new();

// the lock screen's own connection, or why it has none
static LOCKER: OnceLock<Result<Locker, String>> = OnceLock::new();

/*
 * the logind session niri locks, as its object path, or empty when logind names none. niri sets
 * `LockedHint` on its `XDG_SESSION_ID`, which niri-session hands the unit; without one, `AUTO`.
 * Found once logind names one, so a logind not yet answering is asked again
 */
pub fn session() -> &'static str {
    static SESSION: OnceLock<String> = OnceLock::new();

    if let Some(session) = SESSION.get() {
        return session;
    }

    let bus = Bus::system();
    let id = env::var("XDG_SESSION_ID").unwrap_or_else(|_| {
        let id = bus.property(LOGIND, AUTO, "org.freedesktop.login1.Session", "Id");
        id.text().to_owned()
    });
    if id.is_empty() {
        return "";
    }

    let path = bus.call(
        LOGIND,
        "/org/freedesktop/login1",
        "org.freedesktop.login1.Manager",
        "GetSession",
        &[Argument::from(id)],
    );
    let path = path.text();

    if path.is_empty() {
        return "";
    }

    SESSION.get_or_init(|| path.to_owned())
}

/*
 * whether niri has locked `session`, as logind's `LockedHint` says, or None when logind does not
 * answer. niri sets it once every monitor shows a lock screen, whichever locker's, and clears it
 * on unlock; only when it runs as a session, as the unit needs anyway
 */
fn locked(session: &str) -> Option<bool> {
    let hint = Bus::system().property(
        LOGIND,
        session,
        "org.freedesktop.login1.Session",
        "LockedHint",
    );

    match hint {
        Value::Bool(locked) => Some(locked),
        _ => None,
    }
}

/*
 * at start, before the shell runs: starts the lock screen's connection, and a session logind still
 * counts as locked was locked by a shell that died, so this one locks it again. niri refuses while
 * another locker holds it. Without `PAM` it locks all the same, as niri is locked anyway; only
 * ending the session gets out
 */
pub fn relock() {
    // known before the first lock, which may come just before sleep, so it shows them from its first frame
    look_up();

    let started = LOCKER.get_or_init(|| {
        Locker::start(decode, Box::new(submit), Box::new(heard), Box::new(touched))
    });
    if let Err(error) = started {
        eprintln!("kanade: no lock screen: {error}");
    }

    let session = session();

    if session.is_empty() {
        eprintln!("kanade: logind names no session, so a crash while locked is not locked again");
    }

    if locked(session) == Some(true) {
        eprintln!("kanade: the session is locked, locking it again");
        if pam().is_none() {
            eprintln!(
                "kanade: no PAM service {}, so no password will unlock",
                service()
            );
        }
        if let Ok(locker) = started {
            request_lock(&mut requests(), locker);
        }
    }
}

// where a request stands
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settled {
    // niri holds the lock asked for, and it still holds
    Locked,
    Waiting,

    // a password typed on the lock screen unlocks it
    Unlocked,

    // niri refused it, as while another locker holds the session
    Denied,
}

/*
 * `kanade lock`, which asks niri for the lock and answers `later` once niri holds it, with no
 * password typed since. While locked it keeps the lock, so a second one keeps what is being typed,
 * unless it was sent: an unlock asked locks again, and a password being checked has to be typed
 * again
 */
pub fn request(later: Later) {
    later.unsettled(format!(
        "niri did not hold the lock within {}s, and a password typed meanwhile may have unlocked it; \
         the session may still lock",
        LATER.as_secs()
    ));

    match start() {
        Ok(number) => wait(number, later),
        Err(why) => later.refused(why),
    }
}

// asks niri for a lock, and gives the request's number; the lock may not hold yet
pub fn start() -> Result<u64, String> {
    if pam().is_none() {
        return Err(format!(
            "no PAM service {} in {}, so no password would unlock",
            service(),
            PAM_DIRS.join(", ")
        ));
    }

    let locker = locker()?;

    Ok(request_lock(&mut requests(), locker))
}

// answers `later` once the request `number` settles, on a thread of its own as niri may take a while
fn wait(number: u64, later: Later) {
    let spawned = thread::Builder::new()
        .name(String::from("lock wait"))
        .spawn({
            let later = later.clone();

            move || {
                let mut requests = requests();

                while later.open() {
                    let reply = match requests.settled(number) {
                        Settled::Waiting => None,
                        Settled::Locked => Some(Reply::Done(String::new())),
                        Settled::Unlocked => Some(Reply::Unknown(String::from(
                            "a password typed on the lock screen unlocks the session",
                        ))),
                        Settled::Denied => Some(Reply::Refused(String::from(
                            "niri refused the lock: another locker holds the session, or its VT is \
                             not shown",
                        ))),
                    };

                    if let Some(reply) = reply {
                        drop(requests);
                        later.answer(&reply);
                        return;
                    }

                    requests = HEARD
                        .wait_timeout(requests, later.left())
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            }
        });

    if let Err(error) = spawned {
        later.unknown(format!(
            "cannot wait for niri: {error}; the session may still lock"
        ));
    }
}

fn locker() -> Result<&'static Locker, String> {
    match LOCKER.get() {
        Some(Ok(locker)) => Ok(locker),
        Some(Err(error)) => Err(format!("no lock screen: {error}")),
        None => Err(String::from("the lock screen has not started")),
    }
}

// asks niri for a lock, or keeps the one held, and hands back the request's number
fn request_lock(requests: &mut Requests, locker: &Locker) -> u64 {
    let newest = requests.request();

    // only spawns, so nothing waits on it under the requests
    look_up();

    // a screen recording is saved and ends (ADR 0116); only spawns, so nothing waits under the requests
    recording::locked();

    // the scenes read the clock only while locked, so say so and hand them over first: the lock's
    // first frame has them. One that fails to build must not keep the lock
    say(requests);
    let _ = panic::catch_unwind(AssertUnwindSafe(|| scene::hand(locker)));
    locker.lock(newest);
    HEARD.notify_all();
    newest
}

// what `kanade-lock` said, on its thread
fn heard(news: News) {
    if news == News::Gone {
        /*
         * the compositor's connection is gone, and niri stays locked on red if it was: as a crash,
         * so the unit restarts the shell, which locks again
         */
        eprintln!("kanade: the lock screen lost the compositor");
        process::exit(1);
    }

    let mut requests = requests();

    let was = requests.locked;
    requests.heard(news);
    let locked = requests.locked;
    say(&requests);
    drop(requests);
    HEARD.notify_all();

    // the lock's own transition, after the requests are let go: a hook never waits under them
    if locked != was {
        hooks::fire(if locked { Event::Lock } else { Event::Unlock });
    }
}

// whether the session is locked, its lock on its way, or a password being checked on it
pub fn holds() -> bool {
    matches!(requests().holding(), Holding::Held | Holding::Checking)
}

/*
 * the time since boot, suspend included, which `Instant` leaves out: a grace is a time of the
 * user's presence, and a machine asleep for hours must not have 25 s of it left
 */
fn boot() -> Duration {
    let time = rustix::time::clock_gettime(rustix::time::ClockId::Boottime);

    Duration::new(
        time.tv_sec.unsigned_abs(),
        time.tv_nsec.unsigned_abs() as u32,
    )
}

/*
 * input on the lock screen, on its thread: unlocks as an accepted password does if a grace is
 * open (ADR 0066), else nothing. The same steps as `checked`: the unlock is asked under the
 * requests, so no request or sleep falls between the decision and the ask
 */
fn touched() {
    let mut requests = requests();

    if !requests.touch(boot()) {
        return;
    }

    if let Ok(locker) = locker() {
        locker.unlock();
    }
    say(&requests);
    drop(requests);
    HEARD.notify_all();
}

/*
 * the idle Module powered the monitors off (`dark`), or input lit them: the first key after they
 * went off is what woke them, so the lock screen does not type it (ADR 0065). Only while a lock
 * holds; the lock cannot know of monitors powered off any other way
 */
pub fn display(dark: bool) {
    if (!dark || holds())
        && let Ok(locker) = locker()
    {
        locker.dark(dark);
    }
}

// `lock.grace` in `config`, none while it is 0
pub fn grace_of(config: &Config) -> Option<Duration> {
    let seconds = config.lock_grace.min(config::LOCK_GRACE.most);

    (seconds > 0).then(|| Duration::from_secs(u64::from(seconds)))
}

// `lock.grace` in effect
pub fn grace() -> Option<Duration> {
    grace_of(&config::get())
}

// the lines of `kanade status`: the sleep gate, and what the lock options change from the defaults
pub fn lines() -> Vec<String> {
    let mut lines = vec![sleep::status()];
    let service = service();

    if service != PAM {
        lines.push(format!("lock: password checked with PAM service {service}"));
    }
    if let Some(grace) = grace() {
        lines.push(format!(
            "lock: grace {} s: input unlocks a lock the idle Module asks for, without a password",
            grace.as_secs()
        ));
    }

    lines
}

/*
 * locks the session before it sleeps (`crate::sleep`), after `sleeping` shut the gate and dropped
 * a check; only logind opens and shuts it, so a wake heard before this runs stays heard. Asks for
 * a lock and waits until niri holds it, for at most `patience`, so the sleep thread hears logind
 * again by then
 */
pub fn hold(patience: Duration) -> Result<(), String> {
    held(patience, None)
}

/*
 * the lock of idle (`crate::idle`), as `hold` does it, and with `grace` (ADR 0066) if it is some
 * and no lock held or was on its way: input unlocks it, without a password, until `grace` after
 * niri holds it. The caller gives none for a lock asked again after one failed. The grace is the
 * lock of this request alone, and ends with any other request, a sleep, an unlock or the lock
 */
pub fn hold_idle(patience: Duration, grace: Option<Duration>) -> Result<(), String> {
    held(patience, grace)
}

fn held(patience: Duration, grace: Option<Duration>) -> Result<(), String> {
    if pam().is_none() {
        return Err(format!("no PAM service {}, so no lock is asked", service()));
    }

    let locker = locker()?;
    let deadline = Instant::now() + patience;
    let mut requests = requests();
    let mut asked = None;

    // decided before this asks, under the requests: a lock held already, or an unlock on its way,
    // is not one this lock may open
    let mut grace = grace.filter(|_| requests.graceable());

    loop {
        match requests.next(asked) {
            Next::Done => {
                if let (Some(grace), Some(asked)) = (grace, asked) {
                    requests.grant(asked, boot() + grace);
                }
                return Ok(());
            }
            Next::Ask => {
                // asked again, after the first lock ended: that is no fresh lock for a grace
                if asked.is_some() {
                    grace = None;
                }
                asked = Some(request_lock(&mut requests, locker));
                continue;
            }
            Next::Denied => {
                return Err(String::from(
                    "niri refused the lock, as while another locker holds the session",
                ));
            }
            Next::Wait => {}
        }

        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!("niri did not lock within {patience:?}"));
        }

        requests = HEARD
            .wait_timeout(requests, left)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
}

/*
 * the machine is on its way to sleep: from now until `woke`, no password typed goes to PAM, even
 * while another program's inhibitor still delays it. A check under way is dropped, and the lock
 * screen says why
 */
pub fn sleeping() {
    let mut requests = requests();
    requests.sleep();
    say(&requests);
}

// the machine woke, or its sleep failed: passwords go to PAM again
pub fn woke() {
    let mut requests = requests();
    let sleep = requests.sleeps;
    requests.woke(sleep);
    say(&requests);
}

/*
 * a password was typed while sleep shut the gate: logind may have woken with no
 * `PrepareForSleep(false)` heard, as when it restarted, so ask it. Off the lock's thread, as logind
 * may be slow to answer; the password stays in the field to send again. Once per sleep at a time
 * (`Requests::ask`), however often Enter is pressed
 */
fn awake(asking: &mut Requests) {
    let Some(sleep) = asking.ask() else {
        return;
    };

    let asked = thread::Builder::new()
        .name(String::from("sleep ended"))
        .spawn(move || {
            let preparing = sleep::preparing();
            let mut requests = requests();
            requests.asked(sleep);

            if preparing == Some(false) && requests.sleeping && requests.sleeps == sleep {
                requests.woke(sleep);
                say(&requests);
                eprintln!("kanade: the machine woke unheard of; passwords are checked again");
            }
        });

    if let Err(error) = asked {
        asking.asked(sleep);
        eprintln!("kanade: cannot ask logind whether the machine woke: {error}");
    }
}

/*
 * `kanade lock status`: the newest request, and the newest confirmed one while the lock still
 * holds, else what ended it or may, a line each
 */
pub fn status() -> String {
    requests().status()
}

fn requests() -> MutexGuard<'static, Requests> {
    REQUESTS.locked()
}

/*
 * what the lock screens say under the field, and how many passwords were refused; written from the
 * requests (`say`), so the scenes, which read it, never take them
 */
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Said {
    status: Status,

    // what PAM said during the check, which the screen shows in place of the status
    note: Option<Note>,
    wrong: u64,

    // whether a lock holds or is on its way: the scenes read the clock only then
    locked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Status {
    #[default]
    None,
    Checking,
    Sleeping,
    Again,
    Wrong,
}

impl Service for Said {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {}
}

// read first, as a write redraws the scenes
fn say(requests: &Requests) {
    let said = requests.said();

    if *Said::read() != said {
        *Said::write() = said;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kanade_lock::stage::{Stage, Step};

    // an accepted password ends the lock, so sleep asks over it
    #[test]
    fn sleep_asks_over_an_unlock() {
        let mut requests = Requests::new();
        assert_eq!(requests.next(None), Next::Ask);

        let asked = Some(requests.request());
        assert_eq!(requests.next(asked), Next::Wait);
        requests.heard(News::Locked(1));
        assert_eq!(requests.next(asked), Next::Done);

        // typed and checked after niri locked, then accepted
        let check = requests.check().unwrap();
        assert_eq!(requests.next(asked), Next::Wait);
        assert!(requests.checked(check, true));
        assert_eq!(requests.next(asked), Next::Ask);

        // asked over before the unlock is heard: the lock wins, as `kanade-lock` ends the lock
        // asked to, then locks again
        let number = requests.request();
        let asked = Some(number);
        assert!(!requests.unlocking);
        assert_eq!(requests.next(asked), Next::Wait);
        requests.heard(News::Unlocked(1));
        assert_eq!(requests.next(asked), Next::Wait);
        assert_eq!(requests.settled(number), Settled::Waiting);
        requests.heard(News::Locked(2));
        assert_eq!(requests.next(asked), Next::Done);
        assert_eq!(requests.settled(number), Settled::Locked);

        // niri ends that lock itself before `kanade-lock` takes the unlock: the next is still on
        // its way
        let check = requests.check().unwrap();
        assert!(requests.checked(check, true));
        let asked = Some(requests.request());
        requests.heard(News::Finished(2));
        assert_eq!(requests.next(asked), Next::Wait);
        requests.heard(News::Locked(3));
        assert_eq!(requests.next(asked), Next::Done);

        // a wrong password keeps the lock
        let check = requests.check().unwrap();
        assert!(requests.checked(check, false));
        assert_eq!(requests.next(asked), Next::Done);
        assert_eq!(requests.wrong, 1);
    }

    // a lock held for a request, with a grace open until `until`
    fn graced(until: Duration) -> Requests {
        let mut requests = Requests::new();
        assert!(requests.graceable());

        let asked = requests.request();
        requests.heard(News::Locked(asked));
        requests.grant(asked, until);
        requests
    }

    // ADR 0066: input unlocks inside the grace, once, as a password does
    #[test]
    fn input_in_the_grace_unlocks_once() {
        let now = boot();
        let mut requests = graced(now + Duration::from_secs(5));

        assert!(requests.touch(now + Duration::from_secs(1)));
        assert!(requests.unlocking);
        assert_eq!(requests.holding(), Holding::Unlocked);
        assert!(!requests.touch(now + Duration::from_secs(2)));
    }

    #[test]
    fn input_after_the_grace_never_unlocks() {
        let now = boot();
        let until = now + Duration::from_secs(5);
        let mut requests = graced(until);

        // input handled late is late, whatever it was when it happened
        assert!(!requests.touch(until));
        assert!(!requests.touch(now));
        assert_eq!(requests.grace, None);
    }

    // a request, a sleep, the end of the lock and a password each close the grace
    #[test]
    fn the_grace_ends_with_anything_but_the_lock_it_was_given_for() {
        let now = boot();
        let until = now + Duration::from_secs(5);

        let mut requests = graced(until);
        requests.request();
        assert!(!requests.touch(now));

        let mut requests = graced(until);
        requests.sleep();
        assert!(!requests.touch(now));

        let mut requests = graced(until);
        requests.heard(News::Unlocked(1));
        assert!(!requests.touch(now));

        let mut requests = graced(until);
        requests.heard(News::Gone);
        assert!(!requests.touch(now));

        let mut requests = graced(until);
        requests.heard(News::Finished(1));
        assert!(!requests.touch(now));

        // a password being checked is not interrupted, and once accepted leaves no grace
        let mut requests = graced(until);
        let check = requests.check().unwrap();
        assert!(!requests.touch(now));
        assert!(requests.checked(check, true));
        assert_eq!(requests.grace, None);
    }

    // only a lock asked for from fully unlocked and held for that request gets a grace
    #[test]
    fn only_a_fresh_confirmed_lock_gets_a_grace() {
        let now = boot();
        let until = now + Duration::from_secs(5);

        // one already asked, held, or ended by a password
        let mut requests = Requests::new();
        let asked = requests.request();
        assert!(!requests.graceable());
        requests.heard(News::Locked(asked));
        assert!(!requests.graceable());
        let check = requests.check().unwrap();
        assert!(requests.checked(check, true));
        assert!(!requests.graceable());

        // not yet confirmed, or confirmed for an older request
        let mut requests = Requests::new();
        let old = requests.request();
        let new = requests.request();
        requests.heard(News::Locked(old));
        requests.grant(old, until);
        requests.grant(new, until);
        assert_eq!(requests.grace, None);

        // asleep
        let mut requests = Requests::new();
        requests.sleep();
        assert!(!requests.graceable());
    }

    // ADR 0065: PAM's messages show for the check under way; its error stays until the next try
    #[test]
    fn pam_messages_show_for_the_check_under_way() {
        let mut requests = Requests::new();
        let asked = requests.request();
        requests.heard(News::Locked(asked));

        assert!(!requests.noted(1, Note::Info(String::from("early"))));
        let check = requests.check().unwrap();
        assert!(!requests.noted(check + 1, Note::Info(String::from("other"))));
        assert!(requests.noted(check, Note::Info(String::from("Place your finger"))));
        assert_eq!(
            requests.note,
            Some(Note::Info(String::from("Place your finger")))
        );
        assert!(requests.checked(check, false));
        assert_eq!(requests.note, None);

        let check = requests.check().unwrap();
        assert!(requests.noted(check, Note::Error(String::from("Too many tries"))));
        assert!(requests.checked(check, false));
        assert_eq!(
            requests.note,
            Some(Note::Error(String::from("Too many tries")))
        );

        requests.sleep();
        assert_eq!(requests.note, None);
    }

    #[test]
    fn a_pam_service_is_a_file_name_not_a_path() {
        assert!(valid("login"));
        assert!(valid("kanade-lock_2.x"));
        for bad in [
            "", ".hidden", "-x", "../login", "a/b", "a b", "login\n", "é",
        ] {
            assert!(!valid(bad), "{bad:?}");
        }
        assert!(!valid(&"a".repeat(65)));
        assert_eq!(pam_file("../../etc/passwd"), None);
    }

    #[test]
    fn a_lock_niri_refused_is_denied_not_unlocked() {
        let mut requests = Requests::new();

        let asked = Some(requests.request());
        requests.heard(News::Finished(1));
        assert_eq!(requests.next(asked), Next::Denied);

        assert_eq!(requests.settled(1), Settled::Denied);

        // held, then ended by niri
        let asked = Some(requests.request());
        requests.heard(News::Locked(2));
        assert_eq!(requests.next(asked), Next::Done);
        requests.heard(News::Finished(2));
        assert_eq!(requests.next(asked), Next::Ask);
    }

    // on its way to sleep, a password typed goes nowhere until the machine wakes
    #[test]
    fn no_password_goes_to_pam_while_the_machine_sleeps() {
        let mut requests = Requests::new();

        requests.sleep();
        assert_eq!(requests.check(), None);
        assert!(requests.refused);
        assert_eq!(requests.said().status, Status::Sleeping);

        // a wake found late for the sleep before does not open the gate of the one after
        assert!(requests.woke(1));
        requests.sleep();
        assert!(!requests.woke(1));
        assert_eq!(requests.check(), None);

        // the lock screen stops saying it waits once the gate opens
        assert!(requests.woke(2));
        assert!(!requests.refused);
        assert_eq!(requests.check(), Some(1));
        assert_eq!(requests.said().status, Status::Checking);
    }

    // Enter pressed again and again while sleep shuts the gate asks logind once at a time
    #[test]
    fn logind_is_asked_once_per_sleep_at_a_time() {
        let mut requests = Requests::new();
        requests.sleep();

        assert_eq!(requests.ask(), Some(1));
        assert_eq!(requests.ask(), None);
        requests.asked(1);
        assert_eq!(requests.ask(), Some(1));

        // a newer sleep is asked about, and the older answer frees nothing of it
        requests.woke(1);
        assert!(!requests.sleeping);
        requests.sleep();
        assert_eq!(requests.ask(), Some(2));
        requests.asked(1);
        assert_eq!(requests.ask(), None);
        requests.asked(2);
        assert_eq!(requests.ask(), Some(2));
    }

    // a check sleep dropped unlocks nothing, however late PAM ends it
    #[test]
    fn sleep_drops_a_password_being_checked() {
        let mut requests = Requests::new();
        requests.request();
        requests.heard(News::Locked(1));

        let dropped = requests.check().unwrap();
        assert_eq!(requests.holding(), Holding::Checking);

        requests.sleep();
        assert!(requests.refused);
        assert_eq!(requests.holding(), Holding::Held);
        assert_eq!(requests.said().status, Status::Sleeping);

        // accepted after the machine began to sleep, then after it woke
        assert!(!requests.checked(dropped, true));
        assert!(requests.woke(1));
        assert!(!requests.checked(dropped, true));
        assert!(!requests.unlocking);

        // the next check stands, and a wrong password keeps the lock
        let check = requests.check().unwrap();
        assert!(!requests.checked(dropped, true));
        assert!(requests.checked(check, false));
        assert_eq!(requests.holding(), Holding::Held);
        assert_eq!(requests.said().status, Status::Wrong);
    }

    // `kanade lock status` of a shell that was never asked to lock says so
    #[test]
    fn nothing_asked_is_unlocked() {
        let requests = Requests::new();
        assert_eq!(requests.holding(), Holding::Unlocked);
        assert_eq!(requests.status(), "requested #0\nunlocked");
    }

    // a request after another that asked over an unlock waits for the next lock too
    #[test]
    fn a_lock_asked_twice_over_an_unlock_waits_for_the_next() {
        let mut requests = Requests::new();
        requests.request();
        requests.heard(News::Locked(1));

        let check = requests.check().unwrap();
        assert!(requests.checked(check, true));
        requests.request();
        let third = Some(requests.request());
        assert_eq!(requests.confirmed, 0);
        assert_eq!(requests.next(third), Next::Wait);

        requests.heard(News::Unlocked(1));
        assert_eq!(requests.next(third), Next::Wait);
        requests.heard(News::Locked(3));
        assert_eq!(requests.next(third), Next::Done);
    }

    // an unlock asked while niri locks, then a lock: only the newest request's lock is denied
    #[test]
    fn a_lock_asked_over_an_unlock_while_locking_is_denied_only_for_itself() {
        let mut requests = Requests::new();
        requests.request();

        let check = requests.check().unwrap();
        assert!(requests.checked(check, true));
        let asked = Some(requests.request());

        // niri refused #1's lock before `kanade-lock` took #2, which it locks again for
        requests.heard(News::Finished(1));
        assert_eq!(requests.next(asked), Next::Wait);
        requests.heard(News::Finished(2));
        assert_eq!(requests.next(asked), Next::Denied);
    }

    // niri holding a lock an unlock is queued for confirms no request asked after that unlock
    #[test]
    fn an_older_lock_held_confirms_no_newer_request() {
        let mut requests = Requests::new();
        requests.request();

        // accepted while niri locks; #2 asked before `kanade-lock` takes the unlock
        let check = requests.check().unwrap();
        assert!(requests.checked(check, true));
        let asked = Some(requests.request());

        // niri's `locked` for #1, then the unlock, then #2's lock
        requests.heard(News::Locked(1));
        assert_eq!(requests.next(asked), Next::Wait);
        requests.heard(News::Unlocked(1));
        assert_eq!(requests.next(asked), Next::Wait);
        requests.heard(News::Locked(2));
        assert_eq!(requests.next(asked), Next::Done);

        // or the unlock and #2 taken first: the lock niri holds then serves #2
        let mut requests = Requests::new();
        requests.request();
        let check = requests.check().unwrap();
        assert!(requests.checked(check, true));
        let asked = Some(requests.request());
        requests.heard(News::Locked(2));
        assert_eq!(requests.next(asked), Next::Done);
    }

    // an unlock of an older request heard does not hide one asked since, still to come
    #[test]
    fn a_lock_held_before_a_later_unlock_confirms_nothing() {
        let mut requests = Requests::new();
        requests.request();
        requests.heard(News::Locked(1));

        // accepted, asked over, accepted again, all before `kanade-lock` takes any
        let check = requests.check().unwrap();
        assert!(requests.checked(check, true));
        let asked = Some(requests.request());
        let check = requests.check().unwrap();
        assert!(requests.checked(check, true));

        // #1's unlock, then niri holds #2's lock before the second unlock is taken
        requests.heard(News::Unlocked(1));
        requests.heard(News::Locked(2));
        assert_eq!(requests.confirmed, 0);
        assert_eq!(requests.next(asked), Next::Ask);

        requests.heard(News::Unlocked(2));
        assert_eq!(requests.next(asked), Next::Ask);
        assert!(!requests.locked);
    }

    // a lock asked for drops a check under way, and says to type the password again
    #[test]
    fn a_request_drops_a_password_being_checked() {
        let mut requests = Requests::new();
        requests.request();
        requests.heard(News::Locked(1));

        let dropped = requests.check().unwrap();
        requests.request();

        // one PAM check at a time: none begins until the dropped one ends
        assert_eq!(requests.said().status, Status::Checking);
        assert_eq!(requests.check(), None);
        assert!(!requests.checked(dropped, true));
        assert_eq!(requests.said().status, Status::Again);
        assert_eq!(requests.holding(), Holding::Held);

        requests.check().unwrap();
        assert_eq!(requests.said().status, Status::Checking);
    }

    // a lock is confirmed only once niri holds it, and only while no unlock is asked
    #[test]
    fn a_lock_is_confirmed_only_while_no_unlock_is_pending() {
        let mut requests = Requests::new();
        let settled = |requests: &Requests| requests.settled(1);

        assert_eq!(requests.request(), 1);
        assert_eq!(settled(&requests), Settled::Waiting);
        requests.heard(News::Locked(1));
        assert_eq!(settled(&requests), Settled::Locked);

        // accepted: #1 no longer holds, though niri has not unlocked yet
        let check = requests.check().unwrap();
        assert_eq!(settled(&requests), Settled::Waiting);
        assert!(requests.checked(check, true));
        assert_eq!(settled(&requests), Settled::Unlocked);
        requests.heard(News::Unlocked(1));
        assert_eq!(settled(&requests), Settled::Unlocked);

        // asked again while unlocked; niri has not locked yet, so the last confirmation stays gone
        assert_eq!(requests.request(), 2);
        assert_eq!(settled(&requests), Settled::Waiting);
        requests.heard(News::Locked(2));
        assert_eq!(requests.confirmed, 2);
        assert_eq!(settled(&requests), Settled::Locked);

        // asked while held: confirmed as `kanade-lock` says it holds the lock for it too
        assert_eq!(requests.request(), 3);
        assert_eq!(requests.confirmed, 0);
        requests.heard(News::Locked(3));
        assert_eq!(requests.confirmed, 3);
    }

    // an unlock asked while niri was still locking ends the lock: no confirmation from its Locked
    #[test]
    fn an_unlock_asked_while_locking_confirms_nothing() {
        let mut requests = Requests::new();
        let asked = Some(requests.request());

        // accepted on a lock surface shown before niri said it locked
        let check = requests.check().unwrap();
        assert!(requests.checked(check, true));

        // niri's `locked` then comes before the unlock asked ends it
        requests.heard(News::Locked(1));
        assert_eq!(requests.confirmed, 0);
        assert_eq!(requests.next(asked), Next::Ask);

        requests.heard(News::Unlocked(1));
        assert_eq!(requests.next(asked), Next::Ask);
        assert!(!requests.locked);
    }

    // a request is settled by what holds the lock now, and an older one is not denied for a newer
    #[test]
    fn a_request_settles_while_its_lock_holds() {
        let mut requests = Requests::new();

        requests.request();
        requests.request();
        requests.request();
        requests.heard(News::Locked(3));
        assert_eq!(requests.settled(3), Settled::Locked);
        assert_eq!(requests.settled(2), Settled::Locked);

        let check = requests.check().unwrap();
        assert_eq!(requests.settled(3), Settled::Waiting);
        assert!(requests.checked(check, true));
        assert_eq!(requests.settled(3), Settled::Unlocked);
        requests.heard(News::Unlocked(3));

        requests.request();
        requests.heard(News::Finished(4));
        assert_eq!(requests.settled(4), Settled::Denied);
        assert_eq!(requests.settled(3), Settled::Unlocked);
    }

    // the shell and `kanade-lock`'s stage, as the asks between them queue and niri answers
    #[derive(Clone)]
    struct World {
        requests: Requests,
        stage: Stage,
        asks: Vec<Asked>,

        // the check under way, the newest request as it began, and whether a sleep began since
        check: Option<(u64, u64, bool)>,

        // a password was accepted since the newest request
        accepted: bool,
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Asked {
        Lock(u64),
        Unlock,
    }

    impl World {
        fn step(&mut self, step: Step) {
            if let Step::Tell(news) | Step::End(news) = step {
                // the newest request's lock ends by an unlock only once a password was accepted
                if news == News::Unlocked(self.requests.newest) {
                    assert!(self.accepted);
                }
                self.requests.heard(news);
            }
        }

        // whether `action` could be taken, having taken it
        fn act(&mut self, action: u8) -> bool {
            match action {
                0 if self.requests.newest < 3 => {
                    let request = self.requests.request();
                    self.asks.push(Asked::Lock(request));
                    self.accepted = false;
                }
                1 if self.check.is_none() => {
                    let sleeping = self.requests.sleeping;
                    let Some(check) = self.requests.check() else {
                        return false;
                    };

                    // no password goes to PAM while the machine sleeps
                    assert!(!sleeping);
                    self.check = Some((check, self.requests.newest, false));
                }
                2 | 3 if self.check.is_some() => {
                    let (check, newest, slept) = self.check.take().unwrap();

                    // a check stands unless a request or a sleep came while it ran
                    let stands = self.requests.checked(check, action == 2);
                    assert_eq!(stands, newest == self.requests.newest && !slept);
                    if stands && action == 2 {
                        self.asks.push(Asked::Unlock);
                        self.accepted = true;
                    }
                }
                4 if !self.asks.is_empty() => {
                    let step = match self.asks.remove(0) {
                        Asked::Lock(request) => self.stage.lock(request),
                        Asked::Unlock => self.stage.unlock(),
                    };
                    self.step(step);
                }
                5 if matches!(self.stage, Stage::Locking { .. }) => {
                    let step = self.stage.locked();
                    self.step(step);
                }
                6 if self.stage != Stage::Unlocked => {
                    let step = self.stage.finished();
                    self.step(step);
                }
                7 if matches!(self.stage, Stage::Locking { .. }) => {
                    let step = self.stage.refused();
                    self.step(step);
                }
                8 if !self.requests.sleeping => {
                    self.requests.sleep();
                    if let Some((_, _, slept)) = &mut self.check {
                        *slept = true;
                    }
                }
                9 if self.requests.sleeping => {
                    let sleep = self.requests.sleeps;
                    self.requests.woke(sleep);
                }
                _ => return false,
            }
            true
        }

        fn holds(&self) {
            let requests = &self.requests;
            assert_eq!(requests.locked, matches!(self.stage, Stage::Locked { .. }));

            // confirmed only while niri holds the newest request's lock, with no unlock coming
            if requests.confirmed > 0 {
                assert_eq!(requests.confirmed, requests.newest);
                assert_eq!(
                    self.stage,
                    Stage::Locked {
                        request: requests.newest
                    }
                );
                assert!(!self.asks.contains(&Asked::Unlock));
            }

            // denied or ended: no lock is held for the newest request, nor asked
            if matches!(requests.holding(), Holding::Denied)
                || (requests.ended && !requests.unlocking)
            {
                assert!(!matches!(self.stage, Stage::Locked { .. }));
                assert!(!self.asks.iter().any(|ask| matches!(ask, Asked::Lock(_))));
            }
        }

        // the asks taken and niri holding every lock, until nothing is left to do
        fn settle(&mut self) {
            if let Some((check, _, _)) = self.check.take() {
                self.requests.checked(check, false);
            }
            while self.act(4) || self.act(5) {
                self.holds();
            }

            let requests = &self.requests;
            match requests.holding() {
                Holding::Held => {
                    assert_eq!(requests.confirmed, requests.newest);
                }
                Holding::Unlocked => assert_eq!(self.stage, Stage::Unlocked),
                _ => {}
            }
        }
    }

    fn explore(world: &World, depth: u8) {
        let mut settled = world.clone();
        settled.settle();
        if depth == 0 {
            return;
        }

        for action in 0..10 {
            let mut next = world.clone();
            if next.act(action) {
                next.holds();
                explore(&next, depth - 1);
            }
        }
    }

    /*
     * every interleaving of requests, passwords, sleeps, asks taken and niri's answers keeps the
     * lock
     */
    #[test]
    fn no_interleaving_confirms_a_lock_not_held() {
        let world = World {
            requests: Requests::new(),
            stage: Stage::default(),
            asks: Vec::new(),
            check: None,
            accepted: false,
        };
        explore(&world, 10);
    }
}
