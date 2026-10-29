//! Hooks (ADR 0055): commands the user lists in the config, run on a closed set of events. The
//! Module that owns an event fires it from its own Service watcher or source, so one that is off
//! fires nothing, and so does a `hooks` Module that is off. A fire never waits: the command runs on
//! a thread of its own through the bounded child runner (`appearance::child`), under `setpriv
//! --pdeathsig KILL`, for at most `hooks.timeout`. A hook is never run without that: one that
//! outlived Kanade would be a stray process nothing stops, so a system with no setpriv on its PATH
//! runs none and says so once, as `wake::run_guarded` does for the programs a source starts. Its
//! child gets only what an allow-list names of Kanade's environment, as its own value, so a secret
//! Kanade was started with is not handed to a command the config lists. At most one run of an event
//! is in flight; a fire meanwhile is dropped, so a slow command never piles up. What went wrong is
//! kept for `kanade status` and said once on stderr, never as an Activity. Only event names are
//! logged: a payload, like a wallpaper's path, travels in the child's environment alone.

use std::env;
use std::ffi::{OsStr, OsString};
use std::panic::{self, AssertUnwindSafe};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread;
use std::time::Duration;

use kanade_runtime::MutexExt;
use toml::Value;

use crate::appearance::child::{self, Ran};
use crate::config::{self, Field, Kind, Range, Setting, Tail};
use crate::modules;
use crate::sources::wake;

pub const MODULE: &str = "hooks";

// what each fire of an event gives its command as `KANADE_HOOK`, beside the value it may carry
const EVENT: &str = "KANADE_HOOK";

// the longest argv a hook takes, and the longest argument, so a typo never builds a huge command
const MOST_ARGUMENTS: usize = 32;
const LONGEST_ARGUMENT: usize = 4096;

// how long past `hooks.timeout` a wait for a command lasts, for it to be killed and reaped
const SLACK: Duration = Duration::from_secs(2);

// a session end waits for its hook no longer than this, whatever `hooks.timeout` says, so a hooked
// command that hangs cannot hold a log out, a restart or a power off
const ENDING: Duration = Duration::from_secs(3);

// how much of a failure `status` and stderr say
const BRIEF: usize = 120;

// how much of a hook's stderr is kept: only why it failed, so a flood grows neither memory nor,
// through BRIEF, a status line
const STDERR: usize = 4096;

// why no hook runs at all: the guarded runner is not there, and a hook runs only under it. Said
// once for the shell, not at each fire of each event
const NO_SETPRIV: &str =
    "setpriv is not on PATH, so no hook runs; install util-linux, which provides it";

// whether that was said on stderr already
static SAID_NO_SETPRIV: AtomicBool = AtomicBool::new(false);

// a command may run this long unless `hooks.timeout` says
const DEFAULT_TIMEOUT: u32 = 10;
const TIMEOUT: Range = Range {
    least: 1,
    most: 300,
};

// the closed set of events a command may be listed for
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    // niri holds the lock
    Lock,

    // the lock ended
    Unlock,

    // a wallpaper was set or shown, `KANADE_WALLPAPER` its path
    Wallpaper,

    // `appearance.tone` changed, `KANADE_TONE` "dark" or "light"
    Tone,

    // the charger went in or out, `KANADE_CHARGER` "plugged" or "unplugged"
    Charger,

    // the machine is about to sleep, started before its lock is asked
    Suspend,

    // the machine woke, or a sleep failed
    Resume,

    // the shell started, once every Module that is on has started
    Started,

    // Do Not Disturb went on or off, `KANADE_DND` "on" or "off"
    Dnd,

    // the active power profile changed, `KANADE_POWER_PROFILE` its name
    PowerProfile,

    // a log out, restart or power off is about to be asked of logind, whether it counted down or
    // was asked at once; it is asked once the hook ended or `hooks.timeout` passed
    Logout,
    Reboot,
    Poweroff,
}

const COUNT: usize = 13;

impl Event {
    pub const fn name(self) -> &'static str {
        match self {
            Event::Lock => "lock",
            Event::Unlock => "unlock",
            Event::Wallpaper => "wallpaper",
            Event::Tone => "tone",
            Event::Charger => "charger",
            Event::Suspend => "suspend",
            Event::Resume => "resume",
            Event::Started => "started",
            Event::Dnd => "dnd",
            Event::PowerProfile => "power_profile",
            Event::Logout => "logout",
            Event::Reboot => "reboot",
            Event::Poweroff => "poweroff",
        }
    }

    // whether the event ends the session, so its hook is waited for only briefly
    const fn ends_session(self) -> bool {
        matches!(self, Event::Logout | Event::Reboot | Event::Poweroff)
    }

    // the environment variable the event's value goes in, for the events that have one
    const fn variable(self) -> Option<&'static str> {
        match self {
            Event::Wallpaper => Some("KANADE_WALLPAPER"),
            Event::Tone => Some("KANADE_TONE"),
            Event::Charger => Some("KANADE_CHARGER"),
            Event::Dnd => Some("KANADE_DND"),
            Event::PowerProfile => Some("KANADE_POWER_PROFILE"),
            Event::Lock
            | Event::Unlock
            | Event::Suspend
            | Event::Resume
            | Event::Started
            | Event::Logout
            | Event::Reboot
            | Event::Poweroff => None,
        }
    }

    const ALL: [Event; COUNT] = [
        Event::Lock,
        Event::Unlock,
        Event::Wallpaper,
        Event::Tone,
        Event::Charger,
        Event::Suspend,
        Event::Resume,
        Event::Started,
        Event::Dnd,
        Event::PowerProfile,
        Event::Logout,
        Event::Reboot,
        Event::Poweroff,
    ];
}

// `hooks.*` as the config holds it: each event's argv, empty for none
#[derive(Debug, Clone, PartialEq)]
pub struct Commands {
    argv: [Vec<String>; COUNT],

    // whole seconds
    timeout: u32,
}

impl Default for Commands {
    fn default() -> Self {
        Self {
            argv: Default::default(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl Commands {
    fn of(&self, event: Event) -> &[String] {
        &self.argv[event as usize]
    }

    // how many events a command is listed for
    pub fn configured(&self) -> usize {
        self.argv.iter().filter(|argv| !argv.is_empty()).count()
    }
}

// a command as a config says it: a list of strings, the program first
pub fn argv(value: &Value) -> Result<Vec<String>, String> {
    const EXPECTED: &str =
        "expected a command as a list of strings, like [\"notify-send\", \"hi\"]";

    let list = value.as_array().ok_or(EXPECTED)?;

    if list.len() > MOST_ARGUMENTS {
        return Err(format!(
            "at most {MOST_ARGUMENTS} arguments, found {}",
            list.len()
        ));
    }

    let argv = list
        .iter()
        .map(|each| each.as_str().map(str::to_owned).ok_or(EXPECTED))
        .collect::<Result<Vec<_>, _>>()?;

    if argv.first().is_some_and(String::is_empty) {
        return Err(String::from("the program, the first string, is empty"));
    }

    if argv
        .iter()
        .any(|each| each.len() > LONGEST_ARGUMENT || each.contains('\0'))
    {
        return Err(format!(
            "an argument is longer than {LONGEST_ARGUMENT} bytes or holds a NUL"
        ));
    }

    Ok(argv)
}

macro_rules! hook {
    ($key:literal, $label:literal, $event:ident, $help:literal) => {
        Setting {
            key: $key,
            label: $label,
            help: $help,
            kind: Kind::Argv(Field {
                get: |config| config.hooks.argv[Event::$event as usize].clone(),
                set: |config, argv| config.hooks.argv[Event::$event as usize] = argv,
            }),
            tail: Tail {
                hint: None,
                example: Some("[\"notify-send\", \"Kanade\", \"hook\"]"),
                restart: false,
                per_output: false,
            },
        }
    };
}

/*
 * a command is a program run as the user: `kanade config set` refuses it (`cli::keys`), Settings
 * does not list it, and only a config file holds one
 */
pub const SETTINGS: &[Setting] = &[
    hook!(
        "hooks.lock",
        "On lock",
        Lock,
        "a command run, as a list of strings with no shell, when niri holds the lock; the program is looked up on PATH, a leading ~/ in it is your home, and KANADE_HOOK names the event"
    ),
    hook!(
        "hooks.unlock",
        "On unlock",
        Unlock,
        "a command run when the lock ends"
    ),
    hook!(
        "hooks.wallpaper",
        "On wallpaper",
        Wallpaper,
        "a command run when a wallpaper is set or shown, its path in KANADE_WALLPAPER"
    ),
    hook!(
        "hooks.tone",
        "On tone",
        Tone,
        "a command run when appearance.tone changes, \"dark\" or \"light\" in KANADE_TONE"
    ),
    hook!(
        "hooks.charger",
        "On charger",
        Charger,
        "a command run when the charger goes in or out, \"plugged\" or \"unplugged\" in KANADE_CHARGER"
    ),
    hook!(
        "hooks.suspend",
        "On suspend",
        Suspend,
        "a command run as the machine goes to sleep, before the lock is asked; sleep waits for it only until logind's delay ends"
    ),
    hook!(
        "hooks.resume",
        "On resume",
        Resume,
        "a command run when the machine wakes, or a sleep failed"
    ),
    hook!(
        "hooks.started",
        "On start",
        Started,
        "a command run when the shell has started, once every Module that is on has"
    ),
    hook!(
        "hooks.dnd",
        "On Do Not Disturb",
        Dnd,
        "a command run when Do Not Disturb goes on or off, \"on\" or \"off\" in KANADE_DND"
    ),
    hook!(
        "hooks.power_profile",
        "On power profile",
        PowerProfile,
        "a command run when the power profile changes, its name (power-saver, balanced or performance) in KANADE_POWER_PROFILE; not the one at start"
    ),
    hook!(
        "hooks.logout",
        "On log out",
        Logout,
        "a command run before logind is asked to end the session, after the countdown or at once; the log out waits for it up to hooks.timeout, and at most a few seconds"
    ),
    hook!(
        "hooks.reboot",
        "On restart",
        Reboot,
        "a command run before logind is asked to restart, after the countdown or at once; the restart waits for it up to hooks.timeout, and at most a few seconds"
    ),
    hook!(
        "hooks.poweroff",
        "On power off",
        Poweroff,
        "a command run before logind is asked to power off, after the countdown or at once; the power off waits for it up to hooks.timeout, and at most a few seconds"
    ),
    Setting {
        key: "hooks.timeout",
        label: "Hook timeout",
        help: "a command still running after this is killed; one run of an event at a time, a fire meanwhile is dropped",
        kind: Kind::Seconds(
            TIMEOUT,
            Field {
                get: |config| config.hooks.timeout,
                set: |config, seconds| config.hooks.timeout = seconds,
            },
        ),
        tail: Tail::DEFAULT,
    },
];

// why a run ended without a command having done its work
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    // why, as status says it: the command's own words for a run that started
    why: String,

    // whether Kanade stopped the run before a command could act on it, so `why` is Kanade's own
    // words rather than the command's, which may name what it was given
    refused: bool,
}

impl Failure {
    // a run Kanade did not give a command: no guarded runner, a thread that would not start
    fn refused(why: impl Into<String>) -> Self {
        Self {
            why: why.into(),
            refused: true,
        }
    }

    // a command's own failure: what it said, its timeout, or a panic in its runner
    fn command(why: impl Into<String>) -> Self {
        Self {
            why: why.into(),
            refused: false,
        }
    }
}

// one run of a command, to its end or its timeout
pub trait Runner: Send + Sync + 'static {
    fn run(
        &self,
        event: Event,
        argv: &[String],
        value: Option<&OsStr>,
        timeout: Duration,
    ) -> Result<(), Failure>;
}

// the real one: a child of its own, never the shell's environment changed
struct Process;

impl Runner for Process {
    fn run(
        &self,
        event: Event,
        argv: &[String],
        value: Option<&OsStr>,
        timeout: Duration,
    ) -> Result<(), Failure> {
        // never without `--pdeathsig`: a hook that outlived Kanade is a stray process nothing stops
        if !wake::guards() {
            return Err(Failure::refused(NO_SETPRIV));
        }

        let mut command = Command::new(wake::SETPRIV);
        command
            .args(["--pdeathsig", "KILL"])
            .args(argv)
            .stdin(Stdio::null())
            // only what the allow-list names, so a secret Kanade was started with stays here
            .env_clear()
            .envs(inherited(env::vars_os()))
            .env(EVENT, event.name());

        if let (Some(variable), Some(value)) = (event.variable(), value) {
            command.env(variable, value);
        }

        let name = format!("hook {}", event.name());
        let limits = child::Limits {
            stderr: STDERR,
            ..Default::default()
        };

        match child::run_typed_capped::<()>(command, &name, timeout, limits, || None) {
            Ran::Done(Ok(_)) => Ok(()),
            Ran::Done(Err(error)) => Err(classify(error)),
            Ran::Superseded(()) => Err(Failure::command("superseded")),
        }
    }
}

// whether Kanade's side of the run stopped it before the command could act
fn classify(error: child::Error) -> Failure {
    match error {
        child::Error::Spawn(_) | child::Error::Reader(_) => Failure::refused(error.message()),
        child::Error::Exited(_)
        | child::Error::TooLong(_)
        | child::Error::TimedOut(_)
        | child::Error::Aborted(_) => Failure::command(error.message()),
    }
}

/*
 * the variables a hook's child inherits from Kanade's own environment: what finds its program and
 * its home, the session's locale and runtime directory, and the shell's own `KANADE_*`. Nothing
 * else: a hook is the user's own command, not one handed every secret Kanade was started with
 */
fn inherited(vars: impl Iterator<Item = (OsString, OsString)>) -> Vec<(OsString, OsString)> {
    vars.filter(|(key, _)| allowed(key)).collect()
}

fn allowed(key: &OsStr) -> bool {
    // a name that is not UTF-8 is none of the ones here
    let Some(key) = key.to_str() else {
        return false;
    };

    matches!(key, "PATH" | "HOME" | "LANG" | "XDG_RUNTIME_DIR")
        || key.starts_with("LC_")
        || key.starts_with("KANADE_")
}

// where one event stands
#[derive(Debug, Default)]
struct Slot {
    running: bool,

    // said once on stderr for this run, not at each fire it drops
    said: bool,

    // fires dropped while a run was still going, since start
    dropped: u32,

    // runs that failed in a row, and the last one's brief
    failures: u32,
    last: String,
}

// tells whoever waits that the run ended, by being dropped
pub struct Done(Receiver<()>);

impl Done {
    // whether the run ended within `limit`
    pub fn wait(&self, limit: Duration) -> bool {
        !matches!(self.0.recv_timeout(limit), Err(RecvTimeoutError::Timeout))
    }
}

// the first line of what failed, cut short: the command's own words, which may name what it was given
fn brief(why: &str) -> String {
    let line = why.lines().next().unwrap_or_default().trim();

    match line.char_indices().nth(BRIEF) {
        Some((at, _)) => format!("{}...", &line[..at]),
        None => line.to_owned(),
    }
}

struct Dispatcher<R> {
    runner: Arc<R>,
    slots: Arc<Mutex<[Slot; COUNT]>>,

    // why Kanade last stopped a run short, until one works again
    refused: Arc<Mutex<Option<String>>>,
}

impl<R: Runner> Dispatcher<R> {
    fn new(runner: R) -> Self {
        Self {
            runner: Arc::new(runner),
            slots: Arc::new(Mutex::new(Default::default())),
            refused: Arc::new(Mutex::new(None)),
        }
    }

    /*
     * runs `argv` for `event` on a thread of its own and returns at once; none for no command, or a
     * run of this event still going, which drops this fire
     */
    fn fire(
        &self,
        event: Event,
        argv: &[String],
        value: Option<&OsStr>,
        timeout: Duration,
    ) -> Option<Done> {
        if argv.is_empty() {
            return None;
        }

        {
            let mut slots = self.slots.locked();
            let slot = &mut slots[event as usize];

            if slot.running {
                slot.dropped += 1;

                if !std::mem::replace(&mut slot.said, true) {
                    eprintln!(
                        "kanade: hook {} still runs, so its events meanwhile are dropped",
                        event.name()
                    );
                }

                return None;
            }

            slot.running = true;
            slot.said = false;
        }

        let (tell, done) = mpsc::channel::<()>();
        let runner = self.runner.clone();
        let slots = self.slots.clone();
        let refused = self.refused.clone();
        let argv = argv.to_vec();
        let value = value.map(OsString::from);

        let ran = thread::Builder::new()
            .name(format!("hook {}", event.name()))
            .spawn({
                let slots = slots.clone();
                let refused = refused.clone();

                move || {
                    // dropped last, so a waiter hears the result is kept
                    let _tell = tell;
                    let result = panic::catch_unwind(AssertUnwindSafe(|| {
                        runner.run(event, &argv, value.as_deref(), timeout)
                    }))
                    .unwrap_or_else(|_| Err(Failure::command("the run panicked")));

                    finish(&slots, &refused, event, result);
                }
            });

        match ran {
            Ok(_) => Some(Done(done)),
            Err(error) => {
                finish(
                    &slots,
                    &refused,
                    event,
                    Err(Failure::refused(format!("cannot start a thread: {error}"))),
                );
                None
            }
        }
    }

    // one line for `kanade status`: how many commands the config lists, and why Kanade last stopped
    // a run short if one is unanswered, with none of a command's own words or what it was given
    fn summary(&self, commands: usize) -> String {
        let count = match commands {
            0 => String::from("no commands"),
            1 => String::from("1 command"),
            count => format!("{count} commands"),
        };

        match &*self.refused.locked() {
            Some(why) => format!("hooks: {count} configured; last refusal: {why}"),
            None => format!("hooks: {count} configured"),
        }
    }

    // a line for each event that failed on its last run, and each that dropped fires
    fn status(&self) -> Vec<String> {
        let slots = self.slots.locked();

        Event::ALL
            .iter()
            .flat_map(|&event| {
                let slot = &slots[event as usize];
                let failed = (slot.failures > 0).then(|| {
                    format!(
                        "hooks: {} failed {} in a row, last: {}",
                        event.name(),
                        times(slot.failures),
                        slot.last
                    )
                });
                let dropped = (slot.dropped > 0).then(|| {
                    format!(
                        "hooks: {} dropped {} while it still ran",
                        event.name(),
                        if slot.dropped == 1 {
                            String::from("an event")
                        } else {
                            format!("{} events", slot.dropped)
                        }
                    )
                });

                failed.into_iter().chain(dropped)
            })
            .collect()
    }
}

fn times(count: u32) -> String {
    match count {
        1 => String::from("once"),
        count => format!("{count} times"),
    }
}

// keeps how a run ended, and frees its event for the next
fn finish(
    slots: &Mutex<[Slot; COUNT]>,
    refused: &Mutex<Option<String>>,
    event: Event,
    result: Result<(), Failure>,
) {
    let mut slots = slots.locked();
    let slot = &mut slots[event as usize];

    slot.running = false;

    match result {
        Ok(()) => {
            slot.failures = 0;
            // the last refusal is only what no run since has answered
            *refused.locked() = None;
        }
        Err(failure) => {
            slot.failures += 1;
            slot.last = brief(&failure.why);

            if failure.refused {
                *refused.locked() = Some(slot.last.clone());

                // a refusal no fire can change is the Module's line once, not each event's at
                // each fire; any other is said as it comes
                if failure.why == NO_SETPRIV {
                    if !SAID_NO_SETPRIV.swap(true, Ordering::Relaxed) {
                        eprintln!("kanade: {NO_SETPRIV}");
                    }
                } else {
                    eprintln!("kanade: hook {} refused: {}", event.name(), slot.last);
                }
            } else {
                eprintln!("kanade: hook {} failed: {}", event.name(), slot.last);
            }
        }
    }
}

static DISPATCHER: LazyLock<Dispatcher<Process>> = LazyLock::new(|| Dispatcher::new(Process));

// fires an event that carries nothing
pub fn fire(event: Event) -> Option<Done> {
    send(event, None)
}

// fires an event with its value, which the command reads from the environment
pub fn fire_with(event: Event, value: impl AsRef<OsStr>) -> Option<Done> {
    send(event, Some(value.as_ref()))
}

fn send(event: Event, value: Option<&OsStr>) -> Option<Done> {
    if !modules::on(MODULE) {
        return None;
    }

    let config = config::get();

    DISPATCHER.fire(
        event,
        config.hooks.of(event),
        value,
        Duration::from_secs(u64::from(config.hooks.timeout)),
    )
}

/*
 * fires an event and waits for the command to end, for an action that must not begin before it;
 * the child runner kills it at `hooks.timeout`, so this waits a little past that at most, and at
 * ENDING for a session end. Call it off the draw thread
 */
pub fn fire_and_wait(event: Event) {
    let Some(done) = fire(event) else {
        return;
    };
    let limit = Duration::from_secs(u64::from(config::get().hooks.timeout)) + SLACK;
    let limit = if event.ends_session() {
        limit.min(ENDING)
    } else {
        limit
    };

    if !done.wait(limit) {
        eprintln!("kanade: the {} hook still runs", event.name());
    }
}

// the lines `kanade status` adds: what the hooks are, then each event that went wrong
pub fn status() -> Vec<String> {
    let mut lines = vec![DISPATCHER.summary(config::get().hooks.configured())];
    lines.extend(DISPATCHER.status());
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Sender;

    const SECOND: Duration = Duration::from_secs(1);

    type Began = (Event, Option<OsString>, Duration);

    // a run that ends when told, saying with what it began
    struct Fake {
        began: Mutex<Sender<Began>>,
        end: Mutex<Receiver<Result<(), Failure>>>,
    }

    fn fake() -> (
        Dispatcher<Fake>,
        Receiver<Began>,
        Sender<Result<(), Failure>>,
    ) {
        let (began, saw) = mpsc::channel();
        let (end, ends) = mpsc::channel();
        let fake = Fake {
            began: Mutex::new(began),
            end: Mutex::new(ends),
        };

        (Dispatcher::new(fake), saw, end)
    }

    impl Runner for Fake {
        fn run(
            &self,
            event: Event,
            _: &[String],
            value: Option<&OsStr>,
            timeout: Duration,
        ) -> Result<(), Failure> {
            let _ = self
                .began
                .locked()
                .send((event, value.map(OsString::from), timeout));

            self.end.locked().recv().unwrap_or(Ok(()))
        }
    }

    fn command() -> Vec<String> {
        vec![String::from("true")]
    }

    #[test]
    fn one_run_of_an_event_is_in_flight_and_a_fire_meanwhile_is_dropped() {
        let (hooks, began, end) = fake();
        let argv = command();

        let first = hooks.fire(Event::Wallpaper, &argv, Some("/a.png".as_ref()), 5 * SECOND);
        let first = first.expect("the first fire runs");
        let (event, value, timeout) = began.recv_timeout(10 * SECOND).unwrap();

        assert_eq!(event, Event::Wallpaper);
        assert_eq!(value.as_deref(), Some(OsStr::new("/a.png")));
        assert_eq!(timeout, 5 * SECOND);

        // while it runs the same event is dropped, however often, but another event runs
        assert!(hooks.fire(Event::Wallpaper, &argv, None, SECOND).is_none());
        assert!(hooks.fire(Event::Wallpaper, &argv, None, SECOND).is_none());
        assert!(!first.wait(Duration::from_millis(50)));
        let other = hooks.fire(Event::Lock, &argv, None, SECOND);
        assert!(other.is_some());
        assert_eq!(began.recv_timeout(10 * SECOND).unwrap().0, Event::Lock);

        assert_eq!(
            hooks.status(),
            ["hooks: wallpaper dropped 2 events while it still ran"]
        );

        end.send(Ok(())).unwrap();
        end.send(Ok(())).unwrap();
        assert!(first.wait(10 * SECOND));
        assert!(other.unwrap().wait(10 * SECOND));

        // free again
        assert!(hooks.fire(Event::Wallpaper, &argv, None, SECOND).is_some());
        end.send(Ok(())).unwrap();
    }

    #[test]
    fn no_command_runs_nothing_and_a_failure_is_kept_until_a_run_works() {
        let (hooks, began, end) = fake();

        assert!(hooks.fire(Event::Tone, &[], None, SECOND).is_none());
        assert!(began.try_recv().is_err());

        end.send(Err(Failure::command("exited with exit status: 3\nmore")))
            .unwrap();
        let done = hooks.fire(Event::Tone, &command(), None, SECOND).unwrap();
        assert!(done.wait(10 * SECOND));
        assert_eq!(
            hooks.status(),
            ["hooks: tone failed once in a row, last: exited with exit status: 3"]
        );

        end.send(Err(Failure::command("x".repeat(500)))).unwrap();
        let done = hooks.fire(Event::Tone, &command(), None, SECOND).unwrap();
        assert!(done.wait(10 * SECOND));
        assert_eq!(
            hooks.status(),
            [format!(
                "hooks: tone failed 2 times in a row, last: {}...",
                "x".repeat(BRIEF)
            )]
        );

        end.send(Ok(())).unwrap();
        let done = hooks.fire(Event::Tone, &command(), None, SECOND).unwrap();
        assert!(done.wait(10 * SECOND));
        assert!(hooks.status().is_empty());
    }

    // `kanade status` says how many commands are listed, and the last refusal no run has answered,
    // with none of a command's own words or what it was given
    #[test]
    fn the_summary_says_how_many_commands_and_the_last_refusal() {
        let (hooks, _began, end) = fake();

        assert_eq!(hooks.summary(0), "hooks: no commands configured");
        assert_eq!(hooks.summary(2), "hooks: 2 commands configured");

        end.send(Err(Failure::refused(NO_SETPRIV))).unwrap();
        let done = hooks.fire(Event::Tone, &command(), None, SECOND).unwrap();
        assert!(done.wait(10 * SECOND));
        assert_eq!(
            hooks.summary(2),
            format!("hooks: 2 commands configured; last refusal: {NO_SETPRIV}")
        );

        // a run that worked answers it
        end.send(Ok(())).unwrap();
        let done = hooks.fire(Event::Tone, &command(), None, SECOND).unwrap();
        assert!(done.wait(10 * SECOND));
        assert_eq!(hooks.summary(2), "hooks: 2 commands configured");
    }

    // a hook's child gets what the allow-list names and nothing else Kanade holds
    #[test]
    fn a_hook_inherits_only_the_listed_variables() {
        let vars = [
            ("PATH", "/usr/bin"),
            ("HOME", "/home/someone"),
            ("LANG", "en_GB.UTF-8"),
            ("LC_ALL", "C"),
            ("LC_TIME", "en_GB.UTF-8"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("KANADE_FRAMES", "1"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
            ("SSH_AUTH_SOCK", "/run/user/1000/ssh"),
            ("WAYLAND_DISPLAY", "wayland-0"),
        ];

        let kept: Vec<String> = inherited(
            vars.iter()
                .map(|(key, value)| (OsString::from(key), OsString::from(value))),
        )
        .into_iter()
        .map(|(key, _)| key.to_string_lossy().into_owned())
        .collect();

        assert_eq!(
            kept,
            [
                "PATH",
                "HOME",
                "LANG",
                "LC_ALL",
                "LC_TIME",
                "XDG_RUNTIME_DIR",
                "KANADE_FRAMES"
            ]
        );
    }

    // a hook whose stderr floods is read only up to the cap, so no flood grows memory or a status
    // line, and one that writes more still ends cleanly
    #[test]
    fn a_hooks_stderr_is_read_only_up_to_the_cap() {
        let failure = Process
            .run(
                Event::Lock,
                &[
                    String::from("sh"),
                    String::from("-c"),
                    String::from("head -c 1000000 /dev/zero | tr '\\0' x >&2; exit 1"),
                ],
                None,
                10 * SECOND,
            )
            .unwrap_err();

        assert!(!failure.refused, "the command ran and failed");
        assert_eq!(failure.why.len(), STDERR, "{}", failure.why.len());
        assert!(brief(&failure.why).len() <= BRIEF + 3);
    }

    // the real runner: the event's value reaches the child alone, and a child past its timeout is killed
    #[test]
    fn a_child_gets_its_value_in_its_own_environment_and_is_killed_at_the_timeout() {
        let shell = |script: &str| vec![String::from("sh"), String::from("-c"), script.to_owned()];

        assert!(
            Process
                .run(
                    Event::Charger,
                    &shell("test \"$KANADE_HOOK $KANADE_CHARGER\" = 'charger plugged'"),
                    Some("plugged".as_ref()),
                    5 * SECOND,
                )
                .is_ok()
        );
        assert!(std::env::var_os("KANADE_CHARGER").is_none());

        let started = std::time::Instant::now();
        let why = Process
            .run(
                Event::Lock,
                &shell("sleep 30"),
                None,
                Duration::from_millis(200),
            )
            .unwrap_err()
            .why;

        assert!(why.contains("timed out"), "{why}");
        assert!(started.elapsed() < 10 * SECOND);
    }

    // a hook that leaves a background process holding its pipes still ends with it, not with that process
    #[test]
    fn a_hook_that_leaves_a_background_process_does_not_hold_its_slot() {
        let started = std::time::Instant::now();

        Process
            .run(
                Event::Lock,
                &[
                    String::from("sh"),
                    String::from("-c"),
                    String::from("sleep 5 & exit 0"),
                ],
                None,
                5 * SECOND,
            )
            .unwrap();

        assert!(started.elapsed() < 3 * SECOND);
    }

    #[test]
    fn a_command_is_a_short_list_of_strings_with_a_program() {
        let read = |text: &str| argv(&format!("v = {text}").parse::<toml::Table>().unwrap()["v"]);

        assert_eq!(read("[]").unwrap(), Vec::<String>::new());
        assert_eq!(read("[\"a\", \"\"]").unwrap(), ["a", ""]);
        assert!(read("\"a b\"").is_err());
        assert!(read("[\"\"]").is_err());
        assert!(read("[1]").is_err());
        assert!(read(&format!("[{}]", "\"a\",".repeat(MOST_ARGUMENTS + 1))).is_err());
        assert!(read("[\"a\\u0000\"]").is_err());
    }
}
