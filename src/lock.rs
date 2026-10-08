//! The lock screen (#154, ADR 0018): Amane's `Lock`, ext-session-lock with the password checked by
//! PAM, in the shell process. Every monitor shows the time, the date and a password field, and
//! nothing else: no notification, no Activity. The password goes from the field to PAM and nowhere
//! else; the field is emptied as it is sent, and it is never logged or drawn.
//!
//! `kanade lock` exits 0 only once a lock screen's view runs after its request (#196): Amane opens
//! one only once niri says the session is locked, and the request first drops an unlock PAM accepted
//! that Amane has not ended the lock for yet, so a later draw cannot be the last lock on its way
//! out. It must still hold when the client asks: no password typed since is being checked or was
//! accepted. logind's `LockedHint` may still say the last lock right after an unlock, so it is only
//! for a crash.
//!
//! A crash while locked leaves niri locked on its red screen; the user unit (`UNIT`) restarts the
//! shell, which finds logind's `LockedHint` true and locks again, so niri swaps the dead lock for
//! this one.

use std::env;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use amane::{
    Argument, Bus, Center, Column, Key, LayerWindow, Lock, Monitor, Padding, Parent, Rectangle,
    Service, Start, Text, TextInput, Value, children,
};

use crate::clock;
use crate::config;
use crate::sources::sleep;
use crate::theme::{self, radius};

// the systemd user unit that runs the shell, which `dist/` holds and the README installs
pub const UNIT: &str = "kanade.service";

// the PAM service Amane checks the password with, which Amane does not export; Kanade's too
pub const PAM: &str = "login";

// where Linux-PAM looks for a service's file, the admin's first
const PAM_DIRS: [&str; 3] = ["/etc/pam.d", "/usr/lib/pam.d", "/usr/etc/pam.d"];

const LOGIND: &str = "org.freedesktop.login1";

// the session of whoever asks; from a user unit, which has none of its own, the graphical one
const AUTO: &str = "/org/freedesktop/login1/session/auto";

const FIELD: &str = "lock-password";

/*
 * the requests of `kanade lock` and of sleep (`hold`), numbered from 1 in this process (`instance`);
 * each waits until `confirmed` reaches its own while the lock still holds. Taken before Amane's
 * `Lock`, never after, so a request and a password sent from the field never interleave
 */
struct Requests {
    newest: u64,

    // the newest request a lock screen drew after, with no unlock pending
    confirmed: u64,

    // a password went to PAM since the newest request
    tried: bool,

    // from logind's `PrepareForSleep(true)` until `(false)`: no password goes to PAM (#155)
    sleeping: bool,

    // the sleeps begun, so a wake found late opens only the gate of the sleep it was asked about
    sleeps: u64,

    // a password was typed while `sleeping`, or its check dropped, so the lock screen says why
    refused: bool,

    /*
     * Kanade's own PAM check of a password typed, numbered `check`, before Amane's `Lock::unlock`
     * gets it: one sleep dropped is never heard of, however late it ends
     */
    checking: bool,
    check: u64,

    // Kanade's check refused the last password
    failed: bool,

    // a lock is still to be asked once Amane's check outlasting `hold` ends (`owe`)
    owed: bool,

    // the sleep logind is being asked whether it ended (`awake`), one at a time
    asking: Option<u64>,
}

impl Requests {
    const fn new() -> Self {
        Self {
            newest: 0,
            confirmed: 0,
            tried: false,
            sleeping: false,
            sleeps: 0,
            refused: false,
            checking: false,
            check: 0,
            failed: false,
            owed: false,
            asking: None,
        }
    }

    // shuts the gate, and drops a check under way: its password has to be typed again
    fn sleep(&mut self) {
        if !self.sleeping {
            self.sleeping = true;
            self.sleeps += 1;
        }

        if self.drop_check() {
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

    // a password typed now goes to PAM, unless the machine is on its way to sleep
    fn try_password(&mut self) -> bool {
        if self.sleeping {
            self.refused = true;
            return false;
        }

        self.tried = true;
        true
    }

    // a check under way is dropped, whether there was one: as if no password went to PAM
    fn drop_check(&mut self) -> bool {
        let dropped = std::mem::take(&mut self.checking);
        if dropped {
            self.tried = false;
        }
        dropped
    }

    // a check begins, which `checked` ends by its number
    fn check(&mut self) -> u64 {
        self.check += 1;
        self.checking = true;
        self.failed = false;
        self.check
    }

    // whether the check `check` ended stands, so Amane gets a password accepted; not once dropped
    fn checked(&mut self, check: u64, accepted: bool) -> bool {
        if !self.checking || self.check != check {
            return false;
        }

        self.checking = false;
        self.failed = !accepted;
        true
    }

    /*
     * A new request, which only a draw after it confirms. It drops the last confirmation, which an
     * unlock may have ended since: `tried` no longer tells
     */
    fn request(&mut self) -> u64 {
        self.newest += 1;
        self.tried = false;
        self.confirmed = 0;
        self.newest
    }

    /*
     * Amane draws a lock screen only while niri holds the lock, so a draw confirms every request
     * so far, unless a password went to PAM since: being checked, or accepted and on its way out
     */
    fn drew(&mut self, checking: bool, failed: bool) {
        if self.holding(checking, failed) == Holding::Held {
            self.confirmed = self.newest;
        }
    }

    fn status(&self, instance: &str, checking: bool, failed: bool) -> String {
        let holding = match self.holding(checking, failed) {
            Holding::Held => format!("confirmed #{}", self.confirmed),
            Holding::Checking => String::from("checking a password"),
            Holding::Unlocked => String::from("unlocked"),
        };

        format!("instance {instance}\nrequested #{}\n{holding}", self.newest)
    }

    /*
     * what `hold`, having asked `asked`, does next. PAM ends a check with no draw when it accepts,
     * the lock windows closing unseen, so a check is looked at again rather than waited on: one
     * accepted is asked over
     */
    fn next(&self, asked: Option<u64>, checking: bool, failed: bool) -> Next {
        match (asked, self.holding(checking, failed)) {
            (Some(asked), Holding::Held) if self.confirmed >= asked => Next::Done,
            (_, Holding::Checking) => Next::Look,
            (None, _) | (_, Holding::Unlocked) => Next::Ask,
            (Some(_), Holding::Held) => Next::Wait,
        }
    }

    // whether a lock confirmed may still hold, by Kanade's check and Amane's `Lock`
    fn holding(&self, checking: bool, failed: bool) -> Holding {
        let (checking, failed) = (checking || self.checking, failed || self.failed);

        if checking {
            Holding::Checking
        } else if self.tried && !failed {
            Holding::Unlocked
        } else {
            Holding::Held
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holding {
    Held,

    // a password is being checked, which may unlock
    Checking,

    // a password was accepted since the newest request, so the lock ends or has ended
    Unlocked,
}

// where `hold` stands
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    // a lock screen confirmed the request, which still holds
    Done,

    // no request stands, or an unlock ended it: ask
    Ask,

    // asked: wait on a lock screen drawing
    Wait,

    // a password is being checked: look again shortly
    Look,
}

// how often `hold` looks at a password being checked
const LOOK: Duration = Duration::from_millis(50);

static REQUESTS: Mutex<Requests> = Mutex::new(Requests::new());

// told each time a lock screen draws, which may confirm a request or show an unlock
static DRAWN: Condvar = Condvar::new();

const WIDTH: f32 = 280.0;
const HEIGHT: f32 = 40.0;
const STATUS: f32 = 20.0;

// the file PAM reads for `PAM`; without it no password unlocks
pub fn pam() -> Option<PathBuf> {
    PAM_DIRS
        .iter()
        .map(|dir| Path::new(dir).join(PAM))
        .find(|path| path.is_file())
}

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
 * at start, before the shell runs: a session logind still counts as locked was locked by a shell
 * that died, so this one locks it again. niri refuses while another locker holds it, which Amane
 * leaves alone. Without `PAM` it locks all the same, as niri is locked anyway; only ending the
 * session gets out
 */
pub fn relock() {
    let session = session();

    if session.is_empty() {
        eprintln!("kanade: logind names no session, so a crash while locked is not locked again");
    }

    if locked(session) == Some(true) {
        eprintln!("kanade: the session is locked, locking it again");
        if pam().is_none() {
            eprintln!("kanade: no PAM service {PAM}, so no password will unlock");
        }
        Lock::start();
    }
}

// what `start` did
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Started {
    // asked niri for a lock, or kept the one held, as the request it names (`requested`)
    Requested(String),

    // asked nothing, as whether the session stays locked is not known: why
    Unknown(String),
}

// a request `start` named: the shell process it went to, and its number there
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    instance: String,
    number: u64,
}

// where a request stands by `status`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    // a lock screen drew after it, and the lock still holds
    Locked,
    Waiting,

    // a password typed on the lock screen unlocks it
    Unlocked,

    // the shell does not know it, as it restarted since
    Lost,
}

/*
 * `kanade lock`, which answers before niri locks; the client waits until a lock screen confirms
 * the request it names (`status`). While locked it keeps the lock, so a second one keeps what is
 * being typed.
 *
 * A password PAM accepted ends the lock only at Amane's next wake, and until then the lock screen
 * may still draw. So the request puts back a fresh `Lock`, as Amane does for a new lock, which
 * drops that unlock: the lock wins, and the password has to be typed again. While a password is
 * being checked nothing is asked, as Amane's check cannot be dropped. Amane past `6ace43e` may end
 * a lock differently; recheck this on a bump
 */
pub fn start() -> Result<Started, String> {
    if pam().is_none() {
        return Err(format!(
            "no PAM service {PAM} in {}, so no password would unlock",
            PAM_DIRS.join(", ")
        ));
    }

    match request(&mut requests()) {
        Some(newest) => Ok(Started::Requested(format!(
            "requested #{newest} in {}",
            instance()
        ))),
        None => Ok(Started::Unknown(String::from(
            "a password typed on the lock screen is being checked, so whether the session stays \
             locked is not known; nothing was asked, try again",
        ))),
    }
}

/*
 * asks niri for a lock as `start` says and hands back its number, or asks nothing while a password
 * is being checked. Checked and reset under one write, so PAM cannot end a check in between
 */
fn request(requests: &mut Requests) -> Option<u64> {
    let mut lock = Lock::write();

    if requests.checking || lock.checking() {
        return None;
    }

    // a wrong password is no unlock, and its "Wrong password" stays
    if !lock.failed() {
        *lock = Lock::new();
    }
    drop(lock);

    let newest = requests.request();

    // redraws every window, so a lock screen already shown confirms it
    Lock::start();
    Some(newest)
}

/*
 * locks the session before it sleeps (`crate::sleep`), after `sleeping` shut the gate and dropped
 * Kanade's check; only logind opens and shuts it, so a wake heard before this runs stays heard.
 * Asks for a lock and waits until a lock screen confirms it while it still holds, for at most
 * `patience`, so the sleep thread hears logind again by then. Amane's check of a password Kanade's
 * accepted cannot be dropped, so it is waited out, and its unlock undone by asking over; past
 * `patience`, by `owe`
 */
pub fn hold(patience: Duration) -> Result<(), String> {
    if pam().is_none() {
        return Err(format!("no PAM service {PAM}, so no lock is asked"));
    }

    settle(Some(patience))
}

// `hold` once a lock can be asked; with no `patience`, for as long as it takes
fn settle(patience: Option<Duration>) -> Result<(), String> {
    let deadline = patience.map(|patience| Instant::now() + patience);
    // only ever said once it passed
    let patience = patience.unwrap_or_default();
    let mut requests = requests();
    let mut asked = None;

    loop {
        let lock = Lock::read();
        let next = requests.next(asked, lock.checking(), lock.failed());
        drop(lock);

        let left = deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));
        let late = left.is_some_and(|left| left.is_zero());
        let wait = match next {
            Next::Done => return Ok(()),
            Next::Ask => {
                // after it a request stands, or a check began first
                asked = request(&mut requests);
                continue;
            }
            Next::Look if late => {
                owe(&mut requests);
                return Err(format!(
                    "a password was still being checked after {patience:?}; a lock is asked once \
                     the check ends"
                ));
            }
            Next::Wait if late => {
                return Err(format!("no lock screen confirmed it within {patience:?}"));
            }
            Next::Look => left.map_or(LOOK, |left| LOOK.min(left)),
            Next::Wait => match left {
                Some(left) => left,
                None => {
                    requests = DRAWN.wait(requests).unwrap_or_else(PoisonError::into_inner);
                    continue;
                }
            },
        };

        requests = DRAWN
            .wait_timeout(requests, wait)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
}

/*
 * Amane's check outlasted `hold`, and may yet unlock while the machine sleeps: off the sleep
 * thread, so it hears logind meanwhile, wait it out and ask over. One at a time
 */
fn owe(owing: &mut Requests) {
    if std::mem::replace(&mut owing.owed, true) {
        return;
    }

    let spawned = thread::Builder::new()
        .name(String::from("lock owed"))
        .spawn(|| {
            // with no deadline, it ends only once a lock screen confirms a lock
            let _ = settle(None);
            requests().owed = false;
        });

    if let Err(error) = spawned {
        owing.owed = false;
        eprintln!("kanade: cannot wait out a password check to lock after it: {error}");
    }
}

/*
 * the machine is on its way to sleep: from now until `woke`, no password typed goes to PAM, even
 * while another program's inhibitor still delays it. Kanade's check under way is dropped, and the
 * lock screen says why; Amane's of a password already accepted is not (`hold`), so it may still
 * unlock briefly
 */
pub fn sleeping() {
    let mut requests = requests();
    let dropped = requests.checking;
    requests.sleep();
    opened(dropped);
}

// the machine woke, or its sleep failed: passwords go to PAM again
pub fn woke() {
    let mut requests = requests();
    let sleep = requests.sleeps;
    opened(requests.woke(sleep));
}

// the lock screen redraws, as what it says changed
fn opened(changed: bool) {
    if changed {
        drop(Lock::write());
    }
}

/*
 * a password was typed while sleep shut the gate: logind may have woken with no
 * `PrepareForSleep(false)` heard, as when it restarted, so ask it. Off the view's thread, as logind
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
                opened(requests.woke(sleep));
                eprintln!("kanade: the machine woke unheard of; passwords are checked again");
            }
        });

    if let Err(error) = asked {
        asking.asked(sleep);
        eprintln!("kanade: cannot ask logind whether the machine woke: {error}");
    }
}

// the request `start` names
pub fn requested(text: &str) -> Option<Request> {
    let (number, instance) = text.strip_prefix("requested #")?.split_once(" in ")?;

    Some(Request {
        instance: String::from(instance),
        number: number.parse().ok()?,
    })
}

/*
 * `kanade lock status`: this process, the newest request, and the newest confirmed one while the
 * lock still holds, else what may end it, a line each. Read when asked, as a password accepted
 * after a request was confirmed ends that lock too
 */
pub fn status() -> String {
    let requests = requests();
    let lock = Lock::read();

    requests.status(instance(), lock.checking(), lock.failed())
}

// where `request` stands by `status`, if it parses
pub fn settled(status: &str, request: &Request) -> Option<Settled> {
    let mut lines = status.lines();
    let instance = lines.next()?.strip_prefix("instance ")?;
    let newest = lines
        .next()?
        .strip_prefix("requested #")?
        .parse::<u64>()
        .ok()?;
    let holding = lines.next()?;

    // a restarted shell numbers from 1 again
    if instance != request.instance {
        return Some(Settled::Lost);
    }
    if newest < request.number {
        return None;
    }

    Some(match holding {
        "checking a password" => Settled::Waiting,
        "unlocked" => Settled::Unlocked,
        // a later request confirmed is a lock after this one too
        confirmed => match confirmed.strip_prefix("confirmed #")?.parse::<u64>().ok()? {
            confirmed if confirmed >= request.number => Settled::Locked,
            _ => Settled::Waiting,
        },
    })
}

// this shell process, unlike any before it, even one with the same pid
fn instance() -> &'static str {
    static INSTANCE: OnceLock<String> = OnceLock::new();

    INSTANCE.get_or_init(|| {
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();

        format!("{}.{started}", process::id())
    })
}

fn requests() -> MutexGuard<'static, Requests> {
    REQUESTS.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn view(monitor: &Monitor) -> LayerWindow {
    let roles = theme::ISLAND;
    let mut requests = requests();
    let lock = Lock::read();

    requests.drew(lock.checking(), lock.failed());
    let waiting = requests.sleeping && requests.refused;
    let checking = requests.checking || lock.checking();
    let failed = requests.failed || lock.failed();
    drop(requests);
    DRAWN.notify_all();

    let (status, color) = if checking {
        ("Checking…", roles.on_surface_variant)
    } else if waiting {
        ("Locked for sleep", roles.on_surface_variant)
    } else if failed {
        ("Wrong password", theme::SEMANTIC.critical)
    } else {
        ("", roles.on_surface_variant)
    };

    let time = Text::new(clock::now(config::on(&monitor.name).clock))
        .size(theme::text::DISPLAY)
        .color(roles.on_surface)
        .weight(theme::text::SEMIBOLD);

    let date = Text::new(clock::today().format("%A, %-d %B").to_string())
        .size(theme::text::TITLE)
        .color(roles.on_surface_variant)
        .weight(theme::text::MEDIUM);

    let field = Rectangle::new()
        .width(WIDTH)
        .height(HEIGHT)
        .radius(radius::ROW)
        .fill(roles.surface_container_high)
        .padding(Padding {
            top: 0.0,
            right: 12.0,
            bottom: 0.0,
            left: 12.0,
        })
        .align_child(Start, Center)
        .child(
            TextInput::new(FIELD)
                .width(Parent)
                .size(theme::text::BODY)
                .color(roles.on_surface)
                .placeholder("Password")
                .password()
                .focused()
                .on_submit(submit),
        );

    // as tall with no status as with one, so the field stays put when one shows
    let status = Rectangle::new()
        .width(WIDTH)
        .height(STATUS)
        .align_child(Center, Center)
        .child(
            Text::new(status)
                .size(theme::text::BODY)
                .color(color)
                .weight(theme::text::MEDIUM),
        );

    // niri sizes a lock screen to its monitor, so this size is never used. Escape takes the focus
    // from the field and goes on to the window, which redraws after its on_key; the redraw
    // focuses the field again, so the next key types. Without an on_key nothing would redraw
    LayerWindow::new()
        .width(1.0)
        .height(1.0)
        .on_key(|key| {
            if key == Key::Escape {
                TextInput::set_text(FIELD, "");
            }
        })
        .child(
            Rectangle::new()
                .width(Parent)
                .height(Parent)
                .fill(roles.surface)
                .align_child(Center, Center)
                .child(
                    Column::new(children![time, date, field, status])
                        .gap(theme::space::INSET)
                        .align(Center),
                ),
        )
}

/*
 * one try at a time: while PAM checks, or the machine is on its way to sleep, what is typed waits in
 * the field. The field empties before PAM gets the password, so a wrong one is typed again from
 * nothing.
 *
 * Kanade checks it first, as a check Amane runs cannot be dropped: sleep drops Kanade's, so one
 * PAM ends after the machine began to sleep unlocks nothing. Only a password Kanade's check
 * accepted, with the gate open, goes on to Amane's `Lock::unlock`, which alone ends the lock and
 * checks it again; usually at once, else `hold` and `owe` wait it out
 */
fn submit(password: String) {
    if password.is_empty() {
        return;
    }

    // under the requests, so neither a request nor `hold` falls between the two
    let mut requests = requests();
    let mut lock = Lock::write();
    if requests.checking || lock.checking() {
        return;
    }
    if !requests.try_password() {
        drop(lock);
        awake(&mut requests);
        return;
    }

    // a "Wrong password" of Amane's goes only with a fresh `Lock`; one failed holds no unlock
    if lock.failed() {
        *lock = Lock::new();
    }
    drop(lock);

    let check = requests.check();
    TextInput::set_text(FIELD, "");

    let checking = thread::Builder::new()
        .name(String::from("lock check"))
        .spawn(move || checked(check, &password));

    if let Err(error) = checking {
        eprintln!("kanade: cannot check the password: {error}");
        requests.drop_check();
        drop(Lock::write());
    }
}

// runs the check `check` and hands a password accepted on to Amane, unless sleep dropped it
fn checked(check: u64, password: &str) {
    let accepted = env::var("USER").is_ok_and(|user| clock::authenticate(PAM, &user, password));

    // under the requests, so a sleep shutting the gate falls before or after, not between
    let mut requests = requests();
    if !requests.checked(check, accepted) {
        return;
    }

    if accepted {
        Lock::unlock(password);
    } else {
        drop(Lock::write());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // #155: an accepted password ends the lock with no draw, so sleep never waits on one
    #[test]
    fn sleep_asks_over_an_unlock_and_looks_at_a_check_again() {
        let mut requests = Requests::new();

        // asked nothing while a password is checked, then again once it is not
        assert_eq!(requests.next(None, true, false), Next::Look);
        assert_eq!(requests.next(None, false, false), Next::Ask);

        let asked = Some(requests.request());
        assert_eq!(requests.next(asked, false, false), Next::Wait);
        requests.drew(false, false);
        assert_eq!(requests.next(asked, false, false), Next::Done);

        // typed and checked after the lock screen confirmed it, then accepted
        requests.tried = true;
        assert_eq!(requests.next(asked, true, false), Next::Look);
        assert_eq!(requests.next(asked, false, false), Next::Ask);

        // a wrong password keeps the lock
        assert_eq!(requests.next(asked, false, true), Next::Done);
    }

    // #155: on its way to sleep, a password typed goes nowhere until the machine wakes
    #[test]
    fn no_password_goes_to_pam_while_the_machine_sleeps() {
        let mut requests = Requests::new();

        requests.sleep();
        assert!(!requests.try_password());
        assert!(!requests.tried);
        assert!(requests.refused);

        // a wake found late for the sleep before does not open the gate of the one after
        assert!(requests.woke(1));
        requests.sleep();
        assert!(!requests.woke(1));
        assert!(!requests.try_password());

        // the lock screen stops saying it waits once the gate opens
        assert!(requests.woke(2));
        assert!(!requests.refused);
        assert!(requests.try_password());
        assert!(requests.tried);
    }

    // #155: Enter pressed again and again while sleep shuts the gate asks logind once at a time
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

    // #155: a check sleep dropped unlocks nothing, however late PAM ends it
    #[test]
    fn sleep_drops_a_password_being_checked() {
        let mut requests = Requests::new();

        assert!(requests.try_password());
        let dropped = requests.check();
        assert_eq!(requests.holding(false, false), Holding::Checking);

        requests.sleep();
        assert!(requests.refused);
        assert_eq!(requests.holding(false, false), Holding::Held);
        assert_eq!(requests.next(None, false, false), Next::Ask);

        // accepted after the machine began to sleep, then after it woke
        assert!(!requests.checked(dropped, true));
        assert!(requests.woke(1));
        assert!(!requests.checked(dropped, true));

        // the next check stands, and a wrong password keeps the lock
        assert!(requests.try_password());
        let check = requests.check();
        assert!(!requests.checked(dropped, true));
        assert!(requests.checked(check, false));
        assert_eq!(requests.holding(false, false), Holding::Held);
    }

    /*
     * #155: a check outlasting `patience` keeps the sleep thread from logind no longer, and once it
     * ends accepted, a lock is still asked
     */
    #[test]
    fn sleep_waits_on_a_check_only_until_logind_would_sleep() {
        REQUESTS.lock().unwrap().checking = true;

        let started = Instant::now();
        let held = settle(Some(Duration::from_millis(120)));
        let waited = started.elapsed();

        assert_eq!(
            held,
            Err(String::from(
                "a password was still being checked after 120ms; a lock is asked once the check \
                 ends"
            ))
        );
        assert!(waited >= Duration::from_millis(120) && waited < Duration::from_secs(1));

        let newest = {
            let mut requests = REQUESTS.lock().unwrap();
            assert!(requests.owed);
            requests.checking = false;
            requests.tried = true;
            requests.newest
        };
        DRAWN.notify_all();

        let started = Instant::now();
        while REQUESTS.lock().unwrap().newest == newest {
            assert!(started.elapsed() < Duration::from_secs(1), "no lock asked");
            thread::sleep(Duration::from_millis(10));
        }

        // a lock screen confirms it, and the debt is paid
        REQUESTS.lock().unwrap().drew(false, false);
        DRAWN.notify_all();
        while REQUESTS.lock().unwrap().owed {
            assert!(
                started.elapsed() < Duration::from_secs(1),
                "the lock owed never settled"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    // #196: the last lock, on its way out after an unlock, draws once more
    #[test]
    fn a_lock_is_confirmed_only_while_no_unlock_is_pending() {
        let mut requests = Requests::new();

        assert_eq!(requests.request(), 1);
        requests.drew(false, false);
        assert_eq!(requests.confirmed, 1);
        assert_eq!(requests.holding(false, false), Holding::Held);

        // asked again while locked, then the password is typed before the redraw
        assert_eq!(requests.request(), 2);
        requests.tried = true;
        requests.drew(true, false);
        assert_eq!(requests.confirmed, 0);
        assert_eq!(requests.holding(true, false), Holding::Checking);

        // accepted, the lock not ended yet: no draw confirms, and #1 no longer holds either
        requests.drew(false, false);
        assert_eq!(requests.confirmed, 0);
        assert_eq!(requests.holding(false, false), Holding::Unlocked);

        // #1, confirmed before, is not reported as held either
        let request = requested("requested #1 in 42.7").unwrap();
        let unlocking = |checking| settled(&requests.status("42.7", checking, false), &request);
        assert_eq!(unlocking(true), Some(Settled::Waiting));
        assert_eq!(unlocking(false), Some(Settled::Unlocked));

        // a wrong password keeps the lock
        requests.drew(false, true);
        assert_eq!(requests.confirmed, 2);
        assert_eq!(requests.holding(false, true), Holding::Held);

        // asked again after an accepted one, which `start` dropped, so the lock stays
        assert_eq!(requests.request(), 3);
        assert_eq!(requests.holding(false, false), Holding::Held);
        requests.drew(false, false);
        assert_eq!(requests.confirmed, 3);
    }

    // a request after an unlock, before its lock screen draws, does not revive the last confirmation
    #[test]
    fn a_new_request_does_not_revive_a_confirmation_an_unlock_ended() {
        let mut requests = Requests::new();
        let first = requested("requested #1 in 42.7").unwrap();
        let settled = |requests: &Requests| settled(&requests.status("42.7", false, false), &first);

        requests.request();
        requests.drew(false, false);
        assert_eq!(settled(&requests), Some(Settled::Locked));

        // unlocked
        requests.tried = true;
        assert_eq!(settled(&requests), Some(Settled::Unlocked));

        // asked again while unlocked; niri has not locked yet
        assert_eq!(requests.request(), 2);
        assert_eq!(settled(&requests), Some(Settled::Waiting));

        requests.drew(false, false);
        assert_eq!(requests.confirmed, 2);
        assert_eq!(settled(&requests), Some(Settled::Locked));
    }

    #[test]
    fn a_request_settles_while_its_lock_holds_and_is_lost_to_a_restart() {
        let request = requested("requested #3 in 42.7").unwrap();
        assert_eq!(
            request,
            Request {
                instance: String::from("42.7"),
                number: 3
            }
        );
        assert_eq!(requested("requested #3"), None);
        assert_eq!(requested("locked"), None);

        let settled = |status: &str| settled(status, &request);
        assert_eq!(
            settled("instance 42.7\nrequested #3\nconfirmed #2"),
            Some(Settled::Waiting)
        );
        assert_eq!(
            settled("instance 42.7\nrequested #3\nconfirmed #3"),
            Some(Settled::Locked)
        );
        assert_eq!(
            settled("instance 42.7\nrequested #4\nconfirmed #4"),
            Some(Settled::Locked)
        );
        assert_eq!(
            settled("instance 42.7\nrequested #3\nchecking a password"),
            Some(Settled::Waiting)
        );
        assert_eq!(
            settled("instance 42.7\nrequested #3\nunlocked"),
            Some(Settled::Unlocked)
        );
        // another process's #3, confirmed or not, is not this one
        assert_eq!(
            settled("instance 42.9\nrequested #3\nconfirmed #3"),
            Some(Settled::Lost)
        );
        assert_eq!(settled("instance 42.7\nrequested #2\nconfirmed #2"), None);
        assert_eq!(settled("requested #3\nconfirmed #3"), None);
    }
}
