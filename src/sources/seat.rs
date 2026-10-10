//! Whether this session has the seat (ADR 0021). Keyboards and radios are heard whichever session
//! has it, so the OSD shows nothing while another one does, as after a switch to another VT.
//!
//! logind says so in its session's `Active`, followed over a zbus connection of Kanade's own whose
//! match rule names the session, so nothing else on the system bus wakes it. Until logind says, and
//! while it cannot be followed, the session is taken to have the seat, as an OSD lost to a wrong
//! guess is worse than one shown after a VT switch. A logind not yet naming the session, or a bus
//! lost, is tried again, sooner after one that held long.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use zbus::MatchRule;
use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::OwnedValue;

use crate::lock;

const LOGIND: &str = "org.freedesktop.login1";
const SESSION: &str = "org.freedesktop.login1.Session";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

// the wait before trying again, doubled at each failure up to `LONGEST`
const FIRST: Duration = Duration::from_secs(1);
const LONGEST: Duration = Duration::from_secs(300);

// a run that lasted this long was no flapping one, so the next is tried soon
const LASTED: Duration = Duration::from_secs(60);

static SEATED: AtomicBool = AtomicBool::new(true);

// whether this session has the seat
pub fn seated() -> bool {
    SEATED.load(Ordering::Relaxed)
}

// runs on its own thread for good
pub fn follow() {
    let mut wait = FIRST;
    let mut said = None;

    loop {
        let started = Instant::now();
        let why = run();

        SEATED.store(true, Ordering::Relaxed);

        // said once, not at each try
        if said.as_ref() != Some(&why) {
            eprintln!("kanade: the OSD shows whichever session has the seat, for now: {why}");
            said = Some(why);
        }

        if started.elapsed() >= LASTED {
            wait = FIRST;
        }

        thread::sleep(wait);
        wait = (wait * 2).min(LONGEST);
    }
}

// follows the session until the bus is lost, and why it ended
fn run() -> String {
    let session = lock::session();

    if session.is_empty() {
        return String::from("logind names no session");
    }

    let connection = match Connection::system() {
        Ok(connection) => connection,
        Err(error) => return format!("cannot connect: {error}"),
    };

    // subscribed before it is read, so no change falls between
    let rule = format!(
        "type='signal',sender='{LOGIND}',path='{session}',interface='{PROPERTIES}',\
         member='PropertiesChanged',arg0='{SESSION}'"
    );
    let signals = match MatchRule::try_from(rule.as_str())
        .and_then(|rule| MessageIterator::for_match_rule(rule, &connection, None))
    {
        Ok(signals) => signals,
        Err(error) => return format!("cannot follow logind: {error}"),
    };

    read(&connection, session);

    // read again on any change to the session, since one may only say `Active` was invalidated
    for signal in signals {
        if signal.is_ok() {
            read(&connection, session);
        }
    }

    String::from("the system bus was lost")
}

// a session logind does not answer for keeps what it had
fn read(connection: &Connection, session: &str) {
    let active = connection
        .call_method(
            Some(LOGIND),
            session,
            Some(PROPERTIES),
            "Get",
            &(SESSION, "Active"),
        )
        .and_then(|reply| reply.body().deserialize::<OwnedValue>())
        .ok()
        .and_then(|value| bool::try_from(value).ok());

    if let Some(active) = active {
        SEATED.store(active, Ordering::Relaxed);
    }
}
