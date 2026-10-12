//! Whether logind lists an idle inhibitor, as `systemd-inhibit --what=idle` takes: its
//! `BlockInhibited` naming `idle`, followed over a zbus connection of Kanade's own whose match rule
//! names the manager's properties. Until logind says, and while it cannot be followed, it is
//! unknown, which inhibits nothing: an idle lock lost to a wrong guess is worse than one an
//! inhibitor would have held. A bus lost is tried again, sooner after one that held long.

use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::MutexExt;
use zbus::MatchRule;
use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::OwnedValue;

const LOGIND: &str = "org.freedesktop.login1";
const PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

// the wait before trying again, doubled at each failure up to `LONGEST`
const FIRST: Duration = Duration::from_secs(1);
const LONGEST: Duration = Duration::from_secs(300);

// a run that lasted this long was no flapping one, so the next is tried soon
const LASTED: Duration = Duration::from_secs(60);

static INHIBITED: Mutex<Option<bool>> = Mutex::new(None);

// whether logind lists an idle inhibitor, none while unknown
pub fn inhibited() -> Option<bool> {
    *INHIBITED.locked()
}

fn set(inhibited: Option<bool>) {
    let changed = std::mem::replace(&mut *INHIBITED.locked(), inhibited) != inhibited;

    if changed {
        super::look();
    }
}

// runs on its own thread for good
pub fn follow() {
    let mut wait = FIRST;
    let mut said = None;

    loop {
        let started = Instant::now();
        let why = run();

        set(None);

        // said once, not at each try
        if said.as_ref() != Some(&why) {
            eprintln!("kanade: idle does not honor logind's idle inhibitors, for now: {why}");
            said = Some(why);
        }

        if started.elapsed() >= LASTED {
            wait = FIRST;
        }

        thread::sleep(wait);
        wait = (wait * 2).min(LONGEST);
    }
}

// follows the manager until the bus is lost, and why it ended
fn run() -> String {
    let connection = match Connection::system() {
        Ok(connection) => connection,
        Err(error) => return format!("cannot connect: {error}"),
    };

    // subscribed before it is read, so no change falls between
    let rule = format!(
        "type='signal',sender='{LOGIND}',path='{PATH}',interface='{PROPERTIES}',\
         member='PropertiesChanged',arg0='{MANAGER}'"
    );
    let signals = match MatchRule::try_from(rule.as_str())
        .and_then(|rule| MessageIterator::for_match_rule(rule, &connection, None))
    {
        Ok(signals) => signals,
        Err(error) => return format!("cannot follow logind: {error}"),
    };

    read(&connection);

    // read again on any change, since one may only say `BlockInhibited` was invalidated
    for signal in signals {
        if signal.is_ok() {
            read(&connection);
        }
    }

    String::from("the system bus was lost")
}

// a logind that does not answer is unknown
fn read(connection: &Connection) {
    let blocked = connection
        .call_method(
            Some(LOGIND),
            PATH,
            Some(PROPERTIES),
            "Get",
            &(MANAGER, "BlockInhibited"),
        )
        .and_then(|reply| reply.body().deserialize::<OwnedValue>())
        .ok()
        .and_then(|value| String::try_from(value).ok());

    set(blocked.map(|blocked| idle(&blocked)));
}

// `BlockInhibited` is what is inhibited, joined by colons, like "idle:handle-power-key"
fn idle(blocked: &str) -> bool {
    blocked.split(':').any(|what| what == "idle")
}

#[cfg(test)]
mod tests {
    use super::*;

    // parses logind's outside input: only `idle` itself inhibits idle
    #[test]
    fn only_idle_in_the_list_inhibits() {
        assert!(idle("idle"));
        assert!(idle("sleep:idle:handle-lid-switch"));
        assert!(!idle("handle-power-key"));
        assert!(!idle("idle-like"));
        assert!(!idle(""));
    }
}
