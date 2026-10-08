//! Caffeine (#141, docs/design.md Caffeine, ADR 0011): `kanade caffeine on [duration]` keeps the
//! session from going idle, so nothing that waits on idle, like hypridle, locks or suspends it;
//! `off` lets it go idle again and `toggle` does whichever it is not. While on, a Persistent Ongoing
//! Caffeine Activity says so and offers Turn off.
//!
//! logind holds the inhibitor for as long as the program that asked for it runs, and Amane's `Bus`
//! cannot hold the fd it hands back, so `systemd-inhibit` is a holder (ADR 0011): caffeine is on
//! while it runs. A duration is its command, `sleep`, so the inhibitor ends on its own, counted
//! like the timer while the machine is awake. Ending on its own, it ended caffeine, which is never
//! turned on again. It runs only through setpriv, so it dies with Kanade; otherwise it could hold
//! the inhibitor for good, with no Kanade left that knows of it.
//!
//! `on` only starts a holder, answered at once so the draw thread never waits on logind. The
//! holder's command says when logind gave it the inhibitor, and only then does caffeine turn on,
//! letting go of the holder before it, so it never lapses. `kanade` waits on `status` for that.

use std::collections::VecDeque;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read};
use std::process::{Child, ChildStderr, ChildStdout, ExitStatus};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use amane::Service;

use super::capture::SHOWN;
use super::timer;
use super::wake::{self, NOT_FOUND, SETPRIV};
use crate::island::activity::{
    Action, Activity, Awake, Detail, Id, Interrupt, Kind, Lifetime, Priority, Scope,
};
use crate::island::service::IslandService;

pub const INHIBIT: &str = "systemd-inhibit";

// the Activity's action that turns caffeine off
pub const OFF: &str = "off";

// the last lines systemd-inhibit printed, which say why it ended
const SAID: usize = 4;

// what the inhibitor's command says once it runs, so once logind gave systemd-inhibit the inhibitor
const HELD: &str = "held";

// how long a holder may take to say it, after which it is given up on; logind answers in milliseconds
const READY: Duration = Duration::from_secs(2);

static CAFFEINE: Mutex<Caffeine> = Mutex::new(Caffeine {
    holding: None,
    starting: None,
    failed: None,
    issued: 0,
});

#[derive(Default)]
struct Caffeine {
    // the inhibitor's holder, none while caffeine is off
    holding: Option<Holding>,

    // the newest `on`'s holder, until it holds the inhibitor and takes over from `holding`
    starting: Option<Holding>,

    // the newest `on` that failed and why, until the next request; for `status`
    failed: Option<(u64, String)>,

    /*
     * the serials given out since Kanade started, never given again, so a serial names one
     * holder: a stale Turn off or end never mistakes a newer one
     */
    issued: u64,
}

struct Holding {
    serial: u64,
    length: Option<Duration>,

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

/*
 * how caffeine stands, as `status` says it: a line a person reads, then one for an `on` still
 * starting and one for the newest that failed; `kanade` parses it back to wait on `on`
 */
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    // the serial holding the inhibitor and for how long, none while off
    pub on: Option<(u64, Option<Duration>)>,

    pub starting: Option<u64>,
    pub failed: Option<(u64, String)>,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.on {
            Some((serial, Some(length))) => write!(f, "on #{serial} for {}", self::length(length))?,
            Some((serial, None)) => write!(f, "on #{serial} until turned off")?,
            None => write!(f, "off")?,
        }

        if let Some(serial) = self.starting {
            write!(f, "\nstarting #{serial}")?;
        }

        if let Some((serial, why)) = &self.failed {
            write!(f, "\n#{serial} failed: {}", why.replace('\n', "; "))?;
        }

        Ok(())
    }
}

impl Status {
    pub fn parse(text: &str) -> Option<Status> {
        let serial = |text: &str| text.strip_prefix('#')?.parse::<u64>().ok();
        let mut lines = text.lines();

        let on = match lines.next()? {
            "off" => None,
            line => {
                let (number, held) = line.strip_prefix("on ")?.split_once(' ')?;
                let length = match held {
                    "until turned off" => None,
                    held => Some(timer::duration(held.strip_prefix("for ")?)?),
                };

                Some((serial(number)?, length))
            }
        };

        let mut status = Status {
            on,
            starting: None,
            failed: None,
        };

        for line in lines {
            if let Some(number) = line.strip_prefix("starting ") {
                status.starting = Some(serial(number)?);
            } else {
                let (number, why) = line.split_once(" failed: ")?;
                status.failed = Some((serial(number)?, why.to_owned()));
            }
        }

        Some(status)
    }

    // where the `on` that `serial` names stands
    pub fn settled(&self, serial: u64) -> Settled {
        match self {
            Status {
                on: Some((on, length)),
                ..
            } if *on == serial => Settled::On(match length {
                Some(length) => format!("on for {}", self::length(*length)),
                None => String::from("on until turned off"),
            }),
            Status {
                starting: Some(starting),
                ..
            } if *starting == serial => Settled::Waiting,
            Status {
                failed: Some((failed, why)),
                ..
            } if *failed == serial => Settled::Failed(why.clone()),
            _ => Settled::Lost(format!(
                "the shell no longer follows caffeine #{serial}; it says \"{}\"",
                self.to_string().replace('\n', "; ")
            )),
        }
    }
}

// where an `on` stands, as `status` says
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    Waiting,

    // its holder holds the inhibitor, said as `kanade` prints it
    On(String),

    Failed(String),

    // turned off, or another `on` took its place, before it held the inhibitor
    Lost(String),
}

// what the shell answers an `on`: its serial, to wait on through `status`
pub fn starting(reply: &str) -> Option<u64> {
    reply.strip_prefix("starting #")?.parse().ok()
}

fn lock(caffeine: &Mutex<Caffeine>) -> MutexGuard<'_, Caffeine> {
    caffeine.lock().unwrap_or_else(PoisonError::into_inner)
}

/*
 * does what `request` asks and says how caffeine is left, an `on` as starting. Call it from the
 * draw thread, which lives as long as Kanade: systemd-inhibit dies with the thread that started it
 */
pub fn request(request: Request) -> Result<String, String> {
    let mut caffeine = lock(&CAFFEINE);

    match request {
        Request::Status => Ok(caffeine.status().to_string()),
        Request::Off => {
            caffeine.turn_off();
            Ok(String::from("off"))
        }
        Request::Toggle(_) if caffeine.holding.is_some() || caffeine.starting.is_some() => {
            caffeine.turn_off();
            Ok(String::from("off"))
        }
        Request::On(length) | Request::Toggle(length) => caffeine
            .turn_on(length)
            .map(|serial| format!("starting #{serial}")),
    }
}

// turns off the caffeine that `serial` names, as its Activity's Turn off does
pub fn act(key: &str, serial: &str) {
    let mut caffeine = lock(&CAFFEINE);

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
    fn status(&self) -> Status {
        Status {
            on: self
                .holding
                .as_ref()
                .map(|holding| (holding.serial, holding.length)),
            starting: self.starting.as_ref().map(|starting| starting.serial),
            failed: self.failed.clone(),
        }
    }

    /*
     * starts a holder for a new inhibitor, in place of any still starting, and hands back its
     * serial; it turns caffeine on once it holds (`follow`), or is given up on after `READY`
     */
    fn turn_on(&mut self, length: Option<Duration>) -> Result<u64, String> {
        self.failed = None;

        self.start(length).inspect_err(|why| self.refused(why))
    }

    fn start(&mut self, length: Option<Duration>) -> Result<u64, String> {
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
        self.wait_on(&CAFFEINE, child, self.issued, length, READY)?;

        Ok(self.issued)
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
                        lock(home).give_up(serial, patience);
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
            child,
        }));

        Ok(())
    }

    // the holder `serial` names holds the inhibitor: caffeine is on, the holder before let go of
    fn held(&mut self, serial: u64) {
        let Some(holding) = self.starting.take_if(|starting| starting.serial == serial) else {
            return;
        };

        let length = holding.length;
        release(self.holding.replace(holding));

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

        self.failed_start(serial, why);
    }

    // the holder `serial` names did not hold the inhibitor within `patience`
    fn give_up(&mut self, serial: u64, patience: Duration) {
        if let Some(starting) = self.starting.take_if(|starting| starting.serial == serial) {
            release(Some(starting));
            self.failed_start(
                serial,
                format!("{INHIBIT} did not hold the inhibitor within {patience:?}"),
            );
        }
    }

    fn failed_start(&mut self, serial: u64, why: String) {
        eprintln!("caffeine: did not turn on: {why}");
        self.refused(&why);
        self.failed = Some((serial, why));
    }

    // an `on` that failed shows why, as a keybind has no reply to read; one already on stays on
    fn refused(&self, why: &str) {
        if self.holding.is_none() {
            IslandService::write().post(failed(why.to_owned()), Instant::now());
        }
    }

    fn turn_off(&mut self) {
        self.failed = None;
        release(self.starting.take());

        if self.holding.is_some() {
            release(self.holding.take());
            IslandService::write().withdraw(&id(), Instant::now());
        }
    }

    /*
     * reaps the holder `serial` names, done printing `said`, and whether it ran out as asked or
     * why not; none once it was let go of, or another took its place
     */
    fn end(&mut self, serial: u64, said: VecDeque<String>) -> Option<Result<(), String>> {
        let mut holding = self.holding.take_if(|holding| holding.serial == serial)?;

        let ended = ended(holding.child.wait(), said);

        if let Err(why) = &ended {
            eprintln!("caffeine: turned off on its own: {why}");
        }

        Some(ended)
    }
}

// logind lets go of the inhibitor once its holder dies, and the holder's command dies with it
fn release(holding: Option<Holding>) {
    if let Some(mut holding) = holding {
        drop(holding.child.kill());
        drop(holding.child.wait());
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
        lock(home).ended_starting(serial, said);
        return;
    }

    lock(home).held(serial);

    let said = last_lines(said);
    let mut caffeine = lock(home);

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
fn holds(held: Option<impl Read>) -> bool {
    held.into_iter()
        .flat_map(|held| BufReader::new(held).lines().map_while(Result::ok))
        .any(|line| line == HELD)
}

// the last `SAID` lines `said` prints until it closes
fn last_lines(said: Option<impl Read>) -> VecDeque<String> {
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
fn ended(status: io::Result<ExitStatus>, said: VecDeque<String>) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    #[test]
    fn each_request_is_named() {
        let hour = Some(Duration::from_secs(3600));

        assert_eq!(Request::parse(&["on"]), Some(Request::On(None)));
        assert_eq!(Request::parse(&["on", "1h"]), Some(Request::On(hour)));
        assert_eq!(Request::parse(&["off"]), Some(Request::Off));
        assert_eq!(Request::parse(&["status"]), Some(Request::Status));
        assert_eq!(Request::parse(&["toggle"]), Some(Request::Toggle(None)));
        assert_eq!(
            Request::parse(&["toggle", "1h"]),
            Some(Request::Toggle(hour))
        );

        for wrong in [&["on", "soon"][..], &["off", "1h"], &["toggle", "25h"], &[]] {
            assert_eq!(Request::parse(wrong), None, "{wrong:?}");
        }
    }

    #[test]
    fn the_inhibitor_blocks_idle_for_the_duration_or_for_good() {
        let held = |length| arguments(length).join(" ");

        assert_eq!(
            held(Some(Duration::from_secs(5400))),
            "--what=idle --who=Kanade --why=Caffeine is on --mode=block \
             sh -c echo held; exec sleep \"$0\" 5400"
        );
        assert!(held(None).ends_with(" infinity"));
    }

    #[test]
    fn a_length_reads_as_it_is_typed() {
        assert_eq!(length(Duration::from_secs(5400)), "1h30m");
        assert_eq!(length(Duration::from_secs(1500)), "25m");
        assert_eq!(length(Duration::from_secs(90)), "1m30s");
        assert_eq!(length(Duration::from_secs(3601)), "1h1s");
    }

    #[test]
    fn caffeine_is_ongoing_until_turned_off() {
        let caffeine = on(3, Some(Duration::from_secs(60)));

        assert_eq!(caffeine.priority(), Priority::Ongoing);
        assert_eq!(caffeine.lifetime(), Lifetime::Persistent);
        assert_eq!(caffeine.scope(), Scope::Global);
        assert_eq!(caffeine.actions()[0].key, OFF);

        let failed = failed(String::from("denied"));

        assert_eq!(failed.id(), caffeine.id());
        assert!(matches!(failed.lifetime(), Lifetime::Transient(_)));
        assert!(failed.actions().is_empty());
    }

    #[test]
    fn a_status_reads_back_as_it_was_said() {
        let statuses = [
            Status {
                on: None,
                starting: None,
                failed: None,
            },
            Status {
                on: Some((3, Some(Duration::from_secs(5400)))),
                starting: Some(4),
                failed: None,
            },
            Status {
                on: Some((3, None)),
                starting: None,
                failed: Some((5, String::from("systemd-inhibit ended (exit status: 1)"))),
            },
        ];

        for status in statuses {
            assert_eq!(Status::parse(&status.to_string()), Some(status.clone()));
        }

        assert_eq!(
            Status::parse("on #3 for 1h30m\nstarting #4").map(|status| status.to_string()),
            Some(String::from("on #3 for 1h30m\nstarting #4"))
        );

        for wrong in ["", "on", "on #x until turned off", "off\nlater #4"] {
            assert_eq!(Status::parse(wrong), None, "{wrong:?}");
        }
    }

    #[test]
    fn an_on_settles_once_it_holds_fails_or_is_lost() {
        let status = Status {
            on: Some((3, Some(Duration::from_secs(90)))),
            starting: Some(4),
            failed: Some((2, String::from("denied"))),
        };

        assert_eq!(status.settled(3), Settled::On(String::from("on for 1m30s")));
        assert_eq!(status.settled(4), Settled::Waiting);
        assert_eq!(status.settled(2), Settled::Failed(String::from("denied")));
        assert!(matches!(status.settled(1), Settled::Lost(_)));

        assert_eq!(starting("starting #4"), Some(4));
        assert_eq!(starting("off"), None);
    }

    fn holding(serial: u64, child: Child) -> Caffeine {
        Caffeine {
            holding: Some(Holding {
                serial,
                length: None,
                child,
            }),
            issued: serial,
            ..Caffeine::default()
        }
    }

    // a Caffeine of its own, so tests run side by side
    fn home(caffeine: Caffeine) -> &'static Mutex<Caffeine> {
        Box::leak(Box::new(Mutex::new(caffeine)))
    }

    fn spawn(script: &str) -> Child {
        Command::new("sh")
            .args(["-c", script])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("sh runs")
    }

    // starts `script` as `serial`, as `on` does
    fn start(home: &'static Mutex<Caffeine>, script: &str, serial: u64, patience: Duration) {
        let mut caffeine = lock(home);

        caffeine.issued = serial;
        caffeine
            .wait_on(home, spawn(script), serial, None, patience)
            .expect("followed");
    }

    // the status of `home` once nothing starts, at most a few seconds on
    fn settled(home: &Mutex<Caffeine>) -> Status {
        let deadline = Instant::now() + Duration::from_secs(5);

        loop {
            let status = lock(home).status();

            if status.starting.is_none() || Instant::now() > deadline {
                return status;
            }

            thread::sleep(Duration::from_millis(20));
        }
    }

    fn turn_off(home: &Mutex<Caffeine>) {
        let mut caffeine = lock(home);

        release(caffeine.starting.take());
        release(caffeine.holding.take());
    }

    #[test]
    fn a_holder_that_runs_out_ended_caffeine_as_asked() {
        let mut caffeine = holding(1, spawn("true"));

        assert_eq!(caffeine.end(1, VecDeque::new()), Some(Ok(())));
        assert!(caffeine.holding.is_none());
    }

    #[test]
    fn a_holder_that_fails_says_why() {
        let mut child = spawn("echo 'Failed to inhibit: Access denied' >&2; exit 1");
        let said = last_lines(child.stderr.take());
        let mut caffeine = holding(1, child);

        let why = caffeine.end(1, said).unwrap().unwrap_err();

        assert!(why.ends_with(": Failed to inhibit: Access denied"), "{why}");
    }

    #[test]
    fn a_stale_end_leaves_the_newer_holder_alone() {
        let mut caffeine = holding(2, spawn("sleep 30"));

        assert_eq!(caffeine.end(1, VecDeque::new()), None);
        assert!(caffeine.holding.is_some());

        release(caffeine.holding.take());
    }

    // the old holder's pid names a process until it is let go of and reaped
    fn running(pid: u32) -> bool {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }

    #[test]
    fn the_old_holder_is_let_go_of_only_once_the_new_one_holds() {
        let old = spawn("exec sleep 30");
        let pid = old.id();
        let home = home(holding(1, old));
        let started = Instant::now();

        start(home, "sleep 0.5; echo held; exec sleep 30", 2, READY);

        // `on` answered at once, with caffeine still held by the old holder
        assert!(started.elapsed() < Duration::from_millis(250));
        thread::sleep(Duration::from_millis(250));
        assert!(running(pid), "the old holder was let go of too early");
        assert_eq!(lock(home).status().settled(2), Settled::Waiting);
        assert_eq!(lock(home).status().on, Some((1, None)));

        let status = settled(home);

        assert!(started.elapsed() >= Duration::from_millis(500));
        assert_eq!(status.on, Some((2, None)));
        assert!(!running(pid));

        turn_off(home);
    }

    #[test]
    fn a_holder_that_never_holds_is_given_up_on_and_the_old_one_kept() {
        let home = home(holding(1, spawn("exec sleep 30")));

        start(home, "exec sleep 30", 2, Duration::from_millis(200));
        let status = settled(home);

        assert_eq!(status.on, Some((1, None)));
        assert!(
            matches!(status.settled(2), Settled::Failed(why) if why.contains("did not hold")),
            "{status}"
        );

        turn_off(home);
    }

    #[test]
    fn a_holder_that_ends_well_before_it_holds_failed() {
        let home = home(holding(1, spawn("exec sleep 30")));

        start(home, "true", 2, READY);
        let status = settled(home);

        assert_eq!(status.on, Some((1, None)));
        assert!(
            matches!(status.settled(2), Settled::Failed(why) if why.ends_with("ended before it held the inhibitor")),
            "{status}"
        );

        turn_off(home);
    }

    #[test]
    fn a_holder_that_fails_before_it_holds_says_why_and_the_old_one_is_kept() {
        let home = home(holding(1, spawn("exec sleep 30")));

        start(
            home,
            "echo 'Failed to inhibit: Access denied' >&2; exit 1",
            2,
            READY,
        );
        let status = settled(home);

        assert_eq!(status.on, Some((1, None)));
        assert!(
            matches!(status.settled(2), Settled::Failed(why) if why.ends_with(": Failed to inhibit: Access denied")),
            "{status}"
        );

        turn_off(home);
    }

    /*
     * holding just as it is given up on, it either turns caffeine on or fails with the old holder
     * kept, never both or neither, and leaves no holder running that nothing follows
     */
    #[test]
    fn a_holder_that_holds_as_it_is_given_up_on_is_one_or_the_other() {
        let patience = Duration::from_millis(200);

        for late in [180, 190, 195, 200, 205, 210, 220] {
            let old = spawn("exec sleep 30");
            let old_pid = old.id();
            let home = home(holding(1, old));

            start(
                home,
                &format!("sleep 0.{late:03}; echo held; exec sleep 30"),
                2,
                patience,
            );
            let new_pid = lock(home)
                .starting
                .as_ref()
                .map(|starting| starting.child.id());
            let status = settled(home);

            match status.on {
                Some((2, _)) => {
                    assert_eq!(status.failed, None, "{late}ms: {status}");
                    assert!(!running(old_pid), "{late}ms: the old holder kept running");
                }
                Some((1, _)) => {
                    assert!(
                        matches!(status.settled(2), Settled::Failed(why) if why.contains("did not hold")),
                        "{late}ms: {status}"
                    );
                    assert!(running(old_pid), "{late}ms: the old holder was let go of");
                    assert!(
                        !running(new_pid.unwrap()),
                        "{late}ms: the new holder kept running"
                    );
                }
                _ => panic!("{late}ms: {status}"),
            }

            turn_off(home);
        }
    }

    #[test]
    fn a_newer_on_takes_the_place_of_one_still_starting() {
        let home = home(Caffeine::default());

        start(home, "exec sleep 30", 1, READY);
        let pid = lock(home)
            .starting
            .as_ref()
            .map(|starting| starting.child.id());
        start(home, "sleep 0.3; echo held; exec sleep 30", 2, READY);

        assert!(!running(pid.unwrap()));

        let status = settled(home);

        assert_eq!(status.on, Some((2, None)));
        assert_eq!(status.failed, None);
        assert!(matches!(status.settled(1), Settled::Lost(_)));

        turn_off(home);
    }

    #[test]
    fn off_lets_go_of_an_on_still_starting() {
        let home = home(Caffeine::default());

        start(home, "sleep 0.3; echo held; exec sleep 30", 1, READY);
        lock(home).turn_off();
        thread::sleep(Duration::from_millis(500));

        let status = lock(home).status();

        assert_eq!(status.on, None);
        assert_eq!(status.starting, None);
        assert!(matches!(status.settled(1), Settled::Lost(_)));
    }
}
