//! The password typed, checked with PAM off the lock screen's thread.

use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::thread;

use kanade_runtime::service::Service;
use zeroize::Zeroizing;

use crate::clock;

use super::who::Who;
use super::{HEARD, awake, locker, requests, say};

// the PAM service the password is checked with
pub const PAM: &str = "login";

// where Linux-PAM looks for a service's file, the admin's first
pub(super) const PAM_DIRS: [&str; 3] = ["/etc/pam.d", "/usr/lib/pam.d", "/usr/etc/pam.d"];

// the file PAM reads for `PAM`; without it no password unlocks
pub fn pam() -> Option<PathBuf> {
    PAM_DIRS
        .iter()
        .map(|dir| Path::new(dir).join(PAM))
        .find(|path| path.is_file())
}

/*
 * Enter pressed on the lock screen, on its thread: whether the password goes to PAM, which empties
 * the field. One try at a time: while PAM checks, or the machine is on its way to sleep, what is
 * typed waits in the field. Sleep drops a check, so one PAM ends after the machine began to sleep
 * unlocks nothing
 */
pub(super) fn submit(password: String) -> bool {
    // zeroed as it drops, however this ends
    let password = Zeroizing::new(password);
    if password.is_empty() {
        return false;
    }

    let mut requests = requests();
    let Some(check) = requests.check() else {
        if requests.sleeping {
            awake(&mut requests);
        }
        say(&requests);
        return false;
    };
    say(&requests);

    let checking = thread::Builder::new()
        .name(String::from("lock check"))
        .spawn(move || checked(check, password));

    if let Err(error) = checking {
        eprintln!("kanade: cannot check the password: {error}");
        requests.checking = false;
        requests.pam = false;
        say(&requests);
        // kept in the field, to send again
        return false;
    }

    true
}

// runs the check `check`, and asks a lock it accepted to end, unless sleep dropped it
pub(super) fn checked(check: u64, password: Zeroizing<String>) {
    // the account shown, by the uid, not whatever the environment names
    let accepted = panic::catch_unwind(AssertUnwindSafe(|| {
        let login = Who::read().login.clone();
        clock::authenticate(PAM, &login, &password)
    }))
    // a check that panicked accepts nothing, and ends, so the next can begin
    .unwrap_or(false);
    drop(password);

    // under the requests, so a sleep shutting the gate or a request falls before or after
    let mut requests = requests();
    if !requests.checked(check, accepted) {
        // what was typed meanwhile can go to PAM now
        say(&requests);
        return;
    }

    if accepted && let Ok(locker) = locker() {
        locker.unlock();
    }
    say(&requests);
    drop(requests);
    HEARD.notify_all();
}
