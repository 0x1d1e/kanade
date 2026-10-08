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
//! Over Kanade's own zbus connection, not Amane's `Bus`, which never connects again once the
//! system bus is lost. A lost bus is connected to again with a backoff; each connection asks logind
//! whether it is on its way to sleep, as a signal may have been missed in between. Only logind
//! saying it is not opens the gate: a bus lost, or a logind that does not answer, keeps it as it
//! was.
//!
//! An idle lock is the idle daemon's, like hypridle running `kanade lock`, which honors caffeine's
//! idle inhibitor.

use std::convert::Infallible;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::process::{Child, ChildStdout};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use zbus::MatchRule;
use zbus::blocking::fdo::DBusProxy;
use zbus::blocking::{Connection, MessageIterator, connection};
use zbus::names::BusName;
use zbus::zvariant::OwnedValue;

use super::caffeine::{self, HELD, INHIBIT, READY};
use super::wake::{self, SETPRIV};
use crate::{lock, supervise};

const LOGIND: &str = "org.freedesktop.login1";
const PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

// how long logind may take to answer, so a wedged one never stalls a run
const TIMEOUT: Duration = Duration::from_secs(2);

// logind's own default for `InhibitDelayMaxUSec`, for when it does not say
const DELAY: Duration = Duration::from_secs(5);

// the wait before taking an inhibitor or connecting again, doubled at each failure up to `LONGEST`
const FIRST: Duration = Duration::from_secs(1);
const LONGEST: Duration = Duration::from_secs(300);

// a holder or connection that lasted this long was no flapping one, so the next is tried again soon
const LASTED: Duration = Duration::from_secs(60);

// what the sleep thread hears, from logind and from its holders
enum Event {
    Sleep(bool),

    // the holder numbered so ended
    Ended(u64),

    // the system bus was lost, why
    Gone(String),
}

// where the inhibitor stands, for `kanade status`
enum Health {
    Starting,
    Held,

    // let go of, as the machine sleeps
    Sleeping,

    // none, why, and when it is tried again
    Missing(String, Instant),

    // the system bus was lost, why, and when it is connected to again
    Lost(String, Instant),
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
        Health::Lost(why, again) => format!(
            "no system bus, so sleep may come before the lock: {why}; connecting again in {}s",
            again.saturating_duration_since(Instant::now()).as_secs()
        ),
    };

    format!("sleep: {line}")
}

fn health(health: Health) {
    *HEALTH.lock().unwrap_or_else(PoisonError::into_inner) = health;
}

/*
 * runs for good on its own thread, connecting again after the system bus is lost. The gate stays
 * as the run left it: shut while the machine may be on its way to sleep, until a run hears logind
 * say it is not
 */
fn follow() {
    let mut again = Retry::now(Instant::now());

    loop {
        let started = Instant::now();
        let Err(why) = run();

        let now = Instant::now();
        let at = again.ended(started.elapsed(), now);
        eprintln!(
            "kanade: no system bus, so sleep may come before the lock: {why}; connecting again in \
             {:?}",
            at - now
        );
        health(Health::Lost(why, at));

        thread::sleep(at - now);
    }
}

// follows logind until the system bus is lost; the holder is let go of with it
fn run() -> Result<Infallible, String> {
    let connection = connect().map_err(|error| format!("cannot connect: {error}"))?;
    health(Health::Starting);
    let (tell, events) = mpsc::channel();

    // subscribed before logind is asked or an inhibitor taken, so no sleep falls between them
    let rule =
        format!("type='signal',path='{PATH}',interface='{MANAGER}',member='PrepareForSleep'");
    let rule = MatchRule::try_from(rule.as_str()).map_err(|error| error.to_string())?;
    let signals = MessageIterator::for_match_rule(rule, &connection, None)
        .map_err(|error| format!("cannot follow logind: {error}"))?;

    /*
     * a sleep begun or ended while no run was heard, as after a restart or a lost bus; before any
     * signal is, so one after it is not undone. Not knowing is no wake
     */
    match preparing_on(&connection) {
        Some(true) => {
            lock::sleeping();
            let _ = tell.send(Event::Sleep(true));
        }
        Some(false) => lock::woke(),
        None => eprintln!("kanade: logind does not say whether the machine is on its way to sleep"),
    }

    let patience = patience(&connection);

    let told = tell.clone();
    thread::Builder::new()
        .name(String::from("sleep signals"))
        .spawn(move || {
            // a panic loses logind as a closed bus would, so the run ends rather than hangs
            let why =
                panic::catch_unwind(AssertUnwindSafe(|| forward(signals, &connection, &told)))
                    .unwrap_or_else(|_| String::from("following logind panicked"));
            let _ = told.send(Event::Gone(why));
        })
        .map_err(|error| format!("cannot follow sleep: {error}"))?;

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

        // never disconnected: `tell` is kept
        let event = match retry.at.filter(|_| !asleep) {
            Some(at) => match events.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => continue,
            },
            None => match events.recv() {
                Ok(event) => event,
                Err(_) => continue,
            },
        };

        match event {
            // heard twice, as when it began before this run did, it locks once more
            Event::Sleep(true) => {
                asleep = true;

                if delay.is_none() {
                    eprintln!("kanade: no delay inhibitor, so sleep may come before the lock");
                }

                match lock::hold(patience) {
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
            // the gate stays as it is: only logind saying the machine woke opens it
            Event::Gone(why) => return Err(why),
        }
    }
}

/*
 * shuts and opens the gate as logind says, then tells the run, until the bus is lost: why it was.
 * Only logind's own signals count, as anyone may send one
 */
fn forward(signals: MessageIterator, connection: &Connection, tell: &Sender<Event>) -> String {
    let mut logind = None;
    let mut last = None;

    for signal in signals {
        let signal = match signal {
            Ok(signal) => signal,
            Err(error) => {
                last = Some(error.to_string());
                continue;
            }
        };

        let Some(sender) = signal.header().sender().map(|sender| sender.to_string()) else {
            continue;
        };

        // asked again on a stranger, as logind may have restarted
        if logind.as_ref() != Some(&sender) {
            logind = owner(connection);
            if logind.as_ref() != Some(&sender) {
                continue;
            }
        }

        let Ok(sleeping) = signal.body().deserialize::<bool>() else {
            continue;
        };

        // here, not after an inhibitor is taken
        if sleeping {
            lock::sleeping();
        } else {
            lock::woke();
        }

        if tell.send(Event::Sleep(sleeping)).is_err() {
            break;
        }
    }

    last.unwrap_or_else(|| String::from("the system bus closed"))
}

fn connect() -> zbus::Result<Connection> {
    connection::Builder::system()?
        .method_timeout(TIMEOUT)
        .build()
}

// logind's unique name, which its signals name as their sender
fn owner(connection: &Connection) -> Option<String> {
    let name = BusName::try_from(LOGIND).ok()?;

    let owner = DBusProxy::new(connection).ok()?.get_name_owner(name).ok()?;

    Some(owner.to_string())
}

/*
 * whether logind is on its way to sleep, asked on a connection of its own, as for a password typed
 * while the gate is shut; None when it does not answer
 */
pub fn preparing() -> Option<bool> {
    preparing_on(&connect().ok()?)
}

fn preparing_on(connection: &Connection) -> Option<bool> {
    property(connection, "PreparingForSleep")?.try_into().ok()
}

fn property(connection: &Connection, name: &str) -> Option<OwnedValue> {
    connection
        .call_method(
            Some(LOGIND),
            PATH,
            Some(PROPERTIES),
            "Get",
            &(MANAGER, name),
        )
        .and_then(|reply| reply.body().deserialize::<OwnedValue>())
        .ok()
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

// how long logind waits on a delay inhibitor before it sleeps anyway, asked once a run
fn patience(connection: &Connection) -> Duration {
    property(connection, "InhibitDelayMaxUSec")
        .and_then(|micros| u64::try_from(micros).ok())
        .filter(|&micros| micros > 0)
        .map_or(DELAY, Duration::from_micros)
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
