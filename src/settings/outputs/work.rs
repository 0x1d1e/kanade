//! The Outputs section's worker and the state the page draws (see `outputs`). One thread asks
//! niri, runs the trial's `niri msg output` commands and writes the config, so no press waits on
//! them; its results land in the `Settings` Service, which redraws the window. It also keeps the
//! clock of a trial: every quarter second it counts down, and it reverts when the time is out or
//! when the page or the window the trial was made in is gone.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SendError, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::service::Service;
use kanade_runtime::{MutexExt, TextInput};

use super::{Controller, Edit, Ended, Field, Output, Port, Vrr, disagreements, parse_outputs};
use crate::niri_config::{self, Editor, OUTPUT_KEYS, Reading};
use crate::settings::niri::{self as page, PAGE};
use crate::settings::{Settings, WINDOW};
use crate::sources::niri as ipc;

// how long niri gets to apply a change before the page asks what it applied
const SETTLE: Duration = Duration::from_millis(300);

// and to reload the config after a write, which it does when it sees the file change
const RELOAD: Duration = Duration::from_millis(700);

// how often the page asks niri for the outputs while it shows, which is how it sees a monitor
// plugged in or out, or changed by something else
const POLL: Duration = Duration::from_secs(2);

// how often the worker looks at the clock of a trial
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

    fn run(&self, output: &str, args: &[String]) -> Result<(), String> {
        let done = Command::new("niri")
            .args(["msg", "output", output])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("niri msg could not run: {error}"))?;

        if done.status.success() {
            return Ok(());
        }

        let said = String::from_utf8_lossy(&done.stderr);
        let said = said.trim().trim_start_matches("Error: ").trim();

        Err(if said.is_empty() {
            format!("niri did not accept `{}`", args.join(" "))
        } else {
            said.to_owned()
        })
    }

    fn write(&self, output: &Output, edits: &[&Edit]) -> Result<String, String> {
        let file = niri_config::target(None)?;
        let editor = Editor {
            file,
            state: niri_config::state()?,
            validate: &niri_config::niri_validate,
            now_ms: niri_config::now_ms,
        };
        let ops: Vec<_> = edits.iter().flat_map(|edit| edit.ops()).collect();

        editor.edit_output(&output.id(), &ops)
    }

    fn now(&self) -> u64 {
        static START: OnceLock<Instant> = OnceLock::new();

        START.get_or_init(Instant::now).elapsed().as_millis() as u64
    }

    fn unix_ms(&self) -> u64 {
        niri_config::now_ms()
    }
}

// the one trial; the worker holds it for a call at a time, and the exit takes it the same way
static CONTROL: Mutex<Option<Controller>> = Mutex::new(None);

fn controller<R>(call: impl FnOnce(&mut Controller) -> R) -> R {
    let mut control = CONTROL.locked();
    let control =
        control.get_or_insert_with(|| Controller::new(Controller::file().unwrap_or_default()));

    call(control)
}

// ---- the jobs ----

enum Job {
    Refresh,
    Edit { output: String, edit: Edit },
    Keep,
    Revert(&'static str),
    Reset(String),
}

static WORKER: Mutex<Option<Sender<Job>>> = Mutex::new(None);

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

    thread::Builder::new()
        .name(String::from("kanade-outputs"))
        .spawn(move || run(&receiver))
        .ok()
        .map(|_| sender)
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
    post(Job::Keep);
}

/// Puts the output back as it was before the trial.
pub fn revert() {
    post(Job::Revert("Reverted"));
}

/// Removes the keys Kanade sets from the monitor's block (and the block, if nothing else is in it).
pub fn reset(output: &str) {
    post(Job::Reset(output.to_owned()));
}

/// Puts the output back when Kanade exits with a trial unfinished. Waits for the commands.
pub fn revert_on_exit() {
    let _ = controller(|control| control.revert(&Real));
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

            handle(job);
            asked = Instant::now();
        }

        let open = kanade_runtime::window_open(WINDOW);
        opened |= open;

        let gone = {
            let settings = Settings::read();

            settings.page != PAGE
                || settings.searching()
                || (!open && (opened || last_job.elapsed() >= GRACE))
        };

        tick(gone);
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
            let result = controller(|control| control.try_edit(&Real, &output, edit, hint));

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
        Job::Keep => {
            let edits = controller(|control| {
                control.session.as_ref().map(|session| {
                    (
                        session.output.clone(),
                        session.edits.values().cloned().collect::<Vec<_>>(),
                    )
                })
            });
            let result = controller(|control| control.keep(&Real));

            match result {
                Ok(Ended::Kept(said)) => {
                    {
                        let mut settings = Settings::write();

                        settings.outputs.notice = Some((format!("Saved: {said}"), false));
                        settings.niri.undoable = true;
                    }
                    page::load();

                    // niri reloads the config on its own; then the two can be compared
                    thread::sleep(RELOAD);
                    publish();

                    if let Some((name, edits)) = &edits {
                        compare(name, edits);
                    }
                }
                Ok(_) => {}
                Err(why) => {
                    Settings::write().outputs.notice =
                        Some((format!("Not saved, nothing was written: {why}"), true));
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

// says once where niri shows something other than what was written
fn compare(name: &str, edits: &[Edit]) {
    let Ok(outputs) = Real.outputs() else {
        return;
    };
    let Some(output) = outputs.iter().find(|output| output.name == name) else {
        return;
    };
    let refs: Vec<&Edit> = edits.iter().collect();
    let said = disagreements(&refs, output);

    if !said.is_empty() {
        Settings::write().outputs.notice = Some((
            format!("Saved, but niri differs: {}", said.join("; ")),
            true,
        ));
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
            {
                let mut settings = Settings::write();

                settings.outputs.notice = Some((said, false));
                settings.niri.undoable = true;
            }
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

fn read_config(outputs: Vec<Output>) -> (Vec<Monitor>, Option<String>) {
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
