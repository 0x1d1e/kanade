//! Who is signed in: the login, the name shown and their picture, looked up off the view.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use kanade_runtime::service::Service;

use crate::sources::{wake, wallpaper};

/*
 * who is signed in, as the lock screen shows them: their name and picture, looked up as a lock is
 * asked, off the view, as a name may come from a directory over the network; the login until then
 */
#[derive(Debug, Clone, PartialEq)]
pub struct Who {
    // the account, which the password is checked for
    pub(super) login: String,
    pub(super) name: String,

    // their picture (`face`)
    pub(super) face: Option<PathBuf>,
}

impl Service for Who {
    fn new() -> Self {
        let login = login();

        Who {
            name: login.clone(),
            login,
            face: None,
        }
    }

    fn listen() {}
}

// how long the name may take to look up, as from LDAP
const LOOKUP: Duration = Duration::from_secs(2);

/*
 * what the lock screens show besides the clock, looked up at start and again as a lock is asked;
 * the wallpaper even under `lock.backdrop` solid, so a backdrop changed while locked shows it
 */
pub(super) fn look_up() {
    // which look-up is newest, and which wrote last: a slow one never overwrites what a later one found
    static LOOKS: AtomicU64 = AtomicU64::new(0);
    static WROTE: Mutex<u64> = Mutex::new(0);

    wallpaper::look();

    let this = LOOKS.fetch_add(1, Ordering::Relaxed) + 1;

    let spawned = thread::Builder::new()
        .name(String::from("lock who"))
        .spawn(move || {
            let who = who();

            // held while written, so look-ups write in turn
            let mut wrote = WROTE.lock().unwrap_or_else(PoisonError::into_inner);

            // one older than one written is dropped, so a later one is never undone
            if *wrote >= this {
                return;
            }
            *wrote = this;

            // read first, as a write redraws the lock screens
            if *Who::read() != who {
                *Who::write() = who;
            }
        });

    if let Err(error) = spawned {
        eprintln!("kanade: cannot look up who is signed in: {error}");
    }
}

pub(super) fn who() -> Who {
    // by the uid, through NSS, so a user from LDAP, sssd or systemd-homed is found as one in /etc/passwd
    let entry = uid().and_then(|uid| wake::query_within("getent", &["passwd", &uid], LOOKUP).ok());
    let login = entry
        .as_deref()
        .and_then(|entry| entry.split(':').next())
        .filter(|login| !login.is_empty())
        .map_or_else(login, str::to_owned);
    let name = entry
        .as_deref()
        .and_then(named)
        .unwrap_or_else(|| login.clone());

    Who {
        name,
        face: face(&login),
        login,
    }
}

// the login, as the environment names it; else the uid, so the initial is never empty
pub(super) fn login() -> String {
    ["USER", "LOGNAME"]
        .into_iter()
        .find_map(|name| env::var(name).ok().filter(|login| !login.is_empty()))
        .or_else(uid)
        .unwrap_or_else(|| String::from("?"))
}

// the real uid, as `/proc` gives it
pub(super) fn uid() -> Option<String> {
    let status = fs::read_to_string("/proc/self/status").ok()?;

    status.lines().find_map(|line| {
        line.strip_prefix("Uid:")?
            .split_whitespace()
            .next()
            .map(str::to_owned)
    })
}

/*
 * the picture display managers show: the first image `wallpaper` decodes of `~/.face`, `~/.face.icon` and the
 * one AccountsService keeps for `login`, where a link to it leads
 */
pub(super) fn face(login: &str) -> Option<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let mine = [".face", ".face.icon"]
        .into_iter()
        .filter_map(|name| Some(home.as_ref()?.join(name)));
    let kept = Path::new("/var/lib/AccountsService/icons").join(login);

    mine.chain([kept])
        .filter_map(|path| fs::canonicalize(path).ok())
        .find(|path| wallpaper::format(path).is_some())
}

/*
 * the name in a passwd entry: its comment's full name, before the first comma, with `&` standing
 * for the login capitalized; else the login
 */
pub(super) fn named(entry: &str) -> Option<String> {
    let fields: Vec<&str> = entry.lines().next()?.split(':').collect();
    let login = fields.first().filter(|login| !login.is_empty())?;
    let full = fields
        .get(4)
        .and_then(|comment| comment.split(',').next())
        .map_or("", str::trim);

    if full.is_empty() {
        return Some((*login).to_owned());
    }

    let mut letters = login.chars();
    let capitalized: String = letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default();

    Some(full.replace('&', &capitalized))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_is_the_passwd_full_name_or_the_login() {
        assert_eq!(
            named("kim:x:1000:1000:Kim Lee,,,:/home/kim:/bin/fish\n").as_deref(),
            Some("Kim Lee")
        );
        assert_eq!(
            named("kim:x:1000:1000:& the cat:/home/kim:/bin/sh").as_deref(),
            Some("Kim the cat")
        );
        assert_eq!(
            named("kim:x:1000:1000::/home/kim:/bin/sh").as_deref(),
            Some("kim")
        );
        assert_eq!(named(""), None);
    }
}
