//! The Outputs section's worker and the state the page draws (see `outputs`). One thread asks
//! niri, runs the trial's `niri msg output` commands and writes the config, so no press waits on
//! them; its results land in the `Settings` Service, which redraws the window. It also draws the
//! clock of a trial: every quarter second it counts down, and it reverts when the time is out or
//! when the page or the window the trial was made in is gone. A second thread, the trial's own,
//! fires that same retry at the trial's deadline even when the first is dead or blocked, so a
//! worker that panicked cannot leave a trial with no timer.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SendError, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::service::Service;
use kanade_runtime::{MutexExt, TextInput};

use super::{
    Controller, Edit, Ended, Field, Kept, Output, Port, RunError, Vrr, disagreements, parse_outputs,
};
use crate::appearance::child;
use crate::niri_config::{self, Editor, OUTPUT_KEYS, Reading};
use crate::settings::niri::{self as page, PAGE};
use crate::settings::{Settings, WINDOW};
use crate::sources::niri as ipc;

// how long niri gets to apply a change before the page asks what it applied
const SETTLE: Duration = Duration::from_millis(300);

// how long to wait for niri to reload the config after a Keep, in TICK steps
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

// how long to wait for niri to reload the config after a Reset (which has no ConfigLoaded check)
const RELOAD: Duration = Duration::from_millis(700);

// how often the page asks niri for the outputs while it shows, which is how it sees a monitor
// plugged in or out, or changed by something else
const POLL: Duration = Duration::from_secs(2);

// how often the trial's clock looks at a trial: whole seconds of countdown are shown, and a page
// left is noticed, within it
const TICK: Duration = Duration::from_millis(250);

// how long a window asked for is given to open before it counts as closed
const GRACE: Duration = Duration::from_secs(5);

// the text inputs of an output: x and y of the position, and the scale
const ALL_AXES: [char; 3] = ['x', 'y', 's'];

/// What the config says of one monitor.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    // the name a block for it would have
    pub block: String,

    // each key of `OUTPUT_KEYS`, as the engine reads it for the block niri uses
    pub readings: BTreeMap<&'static str, Reading>,

    // when more than one block matches the monitor
    pub note: Option<String>,

    // who owns it, if an include does: no control acts then
    pub owner: Option<String>,

    // whether the config's own block sets any key, so Reset has something to remove
    pub set: bool,

    // what the variable refresh rate key asks for (on demand is seen nowhere else)
    pub vrr: Option<Vrr>,
}

/// A connected monitor and the config's word on it.
#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    pub output: Output,
    pub config: Config,
}

/// The trial the controller has, as the page shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Trial {
    pub id: u64,
    pub output: String,
    pub edits: BTreeMap<Field, Edit>,

    // whole seconds left to Keep
    pub seconds: u64,

    // why the last revert failed
    pub stuck: Option<String>,
}

/// What the section knows, kept in the `Settings` Service.
#[derive(Default)]
pub struct State {
    // whether niri has been asked since the window opened
    pub loaded: bool,

    // why niri could not be asked
    pub error: Option<String>,

    // why the config cannot be changed here; the outputs can still be tried
    pub blocked: Option<String>,
    pub monitors: Vec<Monitor>,
    pub trial: Option<Trial>,

    // the last thing that happened, and whether it warns
    pub notice: Option<(String, bool)>,

    // the output whose mode list is open
    pub mode_open: Option<String>,

    // counts what may need a text input set again: a change in what niri shows, a window opening,
    // a resync. The countdown of a trial is none of them, or typing would be wiped each second
    pub generation: u64,

    // the inputs to set again whatever they hold, because a change of theirs just ended; each is
    // stamped with the `epoch` it was asked at
    pub resync: BTreeMap<(String, char), u64>,
    pub epoch: u64,
}

impl State {
    pub fn monitor(&self, name: &str) -> Option<&Monitor> {
        self.monitors
            .iter()
            .find(|monitor| monitor.output.name == name)
    }

    /// The footer's words while a trial runs: the countdown, or why a change was refused.
    pub fn countdown(&self) -> Option<(String, bool)> {
        let trial = self.trial.as_ref()?;

        if let Some(why) = &trial.stuck {
            return Some((
                format!(
                    "Could not put {} back: {why}. Trying again; Revert tries now",
                    trial.output
                ),
                true,
            ));
        }

        if let Some((text, true)) = &self.notice {
            return Some((text.clone(), true));
        }

        Some((
            format!(
                "Keep these changes to {}? It goes back in {} s",
                trial.output, trial.seconds
            ),
            false,
        ))
    }

    /// A window opening starts over.
    pub fn reopen(&mut self) {
        self.notice = None;
        self.mode_open = None;
        self.generation += 1;
    }

    fn publish(&mut self, monitors: Vec<Monitor>, blocked: Option<String>, error: Option<String>) {
        let changed = !self.loaded
            || self.monitors != monitors
            || self.blocked != blocked
            || self.error != error;

        self.loaded = true;

        if changed {
            self.monitors = monitors;
            self.blocked = blocked;
            self.error = error;
            self.generation += 1;
        }
    }

    // the countdown moves every second; no input is set again for it
    pub(super) fn set_trial(&mut self, trial: Option<Trial>) {
        self.trial = trial;
    }

    /// An output's inputs, `axes` of them, show what niri has again: a change of theirs ended,
    /// kept, reverted or refused, so what is typed there is spent.
    pub fn resync(&mut self, output: &str, axes: &[char]) {
        self.epoch += 1;
        self.generation += 1;

        for &axis in axes {
            self.resync.insert((output.to_owned(), axis), self.epoch);
        }
    }
}

// ---- the real world ----

struct Real;

impl Port for Real {
    fn outputs(&self) -> Result<Vec<Output>, String> {
        ipc::ask("\"Outputs\"")
            .map(|reply| parse_outputs(&reply))
            .map_err(|error| format!("niri could not be asked for its outputs: {error}"))
    }

    fn run(&self, output: &str, args: &[String]) -> Result<(), RunError> {
        let mut command = Command::new("niri");
        command
            .args(["msg", "output", output])
            .args(args)
            .stdin(Stdio::null());

        // about the 2 s niri is given for a question (the PATIENCE shape in sources/niri.rs),
        // through the bounded child runner so a stalled command never blocks the worker thread
        let mut timeout = Duration::from_secs(2);

        if let Some(deadline) = *EXIT_DEADLINE.locked() {
            let now = Instant::now();
            if now >= deadline {
                return Err(RunError::Unanswered(String::from(
                    "revert on exit exceeded deadline",
                )));
            }
            let remaining = deadline.saturating_duration_since(now);
            timeout = remaining.min(Duration::from_millis(800));
        }

        match child::run_typed::<()>(command, "niri msg output", timeout, || None) {
            child::Ran::Done(Ok(_)) => Ok(()),
            child::Ran::Done(Err(error)) => Err(run_error(error)),
            child::Ran::Superseded(()) => unreachable!(),
        }
    }

    fn write(&self, output: &Output, edits: &[&Edit]) -> Result<(String, Option<String>), String> {
        let editor = editor()?;
        let ops: Vec<_> = edits.iter().flat_map(|edit| edit.ops()).collect();
        let said = editor.edit_output(&output.id(), &ops)?;

        Ok((said, editor.last_change_hash().ok()))
    }

    fn undo_write(&self) -> Result<String, String> {
        editor()?.undo()
    }

    fn now(&self) -> u64 {
        static START: OnceLock<Instant> = OnceLock::new();

        START.get_or_init(Instant::now).elapsed().as_millis() as u64
    }

    fn unix_ms(&self) -> u64 {
        niri_config::now_ms()
    }
}

// the engine, on the config this shell edits
fn editor() -> Result<Editor<'static>, String> {
    Ok(Editor {
        file: niri_config::target(None)?,
        state: niri_config::state()?,
        validate: &niri_config::niri_validate,
        now_ms: niri_config::now_ms,
    })
}

// the one trial; the worker holds it for a call at a time, and the exit takes it the same way
static CONTROL: Mutex<Option<Controller>> = Mutex::new(None);

pub(super) fn controller<R>(call: impl FnOnce(&mut Controller) -> R) -> R {
    let mut control = CONTROL.locked();
    let control =
        control.get_or_insert_with(|| Controller::new(Controller::file().unwrap_or_default()));

    call(control)
}

// child::run_typed's own failures mean niri may have applied the change, whether it never started,
// was cut short, said too much or its reader died; only a non-zero exit is niri's own refusal
pub(super) fn run_error(error: child::Error) -> RunError {
    if error.unanswered() {
        RunError::Unanswered(error.message())
    } else {
        RunError::Refused(error.message())
    }
}

// ---- the jobs ----

pub(super) enum Job {
    Refresh,
    Edit { output: String, edit: Edit },
    Keep { id: u64, requested_at: u64 },
    Revert(&'static str),
    Reset(String),
}

pub(super) static WORKER: Mutex<Option<Sender<Job>>> = Mutex::new(None);

fn post(job: Job) {
    let mut worker = WORKER.locked();
    let mut job = Some(job);

    // a worker that died is started again once
    for _ in 0..2 {
        if worker.is_none() {
            *worker = spawn();
        }
        let (Some(sender), Some(posted)) = (worker.as_ref(), job.take()) else {
            break;
        };

        match sender.send(posted) {
            Ok(()) => return,
            Err(SendError(back)) => {
                job = Some(back);
                *worker = None;
            }
        }
    }
    drop(worker);

    Settings::write().outputs.notice = Some((
        String::from("could not start the thread that changes the outputs"),
        true,
    ));
}

fn spawn() -> Option<Sender<Job>> {
    let (sender, receiver) = mpsc::channel();

    start_clock();

    thread::Builder::new()
        .name(String::from("kanade-outputs"))
        .spawn(move || {
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(&receiver);
            }));
            if let Err(payload) = res {
                handle_worker_panic(&Real, &*payload);
                *WORKER.locked() = None;
            }
        })
        .ok()
        .map(|_| sender)
}

// the deadline of a trial, and the retries of a revert that failed, on a thread of its own: the
// worker draws the countdown and reverts when the page is left, but it panics, and it sits in
// `niri msg output` for up to 2 s, and a trial whose deadline died with it would stay on the
// monitor with nothing counting. The clock takes the trial through `controller`, as the worker
// does, and never begins one: it only ever ends a trial that is already there.
static CLOCK: Mutex<Option<Sender<u64>>> = Mutex::new(None);
static CLOCK_STARTED: OnceLock<()> = OnceLock::new();

// tells the clock that the trial began, changed or ended, so it counts again
fn wake_clock() {
    if let Some(woken) = CLOCK.locked().as_ref() {
        let _ = woken.send(CLOCK_GENERATION.fetch_add(1, Ordering::Relaxed));
    }
}

// how many times the clock has been told a trial changed; a clock that saw a different number
// while it was reverting knows what it did is not the last word, and counts again
static CLOCK_GENERATION: AtomicU64 = AtomicU64::new(0);

fn start_clock() {
    CLOCK_STARTED.get_or_init(|| {
        let (woken, woke) = mpsc::channel();
        *CLOCK.locked() = Some(woken);

        crate::supervise::spawn("kanade-outputs-clock", move || clock(&woke, &Real));
    });
}

// one turn of the clock: sleeps until the trial needs it, then ends the trial through `Port`.
// `woken` carries a tick per change of the trial, so a trial begun, extended or ended while the
// clock sleeps is counted from where it is now rather than from the deadline it slept for.
pub(super) fn clock(woken: &Receiver<u64>, port: &dyn Port) {
    loop {
        let wait = match controller(|control| control.wait(port.now())) {
            Some(due) => Duration::from_millis(due.max(1)),
            None => {
                // no trial niri has to be put back from: the worker wakes this thread when that
                // changes. The config holding a trial is no trial to time: the reload decides.
                if woken.recv().is_err() {
                    return;
                }

                continue;
            }
        };

        // a page left is the worker's to notice while it runs: the clock only counts down to the
        // trial's own deadline, and the worker wakes it if the trial changes first
        let seen = CLOCK_GENERATION.load(Ordering::Relaxed);

        match woken.recv_timeout(wait) {
            // its time is out: the worker may be dead or in niri, so the trial is ended here,
            // through the same controller the worker uses
            Err(RecvTimeoutError::Timeout) => {}

            // the trial began, changed, or ended: count from where it is now
            Ok(_) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        }

        clock_due(port);

        // the worker changed the trial while the clock was ending it: what it did may be stale,
        // so it looks again at once instead of sleeping for the deadline that was
        if CLOCK_GENERATION.load(Ordering::Relaxed) != seen {
            continue;
        }
    }
}

// ends the trial the clock waited for, through the controller the worker uses. `gone` is the page
// or the window left, which the clock cannot see: only the worker can, and it is the one that
// normally notices it. A trial the config holds (`Session::kept`) is not one the clock ends.
pub(super) fn clock_due(port: &dyn Port) {
    let ended = controller(|control| {
        let output = control.session.as_ref()?.output.clone();
        let result = control.revert_when_due(port, false)?;

        Some((output, result, control.session.is_none()))
    });

    let Some((output, result, over)) = ended else {
        return;
    };

    ended_trial(result, "Reverted: no answer in time");

    // the page shows what niri has again, as the worker's own tick does
    if over {
        Settings::write().outputs.resync(&output, &ALL_AXES);
    }

    show_trial();
}

/// Asks niri for the outputs again, off the draw thread.
pub fn refresh() {
    post(Job::Refresh);
}
/// Tries a change to an output in niri's memory.
pub fn edit(output: &str, edit: Edit) {
    post(Job::Edit {
        output: output.to_owned(),
        edit,
    });
}

/// Writes the trial's changes to the config.
pub fn keep() {
    let target = Settings::read()
        .outputs
        .trial
        .as_ref()
        .map(|trial| (trial.id, Real.now()))
        .or_else(|| controller(|c| c.session.as_ref().map(|s| (s.id, Real.now()))))
        .unwrap_or((0, Real.now()));
    post(Job::Keep {
        id: target.0,
        requested_at: target.1,
    });
}

/// Puts the output back as it was before the trial.
pub fn revert() {
    post(Job::Revert("Reverted"));
}

/// Removes the keys Kanade sets from the monitor's block (and the block, if nothing else is in it).
pub fn reset(output: &str) {
    post(Job::Reset(output.to_owned()));
}

const EXIT_REVERT_TIMEOUT: Duration = Duration::from_millis(4_000);
static EXIT_DEADLINE: Mutex<Option<Instant>> = Mutex::new(None);

/// Puts the output back when Kanade exits with a trial unfinished. Waits for the commands. A trial
/// the config holds (a Keep written, its reload not confirmed yet) is not put back: the user kept
/// it, and the config says so.
pub fn revert_on_exit() {
    let deadline = Instant::now() + EXIT_REVERT_TIMEOUT;
    *EXIT_DEADLINE.locked() = Some(deadline);
    let _ = controller(|control| control.revert_unkept(&Real));
    *EXIT_DEADLINE.locked() = None;
}

/// At start: puts back what a Kanade that died mid-trial left. niri may not be up yet, so it asks
/// a few times.
pub fn recover_at_start() {
    let Some(file) = Controller::file().filter(|file| file.exists()) else {
        return;
    };

    let _ = thread::Builder::new()
        .name(String::from("kanade-outputs-recover"))
        .spawn(move || {
            for attempt in 0..5 {
                let result = controller(|control| {
                    // the shell's own controller may hold a trial already: leave it be
                    if control.session.is_some() {
                        return Ok(None);
                    }

                    control.recover(&Real)
                });

                match result {
                    Ok(Some(said)) => return eprintln!("outputs: {said}"),
                    Ok(None) => return,
                    Err(why) if attempt == 4 => {
                        return eprintln!("outputs: {why}; the file {} stays", file.display());
                    }
                    Err(_) => thread::sleep(Duration::from_secs(2)),
                }
            }
        });
}

fn run(receiver: &Receiver<Job>) {
    let mut asked = Instant::now();
    let mut last_job = Instant::now();

    // whether the window was seen open since the last job; a window asked for opens a moment later
    let mut opened = false;

    // whether the page and the window are gone, so nothing needs watching
    let mut idle = false;

    loop {
        let session = controller(|control| control.session.is_some());
        let wait = if session {
            Some(TICK)
        } else if idle {
            None
        } else {
            Some(Duration::from_secs(1))
        };

        let job = match wait {
            Some(wait) => match receiver.recv_timeout(wait) {
                Ok(job) => Some(job),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match receiver.recv() {
                Ok(job) => Some(job),
                Err(_) => return,
            },
        };

        if let Some(job) = job {
            opened = false;
            last_job = Instant::now();

            let handle_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                handle(job);
            }));
            if let Err(payload) = handle_res {
                handle_worker_panic(&Real, &*payload);
            }
            asked = Instant::now();

            // a press may have begun, extended or ended a trial: its clock counts from there
            wake_clock();
        }

        let open = kanade_runtime::window_open(WINDOW);
        opened |= open;

        let gone = {
            let settings = Settings::read();

            settings.page != PAGE
                || settings.searching()
                || (!open && (opened || last_job.elapsed() >= GRACE))
        };

        let tick_res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tick(gone);
        }));
        if let Err(payload) = tick_res {
            handle_worker_panic(&Real, &*payload);
        }
        idle = gone;

        if !gone && asked.elapsed() >= POLL {
            asked = Instant::now();
            publish();
        }
    }
}

// the clock of a trial: reverts when its time is out or the page it was made on is gone, and
// shows the seconds left
fn tick(gone: bool) {
    let ended = controller(|control| {
        let output = control.session.as_ref()?.output.clone();
        let said = if gone {
            "Reverted: the page was left"
        } else {
            "Reverted: no answer in time"
        };
        let result = control.revert_when_due(&Real, gone)?;

        // a revert niri refused leaves the trial, and is tried again in 5 s
        Some((output, result, said, control.session.is_none()))
    });

    match ended {
        Some((output, result, said, over)) => {
            ended_trial(result, said);
            thread::sleep(SETTLE);
            publish();

            // only when it is over: a retry would wipe what is typed each time
            if over {
                Settings::write().outputs.resync(&output, &ALL_AXES);
            }
        }
        None => show_trial(),
    }
}

pub(super) fn edit_if_editable<T>(
    output: &str,
    monitor: Option<&Monitor>,
    editable: impl FnOnce(&niri_config::OutputId) -> Result<(), String>,
    edit: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let id = monitor
        .map(|monitor| monitor.output.id())
        .unwrap_or_else(|| niri_config::OutputId::named(output));

    editable(&id)?;
    edit()
}

fn handle(job: Job) {
    match job {
        Job::Refresh => publish(),
        Job::Edit { output, edit } => {
            let axes: Option<&[char]> = match edit {
                Edit::Position(..) => Some(&['x', 'y']),
                Edit::Scale(_) => Some(&['s']),
                _ => None,
            };
            let hint = Settings::read()
                .outputs
                .monitor(&output)
                .and_then(|monitor| monitor.config.vrr);
            let monitor = Settings::read().outputs.monitor(&output).cloned();
            let result = edit_if_editable(
                &output,
                monitor.as_ref(),
                niri_config::output_editable,
                || controller(|control| control.try_edit(&Real, &output, edit, hint)),
            );

            {
                let mut settings = Settings::write();

                match result {
                    Ok(()) => settings.outputs.notice = None,
                    Err(why) => settings.outputs.notice = Some((why, true)),
                }
            }

            // what niri applied, which may not be what was asked
            thread::sleep(SETTLE);
            publish();

            // and the field the change came from shows it, applied, rounded or refused
            if let Some(axes) = axes {
                Settings::write().outputs.resync(&output, axes);
            }
        }
        Job::Keep { id, requested_at } => {
            let edits = controller(|control| {
                control
                    .session
                    .as_ref()
                    .filter(|s| s.id == id)
                    .map(|session| {
                        (
                            session.output.clone(),
                            session.edits.values().cloned().collect::<Vec<_>>(),
                        )
                    })
            });

            // snapshot the config-load counter before writing, so the worker can tell
            // when niri has reloaded (same thread, no race with the write)
            let before = ipc::config_load_state();

            let result = controller(|control| control.keep_request(&Real, id, requested_at));

            match result {
                Ok(Ended::Kept(said, after)) => {
                    Settings::write().outputs.notice = Some((format!("Saved: {said}"), false));

                    // the page's Undo is this change and no other: the hash it left, and the file
                    // as it left it, so the read the Keep posts does not take it away
                    page::State::set_undoable(after);
                    page::load();

                    // wait for niri to reload the config (ConfigLoaded on the event stream),
                    // up to about 2 s in TICK steps so a superseding job is still noticed
                    let deadline = Instant::now() + CONFIRM_TIMEOUT;
                    let mut loaded = None;

                    while Instant::now() < deadline {
                        thread::sleep(TICK);
                        let now = ipc::config_load_state();
                        if now.count != before.count {
                            loaded = Some(now);
                            break;
                        }
                    }

                    publish();

                    // `edits` was read from the session before `keep`, which returns
                    // Err when there is no session, so reaching Kept means edits is Some.
                    let (name, edits) = edits.as_ref().unwrap();
                    let kept = controller(|control| control.resolve_keep_reload(&Real, loaded));

                    match kept {
                        Kept::Undone(reverted) => {
                            Settings::write().outputs.notice = Some(match reverted {
                                Ok(_) => (
                                    format!(
                                        "niri rejected the saved config; \
                                         pre-trial config restored and {name} put back"
                                    ),
                                    true,
                                ),
                                Err(why) => (
                                    format!(
                                        "niri rejected the saved config; \
                                         pre-trial config restored, but {name} is still as it was: {why}"
                                    ),
                                    true,
                                ),
                            });
                        }
                        Kept::NotUndone(why_undo, reverted) => {
                            Settings::write().outputs.notice = Some(match reverted {
                                Ok(()) => (
                                    format!(
                                        "niri rejected the saved config and restoring pre-trial config failed ({why_undo}); \
                                         {name} put back"
                                    ),
                                    true,
                                ),
                                Err(why_rev) => (
                                    format!(
                                        "niri rejected the saved config, restoring pre-trial config failed ({why_undo}), \
                                         and {name} is still as it was: {why_rev}"
                                    ),
                                    true,
                                ),
                            });
                        }
                        Kept::Confirmed => {
                            // disagreements (a scale niri rounded) are purely informational
                            check_disagreements(name, edits);
                        }
                        Kept::Unconfirmed => {
                            // the trial ended but the pending file stays, marked saved, so a later
                            // start clears it without putting the monitor back over the change;
                            // the monitor already holds what the config now says
                            unconfirmed(name, edits);
                        }
                    }
                }
                Ok(_) => {}
                Err(why) => {
                    Settings::write().outputs.notice =
                        Some((format!("Not saved, nothing was written: {why}"), true));
                    publish();
                }
            }

            // the trial is over whichever way: what niri has is what the fields show
            if let Some((name, _)) = &edits {
                Settings::write().outputs.resync(name, &ALL_AXES);
            }
        }
        Job::Revert(said) => {
            let output = controller(|control| control.session.as_ref().map(|s| s.output.clone()));
            let result = controller(|control| control.revert(&Real));

            let over = controller(|control| control.session.is_none());

            if output.is_some() {
                ended_trial(result, said);
            }
            thread::sleep(SETTLE);
            publish();

            if let Some(output) = output.filter(|_| over) {
                Settings::write().outputs.resync(&output, &ALL_AXES);
            }
        }
        Job::Reset(name) => reset_block(&name),
    }
}

// what became of a revert
fn ended_trial(result: Result<Ended, String>, said: &str) {
    let mut settings = Settings::write();

    settings.outputs.notice = match result {
        Ok(Ended::LeftOn(left)) => Some((format!("{said}; {left}"), true)),
        Ok(_) => Some((said.to_owned(), false)),
        Err(why) => Some((format!("Could not put the output back: {why}"), true)),
    };
}

pub(super) fn handle_worker_panic(port: &dyn Port, payload: &(dyn std::any::Any + Send)) {
    crate::supervise::panicked("kanade-outputs", payload);
    // a trial the config holds is the reload's, not this revert's
    controller(|control| {
        let _ = control.revert_unkept(port);
    });

    // a trial this took a revert of, or one the panic left behind because niri refused it, is the
    // clock's now: it reverts at the deadline even though this worker is going
    wake_clock();
}

// the Keep's answer when niri never said: the file stays, marked saved, as a record of a change the
// config holds; what niri shows is compared with it once, for the footer's word
fn unconfirmed(name: &str, edits: &[Edit]) {
    match Real.outputs() {
        Err(why) => {
            Settings::write().outputs.notice = Some((
                format!("Saved, but niri reload timed out and outputs could not be checked: {why}"),
                true,
            ));
        }
        Ok(outputs) => {
            let Some(output) = outputs.iter().find(|output| output.name == name) else {
                Settings::write().outputs.notice = Some((
                    format!("Saved, but niri reload timed out and {name} is no longer connected"),
                    true,
                ));

                return;
            };
            let refs: Vec<&Edit> = edits.iter().collect();
            let said = disagreements(&refs, output);
            let text = if said.is_empty() {
                String::from("Saved, but niri reload timed out; changes may not have reloaded")
            } else {
                format!(
                    "Saved, but niri reload timed out and niri differs: {}",
                    said.join("; ")
                )
            };

            Settings::write().outputs.notice = Some((text, true));
        }
    }
}

// checks where niri shows something other than what was written; posts a notice if so.
// returns true when there are disagreements
fn check_disagreements(name: &str, edits: &[Edit]) -> bool {
    let Ok(outputs) = Real.outputs() else {
        return false;
    };
    let Some(output) = outputs.iter().find(|output| output.name == name) else {
        return false;
    };
    let refs: Vec<&Edit> = edits.iter().collect();
    let said = disagreements(&refs, output);

    if !said.is_empty() {
        Settings::write().outputs.notice = Some((
            format!("Saved, but niri differs: {}", said.join("; ")),
            true,
        ));
        true
    } else {
        false
    }
}

fn reset_block(name: &str) {
    if controller(|control| control.session.is_some()) {
        Settings::write().outputs.notice = Some((
            String::from("Keep or revert the change in progress first"),
            true,
        ));

        return;
    }

    let Some(id) = Settings::read()
        .outputs
        .monitor(name)
        .map(|monitor| monitor.output.id())
    else {
        return;
    };
    let result = niri_config::target(None).and_then(|file| {
        let editor = Editor {
            file,
            state: niri_config::state()?,
            validate: &niri_config::niri_validate,
            now_ms: niri_config::now_ms,
        };

        editor.reset_output(&id, OUTPUT_KEYS)
    });

    match result {
        Ok(said) => {
            Settings::write().outputs.notice = Some((said, false));

            // the page's Undo is this reset and no other, as for a Keep
            page::State::set_undoable(
                editor()
                    .ok()
                    .and_then(|editor| editor.last_change_hash().ok()),
            );
            page::load();
            thread::sleep(RELOAD);
            publish();
        }
        Err(why) => Settings::write().outputs.notice = Some((why, true)),
    }
}

// the trial as the page shows it
fn show_trial() {
    let trial = controller(|control| {
        let now = Real.now();

        control.session.as_ref().map(|session| Trial {
            id: session.id,
            output: session.output.clone(),
            edits: session.edits.clone(),
            seconds: control.remaining(now).unwrap_or(0).div_ceil(1000),
            stuck: session.stuck.clone(),
        })
    });

    if Settings::read().outputs.trial != trial {
        Settings::write().outputs.set_trial(trial);
    }
}

// asks niri for the outputs and the config what it says of each, and writes what changed
fn publish() {
    let (monitors, blocked, error) = match Real.outputs() {
        Ok(outputs) => {
            let (monitors, blocked) = read_config(outputs);

            (monitors, blocked, None)
        }
        Err(why) => (Vec::new(), None, Some(why)),
    };

    show_trial();
    Settings::write().outputs.publish(monitors, blocked, error);
}

pub(super) fn read_config(outputs: Vec<Output>) -> (Vec<Monitor>, Option<String>) {
    let setup = niri_config::target(None).and_then(|file| Ok((file, niri_config::state()?)));
    let (file, state) = match setup {
        Ok(setup) => setup,
        Err(why) => return (without_config(outputs), Some(why)),
    };
    let editor = Editor {
        file,
        state,
        validate: &niri_config::niri_validate,
        now_ms: niri_config::now_ms,
    };
    let blocked = editor.editable().err();
    let mut monitors = Vec::new();

    for output in outputs {
        let id = output.id();
        let mut config = Config {
            block: id.block_name(),
            ..Config::default()
        };
        let mut failed = None;

        for &key in OUTPUT_KEYS {
            match editor.read_output(&id, key) {
                Ok(read) => {
                    config.note = config.note.or(read.note);

                    if let Some(owner) = &read.block_owner {
                        config.owner = config.owner.or_else(|| Some(page::owner_text(owner)));
                    }

                    match &read.reading {
                        Reading::Own(_) => config.set = true,
                        Reading::Included(_, file) | Reading::Overridden(_, file) => {
                            config.owner = config.owner.or_else(|| Some(page::owner_text(file)));
                        }
                        Reading::Unset => {}
                    }

                    if key == "variable-refresh-rate" {
                        config.vrr = match &read.reading {
                            Reading::Unset => None,
                            Reading::Own(shown)
                            | Reading::Included(shown, _)
                            | Reading::Overridden(shown, _) => {
                                Some(if shown.contains("on-demand=true") {
                                    Vrr::OnDemand
                                } else {
                                    Vrr::On
                                })
                            }
                        };
                    }

                    config.readings.insert(key, read.reading);
                }
                Err(why) => {
                    failed = Some(why);
                    break;
                }
            }
        }

        if let Some(why) = failed {
            config.owner = Some(format!("niri's config cannot be read: {why}"));
        }

        monitors.push(Monitor { output, config });
    }

    (monitors, blocked)
}

// outputs shown with no config reading, as when the config cannot be found: nothing is editable
fn without_config(outputs: Vec<Output>) -> Vec<Monitor> {
    outputs
        .into_iter()
        .map(|output| Monitor {
            config: Config {
                block: output.id().block_name(),
                ..Config::default()
            },
            output,
        })
        .collect()
}

// ---- the text inputs ----

thread_local! {
    // the text input of each axis of a position, by output and axis
    static INPUTS: RefCell<HashMap<(String, char), &'static str>> = RefCell::new(HashMap::new());

    // what each holds, as typed
    static TYPED: RefCell<HashMap<&'static str, String>> = RefCell::new(HashMap::new());

    // what each was last set from
    static SYNCED: RefCell<HashMap<&'static str, String>> = RefCell::new(HashMap::new());

    // the newest resync of each that was acted on
    static FORCED: RefCell<HashMap<&'static str, u64>> = RefCell::new(HashMap::new());

    static LOOKED: std::cell::Cell<u64> = const { std::cell::Cell::new(u64::MAX) };
}

/// The id of the text input of an output's `x` or `y` position, or its scale (`s`).
pub fn input(output: &str, axis: char) -> &'static str {
    INPUTS.with_borrow_mut(|inputs| {
        *inputs.entry((output.to_owned(), axis)).or_insert_with(|| {
            Box::leak(format!("settings outputs {output} {axis}").into_boxed_str())
        })
    })
}

/// Records what a position input holds, as it is typed.
pub fn typed(id: &'static str, text: String) {
    TYPED.with_borrow_mut(|typed| typed.insert(id, text));
}

/// The position typed in an output's two inputs.
pub fn typed_position(output: &str) -> Result<(i32, i32), String> {
    let read = |axis: char| {
        let id = input(output, axis);
        let text = TYPED
            .with_borrow(|typed| typed.get(id).cloned())
            .unwrap_or_default();

        text.trim()
            .parse::<i32>()
            .map_err(|_| format!("Position {axis}: expected a whole number, as -1920 or 0"))
    };

    Ok((read('x')?, read('y')?))
}

/// Whether an input is set to what niri shows. `typed` is what it holds, `synced` what it was last
/// set to, `forced` that a change of its own just ended. Text the user typed and has not applied
/// (`typed` other than `synced`) is theirs until then: a poll or a countdown does not take it.
pub(super) fn should_set(
    niri: &str,
    synced: Option<&str>,
    typed: Option<&str>,
    forced: bool,
) -> bool {
    if forced {
        return synced != Some(niri) || typed != Some(niri);
    }

    let unapplied = typed.is_some() && typed != synced;

    synced != Some(niri) && !unapplied
}

/// Sets each position and scale input to what niri shows when that has changed or a change of its
/// own just ended, and not over text being typed. On the draw thread, which the inputs live on;
/// the view calls it before it builds them.
pub fn sync_inputs(state: &State) {
    if LOOKED.get() == state.generation {
        return;
    }
    LOOKED.set(state.generation);

    for monitor in &state.monitors {
        let name = &monitor.output.name;
        let (x, y) = monitor
            .output
            .logical
            .map_or((0, 0), |logical| (logical.x, logical.y));
        let scale = monitor.output.logical.map_or(1.0, |logical| logical.scale);

        for (axis, text) in [
            ('x', x.to_string()),
            ('y', y.to_string()),
            ('s', page::format_number(scale, 4)),
        ] {
            let id = input(name, axis);
            let asked = state
                .resync
                .get(&(name.clone(), axis))
                .copied()
                .unwrap_or(0);
            let forced = FORCED.with_borrow_mut(|seen| {
                let seen = seen.entry(id).or_insert(0);
                let forced = asked > *seen;

                *seen = asked;
                forced
            });
            let typed_text = TYPED.with_borrow(|typed| typed.get(id).cloned());
            let synced_text = SYNCED.with_borrow(|synced| synced.get(id).cloned());

            if should_set(&text, synced_text.as_deref(), typed_text.as_deref(), forced) {
                TextInput::set_text(id, &text);
                SYNCED.with_borrow_mut(|synced| synced.insert(id, text.clone()));
                typed(id, text);
            }
        }
    }
}
