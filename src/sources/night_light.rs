//! Night light (ADR 0046): `night-light.mode` warms the screen all day or from sunset to sunrise,
//! by `wlsunset`, which Kanade holds while it is on. The compositor gives the gamma back once
//! wlsunset dies, so off is killing it. It runs only through setpriv, so it dies with Kanade.
//!
//! The schedule is `night-light.sunset` and `night-light.sunrise` when both are set, else the sun at
//! `weather.location`. A config change starts it again with what the config now says. One that
//! ends on its own runs again after a backoff, and its first failure since the config last changed,
//! or since it last ran for a while, shows as a Failed Activity.
//!
//! The Controls tile and `kanade night-light` turn it on and off by `night-light.mode` in the
//! settings file, as Settings would, so it stays as left across restarts.

use std::collections::VecDeque;
use std::io;
use std::process::{Child, ExitStatus};
use std::sync::LazyLock;
use std::sync::mpsc::RecvTimeoutError;
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::queue::Queue;
use kanade_runtime::service::Service;
use toml::Value;

use super::caffeine;
use super::capture::SHOWN;
use super::wake::{self, NOT_FOUND, SETPRIV};
use crate::config::{self, Location, NightLight, NightMode};
use crate::island::activity::{Activity, Detail, Id, Interrupt, Kind, Lifetime, Priority, Scope};
use crate::island::service::IslandService;
use crate::{reload, settings};

pub const WLSUNSET: &str = "wlsunset";

// how long until wlsunset runs again after it ended, doubling up to `LAST_RETRY` while it keeps
// failing
const FIRST_RETRY: Duration = Duration::from_secs(1);
const LAST_RETRY: Duration = Duration::from_secs(60);

// how long it must have run for its end to count as a new failure, shown again
const HEALTHY: Duration = Duration::from_secs(60);

// wlsunset's own daytime temperature, which a schedule turns back to
const NEUTRAL: u32 = 6500;

static QUEUE: LazyLock<Queue<Message>> = LazyLock::new(Queue::new);

enum Message {
    // the config was read again
    Reread,

    // the wlsunset `serial` names closed its stderr, having said `said`, so it ended
    Ended { serial: u64, said: VecDeque<String> },
}

/*
 * how the night light stands, for the Controls tile and `status`; written only by `follow`, the
 * mode itself being the config's
 */
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shade {
    // wlsunset runs now
    pub running: bool,

    // why it is not running while it should, none while it is or should not be
    pub failed: Option<String>,

    // wlsunset is not installed, so nothing can turn it on
    pub missing: bool,
}

impl Service for Shade {
    fn new() -> Self {
        Shade::default()
    }

    fn listen() {}
}

// what `kanade night-light` asks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    On,
    Off,
    Toggle,
    Status,
}

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        match arguments {
            ["on"] => Some(Request::On),
            ["off"] => Some(Request::Off),
            ["toggle"] => Some(Request::Toggle),
            ["status"] => Some(Request::Status),
            _ => None,
        }
    }
}

/*
 * does what `request` asks and says how the night light is left. Call it from the draw thread,
 * as the settings file's writes are made there
 */
pub fn request(request: Request) -> Result<String, String> {
    match request {
        Request::Status => return Ok(status()),
        Request::On => turn(true)?,
        Request::Off => turn(false)?,
        Request::Toggle => toggle()?,
    }

    Ok(said(&config::get().night_light))
}

// on if off, else off
pub fn toggle() -> Result<(), String> {
    turn(config::get().night_light.mode == NightMode::Off)
}

// the Controls tile pressed: as `turn`, a refusal shown as the night light's failure
pub fn switch(on: bool) {
    if let Err(why) = turn(on) {
        eprintln!("kanade: night light: {why}");
        IslandService::write().post(failed(why), Instant::now());
    }
}

/*
 * `night-light.mode` in the settings file: off, or on as the files below it say, a schedule if they
 * set one, so turning it off and on again keeps the schedule. Applied at once, not on the watch's
 * debounce, so the tile and `status` show it
 */
pub fn turn(on: bool) -> Result<(), String> {
    let mode = if on { on_mode() } else { NightMode::Off };

    settings::set(
        &["night-light", "mode"],
        Some(Value::String(mode.name().to_owned())),
    )?;

    match reload::reload() {
        reload::Outcome::Valid(_) => Ok(()),
        reload::Outcome::Invalid(_) => Err(String::from(
            "saved, but the config did not reload, see `kanade status`",
        )),
    }
}

// the mode on means: the files' below the settings file, else a schedule where both times are set
fn on_mode() -> NightMode {
    let (below, _) = config::layers(false);
    let night = config::get().night_light;

    match below.night_light.mode {
        NightMode::Off if night.sunset.is_some() && night.sunrise.is_some() => NightMode::Schedule,
        NightMode::Off => NightMode::On,
        mode => mode,
    }
}

// the night light as `status` says it: the mode, then why it is not running, if it should be
pub fn status() -> String {
    let config = config::get();
    let mut lines = vec![said(&config.night_light)];
    let shade = Shade::read().clone();

    if let Some(why) = shade.failed {
        lines.push(format!("not running: {}", why.replace('\n', "; ")));
    } else if shade.missing {
        lines.push(format!("{WLSUNSET} not found, so it cannot turn on"));
    } else if config.night_light.mode != NightMode::Off && !shade.running {
        lines.push(format!("{WLSUNSET} is starting"));
    }

    lines.join("\n")
}

// like "on at 4000 K" or "schedule 20:00 to 07:00 at 4000 K"
fn said(night: &NightLight) -> String {
    let kelvin = night.temperature;

    match (night.mode, night.sunset, night.sunrise) {
        (NightMode::Off, ..) => String::from("off"),
        (NightMode::On, ..) => format!("on at {kelvin} K"),
        (NightMode::Schedule, Some(sunset), Some(sunrise)) => {
            format!("schedule {sunset} to {sunrise} at {kelvin} K")
        }
        (NightMode::Schedule, ..) => format!("schedule sunset to sunrise at {kelvin} K"),
    }
}

// the config was read again; one that changed the night light starts wlsunset again
pub fn reread() {
    QUEUE.send(Message::Reread);
}

/*
 * what wlsunset runs with for `night`, none while off, or why it cannot run. It always follows a
 * schedule, so on is a night and day only 1 K apart, either the temperature asked for
 */
fn arguments(
    night: &NightLight,
    location: Option<Location>,
) -> Result<Option<Vec<String>>, String> {
    let low = night.temperature;
    let temperatures = |high: u32| {
        vec![
            String::from("-t"),
            low.to_string(),
            String::from("-T"),
            high.to_string(),
        ]
    };

    let mut arguments = match night.mode {
        NightMode::Off => return Ok(None),
        NightMode::On => {
            let mut arguments = temperatures(low + 1);
            arguments.extend(["-S", "06:00", "-s", "18:00"].map(String::from));
            arguments
        }
        NightMode::Schedule => temperatures(NEUTRAL.max(low + 1)),
    };

    if night.mode == NightMode::Schedule {
        match (night.sunset, night.sunrise, location) {
            (Some(sunset), Some(sunrise), _) => arguments.extend([
                String::from("-S"),
                sunrise.to_string(),
                String::from("-s"),
                sunset.to_string(),
            ]),
            (_, _, Some(location)) => arguments.extend([
                String::from("-l"),
                location.latitude.to_string(),
                String::from("-L"),
                location.longitude.to_string(),
            ]),
            _ => {
                return Err(String::from(
                    "a schedule needs night-light.sunset and night-light.sunrise, or \
                     weather.location",
                ));
            }
        }
    }

    Ok(Some(arguments))
}

// the wlsunset running, killed once dropped, so a panic leaves none running for nobody
struct Running {
    serial: u64,
    started: Instant,
    child: Child,
}

impl Drop for Running {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

// what `follow` keeps between messages
#[derive(Default)]
struct Holder {
    // what wlsunset should run with, none while off, or why it cannot run
    wanted: Option<Result<Option<Vec<String>>, String>>,

    running: Option<Running>,

    // when it runs again after it ended, and how long the next wait after that is
    retry: Option<Instant>,
    wait: Option<Duration>,

    // whether a failure showed since the config last changed or wlsunset last ran for a while
    shown: bool,

    // the serials given out, never given again, so a stale end never mistakes a newer one
    issued: u64,

    // wlsunset was not found when the config was last read
    missing: bool,
}

/*
 * runs on its own thread for good, which `supervise::spawn` runs again after a panic: wlsunset dies
 * with the thread that started it, as setpriv says, and a panic drops the one running
 */
pub fn follow() {
    let mut holder = Holder::default();
    holder.reread();

    let queue = QUEUE.receiver();

    loop {
        let message = match holder.retry {
            Some(at) => queue.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => queue.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };

        match message {
            Ok(Message::Reread) => holder.reread(),
            Ok(Message::Ended { serial, said }) => holder.ended(serial, said),
            Err(RecvTimeoutError::Timeout) => {
                holder.retry = None;
                holder.start();
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

impl Holder {
    // what the config says now; a change starts wlsunset again, or stops it
    fn reread(&mut self) {
        let config = config::get();
        let wanted = arguments(&config.night_light, config.location);

        // looked for again on each change, so installing it needs no restart
        self.missing = !wake::found(WLSUNSET);

        if self.wanted.as_ref() == Some(&wanted) {
            return self.publish(self.running.is_some(), Shade::read().failed.clone());
        }

        self.running = None;
        self.retry = None;
        self.wait = None;
        self.shown = false;
        self.wanted = Some(wanted);

        self.start();
    }

    // starts wlsunset as wanted, or says why it cannot
    fn start(&mut self) {
        let arguments = match &self.wanted {
            Some(Ok(Some(arguments))) => arguments.clone(),
            Some(Err(why)) => {
                let why = why.clone();
                return self.failed(why, false);
            }
            _ => return self.publish(false, None),
        };

        // setpriv says a missing program only once running, so it would run again for nothing
        if self.missing {
            return self.failed(format!("{WLSUNSET} not found"), false);
        }

        // without it wlsunset outlives Kanade, warming the screen for good with nothing to stop it
        if !wake::found(SETPRIV) {
            return self.failed(
                format!("{SETPRIV} not found, so {WLSUNSET} could outlive Kanade"),
                false,
            );
        }

        let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();

        let mut child = match wake::hold_quiet(WLSUNSET, &arguments) {
            Ok(child) => child,
            Err(error) => return self.failed(format!("cannot run {WLSUNSET}: {error}"), true),
        };

        self.issued += 1;
        let serial = self.issued;
        let said = child.stderr.take();

        let followed = thread::Builder::new()
            .name(String::from("night-light wait"))
            .spawn(move || {
                let said = caffeine::last_lines(said);
                QUEUE.send(Message::Ended { serial, said });
            });

        let running = Running {
            serial,
            started: Instant::now(),
            child,
        };

        if let Err(error) = followed {
            drop(running);
            return self.failed(format!("cannot follow {WLSUNSET}: {error}"), true);
        }

        self.running = Some(running);
        self.publish(true, None);
    }

    // the wlsunset `serial` names ended, having said `said`; one stopped on purpose is gone already
    fn ended(&mut self, serial: u64, said: VecDeque<String>) {
        let Some(mut running) = self.running.take_if(|running| running.serial == serial) else {
            return;
        };

        let status = running.child.wait();
        let ran = running.started.elapsed();
        drop(running);

        // one that ran for a while failed anew, so it shows again and waits as at first
        if ran >= HEALTHY {
            self.shown = false;
            self.wait = None;
        }

        let missing = matches!(&status, Ok(status) if status.code() == Some(NOT_FOUND));
        self.failed(ended(status, said), !missing);
    }

    /*
     * wlsunset is not running as wanted, for `why`, shown once until the config changes or it ran
     * for a while; `again` runs it again after a backoff
     */
    fn failed(&mut self, why: String, again: bool) {
        eprintln!("kanade: night light: {why}");

        if !self.shown {
            self.shown = true;
            IslandService::write().post(failed(why.clone()), Instant::now());
        }

        if again {
            let wait = self.wait.unwrap_or(FIRST_RETRY);

            self.retry = Some(Instant::now() + wait);
            self.wait = Some((wait * 2).min(LAST_RETRY));
        }

        self.publish(false, Some(why));
    }

    // written only when it changed, so a view that reads it draws again only then
    fn publish(&self, running: bool, failed: Option<String>) {
        let shade = Shade {
            running,
            failed,
            missing: self.missing,
        };

        if *Shade::read() != shade {
            *Shade::write() = shade;
        }
    }
}

// why wlsunset, exited as `status` having said `said`, ended; it never ends on its own when well
fn ended(status: io::Result<ExitStatus>, said: VecDeque<String>) -> String {
    match status {
        Ok(status) if status.code() == Some(NOT_FOUND) => format!("{WLSUNSET} not found"),
        Ok(status) => {
            let said = Vec::from(said).join("; ");

            match said.is_empty() {
                true => format!("{WLSUNSET} ended ({status})"),
                false => format!("{WLSUNSET} ended ({status}): {said}"),
            }
        }
        Err(error) => format!("{WLSUNSET}: {error}"),
    }
}

fn id() -> Id {
    Id::new(Kind::NightLight, "night-light")
}

// as a failed recording shows, on the output the user is looking at
fn failed(why: String) -> Activity {
    Activity::new(
        id(),
        Priority::Actionable,
        Lifetime::Transient(SHOWN),
        Scope::FocusedOutput,
        Interrupt::None,
    )
    .expect("a Transient that does not auto-expand is valid")
    .with_detail(Detail::NightLight(why))
}
