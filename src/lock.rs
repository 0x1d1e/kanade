//! The lock screen (#154, ADR 0018): Amane's `Lock`, ext-session-lock with the password checked by
//! PAM, in the shell process. Every monitor shows the time, the date and a password field, and
//! nothing else: no notification, no Activity. The password goes from the field to PAM and nowhere
//! else; the field is emptied as it is sent, and it is never logged or drawn.
//!
//! `kanade lock` exits 0 only once a lock screen's view runs after its request (#196): Amane opens
//! one only once niri says the session is locked, and the request first drops an unlock PAM accepted
//! that Amane has not ended the lock for yet, so a later draw cannot be the last lock on its way
//! out. logind's `LockedHint` may still say the last lock right after an unlock, so it is only for
//! a crash.
//!
//! A crash while locked leaves niri locked on its red screen; the user unit (`UNIT`) restarts the
//! shell, which finds logind's `LockedHint` true and locks again, so niri swaps the dead lock for
//! this one.

use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use amane::{
    Argument, Bus, Center, Column, Key, LayerWindow, Lock, Monitor, Padding, Parent, Rectangle,
    Service, Start, Text, TextInput, Value, children,
};

use crate::clock;
use crate::config;
use crate::theme::{self, radius};

// the systemd user unit that runs the shell, which `dist/` holds and the README installs
pub const UNIT: &str = "kanade.service";

// the PAM service Amane checks the password with, which Amane does not export
pub const PAM: &str = "login";

// where Linux-PAM looks for a service's file, the admin's first
const PAM_DIRS: [&str; 3] = ["/etc/pam.d", "/usr/lib/pam.d", "/usr/etc/pam.d"];

const LOGIND: &str = "org.freedesktop.login1";

// the session of whoever asks; from a user unit, which has none of its own, the graphical one
const AUTO: &str = "/org/freedesktop/login1/session/auto";

const FIELD: &str = "lock-password";

/*
 * `kanade lock`'s requests, numbered from 1; the client waits until `confirmed` reaches its own.
 * Only the draw thread touches them, from the IPC handler and the view
 */
struct Requests {
    newest: u64,

    // the newest request a lock screen drew after, with no unlock pending
    confirmed: u64,

    // a password went to PAM since the newest request
    tried: bool,
}

impl Requests {
    // a new request, which only a draw after it confirms
    fn request(&mut self) -> u64 {
        self.newest += 1;
        self.tried = false;
        self.newest
    }

    /*
     * Amane draws a lock screen only while niri holds the lock, so a draw confirms every request
     * so far, unless a password went to PAM since: being checked, or accepted and on its way out
     */
    fn drew(&mut self, checking: bool, failed: bool) {
        if !checking && !(self.tried && !failed) {
            self.confirmed = self.newest;
        }
    }
}

static REQUESTS: Mutex<Requests> = Mutex::new(Requests {
    newest: 0,
    confirmed: 0,
    tried: false,
});

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
 * Found once, at start
 */
fn session() -> &'static str {
    static SESSION: OnceLock<String> = OnceLock::new();

    SESSION.get_or_init(|| {
        let bus = Bus::system();
        let id = env::var("XDG_SESSION_ID").unwrap_or_else(|_| {
            let id = bus.property(LOGIND, AUTO, "org.freedesktop.login1.Session", "Id");
            id.text().to_owned()
        });
        if id.is_empty() {
            return String::new();
        }

        let path = bus.call(
            LOGIND,
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
            "GetSession",
            &[Argument::from(id)],
        );
        path.text().to_owned()
    })
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
    if session().is_empty() {
        eprintln!("kanade: logind names no session, so a crash while locked is not locked again");
    }

    if locked(session()) == Some(true) {
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

// where a request stands by `status`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settled {
    // a lock screen drew after it, with no unlock pending
    Locked,
    Waiting,

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
 * drops that unlock: the lock wins, and the password has to be typed again. A password still
 * being checked cannot be dropped, so then nothing is asked. Amane past `6ace43e` may end a lock
 * differently; recheck this on a bump
 */
pub fn start() -> Result<Started, String> {
    if pam().is_none() {
        return Err(format!(
            "no PAM service {PAM} in {}, so no password would unlock",
            PAM_DIRS.join(", ")
        ));
    }

    let lock = Lock::read();
    let (checking, failed) = (lock.checking(), lock.failed());
    drop(lock);

    if checking {
        return Ok(Started::Unknown(String::from(
            "a password typed on the lock screen is being checked, so whether the session stays \
             locked is not known; nothing was asked, try again",
        )));
    }

    // a wrong password is no unlock, and its "Wrong password" stays
    if !failed {
        *Lock::write() = Lock::new();
    }

    let newest = requests().request();

    // redraws every window, so a lock screen already shown confirms it
    Lock::start();
    Ok(Started::Requested(format!("requested #{newest}")))
}

// the request `start` names
pub fn requested(text: &str) -> Option<u64> {
    text.strip_prefix("requested #")?.parse().ok()
}

// `kanade lock status`: the newest request and the newest confirmed one, a line each
pub fn status() -> String {
    let requests = requests();
    format!(
        "requested #{}\nconfirmed #{}",
        requests.newest, requests.confirmed
    )
}

// where request `number` stands by `status`, if it parses
pub fn settled(status: &str, number: u64) -> Option<Settled> {
    let serial = |line: Option<&str>, word: &str| {
        line?
            .strip_prefix(word)?
            .strip_prefix(" #")?
            .parse::<u64>()
            .ok()
    };
    let mut lines = status.lines();
    let newest = serial(lines.next(), "requested")?;
    let confirmed = serial(lines.next(), "confirmed")?;

    // a later request confirmed is a lock after this one too
    Some(if confirmed >= number {
        Settled::Locked
    } else if newest >= number {
        Settled::Waiting
    } else {
        Settled::Lost
    })
}

fn requests() -> MutexGuard<'static, Requests> {
    REQUESTS.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn view(monitor: &Monitor) -> LayerWindow {
    let roles = theme::ISLAND;
    let lock = Lock::read();

    requests().drew(lock.checking(), lock.failed());

    let (status, color) = if lock.checking() {
        ("Checking…", roles.on_surface_variant)
    } else if lock.failed() {
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
 * one try at a time: while PAM checks, what is typed waits in the field. The field empties before
 * PAM gets the password, so a wrong one is typed again from nothing
 */
fn submit(password: String) {
    if password.is_empty() || Lock::read().checking() {
        return;
    }

    TextInput::set_text(FIELD, "");
    requests().tried = true;
    Lock::unlock(&password);
}

#[cfg(test)]
mod tests {
    use super::*;

    // #196: the last lock, on its way out after an unlock, draws once more
    #[test]
    fn only_a_draw_with_no_unlock_pending_confirms() {
        let mut requests = Requests {
            newest: 0,
            confirmed: 0,
            tried: false,
        };

        assert_eq!(requests.request(), 1);
        requests.drew(false, false);
        assert_eq!(requests.confirmed, 1);

        // asked again while locked, then the password is typed before the redraw
        assert_eq!(requests.request(), 2);
        requests.tried = true;
        requests.drew(true, false);
        assert_eq!(requests.confirmed, 1);
        // accepted, the lock not ended yet
        requests.drew(false, false);
        assert_eq!(requests.confirmed, 1);

        // a wrong password keeps the lock
        requests.drew(false, true);
        assert_eq!(requests.confirmed, 2);

        // asked again after an accepted one, which `start` dropped, so the lock stays
        assert_eq!(requests.request(), 3);
        requests.drew(false, false);
        assert_eq!(requests.confirmed, 3);
    }

    #[test]
    fn a_request_settles_once_confirmed_and_is_lost_to_a_restart() {
        assert_eq!(requested("requested #3"), Some(3));
        assert_eq!(requested("locked"), None);

        assert_eq!(
            settled("requested #3\nconfirmed #2", 3),
            Some(Settled::Waiting)
        );
        assert_eq!(
            settled("requested #3\nconfirmed #3", 3),
            Some(Settled::Locked)
        );
        assert_eq!(
            settled("requested #4\nconfirmed #4", 3),
            Some(Settled::Locked)
        );
        assert_eq!(
            settled("requested #1\nconfirmed #0", 3),
            Some(Settled::Lost)
        );
        assert_eq!(settled("locked", 3), None);
    }
}
