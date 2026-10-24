//! Caffeine (docs/archived/design.md Caffeine, ADR 0011): `kanade caffeine on [duration]` keeps the
//! session from going idle, so nothing that waits on idle, like the idle Module or hypridle, locks
//! or suspends it;
//! `off` lets it go idle again and `toggle` does whichever it is not. While on, a Persistent Ongoing
//! Caffeine Activity says so and offers Turn off.
//!
//! logind holds the inhibitor for as long as the program that asked for it runs, and `src/bus.rs`'s `Bus`
//! cannot hold the fd it hands back, so `systemd-inhibit` is a holder (ADR 0011): caffeine is on
//! while it runs. A duration is its command, `sleep`, so the inhibitor ends on its own, counted
//! like the timer while the machine is awake. Ending on its own, it ended caffeine, which is never
//! turned on again. It runs only through setpriv, so it dies with Kanade; otherwise it could hold
//! the inhibitor for good, with no Kanade left that knows of it.
//!
//! `on` only starts a holder, answered at once so the draw thread never waits on logind. The
//! holder's command says when logind gave it the inhibitor, and only then does caffeine turn on,
//! letting go of the holder before it, so it never lapses. The call is answered then (`Later`).

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read};
use std::process::{Child, ChildStderr, ChildStdout, ExitStatus};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::{MutexExt, service::Service};

use super::capture::SHOWN;
use super::timer;
use super::wake::{self, NOT_FOUND, SETPRIV};
use crate::island::activity::{
    Action, Activity, Awake, Detail, Id, Interrupt, Kind, Lifetime, Priority, Scope,
};
use crate::island::service::IslandService;
use crate::later::Later;

pub const INHIBIT: &str = "systemd-inhibit";

// the Activity's action that turns caffeine off
pub const OFF: &str = "off";

// the last lines systemd-inhibit printed, which say why it ended
const SAID: usize = 4;

// what the inhibitor's command says once it runs, so once logind gave systemd-inhibit the inhibitor
pub const HELD: &str = "held";

// how long a holder may take to say it, after which it is given up on; logind answers in milliseconds
pub const READY: Duration = Duration::from_secs(2);

static CAFFEINE: Mutex<Caffeine> = Mutex::new(Caffeine {
    holding: None,
    starting: None,
    issued: 0,
});

#[derive(Default)]
struct Caffeine {
    // the inhibitor's holder, none while caffeine is off
    holding: Option<Holding>,

    // the newest `on`'s holder, until it holds the inhibitor and takes over from `holding`
    starting: Option<Holding>,

    /*
     * the serials given out since Kanade started, never given again, so a serial names one
     * holder: a stale Turn off or end never mistakes a newer one
     */
    issued: u64,
}

struct Holding {
    serial: u64,
    length: Option<Duration>,

    // the `kanade` call waiting for it to hold the inhibitor, while it starts
    later: Option<Later>,

    // reaped only once taken out of `holding` or `starting`, so while there its pid is its own
    child: Child,
}

// what `kanade caffeine` asks; a duration ends it on its own, none holds it until turned off
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    On(Option<Duration>),
    Off,
    Toggle(Option<Duration>),
    Status,
}

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        let lasting = |text| timer::duration(text).map(Some);

        match arguments {
            ["on"] => Some(Request::On(None)),
            ["on", text] => lasting(text).map(Request::On),
            ["off"] => Some(Request::Off),
            ["toggle"] => Some(Request::Toggle(None)),
            ["toggle", text] => lasting(text).map(Request::Toggle),
            ["status"] => Some(Request::Status),
            _ => None,
        }
    }
}

// the length caffeine holds for, as `kanade` prints it
fn held_for(length: Option<Duration>) -> String {
    match length {
        Some(length) => format!("on for {}", self::length(length)),
        None => String::from("on until turned off"),
    }
}

// whether caffeine holds its inhibitor now
pub fn holding() -> bool {
    CAFFEINE.locked().holding.is_some()
}

/*
 * the serial of the newest holder, one still starting or the one holding; none while caffeine is
 * off. A serial is never given again, so it names one `on`, and another `on` or an end changes it
 */
pub fn newest() -> Option<u64> {
    let caffeine = CAFFEINE.locked();

    caffeine
        .starting
        .as_ref()
        .or(caffeine.holding.as_ref())
        .map(|holder| holder.serial)
}

/*
 * does what `request` asks and answers `later` with how caffeine is left, an `on` once its holder
 * holds the inhibitor. Call it from the draw thread, which lives as long as Kanade: systemd-inhibit
 * dies with the thread that started it
 */
pub fn request(request: Request, later: Later) {
    let mut caffeine = CAFFEINE.locked();

    match request {
        Request::Status => later.done(caffeine.status()),
        Request::Off => {
            caffeine.turn_off();
            later.done("off");
        }
        Request::Toggle(_) if caffeine.holding.is_some() || caffeine.starting.is_some() => {
            caffeine.turn_off();
            later.done("off");
        }
        Request::On(length) | Request::Toggle(length) => {
            if let Err(why) = caffeine.turn_on(length, &later) {
                later.refused(why);
            }
        }
    }
}

// turns off the caffeine that `serial` names, as its Activity's Turn off does
pub fn act(key: &str, serial: &str) {
    let mut caffeine = CAFFEINE.locked();

    if key == OFF
        && let Ok(serial) = serial.parse::<u64>()
        && caffeine
            .holding
            .as_ref()
            .is_some_and(|holding| holding.serial == serial)
    {
        caffeine.turn_off();
    }
}

impl Caffeine {
    // like "on for 1h", "off", and a line more while an `on` still starts
    fn status(&self) -> String {
        let mut lines = vec![match &self.holding {
            Some(holding) => held_for(holding.length),
            None => String::from("off"),
        }];

        if self.starting.is_some() {
            lines.push(String::from("starting"));
        }

        lines.join("\n")
    }

    /*
     * starts a holder for a new inhibitor, in place of any still starting, which answers `later`
     * once it holds (`follow`), or is given up on after `READY`
     */
    fn turn_on(&mut self, length: Option<Duration>, later: &Later) -> Result<(), String> {
        self.start(length, later)
            .inspect_err(|why| self.refused(why))
    }

    fn start(&mut self, length: Option<Duration>, later: &Later) -> Result<(), String> {
        // setpriv says a missing program only once running, which is too late to refuse `on`
        if !wake::found(INHIBIT) {
            return Err(format!("{INHIBIT} not found"));
        }

        // without it the inhibitor outlives Kanade, held for good by nothing that can let go of it
        if !wake::found(SETPRIV) {
            return Err(format!(
                "{SETPRIV} not found, so the inhibitor could outlive Kanade"
            ));
        }

        let arguments = arguments(length);
        let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();

        let child = wake::hold_lock(INHIBIT, &arguments)
            .map_err(|error| format!("cannot run {INHIBIT}: {error}"))?;

        self.issued += 1;
        self.wait_on(&CAFFEINE, child, self.issued, length, later, READY)
    }

    /*
     * makes `child`, as `serial`, the starting holder of `home`, which is `self` once unlocked,
     * followed until it holds or ends and given up on after `patience`
     */
    fn wait_on(
        &mut self,
        home: &'static Mutex<Caffeine>,
        mut child: Child,
        serial: u64,
        length: Option<Duration>,
        later: &Later,
        patience: Duration,
    ) -> Result<(), String> {
        let (held, said) = (child.stdout.take(), child.stderr.take());

        let followed = thread::Builder::new()
            .name(String::from("caffeine"))
            .spawn(move || follow(home, held, said, serial))
            .and_then(|_| {
                thread::Builder::new()
                    .name(String::from("caffeine wait"))
                    .spawn(move || {
                        thread::sleep(patience);
                        home.locked().give_up(serial, patience);
                    })
            });

        // either thread left finds it no longer starting
        if let Err(error) = followed {
            drop(child.kill());
            drop(child.wait());

            return Err(format!("cannot follow {INHIBIT}: {error}"));
        }

        release(self.starting.replace(Holding {
            serial,
            length,
            later: Some(later.clone()),
            child,
        }));

        Ok(())
    }

    // the holder `serial` names holds the inhibitor: caffeine is on, the holder before let go of
    fn held(&mut self, serial: u64) {
        let Some(mut holding) = self.starting.take_if(|starting| starting.serial == serial) else {
            return;
        };

        let length = holding.length;

        if let Some(later) = holding.later.take() {
            later.done(held_for(length));
        }

        release(self.holding.replace(holding));
        crate::idle::look();

        // under the lock, so its end, which takes it, posts after
        IslandService::write().post(on(serial, length), Instant::now());
    }

    // the holder `serial` names ended, having printed `said`, before it held the inhibitor
    fn ended_starting(&mut self, serial: u64, said: VecDeque<String>) {
        let Some(mut starting) = self.starting.take_if(|starting| starting.serial == serial) else {
            return;
        };

        // ending at all before it held, it failed, even as `sleep` would end
        let why = ended(starting.child.wait(), said)
            .err()
            .unwrap_or_else(|| format!("{INHIBIT} ended before it held the inhibitor"));

        self.failed_start(starting.later.take(), why);
    }

    // the holder `serial` names did not hold the inhibitor within `patience`
    fn give_up(&mut self, serial: u64, patience: Duration) {
        if let Some(mut starting) = self.starting.take_if(|starting| starting.serial == serial) {
            let later = starting.later.take();

            release(Some(starting));
            self.failed_start(
                later,
                format!("{INHIBIT} did not hold the inhibitor within {patience:?}"),
            );
        }
    }

    // the `on` waited on is refused, and shown so as a keybind has no reply to read
    fn failed_start(&self, later: Option<Later>, why: String) {
        eprintln!("caffeine: did not turn on: {why}");
        self.refused(&why);

        if let Some(later) = later {
            later.refused(why);
        }
    }

    // an `on` that failed shows why, as a keybind has no reply to read; one already on stays on
    fn refused(&self, why: &str) {
        if self.holding.is_none() {
            IslandService::write().post(failed(why.to_owned()), Instant::now());
        }
    }

    fn turn_off(&mut self) {
        release(self.starting.take());

        if self.holding.is_some() {
            release(self.holding.take());
            crate::idle::look();
            IslandService::write().withdraw(&id(), Instant::now());
        }
    }

    /*
     * reaps the holder `serial` names, done printing `said`, and whether it ran out as asked or
     * why not; none once it was let go of, or another took its place
     */
    fn end(&mut self, serial: u64, said: VecDeque<String>) -> Option<Result<(), String>> {
        let mut holding = self.holding.take_if(|holding| holding.serial == serial)?;
        crate::idle::look();

        let ended = ended(holding.child.wait(), said);

        if let Err(why) = &ended {
            eprintln!("caffeine: turned off on its own: {why}");
        }

        Some(ended)
    }
}

/*
 * logind lets go of the inhibitor once its holder dies, and the holder's command dies with it; a
 * call still waiting on it is left not knowing, as it was turned off or another `on` took its place
 */
fn release(holding: Option<Holding>) {
    if let Some(mut holding) = holding {
        drop(holding.child.kill());
        drop(holding.child.wait());

        if let Some(later) = holding.later {
            later.unknown(format!(
                "caffeine was turned off, or another on took its place, before {INHIBIT} held the inhibitor"
            ));
        }
    }
}

/*
 * follows the holder `serial` names: turns caffeine on once it says it holds the inhibitor, then
 * reads what it prints until it exits, and takes the Activity away, or shows why caffeine failed
 */
fn follow(
    home: &Mutex<Caffeine>,
    held: Option<ChildStdout>,
    said: Option<ChildStderr>,
    serial: u64,
) {
    if !holds(held) {
        let said = last_lines(said);
        home.locked().ended_starting(serial, said);
        return;
    }

    home.locked().held(serial);

    let said = last_lines(said);
    let mut caffeine = home.locked();

    // under the lock, so a newer holder's start posts after
    match caffeine.end(serial, said) {
        Some(Ok(())) => IslandService::write().withdraw(&id(), Instant::now()),
        Some(Err(why)) => IslandService::write().post(failed(why), Instant::now()),
        None => {}
    }
}

/*
 * whether `held` says `HELD` before it closes: systemd-inhibit runs its command, which says it,
 * only once logind gave it the inhibitor
 */
pub fn holds(held: Option<impl Read>) -> bool {
    held.into_iter()
        .flat_map(|held| BufReader::new(held).lines().map_while(Result::ok))
        .any(|line| line == HELD)
}

// the last `SAID` lines `said` prints until it closes
pub fn last_lines(said: Option<impl Read>) -> VecDeque<String> {
    let mut last = VecDeque::with_capacity(SAID);

    for line in said
        .into_iter()
        .flat_map(|said| BufReader::new(said).lines().map_while(Result::ok))
    {
        if last.len() == SAID {
            last.pop_front();
        }

        last.push_back(line);
    }

    last
}

// whether a holder that exited as `status`, having printed `said`, ran out as asked, or why not
pub fn ended(status: io::Result<ExitStatus>, said: VecDeque<String>) -> Result<(), String> {
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) if status.code() == Some(NOT_FOUND) => Err(format!("{INHIBIT} not found")),
        Ok(status) => {
            let said = Vec::from(said).join("; ");

            Err(match said.is_empty() {
                true => format!("{INHIBIT} ended ({status})"),
                false => format!("{INHIBIT} ended ({status}): {said}"),
            })
        }
        Err(error) => Err(format!("{INHIBIT}: {error}")),
    }
}

/*
 * an idle inhibitor that blocks, held by its command, which says `HELD`, then sleeps for the
 * duration, else for good
 */
fn arguments(length: Option<Duration>) -> Vec<String> {
    let held = match length {
        Some(length) => length.as_secs().to_string(),
        None => String::from("infinity"),
    };

    [
        "--what=idle",
        "--who=Kanade",
        "--why=Caffeine is on",
        "--mode=block",
        "sh",
        "-c",
        &format!("echo {HELD}; exec sleep \"$0\""),
        &held,
    ]
    .map(String::from)
    .to_vec()
}

// like 1h30m, 25m or 90s as 1m30s, as `timer start` takes it
pub fn length(length: Duration) -> String {
    let seconds = length.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);

    [(hours, "h"), (minutes, "m"), (seconds, "s")]
        .iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, unit)| format!("{count}{unit}"))
        .collect()
}

// one caffeine at a time, a failure replacing it
fn id() -> Id {
    Id::new(Kind::Caffeine, "caffeine")
}

// Ongoing like a timer: over Media, a Satellite beside a higher primary, on every island
fn on(serial: u64, length: Option<Duration>) -> Activity {
    Activity::new(
        id(),
        Priority::Ongoing,
        Lifetime::Persistent,
        Scope::Global,
        Interrupt::None,
    )
    .expect("a Persistent that does not auto-expand is valid")
    .with_actions(vec![Action {
        key: String::from(OFF),
        label: String::from("Turn off"),
    }])
    .with_detail(Detail::Caffeine(Awake::On {
        serial: serial.to_string(),
        length,
    }))
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
    .with_detail(Detail::Caffeine(Awake::Failed { why }))
}
