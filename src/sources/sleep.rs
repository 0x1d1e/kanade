//! Lock before sleep (#155, docs/design.md Idle/suspend): logind says `PrepareForSleep(true)` as the
//! machine is about to suspend or hibernate, and waits on delay inhibitors, up to its
//! `InhibitDelayMaxUSec`, before it sleeps. While awake Kanade holds one, asks for a lock as sleep
//! comes, and lets go once a lock screen confirms it (`lock::hold`), so the session wakes locked
//! and shows nothing of it on the way. Waking, `PrepareForSleep(false)`, it takes one again.
//!
//! Amane's `Bus` cannot hold the fd logind's `Inhibit` hands back, so `systemd-inhibit` holds it
//! (ADR 0011), through setpriv so it dies with Kanade, as caffeine's does. Caffeine is about idle,
//! not sleep: an asked-for suspend locks with it on too.
//!
//! An idle lock is the idle daemon's, like hypridle running `kanade lock`, which honors caffeine's
//! idle inhibitor.

use std::process::Child;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use amane::{Bus, Value};

use super::bus;
use super::caffeine::{self, HELD, INHIBIT, READY};
use super::wake::{self, SETPRIV};
use crate::{lock, supervise};

const LOGIND: &str = "org.freedesktop.login1";
const PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";

// logind's own default for `InhibitDelayMaxUSec`, for when it does not say
const DELAY: Duration = Duration::from_secs(5);

// a delay inhibitor, held until dropped; a panic that drops it lets go of it too
struct Delay(Child);

impl Delay {
    // whether the holder still runs, so still holds the inhibitor
    fn holds(&mut self) -> bool {
        matches!(self.0.try_wait(), Ok(None))
    }
}

impl Drop for Delay {
    fn drop(&mut self) {
        drop(self.0.kill());
        drop(self.0.wait());
    }
}

pub fn spawn() {
    supervise::spawn("sleep", follow);
}

// runs on its own thread for as long as the system bus does: the holder dies with the thread
fn follow() {
    let bus = Bus::system();

    // subscribed before the inhibitor is taken, so no sleep falls between them
    let signals = bus.signals(MANAGER, "PrepareForSleep");
    let mut delay = take();

    for signal in signals {
        if signal.path() != PATH || bus::unique(bus, LOGIND).as_deref() != Some(signal.sender()) {
            continue;
        }

        match signal.arguments().first() {
            Some(Value::Bool(true)) => {
                if !delay.as_mut().is_some_and(Delay::holds) {
                    eprintln!("kanade: no delay inhibitor, so sleep may come before the lock");
                }

                match lock::hold(patience(bus)) {
                    Ok(()) => eprintln!("kanade: locked before sleep"),
                    Err(why) => eprintln!("kanade: did not lock before sleep: {why}"),
                }

                delay = None;
            }
            Some(Value::Bool(false)) if delay.is_none() => delay = take(),
            _ => {}
        }
    }

    eprintln!("kanade: the system bus went away, so the session no longer locks before sleep");
}

// a delay inhibitor on sleep, held once logind gave it, else none and why is logged
fn take() -> Option<Delay> {
    // setpriv says a missing program only once running; without it the inhibitor could outlive
    // Kanade and delay every sleep for good
    for program in [INHIBIT, SETPRIV] {
        if !wake::found(program) {
            eprintln!("kanade: {program} not found, so sleep may come before the lock");
            return None;
        }
    }

    let arguments = arguments();
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();

    let mut child = wake::hold_lock(INHIBIT, &arguments)
        .inspect_err(|error| eprintln!("kanade: cannot run {INHIBIT}: {error}"))
        .ok()?;

    // logind answers in milliseconds; a holder silent past `READY` is given up on, so a wedged
    // logind never stalls this thread past the signals that follow
    let (told, heard) = mpsc::channel();
    let held = child.stdout.take();
    let read = thread::Builder::new()
        .name(String::from("sleep inhibit"))
        .spawn(move || {
            // heard no more once given up on
            let _ = told.send(caffeine::holds(held));
        });

    if let Err(error) = read {
        drop(Delay(child));
        eprintln!("kanade: cannot follow {INHIBIT}: {error}");
        return None;
    }

    match heard.recv_timeout(READY) {
        Ok(true) => return Some(Delay(child)),
        Ok(false) => {}
        Err(_) => {
            drop(Delay(child));
            eprintln!(
                "kanade: {INHIBIT} did not hold the delay inhibitor within {READY:?}, so sleep may \
                 come before the lock"
            );
            return None;
        }
    }

    let said = caffeine::last_lines(child.stderr.take());
    let why = caffeine::ended(child.wait(), said)
        .err()
        .unwrap_or_else(|| format!("{INHIBIT} ended before it held the inhibitor"));

    eprintln!("kanade: no delay inhibitor, so sleep may come before the lock: {why}");
    None
}

// a delay inhibitor on sleep, held by its command, which says `HELD`, then sleeps for good
fn arguments() -> Vec<String> {
    [
        "--what=sleep",
        "--who=Kanade",
        "--why=Locking the session before sleep",
        "--mode=delay",
        "sh",
        "-c",
        &format!("echo {HELD}; exec sleep infinity"),
    ]
    .map(String::from)
    .to_vec()
}

// how long logind waits on a delay inhibitor before it sleeps anyway
fn patience(bus: Bus) -> Duration {
    match bus.property(LOGIND, PATH, MANAGER, "InhibitDelayMaxUSec") {
        Value::Number(micros) if micros > 0.0 => Duration::from_micros(micros as u64),
        _ => DELAY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inhibitor_delays_sleep_for_good() {
        assert_eq!(
            arguments().join(" "),
            "--what=sleep --who=Kanade --why=Locking the session before sleep --mode=delay \
             sh -c echo held; exec sleep infinity"
        );
    }
}
