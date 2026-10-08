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

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read};
use std::process::{Child, ChildStderr, ChildStdout, ExitStatus};
use std::sync::mpsc::{self, Sender};
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

// how long `on` waits for that on the draw thread; logind answers in milliseconds
const READY: Duration = Duration::from_secs(2);

static CAFFEINE: Mutex<Caffeine> = Mutex::new(Caffeine {
    holding: None,
    issued: 0,
});

struct Caffeine {
    // the inhibitor's holder, none while caffeine is off
    holding: Option<Holding>,

    /*
     * the serials given out since Kanade started, never given again, so a serial names one
     * holder: a stale Turn off or end never mistakes a newer one
     */
    issued: u64,
}

struct Holding {
    serial: u64,

    // reaped only once taken out of `holding`, so while there its pid is its own
    child: Child,
}

// what `kanade caffeine` asks; a duration ends it on its own, none holds it until turned off
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    On(Option<Duration>),
    Off,
    Toggle(Option<Duration>),
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
            _ => None,
        }
    }
}

fn lock() -> MutexGuard<'static, Caffeine> {
    CAFFEINE.lock().unwrap_or_else(PoisonError::into_inner)
}

/*
 * does what `request` asks and says how caffeine is left. Call it from the draw thread, which
 * lives as long as Kanade: systemd-inhibit dies with the thread that started it
 */
pub fn request(request: Request) -> Result<String, String> {
    let mut caffeine = lock();

    match request {
        Request::Off => {
            turn_off(&mut caffeine);
            Ok(String::from("off"))
        }
        Request::Toggle(_) if caffeine.holding.is_some() => {
            turn_off(&mut caffeine);
            Ok(String::from("off"))
        }
        Request::On(length) | Request::Toggle(length) => turn_on(&mut caffeine, length),
    }
}

/*
 * holds a new inhibitor, then lets go of the old one, if on, so caffeine never lapses; on again
 * starts its duration over. A new one that fails leaves the old one on
 */
fn turn_on(caffeine: &mut Caffeine, length: Option<Duration>) -> Result<String, String> {
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

    caffeine.issued += 1;
    let serial = caffeine.issued;

    if let Err(why) = swap(caffeine, child, serial, READY) {
        // a keybind has no reply to read
        if caffeine.holding.is_none() {
            IslandService::write().post(failed(why.clone()), Instant::now());
        }

        return Err(why);
    }

    // under the lock, so its end, which takes it, posts after
    IslandService::write().post(on(serial, length), Instant::now());

    Ok(match length {
        Some(length) => format!("on for {}", self::length(length)),
        None => String::from("on until turned off"),
    })
}

/*
 * makes `child`, as `serial`, the holder once it says it holds the inhibitor, letting go of the old
 * one only then; kills it instead when it does not say so within `patience`
 */
fn swap(
    caffeine: &mut Caffeine,
    mut child: Child,
    serial: u64,
    patience: Duration,
) -> Result<(), String> {
    let (held, said) = (child.stdout.take(), child.stderr.take());
    let (ready, readiness) = mpsc::channel();

    let followed = thread::Builder::new()
        .name(String::from("caffeine"))
        .spawn(move || follow(held, said, serial, ready));

    if let Err(error) = followed {
        drop(child.kill());
        drop(child.wait());

        return Err(format!("cannot follow {INHIBIT}: {error}"));
    }

    match readiness.recv_timeout(patience) {
        Ok(Ok(())) => {
            release(caffeine.holding.replace(Holding { serial, child }));
            Ok(())
        }
        // ending at all before it held, it failed, even as `sleep` would end
        Ok(Err(said)) => ended(child.wait(), said)
            .and_then(|()| Err(format!("{INHIBIT} ended before it held the inhibitor"))),
        Err(_) => {
            drop(child.kill());
            drop(child.wait());

            Err(format!(
                "{INHIBIT} did not hold the inhibitor within {patience:?}"
            ))
        }
    }
}

fn turn_off(caffeine: &mut Caffeine) {
    if caffeine.holding.is_some() {
        release(caffeine.holding.take());
        IslandService::write().withdraw(&id(), Instant::now());
    }
}

// logind lets go of the inhibitor once its holder dies, and the holder's command dies with it
fn release(holding: Option<Holding>) {
    if let Some(mut holding) = holding {
        drop(holding.child.kill());
        drop(holding.child.wait());
    }
}

// turns off the caffeine that `serial` names, as its Activity's Turn off does
pub fn act(key: &str, serial: &str) {
    let mut caffeine = lock();

    if key == OFF
        && let Ok(serial) = serial.parse::<u64>()
        && caffeine
            .holding
            .as_ref()
            .is_some_and(|holding| holding.serial == serial)
    {
        turn_off(&mut caffeine);
    }
}

/*
 * tells `ready` once the holder says it holds the inhibitor, or what it printed before it ended
 * without. Then reads what it prints until it exits, and takes the Activity away, or shows why
 * caffeine failed
 */
fn follow(
    held: Option<ChildStdout>,
    said: Option<ChildStderr>,
    serial: u64,
    ready: Sender<Result<(), VecDeque<String>>>,
) {
    if !holds(held) {
        drop(ready.send(Err(last_lines(said))));
        return;
    }

    // none listens once it was given up on, and its end below finds it let go of
    drop(ready.send(Ok(())));

    let said = last_lines(said);
    let mut caffeine = lock();

    // under the lock, so a newer holder's start posts after
    match end(&mut caffeine, serial, said) {
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

/*
 * reaps the holder `serial` names, done printing `said`, and whether it ran out as asked or why
 * not; none once it was let go of, or another took its place
 */
fn end(caffeine: &mut Caffeine, serial: u64, said: VecDeque<String>) -> Option<Result<(), String>> {
    let mut holding = caffeine
        .holding
        .take_if(|holding| holding.serial == serial)?;

    let ended = ended(holding.child.wait(), said);

    if let Err(why) = &ended {
        eprintln!("caffeine: turned off on its own: {why}");
    }

    Some(ended)
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

    fn holding(serial: u64, child: Child) -> Caffeine {
        Caffeine {
            holding: Some(Holding { serial, child }),
            issued: serial,
        }
    }

    fn spawn(script: &str) -> Child {
        Command::new("sh")
            .args(["-c", script])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("sh runs")
    }

    #[test]
    fn a_holder_that_runs_out_ended_caffeine_as_asked() {
        let mut caffeine = holding(1, spawn("true"));

        assert_eq!(end(&mut caffeine, 1, VecDeque::new()), Some(Ok(())));
        assert!(caffeine.holding.is_none());
    }

    #[test]
    fn a_holder_that_fails_says_why() {
        let mut child = spawn("echo 'Failed to inhibit: Access denied' >&2; exit 1");
        let said = last_lines(child.stderr.take());
        let mut caffeine = holding(1, child);

        let why = end(&mut caffeine, 1, said).unwrap().unwrap_err();

        assert!(why.ends_with(": Failed to inhibit: Access denied"), "{why}");
    }

    #[test]
    fn a_stale_end_leaves_the_newer_holder_alone() {
        let mut caffeine = holding(2, spawn("sleep 30"));

        assert_eq!(end(&mut caffeine, 1, VecDeque::new()), None);
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
        let mut caffeine = holding(1, old);
        let started = Instant::now();

        let swapped = thread::scope(|scope| {
            let swapping = scope.spawn(|| {
                swap(
                    &mut caffeine,
                    spawn("sleep 0.5; echo held; exec sleep 30"),
                    2,
                    READY,
                )
            });

            thread::sleep(Duration::from_millis(250));
            assert!(running(pid), "the old holder was let go of too early");

            swapping.join().unwrap()
        });

        assert_eq!(swapped, Ok(()));
        assert!(started.elapsed() >= Duration::from_millis(500));
        assert!(!running(pid));
        assert_eq!(
            caffeine.holding.as_ref().map(|holding| holding.serial),
            Some(2)
        );

        release(caffeine.holding.take());
    }

    #[test]
    fn a_holder_that_never_holds_is_given_up_on_and_the_old_one_kept() {
        let mut caffeine = holding(1, spawn("exec sleep 30"));

        let why = swap(
            &mut caffeine,
            spawn("exec sleep 30"),
            2,
            Duration::from_millis(200),
        )
        .unwrap_err();

        assert!(why.contains("did not hold the inhibitor"), "{why}");
        assert_eq!(
            caffeine.holding.as_ref().map(|holding| holding.serial),
            Some(1)
        );

        release(caffeine.holding.take());
    }

    #[test]
    fn a_holder_that_ends_well_before_it_holds_failed() {
        let mut caffeine = holding(1, spawn("exec sleep 30"));

        let why = swap(&mut caffeine, spawn("true"), 2, READY).unwrap_err();

        assert!(why.ends_with("ended before it held the inhibitor"), "{why}");
        assert_eq!(
            caffeine.holding.as_ref().map(|holding| holding.serial),
            Some(1)
        );

        release(caffeine.holding.take());
    }

    #[test]
    fn a_holder_that_fails_before_it_holds_says_why_and_the_old_one_is_kept() {
        let mut caffeine = holding(1, spawn("exec sleep 30"));

        let why = swap(
            &mut caffeine,
            spawn("echo 'Failed to inhibit: Access denied' >&2; exit 1"),
            2,
            READY,
        )
        .unwrap_err();

        assert!(why.ends_with(": Failed to inhibit: Access denied"), "{why}");
        assert_eq!(
            caffeine.holding.as_ref().map(|holding| holding.serial),
            Some(1)
        );

        release(caffeine.holding.take());
    }
}
