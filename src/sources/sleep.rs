//! Lock before sleep (#155, docs/design.md Idle/suspend): logind says `PrepareForSleep(true)` as the
//! machine is about to suspend or hibernate, and waits on delay inhibitors, up to its
//! `InhibitDelayMaxUSec`, before it sleeps. While awake Kanade holds one, asks for a lock as sleep
//! comes, and lets go once a lock screen confirms it (`lock::hold`), so the session wakes locked
//! and shows nothing of it on the way. Until it wakes, `PrepareForSleep(false)`, no password
//! unlocks (`lock::sleeping`); then it takes one again.
//!
//! Amane's `Bus` cannot hold the fd logind's `Inhibit` hands back, so `systemd-inhibit` holds it
//! (ADR 0011), through setpriv so it dies with Kanade, as caffeine's does. A holder that ends or
//! cannot start is taken again with a backoff, and `kanade status` says how it stands. Caffeine is
//! about idle, not sleep: an asked-for suspend locks with it on too.
//!
//! An idle lock is the idle daemon's, like hypridle running `kanade lock`, which honors caffeine's
//! idle inhibitor.

use std::io;
use std::process::{Child, ChildStdout};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

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

// the wait before taking an inhibitor again, doubled each time it fails up to `LONGEST`
const FIRST: Duration = Duration::from_secs(1);
const LONGEST: Duration = Duration::from_secs(300);

// a holder that lasted this long was no flapping one, so the next is tried again soon
const LASTED: Duration = Duration::from_secs(60);

// what the sleep thread hears, from logind and from its holders
enum Event {
    Sleep(bool),

    // the holder numbered so ended
    Ended(u64),

    // the system bus closed
    Gone,
}

// where the inhibitor stands, for `kanade status`
enum Health {
    Starting,
    Held,

    // let go of, as the machine sleeps
    Sleeping,

    // none, why, and when it is tried again
    Missing(String, Instant),

    // the system bus closed: no sleep is heard of again
    Gone,
}

static HEALTH: Mutex<Health> = Mutex::new(Health::Starting);

// a delay inhibitor, held until dropped; a panic that drops it lets go of it too
struct Delay {
    child: Child,
    serial: u64,
    since: Instant,
}

impl Drop for Delay {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

// when to take an inhibitor again: soon at first, then each failure waits twice as long
struct Retry {
    wait: Duration,

    // None while one is held, or while the machine sleeps
    at: Option<Instant>,
}

impl Retry {
    fn now(now: Instant) -> Self {
        Self {
            wait: FIRST,
            at: Some(now),
        }
    }

    fn due(&self, now: Instant) -> bool {
        self.at.is_some_and(|at| at <= now)
    }

    fn held(&mut self) {
        self.at = None;
    }

    // taking one failed at `now`: when to try again
    fn failed(&mut self, now: Instant) -> Instant {
        let at = now + self.wait;
        self.at = Some(at);
        self.wait = (self.wait * 2).min(LONGEST);
        at
    }

    // the one held for `lasted` ended at `now`; one that held long was no flapping one
    fn ended(&mut self, lasted: Duration, now: Instant) -> Instant {
        if lasted >= LASTED {
            self.wait = FIRST;
        }
        self.failed(now)
    }
}

pub fn spawn() {
    supervise::spawn("sleep", follow);
}

// for `kanade status`
pub fn status() -> String {
    let line = match &*HEALTH.lock().unwrap_or_else(PoisonError::into_inner) {
        Health::Starting => String::from("taking a delay inhibitor"),
        Health::Held => String::from("delay inhibitor held"),
        Health::Sleeping => String::from("delay inhibitor let go of, for sleep"),
        Health::Missing(why, again) => format!(
            "no delay inhibitor, so sleep may come before the lock: {why}; trying again in {}s",
            again.saturating_duration_since(Instant::now()).as_secs()
        ),
        Health::Gone => {
            String::from("the system bus closed, so the session no longer locks before sleep")
        }
    };

    format!("sleep: {line}")
}

fn health(health: Health) {
    *HEALTH.lock().unwrap_or_else(PoisonError::into_inner) = health;
}

// runs on its own thread for as long as the system bus does: the holder dies with the thread
fn follow() {
    let bus = Bus::system();
    let (tell, events) = mpsc::channel();

    // subscribed before an inhibitor is taken, so no sleep falls between them
    let signals = bus.signals(MANAGER, "PrepareForSleep");

    // a sleep begun before this thread was, as after a restart; before any signal is heard, so a
    // wake that follows is not undone
    if lock::preparing() == Some(true) {
        lock::sleeping();
        let _ = tell.send(Event::Sleep(true));
    }

    let told = tell.clone();
    let heard = thread::Builder::new()
        .name(String::from("sleep signals"))
        .spawn(move || {
            for signal in signals {
                if signal.path() != PATH
                    || bus::unique(bus, LOGIND).as_deref() != Some(signal.sender())
                {
                    continue;
                }

                let Some(&Value::Bool(sleeping)) = signal.arguments().first() else {
                    continue;
                };

                // here, not after an inhibitor is taken or `InhibitDelayMaxUSec` asked
                if sleeping {
                    lock::sleeping();
                } else {
                    lock::woke();
                }

                if told.send(Event::Sleep(sleeping)).is_err() {
                    return;
                }
            }

            let _ = told.send(Event::Gone);
        });

    if let Err(error) = heard {
        health(Health::Gone);
        eprintln!("kanade: cannot follow sleep: {error}");
        return;
    }

    let mut delay: Option<Delay> = None;
    let mut asleep = false;
    let mut serial = 0;
    let mut retry = Retry::now(Instant::now());
    let mut said = String::new();

    loop {
        if !asleep && retry.due(Instant::now()) {
            serial += 1;

            match take(serial, &tell) {
                Ok(held) => {
                    delay = Some(held);
                    retry.held();
                    said.clear();
                    health(Health::Held);
                }
                Err(why) => {
                    // said once, not at every try
                    if why != said {
                        eprintln!(
                            "kanade: no delay inhibitor, so sleep may come before the lock: {why}"
                        );
                    }

                    let at = retry.failed(Instant::now());
                    health(Health::Missing(why.clone(), at));
                    said = why;
                }
            }
        }

        let event = match retry.at.filter(|_| !asleep) {
            Some(at) => match events.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => Event::Gone,
            },
            None => events.recv().unwrap_or(Event::Gone),
        };

        match event {
            // heard twice, as when it began before this thread did, it locks once more
            Event::Sleep(true) => {
                asleep = true;

                if delay.is_none() {
                    eprintln!("kanade: no delay inhibitor, so sleep may come before the lock");
                }

                match lock::hold(patience(bus)) {
                    Ok(()) => eprintln!("kanade: locked before sleep"),
                    Err(why) => eprintln!("kanade: did not lock before sleep: {why}"),
                }

                delay = None;
                health(Health::Sleeping);
            }
            // also said when a sleep failed, or never began here
            Event::Sleep(false) => {
                asleep = false;

                if delay.is_none() {
                    retry = Retry::now(Instant::now());
                }
            }
            Event::Ended(ended) => {
                let Some(held) = delay.take_if(|held| held.serial == ended) else {
                    continue;
                };

                let lasted = held.since.elapsed();
                drop(held);

                let now = Instant::now();
                let at = retry.ended(lasted, now);
                let why = format!("{INHIBIT} ended");
                eprintln!(
                    "kanade: {why}, taking the delay inhibitor again in {:?}",
                    at - now
                );
                health(Health::Missing(why.clone(), at));
                said = why;
            }
            // no wake is heard of again, nor can logind be asked: the gate opens
            Event::Gone => {
                lock::woke();
                health(Health::Gone);
                eprintln!(
                    "kanade: the system bus went away, so the session no longer locks before sleep"
                );
                return;
            }
        }
    }
}

// a delay inhibitor on sleep, numbered `serial`, held once logind gave it; `tell` hears it end
fn take(serial: u64, tell: &Sender<Event>) -> Result<Delay, String> {
    // setpriv says a missing program only once running; without it the inhibitor could outlive
    // Kanade and delay every sleep for good
    for program in [INHIBIT, SETPRIV] {
        if !wake::found(program) {
            return Err(format!("{program} not found"));
        }
    }

    let arguments = arguments();
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();

    let mut child = wake::hold_lock(INHIBIT, &arguments)
        .map_err(|error| format!("cannot run {INHIBIT}: {error}"))?;

    // logind answers in milliseconds; a holder silent past `READY` is given up on, so a wedged
    // logind never stalls this thread past the signals that follow
    let (ready, held) = mpsc::channel();
    let out = child.stdout.take();
    let tell = tell.clone();
    let read = thread::Builder::new()
        .name(String::from("sleep inhibit"))
        .spawn(move || watch(serial, out, &ready, &tell));

    let since = Instant::now();
    let delay = Delay {
        child,
        serial,
        since,
    };

    if let Err(error) = read {
        return Err(format!("cannot follow {INHIBIT}: {error}"));
    }

    match held.recv_timeout(READY) {
        Ok(true) => Ok(delay),
        Ok(false) => {
            let mut delay = delay;
            let said = caffeine::last_lines(delay.child.stderr.take());

            Err(caffeine::ended(delay.child.wait(), said)
                .err()
                .unwrap_or_else(|| format!("{INHIBIT} ended before it held the inhibitor")))
        }
        Err(_) => Err(format!("{INHIBIT} did not hold it within {READY:?}")),
    }
}

/*
 * says on `ready` whether the holder `serial` holds, then on `tell` once it ended: its stdout
 * closes only as the holder and its command do
 */
fn watch(serial: u64, out: Option<ChildStdout>, ready: &Sender<bool>, tell: &Sender<Event>) {
    let Some(mut out) = out else {
        let _ = ready.send(false);
        return;
    };

    // not heard once given up on
    let holds = caffeine::holds(Some(&mut out));
    let _ = ready.send(holds);

    if holds {
        drop(io::copy(&mut out, &mut io::sink()));
        let _ = tell.send(Event::Ended(serial));
    }
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

    // P2 of #155: a lost inhibitor is taken again, soon, then backing off, never spinning
    #[test]
    fn an_inhibitor_is_taken_again_with_a_backoff() {
        let start = Instant::now();
        let mut retry = Retry::now(start);
        assert!(retry.due(start));

        let waits: Vec<_> = (0..10)
            .map(|_| retry.failed(start) - start)
            .map(|wait| wait.as_secs())
            .collect();
        assert_eq!(waits, [1, 2, 4, 8, 16, 32, 64, 128, 256, 300]);
        assert!(!retry.due(start));
        assert!(retry.due(start + LONGEST));

        retry.held();
        assert!(!retry.due(start + LONGEST));

        // a holder that flaps keeps backing off; one that held long is taken again soon
        assert_eq!(retry.ended(Duration::from_secs(1), start) - start, LONGEST);
        assert_eq!(retry.ended(LASTED, start) - start, FIRST);
        assert_eq!(retry.ended(Duration::ZERO, start) - start, FIRST * 2);
    }
}
