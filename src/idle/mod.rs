//! Idle in the shell (ADR 0047): after `idle.dim` the outputs fade toward dark, at `idle.lock` the
//! session locks as `kanade lock` does, then `idle.screen_off` powers the monitors off through niri
//! and `idle.suspend` asks logind to suspend. Input at any point takes the dim away at once.
//!
//! The compositor says when the user went idle (`kanade_runtime::idle`), through
//! `ext-idle-notify-v1`, which honors the Wayland idle inhibitors apps hold. logind's idle
//! inhibitors (`logind`) and caffeine hold every stage too. `schedule` says what fires when;
//! this thread does it. The lock never waits on the dim: a dim that cannot draw still locks on
//! time, and a lock that fails is asked again while the user stays idle.

mod logind;
pub mod overlay;
pub mod schedule;

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use kanade_runtime::MutexExt;
use kanade_runtime::idle::{self as compositor, Idle};
use kanade_runtime::service::{self, Service};

use crate::config::{self, Reloads};
use crate::island::activity::Leave;
use crate::island::motion::Mode;
use crate::sources::{caffeine, niri, session};
use crate::{lock, modules, supervise};

use schedule::{Action, Schedule, Stages};

// how long a lock may take to hold before it counts as failed, and is asked again
const PATIENCE: Duration = Duration::from_secs(10);

const POWER_OFF_MONITORS: &str = r#"{"Action":{"PowerOffMonitors":{}}}"#;

// wakes the idle thread to look again at what it follows
static LOOK: OnceLock<Sender<()>> = OnceLock::new();

// what `kanade idle status` says beyond the config
static SEEN: Mutex<Seen> = Mutex::new(Seen {
    counting: None,
    idle: false,
    logind: None,
    caffeine: false,
    failed: None,
});

struct Seen {
    // since when the stages count, none while not idle or inhibited
    counting: Option<Instant>,

    idle: bool,

    // whether logind lists an idle inhibitor, none while it cannot be asked
    logind: Option<bool>,

    caffeine: bool,

    // why the newest lock failed, until one holds
    failed: Option<String>,
}

// the idle Module's start: the thread that follows idle, and the ones that wake it
pub fn spawn() {
    let (look, looks) = mpsc::channel();
    let _ = LOOK.set(look);

    service::watch::<Idle>(self::look);
    service::watch::<Reloads>(self::look);

    supervise::spawn("idle-logind", logind::follow);
    supervise::spawn("idle", move || follow(&looks));
}

// something the idle thread follows may have changed; nothing while the Module is off
pub fn look() {
    if let Some(look) = LOOK.get() {
        let _ = look.send(());
    }
}

fn follow(looks: &mpsc::Receiver<()>) {
    let mut schedule = Schedule::new(Stages::default());
    let mut seen: Option<Instant> = None;
    let mut first = None;

    // a restart after a panic starts from what holds now
    overlay::undim();

    loop {
        let now = Instant::now();
        let stages = config::get().idle;

        // new stages count from the start, as after input
        if stages != schedule.stages() {
            let actions = schedule.resumed();
            act(&mut schedule, actions, now);
            schedule = Schedule::new(stages);
            seen = None;

            if first != stages.first() {
                first = stages.first();
                compositor::watch(first.as_slice());
            }
        }

        let idled = first.and_then(|first| Idle::read().idled(first));

        if idled != seen {
            if seen.is_some() {
                let actions = schedule.resumed();
                act(&mut schedule, actions, now);
            }

            if let Some(at) = idled {
                schedule.idled(at);
            }

            seen = idled;
        }

        let logind = logind::inhibited();
        let caffeine = caffeine::holding();
        let actions = schedule.inhibit(logind == Some(true) || caffeine, now);
        act(&mut schedule, actions, now);

        let actions = schedule.tick(now);
        act(&mut schedule, actions, Instant::now());

        {
            let mut said = SEEN.locked();
            said.counting = schedule.counting();
            said.idle = seen.is_some();
            said.logind = logind;
            said.caffeine = caffeine;
        }

        let woke = match schedule.next() {
            Some(next) => looks.recv_timeout(next.saturating_duration_since(Instant::now())),
            None => looks.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };

        if woke == Err(RecvTimeoutError::Disconnected) {
            return;
        }
    }
}

// does what the schedule says at `now`
fn act(schedule: &mut Schedule, actions: Vec<Action>, now: Instant) {
    for action in actions {
        match action {
            Action::Dim { until, dark } => {
                // reduced motion has no fade: the lock comes at its time; a lock held shows no dim
                if config::get().island.motion != Mode::Reduced && !lock::holds() {
                    overlay::dim(now, until, dark);
                }
            }
            Action::Undim => overlay::undim(),
            Action::Lock => {
                if let Err(why) = locked() {
                    eprintln!("kanade: idle did not lock, asking again: {why}");
                    SEEN.locked().failed = Some(why);
                    schedule.lock_failed(Instant::now());
                } else {
                    SEEN.locked().failed = None;
                }
            }
            Action::ScreenOff => {
                if let Err(error) = niri::act(POWER_OFF_MONITORS) {
                    eprintln!("kanade: idle did not power the monitors off: {error}");
                }
            }
            Action::Suspend => {
                if !modules::on("session") {
                    eprintln!("kanade: idle did not suspend: module session is off");
                } else if let Err(why) = session::request(Leave::Sleep) {
                    eprintln!("kanade: idle did not suspend: {why}");
                }
            }
        }
    }
}

// the lock as `kanade lock` asks it, held once niri says so; one already held is kept
fn locked() -> Result<(), String> {
    if !modules::on("lock") {
        return Err(String::from("module lock is off"));
    }

    if lock::holds() {
        return Ok(());
    }

    lock::hold(PATIENCE)
}

fn seconds(after: Option<Duration>) -> String {
    after.map_or_else(
        || String::from("never"),
        |after| format!("after {} s", after.as_secs()),
    )
}

// `kanade idle status`: the stages, where the count stands, and what inhibits it
pub fn status() -> String {
    let stages = config::get().idle;
    let seen = SEEN.locked();
    let mut lines = vec![
        format!("dim {}", seconds(stages.dim)),
        format!("lock {}", seconds(stages.lock)),
        format!("screen off {}", seconds(stages.screen_off)),
        format!("suspend {}", seconds(stages.suspend)),
    ];

    match Idle::read().unavailable() {
        Some(why) => lines.push(format!("not followed: {why}")),
        None => lines.push(match (seen.counting, seen.idle) {
            (Some(since), _) => format!("idle for {} s", since.elapsed().as_secs()),
            (None, true) => String::from("idle, held by an inhibitor"),
            (None, false) => String::from("not idle"),
        }),
    }

    let mut inhibitors = Vec::new();
    if seen.logind == Some(true) {
        inhibitors.push("a logind idle inhibitor, which `systemd-inhibit --list` names");
    }
    if seen.caffeine {
        inhibitors.push("caffeine");
    }
    if inhibitors.is_empty() {
        lines.push(String::from("no inhibitor holds it"));
    } else {
        lines.push(format!("held by {}", inhibitors.join(" and ")));
    }
    if seen.logind.is_none() {
        lines.push(String::from(
            "logind cannot be asked, so its idle inhibitors are not honored",
        ));
    }
    lines.push(String::from(
        "an app's Wayland idle inhibitor, as a playing video's, keeps niri from saying idle",
    ));

    if let Some(why) = &seen.failed {
        lines.push(format!("the last lock failed: {why}"));
    }

    lines.join("\n")
}
