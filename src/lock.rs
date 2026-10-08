//! The lock screen (#154, ADR 0018): Amane's `Lock`, ext-session-lock with the password checked by
//! PAM, in the shell process. Every monitor shows the time, the date and a password field, and
//! nothing else: no notification, no Activity. The password goes from the field to PAM and nowhere
//! else; the field is emptied as it is sent, and it is never logged or drawn.
//!
//! A crash while locked leaves niri locked on its red screen; the user unit (`UNIT`) restarts the
//! shell, which finds logind's `LockedHint` true and locks again, so niri swaps the dead lock for
//! this one.

use std::path::{Path, PathBuf};

use amane::{
    Bus, Center, Column, Key, LayerWindow, Lock, Monitor, Padding, Parent, Rectangle, Service,
    Start, Text, TextInput, children,
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
const SESSION: &str = "/org/freedesktop/login1/session/auto";

const FIELD: &str = "lock-password";

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
 * at start, before the shell runs: a session logind still counts as locked was locked by a shell
 * that died, so this one locks it again. niri refuses while another locker holds it, which Amane
 * leaves alone. Without `PAM` it locks all the same, as niri is locked anyway; only ending the
 * session gets out
 */
pub fn relock() {
    let locked = Bus::system()
        .property(
            LOGIND,
            SESSION,
            "org.freedesktop.login1.Session",
            "LockedHint",
        )
        .bool();

    if locked {
        eprintln!("kanade: the session is locked, locking it again");
        if pam().is_none() {
            eprintln!("kanade: no PAM service {PAM}, so no password will unlock");
        }
        Lock::start();
    }
}

// `kanade lock`; while locked it does nothing, so a second one keeps what is being typed
pub fn start() -> Result<(), String> {
    if pam().is_none() {
        return Err(format!(
            "no PAM service {PAM} in {}, so no password would unlock",
            PAM_DIRS.join(", ")
        ));
    }

    Lock::start();
    Ok(())
}

pub fn view(monitor: &Monitor) -> LayerWindow {
    let roles = theme::ISLAND;
    let lock = Lock::read();

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
    Lock::unlock(&password);
}
