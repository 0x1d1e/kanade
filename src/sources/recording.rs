//! Screen recording (docs/design.md Capture, ADR 0013, ADR 0116, ADR 0117): `kanade capture record
//! start` records the focused output or an area with wf-recorder, or a window with niri's screen
//! cast (`window`), encoded on the GPU through VA-API or NVENC, else on the CPU, with no sound, the speakers' or the
//! microphone's, to `~/Videos/Screencasts`; `pause` and `resume` end
//! the recorder's file and start another, `stop` saves the lot as one file, and `status` says how
//! the recording stands. wf-recorder captures through niri's wlr-screencopy, which niri counts as a
//! cast, so the privacy cluster shows it, and the microphone, without asking this Module. While it
//! records, a Persistent Ongoing Recording Activity offers Pause and Stop; once saved, a Transient
//! one copies its path or opens it.
//!
//! niri hands a recorder a frame only once the screen changes, and on a still screen that is often
//! the island the draw thread, which answers IPC, would draw, so neither `start` nor `stop` waits on
//! the recorder: each is answered later (`Later`), `start` once the file has its first frame and
//! `stop` once it is saved.
//!
//! wf-recorder is a holder (ADR 0011): one recording runs while it does. The first segment is written
//! to the recording's own file, so one that is never paused is there from its first frame; segments
//! after a pause live in a hidden folder beside it, joined with the first at the end. `machine` moves
//! the recording between states; the programs it runs (the recorder, the area picker, the joiner)
//! all start from one worker thread, as a child started through `setpriv --pdeathsig` dies with the
//! thread that started it. A window is recorded by two programs and a niri session (`window`), but
//! is one segment to the machine, whose end is the file's writer ending.

mod encode;
mod machine;
mod plan;
mod probe;
mod window;

use std::collections::VecDeque;
use std::fmt;
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::os::fd::OwnedFd;
use std::os::unix::process::ExitStatusExt;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ExitStatus};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::{MutexExt, service::Service};
use rustix::fs::{CWD, RenameFlags, renameat_with};
use rustix::io::Errno;

use self::encode::QUIET;
use self::machine::{Effect, Event, Machine, Phase, Pick, Segment};
pub use self::plan::{Audio, ENCODER, JOINER, PICKER, RECORDER, Rect};
use self::plan::{MUXER, Source};
use super::capture::{self, COPY, SHOW, SHOWN};
use super::json::Json;
use super::niri::{self, Heard};
use super::wake::{self, Failed, NOT_FOUND};
use crate::clock;
use crate::config;
use crate::island::activity::{
    Action, Activity, Clip, Detail, Id, Interrupt, Kind, Lifetime, Priority, Scope,
};
use crate::island::presentation::Surface;
use crate::island::service::IslandService;
use crate::later::{LATER, Later};
use crate::lock;

// the recording's actions
pub const STOP: &str = "stop";
pub const PAUSE: &str = "pause";
pub const RESUME: &str = "resume";

// the last lines the recorder printed, which say why it ended
const SAID: usize = 4;

// how often a `start` looks for the first frame, which niri sends only when the screen changes
const LOOK: Duration = Duration::from_millis(50);

// how long the joiner has to put the segments together, which only copies them
const JOINING: Duration = Duration::from_secs(600);

// where recordings are saved, under the home
const FOLDER: &str = "Videos/Screencasts";

static RUNTIME: Mutex<Runtime> = Mutex::new(Runtime::new());

/*
 * whether a recording is in any state but none, which the lock reads without taking `RUNTIME`:
 * the lock's hook runs under the lock's own mutex, which `lock::holds` (asked before `RUNTIME`
 * is taken, never under it) takes too
 */
static ACTIVE: AtomicBool = AtomicBool::new(false);

struct Runtime {
    machine: Machine,

    // the recording the machine is in, none while it is idle
    take: Option<Take>,

    // what runs for the segment, reaped only once taken out, so while here its pids are its own
    child: Option<Held>,

    // the area picker's pid while it runs, for the one who ends it; reaped by the worker
    picker: Option<u32>,

    // the `kanade` calls waiting on something of the recording
    waiting: Vec<(Wait, Later)>,

    // the serial of a segment whose encoder a stop killed after it did not end of itself, so a
    // writer that only found the stream short is not reported as one that failed
    escalated: Option<u64>,

    // the last recording that ended, until another starts
    ended: Option<Ended>,

    /*
     * every path a recording was given since Kanade started, never given again, so a path names
     * one recording: a stale Stop or `kanade` waiting on an older one never mistakes a newer one
     */
    issued: Vec<String>,
}

// what runs for one segment
struct Held {
    serial: u64,

    // the one that writes the file, whose end is the segment's
    writer: Child,

    // a window's encoder, which feeds the writer and is interrupted in its place
    encoder: Option<Child>,

    // a window's niri session, stopped by whoever takes this out, on every path
    cast: Option<window::Cast>,

    // the encoder's reader, kept with the segment it read so its verdict outlives the join: a
    // descendant holding the pipe can keep the reader past `SAID_JOINED`
    said: Option<Arc<Reading>>,
}

// what a waiting call waits for
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wait {
    // the pause
    Paused,

    // the end of the recording at the latest
    End,
}

// one recording, from its start to its end
struct Take {
    // the file it is saved to, which is also where the first segment is written
    path: String,

    // the hidden folder beside it that holds the segments after the first, and all once joined
    parts: PathBuf,

    // none until the area is chosen
    source: Option<Source>,
    audio: Audio,

    // whether it can pause: the joiner is there
    pausable: bool,

    // the window it records closed, which ended it
    closed: bool,

    // why it records with the software encoder when the one chosen did not start (ADR 0116); none
    // while it has not had to
    software: Option<String>,

    // a window's frames could not be downloaded through the VA-API post-processor, which failed
    // before a frame was written (a node that encodes a test frame may not import the GPU niri casts
    // from): the rest of the recording downloads through EGL, which the trial of a chain tells
    no_va: bool,

    // the recording settings as it began: a reload of the config between a pause and the resume does
    // not change the encoder, quality or device of what is one file
    recording: config::Recording,
}

impl Take {
    // where segment `number` is written: the first to the file itself, the rest to the parts folder
    fn segment(&self, number: u32) -> PathBuf {
        match number {
            1 => PathBuf::from(&self.path),
            _ => self.parts.join(plan::segment(number)),
        }
    }
}

struct Ended {
    path: String,

    // the window it recorded closed
    closed: bool,

    // none once saved, else why nothing was
    lost: Option<String>,
}

// what the worker thread is asked
enum Job {
    Pick { serial: u64 },
    Spawn { serial: u64, segment: u32 },
    Join { segments: u32 },
}

impl Runtime {
    const fn new() -> Runtime {
        Runtime {
            machine: Machine::new(),
            take: None,
            child: None,
            picker: None,
            waiting: Vec::new(),
            escalated: None,
            ended: None,
            issued: Vec::new(),
        }
    }

    // hands the machine `event` and does what it says; its refusal, if any
    fn feed(&mut self, event: Event) -> Result<(), String> {
        let step = self.machine.step(event);

        ACTIVE.store(self.machine.busy(), Ordering::Release);

        if let Some(why) = step.refused {
            return Err(why);
        }

        for effect in step.effects {
            self.perform(effect);
        }

        Ok(())
    }

    fn perform(&mut self, effect: Effect) {
        match effect {
            Effect::Pick(serial) => self.hand(Job::Pick { serial }, |why| Event::Picked {
                serial,
                pick: Pick::Failed(why),
            }),
            Effect::Cancel => {
                if let Some(pid) = self.picker {
                    // SIGTERM: the picker inherits Kanade's ignored SIGINT
                    drop(clock::terminate_pid(pid));

                    // a picker that ignores SIGTERM would hold the worker, and every later job
                    escalate_picker(pid);
                }
            }
            Effect::Spawn { serial, segment } => {
                if let Some(take) = &self.take {
                    post(running(take, false));
                }

                // a new segment's encoder is not the one a stop escalated
                self.escalated = None;

                self.hand(Job::Spawn { serial, segment }, |why| Event::Spawned {
                    serial,
                    started: Err(why),
                });
            }
            Effect::Interrupt => {
                if let Some(held) = &self.child {
                    if let Err(error) =
                        clock::interrupt(held.encoder.as_ref().unwrap_or(&held.writer))
                    {
                        eprintln!("capture: cannot stop the recorder: {error}");
                    }

                    // a window's encoder that does not end is killed, whatever the writer waits on
                    if held.encoder.is_some() {
                        self.escalated = Some(held.serial);
                        escalate(held.serial);
                    }
                }
            }
            Effect::Join { segments } => {
                // one segment is the file already; joining more takes a while, which is shown
                if segments > 1
                    && let Some(path) = self.take.as_ref().map(|take| take.path.clone())
                {
                    if let Some(take) = &self.take {
                        post(saving(take));
                    }

                    /*
                     * the file is there already, so a call waiting for it is answered now rather
                     * than after a join that can outlast the CLI's patience. A failed join still
                     * refuses whoever waits after it (there is nobody left by then)
                     */
                    for later in self.answer(Wait::End) {
                        later.done(path.clone());
                    }
                }

                self.hand(Job::Join { segments }, |why| Event::Joined(Err(why)))
            }
            Effect::Paused => {
                if let Some(take) = &self.take {
                    post(running(take, true));
                }

                for later in self.answer(Wait::Paused) {
                    later.done(self.path());
                }
            }
            Effect::Declined(why) => {
                eprintln!("capture: cannot resume the recording: {why}");

                // the window closed while paused: what there is of it is saved, not left paused
                let closed = self.take.as_ref().is_some_and(|take| take.closed);

                if let Some(take) = &self.take
                    && !closed
                {
                    post(running(take, true));
                }

                for later in self
                    .answer(Wait::Paused)
                    .into_iter()
                    .chain(self.answer(Wait::End))
                {
                    later.refused(why.clone());
                }

                if closed {
                    drop(self.feed(Event::Stop));
                }
            }
            Effect::Left(why) => {
                self.take = None;

                for later in self
                    .answer(Wait::Paused)
                    .into_iter()
                    .chain(self.answer(Wait::End))
                {
                    later.refused(why.clone());
                }
            }
            Effect::Failed(why) => self.failed(why),
            Effect::Over(result) => self.over(result),
        }
    }

    // gives the worker `job`, or makes `failed` of why it cannot
    fn hand(&mut self, job: Job, failed: impl FnOnce(String) -> Event) {
        if let Err(why) = ask(job) {
            drop(self.feed(failed(why)));
        }
    }

    fn path(&self) -> String {
        self.take
            .as_ref()
            .map(|take| take.path.clone())
            .unwrap_or_default()
    }

    /*
     * the wait for a segment's first frame starts once the recorder runs, not when the call
     * arrived: the probe can take seconds before the recorder is even started, and a call answered
     * unknown while the frame is still coming says nothing of what happened
     */
    fn postpone(&self) {
        for (waited, later) in &self.waiting {
            if *waited == Wait::End {
                later.rearm();
            }
        }
    }

    // the calls waiting for `wait`, no longer waiting
    fn answer(&mut self, wait: Wait) -> Vec<Later> {
        let (answered, waiting) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|(waited, _)| *waited == wait);

        self.waiting = waiting;
        answered.into_iter().map(|(_, later)| later).collect()
    }

    // the recording never began, why: said to whoever asked and shown, as nothing else would
    fn failed(&mut self, why: String) {
        eprintln!("capture: cannot record: {why}");
        self.take = None;

        for later in self
            .answer(Wait::Paused)
            .into_iter()
            .chain(self.answer(Wait::End))
        {
            later.refused(why.clone());
        }

        self.ended = Some(Ended {
            path: String::new(),
            closed: false,
            lost: Some(why.clone()),
        });
        post(ended(
            Clip::Failed {
                path: String::new(),
                why,
            },
            true,
        ));
    }

    // the recording is over, saved or not: its calls are answered and its end shown
    fn over(&mut self, result: Result<(), String>) {
        let Some(take) = self.take.take() else {
            return;
        };

        let lost = result
            .err()
            .map(|why| kept(Path::new(&take.path), &take.parts, why));

        if let Some(why) = &lost {
            eprintln!("capture: the recording ended: {why}");
        }

        for later in self
            .answer(Wait::Paused)
            .into_iter()
            .chain(self.answer(Wait::End))
        {
            match &lost {
                None => later.done(take.path.clone()),
                Some(why) => later.refused(why.clone()),
            }
        }

        let clip = match &lost {
            None => Clip::Saved {
                path: take.path.clone(),
                copied: false,
                closed: take.closed,
            },
            Some(why) => Clip::Failed {
                path: take.path.clone(),
                why: why.clone(),
            },
        };

        self.ended = Some(Ended {
            path: take.path,
            closed: take.closed,
            lost,
        });
        post(ended(clip, false));
    }

    /*
     * answers `later` with the path once the segment has its first frame, if anyone waits for it:
     * a press on the island's own targets does not
     */
    fn watch(&self, later: &Later, segment: u32) {
        let Some(take) = &self.take else {
            return;
        };

        if !later.open() {
            return;
        }

        let (watching, frame, path) = (later.clone(), take.segment(segment), take.path.clone());
        let watched = thread::Builder::new()
            .name(String::from("capture-record frame"))
            .spawn(move || {
                while watching.open() {
                    if frame.exists() {
                        watching.done(path);
                        return;
                    }

                    thread::sleep(LOOK);
                }
            });

        if let Err(error) = watched {
            later.unknown(format!(
                "cannot watch for the first frame: {error}; it may still record to {}",
                take.path
            ));
        }
    }
}

// shown for the island, which `Island::write` takes after the runtime's lock, so its end posts after
fn post(activity: Activity) {
    IslandService::write().post(activity, Instant::now());
    refit();
}

/*
 * a recording that starts, pauses or ends while the Launcher is open changes which rows it offers,
 * and the island asks for the body height only when a watcher does: the view draws the rows fresh
 * every frame but may not post a fit itself
 */
fn refit() {
    let launcher = IslandService::read().surface() == Some(Surface::Launcher);

    if launcher {
        crate::surfaces::launcher::fit();
    }
}

/*
 * `why` a recording was lost, and where what was recorded of it is, if any: the file and the
 * segments are kept, never deleted unless they were joined. A folder with no more than the empty
 * files Kanade made in it is let go, and says nothing of what was recorded. A file at `path` is
 * named only while it is Kanade's: with the first segment still in the folder it was not put back,
 * so the file there appeared meanwhile and is someone else's, the name is said to be taken, and the
 * footage is in the folder alone
 */
fn kept(path: &Path, parts: &Path, why: String) -> String {
    let named = fs::metadata(path).is_ok_and(|file| file.is_file() && file.len() > 0);
    let taken = named && parts.join(plan::segment(1)).exists();
    let mut places = Vec::new();

    if named && !taken {
        places.push(path.display().to_string());
    }

    if holds_footage(parts) {
        places.push(parts.display().to_string());
    }

    match (places.is_empty(), taken) {
        (true, _) => why,
        (false, true) => format!(
            "{why}; the name {} was taken, and what was recorded is kept in {}",
            path.display(),
            places.join(" and ")
        ),
        (false, false) => format!(
            "{why}; what was recorded is kept in {}",
            places.join(" and ")
        ),
    }
}

/*
 * whether the parts folder holds anything but the empty files Kanade made: those it deletes, and
 * the folder with them once nothing is left. A folder that cannot be read, or has a file of someone
 * else's, is left alone
 */
fn holds_footage(parts: &Path) -> bool {
    let Ok(entries) = fs::read_dir(parts) else {
        return false;
    };

    let mut footage = false;

    for entry in entries.filter_map(Result::ok) {
        let empty = entry
            .metadata()
            .is_ok_and(|file| file.is_file() && file.len() == 0);
        let made = entry.file_name().to_str().is_some_and(plan::made);

        if empty && made {
            drop(fs::remove_file(entry.path()));
        } else {
            footage = true;
        }
    }

    if !footage {
        drop(fs::remove_dir(parts));
    }

    footage
}

// what `kanade capture record` asks
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Start(Start),
    Pause,
    Resume,
    Stop,
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    pub target: Target,
    pub audio: Audio,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    // the focused output
    Screen,

    // the rectangle, or one the user chooses on screen
    Area(Option<Rect>),

    // the window of niri with this id, or the focused one at the start; the id, not the title,
    // which changes
    Window(Option<u64>),
}

impl Request {
    /*
     * `start [screen|area [<x,y WxH>]|window [<id>]] [none|system|mic]`, `pause`, `resume`,
     * `stop` or `status`, the words after `record`. The rectangle is one word or two
     */
    pub fn parse(words: &[&str]) -> Option<Request> {
        match words {
            ["start", rest @ ..] => Start::parse(rest).map(Request::Start),
            ["pause"] => Some(Request::Pause),
            ["resume"] => Some(Request::Resume),
            ["stop"] => Some(Request::Stop),
            ["status"] => Some(Request::Status),
            _ => None,
        }
    }
}

impl Start {
    fn parse(words: &[&str]) -> Option<Start> {
        let (words, audio) = match words.split_last() {
            Some((last, rest)) => match Audio::parse(last) {
                Some(audio) => (rest, audio),
                None => (words, Audio::None),
            },
            None => (words, Audio::None),
        };

        let target = match words {
            [] | ["screen"] => Target::Screen,
            ["area"] => Target::Area(None),
            ["area", rectangle @ ..] => Target::Area(Some(Rect::parse(&rectangle.join(" "))?)),
            ["window"] => Target::Window(None),
            ["window", id] => Target::Window(Some(id.parse().ok()?)),
            _ => return None,
        };

        Some(Start { target, audio })
    }
}

// how the recording stands, as `status` says it
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    // none since Kanade started
    Idle,

    // the user is choosing an area
    Picking,

    // the recorder runs but wrote no frame yet; `what` says what it records and hears
    Starting { what: String, path: String },

    Recording { what: String, path: String },

    Paused { what: String, path: String },

    // asked to finish the file
    Finishing { path: String },

    Saved { path: String },

    Failed { path: String, why: String },
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Status::Idle => write!(f, "not recording"),
            Status::Picking => write!(f, "choosing an area to record"),
            Status::Starting { what, path } => write!(f, "starting to record {what} to {path}"),
            Status::Recording { what, path } => write!(f, "recording {what} to {path}"),
            Status::Paused { what, path } => write!(f, "paused recording {what} to {path}"),
            Status::Finishing { path } => write!(f, "finishing {path}"),
            Status::Saved { path } => write!(f, "saved {path}"),
            Status::Failed { path, why } if path.is_empty() => write!(f, "failed to record\n{why}"),
            Status::Failed { path, why } => write!(f, "failed to record to {path}\n{why}"),
        }
    }
}

/*
 * does what `request` asks. A start answers `later` with the path once the first frame is written,
 * a stop once the file is saved
 */
pub fn request(request: Request, later: Later) {
    if request == Request::Status {
        later.done(status().to_string());
        return;
    }

    if let Err(why) = ask_for(request, &later) {
        later.refused(why);
    }
}

/*
 * does what a press on the island asks, which has no one to answer: a start that fails is shown by
 * a Failed Activity, as nothing else would say so
 */
pub fn press(request: Request) {
    let start = matches!(request, Request::Start(_));

    if let Err(why) = ask_for(request, &Later::detached()) {
        eprintln!("capture: {why}");

        let runtime = RUNTIME.locked();

        if start && !runtime.machine.busy() {
            post(ended(
                Clip::Failed {
                    path: String::new(),
                    why,
                },
                true,
            ));
        }
    }
}

// refused while the session is locked, or locking: nothing is started or taken up again then
fn allowed(request: &Request, locked: bool) -> Result<(), String> {
    match request {
        Request::Start(_) | Request::Resume if locked => Err(String::from("the session is locked")),
        _ => Ok(()),
    }
}

fn ask_for(request: Request, later: &Later) -> Result<(), String> {
    // before the runtime is taken: the lock's mutex is never asked for under it
    allowed(&request, lock::holds())?;

    let begins = matches!(request, Request::Start(_) | Request::Resume);

    match request {
        Request::Start(start) => begin(start, later)?,
        Request::Pause => pause(later)?,
        Request::Resume => resume(later)?,
        Request::Stop => stop(later)?,
        Request::Status => {}
    }

    // the session locked since it was looked at, and the lock's hook found nothing recording
    if begins && lock::holds() {
        drop(RUNTIME.locked().feed(Event::Lock));
    }

    Ok(())
}

// starts recording what `start` says, to a path `later` is answered with
fn begin(start: Start, later: &Later) -> Result<(), String> {
    let Start { target, audio } = start;
    let pick = target == Target::Area(None);

    let source = match target {
        Target::Screen => Some(Source::Output(
            focused_output()
                .map_err(|error| format!("niri: {error}"))?
                .ok_or("no output to record")?,
        )),
        Target::Area(Some(area)) => Some(Source::Area(area.fitted()?)),
        Target::Window(id) => {
            for program in [ENCODER, MUXER] {
                if !wake::found(program) {
                    return Err(format!("{program} is not installed; it records a window"));
                }
            }

            let found = window::find(id)?;

            Some(Source::window(
                found.id,
                found.width,
                found.height,
                found.app,
            )?)
        }
        Target::Area(None) => {
            if !wake::found(PICKER) {
                return Err(format!(
                    "{PICKER} is not installed; it chooses the area, or give one as x,y WxH"
                ));
            }

            None
        }
    };

    let dir = capture::home(FOLDER).ok_or("no recording folder: HOME is not set")?;

    fs::create_dir_all(&dir)
        .map_err(|error| format!("no recording folder {}: {error}", dir.display()))?;

    let pausable = wake::found(JOINER);
    let mut runtime = RUNTIME.locked();

    if runtime.machine.busy() {
        return Err(match &runtime.take {
            Some(_) if matches!(runtime.machine.phase(), Phase::Picking { .. }) => {
                String::from("already choosing an area to record")
            }
            Some(take) => format!("already recording to {}", take.path),
            None => String::from("already recording"),
        });
    }

    let path = capture::free(
        &dir,
        &format!("Screencast from {}", clock::stamp()),
        "mp4",
        |path| {
            path.exists()
                || parts(path).exists()
                || runtime
                    .issued
                    .iter()
                    .any(|issued| path == Path::new(issued))
        },
    );
    let parts = parts(&path);
    let path = path
        .to_str()
        .ok_or_else(|| format!("{} is not UTF-8", path.display()))?
        .to_owned();

    runtime.issued.push(path.clone());
    runtime.ended = None;
    runtime.take = Some(Take {
        path: path.clone(),
        parts,
        source,
        audio,
        pausable,
        closed: false,
        software: None,
        no_va: false,
        recording: config::get().recording.clone(),
    });

    if pick {
        // the user takes their time, which `later` would not wait for
        runtime.feed(Event::Start { pick })?;
        later.done("choose the area on screen; recording starts when you have");
        return Ok(());
    }

    later.unsettled(format!(
        "no frame was written within {}s; it may still record to {path}",
        LATER.as_secs()
    ));
    runtime.waiting.push((Wait::End, later.clone()));
    runtime.feed(Event::Start { pick })?;
    runtime.watch(later, 1);

    Ok(())
}

// the folder beside the file at `path` that holds its segments, hidden
fn parts(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();

    path.with_file_name(format!(".{stem}.parts"))
}

// ends the segment the recorder writes, so as to pause between it and the next
fn pause(later: &Later) -> Result<(), String> {
    let mut runtime = RUNTIME.locked();
    let take = runtime.take.as_ref().ok_or("not recording")?;

    if !take.pausable {
        return Err(format!(
            "{JOINER} is not installed; it joins the segments of a paused recording"
        ));
    }

    // niri sends the first frame only once the screen changes, and a segment without one is empty
    let framed = match runtime.machine.phase() {
        Phase::Running { segment, .. } => take.segment(segment).exists(),
        _ => false,
    };

    later.unsettled(format!(
        "the segment did not finish within {}s; the recording may still pause",
        LATER.as_secs()
    ));
    runtime.waiting.push((Wait::Paused, later.clone()));

    if let Err(why) = runtime.feed(Event::Pause { framed }) {
        runtime.waiting.pop();
        return Err(why);
    }

    Ok(())
}

// starts the next segment, answering `later` with the path once it has its first frame
fn resume(later: &Later) -> Result<(), String> {
    let mut runtime = RUNTIME.locked();

    later.unsettled(format!(
        "no frame was written within {}s; it may still record to {}",
        LATER.as_secs(),
        runtime.path()
    ));
    runtime.waiting.push((Wait::End, later.clone()));

    if let Err(why) = runtime.feed(Event::Resume) {
        runtime.waiting.pop();
        return Err(why);
    }

    if let Phase::Running { segment, .. } = runtime.machine.phase() {
        runtime.watch(later, segment);
    }

    Ok(())
}

// ends the recording, whatever it does, which answers `later` once the file is saved
fn stop(later: &Later) -> Result<(), String> {
    let mut runtime = RUNTIME.locked();

    // an area not yet chosen has no file to wait for
    if matches!(runtime.machine.phase(), Phase::Picking { .. }) {
        runtime.feed(Event::Stop)?;
        later.done(String::new());
        return Ok(());
    }

    later.unsettled(format!(
        "the recording did not finish within {}s; it may still be saved at {}",
        LATER.as_secs(),
        runtime.path()
    ));
    runtime.waiting.push((Wait::End, later.clone()));

    if let Err(why) = runtime.feed(Event::Stop) {
        runtime.waiting.pop();
        return Err(why);
    }

    Ok(())
}

pub fn status() -> Status {
    let runtime = RUNTIME.locked();
    let what = |take: &Take| {
        let what = take.source.as_ref().map(Source::said).unwrap_or_default();
        let what = match take.audio.heard() {
            Some(heard) => format!("{what}, {heard}"),
            None => what,
        };

        match &take.software {
            Some(_) => format!("{what}, software encoding"),
            None => what,
        }
    };

    match (runtime.machine.phase(), &runtime.take) {
        (Phase::Picking { .. }, _) => Status::Picking,
        (Phase::Joining, Some(take)) => Status::Finishing {
            path: take.path.clone(),
        },
        (Phase::Running { stopping: true, .. }, Some(take)) => Status::Finishing {
            path: take.path.clone(),
        },
        (Phase::Running { segment, .. }, Some(take)) if take.segment(segment).exists() => {
            Status::Recording {
                what: what(take),
                path: take.path.clone(),
            }
        }
        (Phase::Running { .. }, Some(take)) => Status::Starting {
            what: what(take),
            path: take.path.clone(),
        },
        (Phase::Paused { .. }, Some(take)) => Status::Paused {
            what: what(take),
            path: take.path.clone(),
        },
        _ => match &runtime.ended {
            Some(Ended {
                path, lost: None, ..
            }) => Status::Saved { path: path.clone() },
            Some(Ended {
                path,
                lost: Some(why),
                ..
            }) => Status::Failed {
                path: path.clone(),
                why: why.clone(),
            },
            None => Status::Idle,
        },
    }
}

/*
 * an action of the recording at `path`, as its Activity's Stop, Pause or Resume does; none if that
 * recording is over, so a stale one never reaches a newer recording
 */
pub fn stop_at(path: &str) {
    act_at(path, Request::Stop);
}

pub fn pause_at(path: &str) {
    act_at(path, Request::Pause);
}

pub fn resume_at(path: &str) {
    act_at(path, Request::Resume);
}

fn act_at(path: &str, request: Request) {
    if RUNTIME.locked().path() != path {
        return;
    }

    if let Err(why) = ask_for(request, &Later::detached()) {
        eprintln!("capture: {why}");
    }
}

/*
 * the session is locking: whatever is recorded is saved and no more is. Called under the lock's own
 * mutex, so it only looks at a flag and starts a thread
 */
pub fn locked() {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }

    let spawned = thread::Builder::new()
        .name(String::from("capture-lock"))
        .spawn(|| drop(RUNTIME.locked().feed(Event::Lock)));

    if let Err(error) = spawned {
        eprintln!("capture: cannot save the recording as the session locks: {error}");
    }
}

// the recording at `path` copied, shown so unless a newer recording took its place
pub fn copied(path: &str) {
    let runtime = RUNTIME.locked();

    let last = match &runtime.ended {
        Some(Ended {
            path: saved,
            closed,
            lost: None,
        }) if saved == path => Some(*closed),
        _ => None,
    };

    if !runtime.machine.busy()
        && let Some(closed) = last
    {
        post(ended(
            Clip::Saved {
                path: path.to_owned(),
                copied: true,
                closed,
            },
            false,
        ));
    }
}

// the focused output's name, none while niri focuses none
fn focused_output() -> io::Result<Option<String>> {
    Ok(output_name(&niri::ask("\"FocusedOutput\"")?))
}

fn output_name(reply: &Json) -> Option<String> {
    reply
        .get("FocusedOutput")?
        .get("name")?
        .as_str()
        .map(String::from)
}

/*
 * the one worker thread, started by the first job and living as long as Kanade. This is the thread
 * that starts the recorder, the area picker and the joiner, and that matters: a child started
 * through `setpriv --pdeathsig` dies when the THREAD that started it ends, not the process (ADR
 * 0027; `wake::run`). A thread made for one request and gone after it would end the recorder as it
 * exits. It is never held on `RUNTIME`.
 */
static WORKER: Mutex<Option<Sender<Job>>> = Mutex::new(None);

fn ask(job: Job) -> Result<(), String> {
    let mut worker = WORKER.locked();

    if let Some(sender) = worker.as_ref() {
        match sender.send(job) {
            Ok(()) => return Ok(()),
            // the thread is gone, which a panic it did not survive could do; start another
            Err(mpsc::SendError(back)) => {
                *worker = None;
                return start_worker(&mut worker, back);
            }
        }
    }

    start_worker(&mut worker, job)
}

fn start_worker(worker: &mut Option<Sender<Job>>, job: Job) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel::<Job>();

    thread::Builder::new()
        .name(String::from("capture-record worker"))
        .spawn(move || {
            for job in receiver {
                work(job);
            }
        })
        .map_err(|error| format!("cannot start a thread: {error}"))?;

    sender
        .send(job)
        .map_err(|_| String::from("the worker thread ended"))?;
    *worker = Some(sender);

    Ok(())
}

// runs `job`; a panic in it ends what it was for, so a failure of the thread ends the recording
fn work(job: Job) {
    let internal = || String::from("internal error");

    match job {
        Job::Pick { serial } => survive(
            || pick(serial),
            || {
                feed(Event::Picked {
                    serial,
                    pick: Pick::Failed(internal()),
                })
            },
        ),
        Job::Spawn { serial, segment } => survive(
            || spawn(serial, segment),
            || {
                feed(Event::Spawned {
                    serial,
                    started: Err(internal()),
                })
            },
        ),
        Job::Join { segments } => {
            survive(|| join(segments), || feed(Event::Joined(Err(internal()))))
        }
    }
}

fn survive(work: impl FnOnce(), otherwise: impl FnOnce()) {
    if panic::catch_unwind(AssertUnwindSafe(work)).is_err() {
        otherwise();
    }
}

fn feed(event: Event) {
    drop(RUNTIME.locked().feed(event));
}

// lets the user choose an area with the picker, then goes on recording it or does not
fn pick(serial: u64) {
    let started = wake::hold_stream(PICKER, &plan::picker());
    let mut child = match started {
        Ok(child) => child,
        Err(error) => {
            return feed(Event::Picked {
                serial,
                pick: Pick::Failed(format!("cannot run {PICKER}: {error}")),
            });
        }
    };

    {
        let mut runtime = RUNTIME.locked();

        // ended while it started
        if runtime.machine.serial() != Some(serial) {
            drop(child.kill());
            drop(child.wait());
            return;
        }

        runtime.picker = Some(child.id());
    }

    let mut chosen = String::new();
    let mut said = String::new();

    if let Some(mut out) = child.stdout.take() {
        drop(out.read_to_string(&mut chosen));
    }

    if let Some(mut err) = child.stderr.take() {
        drop(err.read_to_string(&mut said));
    }

    // let go before it is reaped, so nobody signals a pid that is no longer its own
    {
        let mut runtime = RUNTIME.locked();

        if runtime.picker == Some(child.id()) {
            runtime.picker = None;
        }
    }

    let status = child.wait();
    let picked = chosen_area(status, &chosen, &said);
    let mut runtime = RUNTIME.locked();

    let pick = match picked {
        Ok(Some(area)) if runtime.machine.serial() == Some(serial) => match runtime.take.as_mut() {
            Some(take) => {
                take.source = Some(Source::Area(area));
                Pick::Chosen
            }
            None => Pick::Left,
        },
        Ok(Some(_) | None) => Pick::Left,
        Err(why) => Pick::Failed(why),
    };

    drop(runtime.feed(Event::Picked { serial, pick }));
}

/*
 * the area the picker printed, as the recorder takes it; none when the user left the picker (it
 * exits 1, printing nothing)
 */
fn chosen_area(
    status: io::Result<ExitStatus>,
    chosen: &str,
    said: &str,
) -> Result<Option<Rect>, String> {
    match status {
        Ok(status) if status.success() => Rect::parse(chosen)
            .ok_or_else(|| format!("{PICKER} chose an area that is no rectangle: {chosen:?}"))
            .and_then(Rect::fitted)
            .map(Some),
        Ok(status) if status.code() == Some(NOT_FOUND) => Err(format!("{PICKER} not found")),
        Ok(_) if chosen.trim().is_empty() && said.trim().is_empty() => Ok(None),
        Ok(status) => Err(format!("{PICKER} ended ({status}): {}", said.trim())),
        Err(error) => Err(format!("{PICKER}: {error}")),
    }
}

// starts the recorder of `segment`, if the recording still waits for it
fn spawn(serial: u64, segment: u32) {
    let chosen = {
        let runtime = RUNTIME.locked();

        match (runtime.machine.phase(), &runtime.take) {
            (Phase::Running { serial: now, .. }, Some(take)) if now == serial => {
                let Some(source) = take.source.clone() else {
                    return;
                };

                (
                    source,
                    take.audio,
                    runtime.machine.phase(),
                    take.segment(segment),
                    (take.software.is_some(), take.no_va),
                    take.recording.clone(),
                )
            }
            _ => return,
        }
    };

    let (source, audio, phase, file, (software, no_va), recording) = chosen;

    // a stop or a lock came while it was handed over: nothing is recorded, not even a microphone
    if matches!(phase, Phase::Running { stopping: true, .. }) {
        return feed(Event::Ended {
            serial,
            segment: Segment::Empty,
        });
    }

    // what encodes, from the settings the recording began with, and a window's by the pipeline it
    // runs
    let windowed = matches!(source, Source::Window { .. });
    let deadline = Instant::now() + encode::PROBE_BUDGET;
    let found =
        probe::encoding_within(&recording, software, deadline).and_then(
            |encoding| match windowed {
                true => probe::window_within(encoding, &recording, no_va, deadline),
                false => Ok((encoding, None)),
            },
        );
    let (encoding, gave_way) = match found {
        Ok(found) => found,
        Err(why) => {
            return feed(Event::Spawned {
                serial,
                started: Err(why),
            });
        }
    };

    if let Some(why) = gave_way {
        gives_way(serial, &why);
    }

    // the frame the call waits for can only come now, the probe having taken part of its wait
    RUNTIME.locked().postpone();

    eprintln!("capture: encoding with {}", encoding.encoder);

    if let Source::Window { id, size, .. } = source {
        return spawn_window(serial, segment, (id, size), audio, file, encoding);
    }

    let folder = file.parent().map(Path::to_path_buf).unwrap_or_default();
    let ran = Ran::of(&encoding);
    let started = fs::create_dir_all(&folder)
        .map_err(|error| format!("no folder {}: {error}", folder.display()))
        .and_then(|()| {
            let file = file
                .to_str()
                .ok_or_else(|| format!("{} is not UTF-8", file.display()))?;
            let arguments = plan::recorder(&source, audio, file, &encoding);
            let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();

            wake::hold_file(RECORDER, &arguments)
                .map_err(|error| format!("cannot run {RECORDER}: {error}"))
        });

    let mut child = match started {
        Ok(child) => child,
        Err(why) => {
            return feed(Event::Spawned {
                serial,
                started: Err(why),
            });
        }
    };

    let said = child.stderr.take();
    let mut runtime = RUNTIME.locked();

    // ended while it started
    if !matches!(runtime.machine.phase(), Phase::Running { serial: now, .. } if now == serial) {
        drop(child.kill());
        drop(child.wait());
        return;
    }

    let followed = thread::Builder::new()
        .name(String::from("capture-record"))
        .spawn(move || follow(said, serial, file, ran));

    if let Err(error) = followed {
        drop(child.kill());
        drop(child.wait());

        return drop(runtime.feed(Event::Spawned {
            serial,
            started: Err(format!("cannot follow {RECORDER}: {error}")),
        }));
    }

    runtime.child = Some(Held {
        serial,
        writer: child,
        encoder: None,
        cast: None,
        said: None,
    });
    drop(runtime.feed(Event::Spawned {
        serial,
        started: Ok(()),
    }));
}

// how a segment's encoder ran, which says what to start again with if it fails before a frame
#[derive(Clone, Copy)]
struct Ran {
    // the software encoder
    software: bool,

    // a window's frames downloaded through the VA-API post-processor
    va: bool,
}

impl Ran {
    fn of(encoding: &encode::Encoding) -> Self {
        Self {
            software: encoding.encoder.kind == encode::Kind::Software,
            va: matches!(encoding.download, encode::Download::Va { .. }),
        }
    }
}

// what a segment that failed before it wrote a frame is started again with
#[derive(Debug, PartialEq)]
enum Again {
    // EGL for the window's frames, the same encoder
    Egl,

    // the software encoder, with the VA-API download tried again
    Software,
}

/*
 * what to start again with, from how the segment ran and what the recording has already given up.
 * A failed VA-API download, when the encoder is what failed (`fault` is `Fault::Encoder`), goes to
 * EGL once. An encoder that is not software goes to software once, which tries the VA-API download
 * afresh, since the encoder may have failed for its own reasons (a session limit) and software with
 * VA-API can work where EGL cannot import the cast. Nothing else, so no failure loops: EGL is taken
 * once per `no_va` and `no_va` is cleared only by the one step to software. A `Fault::Vanished`
 * output is not the encoder's doing and is not tried again: the same target is gone whichever
 * encoder reads it
 */
fn again(ran: Ran, given: Given, fault: Fault) -> Option<Again> {
    if fault == Fault::Vanished {
        return None;
    }

    // a stall says nothing about which stage stopped, so the download that may have stalled is
    // changed first, as for an encoder that failed
    let encoder = matches!(fault, Fault::Encoder | Fault::Stalled);

    if ran.va && !given.no_va && encoder {
        Some(Again::Egl)
    } else if !ran.software && !given.software {
        Some(Again::Software)
    } else {
        None
    }
}

// what the recording has already given up on, as `Take` keeps it
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Given {
    // the VA-API download failed, so EGL is used
    no_va: bool,

    // the software encoder was asked for
    software: bool,
}

// what made a segment end without a frame, as far as another start could mend it
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    // the encoder, which another download or the software encoder may start
    Encoder,

    // the chain stopped receiving buffers, so nothing was written: the cast, the download or the
    // encoder is what stalled, and only another chain can tell. Walked as the encoder is, since a
    // stalled take has nothing to lose and a cast that truly delivers nothing ends the walk
    Stalled,

    // the muxer, which no other encoder start mends
    Muxer,

    // the output or window recorded is gone, which nothing mends
    Vanished,
}

/*
 * one step of `falls_back`, pure so the walk is testable: what to start again with and what the
 * recording has given up afterwards, none when there is nowhere left to go
 */
fn step(given: Given, ran: Ran, fault: Fault) -> Option<(Again, Given)> {
    let again = again(ran, given, fault)?;

    let given = match again {
        Again::Egl => Given {
            no_va: true,
            ..given
        },
        Again::Software => Given {
            no_va: false,
            software: true,
        },
    };

    Some((again, given))
}

/*
 * a segment whose program failed, when the recording has a way left to try (`again`, ADR 0116):
 * says so, notes it on the recording and gives the number of the segment to start again, which is
 * the one it was. None when it is not to be tried. `fault` says what failed, and only the encoder's
 * own failure, or a stall, is worth another start. A segment that wrote frames keeps them: the one
 * started again writes to its own file, so nothing recorded is written over or lost
 */
fn falls_back(
    runtime: &mut Runtime,
    serial: u64,
    segment: &Segment,
    ran: Ran,
    fault: Fault,
) -> Option<u32> {
    let Segment::Lost(why) = segment else {
        return None;
    };

    let Phase::Running {
        serial: now,
        segment: number,
        ..
    } = runtime.machine.phase()
    else {
        return None;
    };

    // a program that is not there is said as it is; the software encoder needs the same ones
    if now != serial || why.ends_with("not found") {
        return None;
    }

    let take = runtime.take.as_mut()?;
    let given = Given {
        no_va: take.no_va,
        software: take.software.is_some(),
    };
    let (again, given) = step(given, ran, fault)?;

    take.no_va = given.no_va;

    match again {
        // a stall is not the download's doing only: the chain says nothing about what stopped
        Again::Egl => match fault {
            Fault::Stalled => {
                eprintln!("capture: {why}; downloading through EGL instead and recording on")
            }
            _ => eprintln!(
                "capture: the encoder failed to start with the VA-API download ({why}); downloading through EGL instead"
            ),
        },
        Again::Software => {
            take.software = Some(why.clone());

            // the muxer's own failure is not the encoder's, and says so
            let failed = match fault {
                Fault::Muxer => MUXER,
                Fault::Stalled => "the picture",
                _ => "the encoder",
            };

            match fault {
                Fault::Stalled => eprintln!("capture: {failed} stopped ({why}); recording on"),
                _ => eprintln!(
                    "capture: {failed} failed to start ({why}); recording with software x264 instead"
                ),
            }
        }
    }

    Some(number)
}

// the recording goes on with the software encoder after `why`, said once
fn gives_way(serial: u64, why: &str) {
    let mut runtime = RUNTIME.locked();

    if runtime.machine.serial() == Some(serial)
        && let Some(take) = runtime.take.as_mut().filter(|take| take.software.is_none())
    {
        take.software = Some(why.to_owned());
        take.no_va = false;
        eprintln!("capture: {why}; recording with software x264 instead");
    }
}

/*
 * reads what the recorder prints until it exits, then tells the machine how its segment ended, or
 * starts that segment again (`falls_back`) if the recording has a way left to try
 */
fn follow(said: Option<ChildStderr>, serial: u64, file: PathBuf, ran: Ran) {
    survive(
        || {
            let said = last_lines(said);
            let mut runtime = RUNTIME.locked();

            // under the lock, so a stop that interrupts it never signals a pid that is reaped
            let Some(Held {
                writer: mut child, ..
            }) = runtime.child.take_if(|held| held.serial == serial)
            else {
                return;
            };

            let status = child.wait();
            let written = fs::metadata(&file).is_ok_and(|file| file.len() > 0);

            // an empty file is one Kanade's recorder made, and is no recording
            if !written {
                discard_empty(&file);
            }

            let segment = classify(status, written, said);

            // a vanished output is not the encoder's doing: the first error stands
            let fault = match &segment {
                Segment::Lost(why) if vanished(why) => Fault::Vanished,
                _ => Fault::Encoder,
            };

            if let Some(number) = falls_back(&mut runtime, serial, &segment, ran, fault) {
                // the worker starts it, as any recorder: a program's life is tied to its spawning thread
                return runtime.hand(
                    Job::Spawn {
                        serial,
                        segment: number,
                    },
                    |why| Event::Spawned {
                        serial,
                        started: Err(why),
                    },
                );
            }

            drop(runtime.feed(Event::Ended { serial, segment }));
        },
        || {
            let mut runtime = RUNTIME.locked();

            if let Some(held) = runtime.child.take_if(|held| held.serial == serial) {
                drop(runtime);
                abandon(held);
                runtime = RUNTIME.locked();
            }

            drop(runtime.feed(Event::Ended {
                serial,
                segment: Segment::Lost(String::from("internal error")),
            }));
        },
    );
}

// how long an encoder told to stop has before it is killed, and then the writer after it
const STOPPING: Duration = Duration::from_secs(5);

/*
 * kills the picker `pid` after `STOPPING` if it is still the one running: a `slurp` that ignores
 * SIGTERM holds the worker thread, and with it every later recording job
 */
fn escalate_picker(pid: u32) {
    let watching = thread::Builder::new()
        .name(String::from("capture-record watchdog"))
        .spawn(move || {
            thread::sleep(STOPPING);

            if RUNTIME.locked().picker != Some(pid) {
                return;
            }

            eprintln!("capture: the picker does not end; killing it");
            drop(clock::kill_pid(pid));
        });

    if let Err(error) = watching {
        eprintln!("capture: cannot watch the picker end: {error}");
    }
}

/*
 * kills the encoder of the segment `serial` after `STOPPING` if it is still held, then the writer after
 * as long again: a GStreamer pipeline stuck on a window that no longer draws never ends of itself,
 * and the recording, its cast and its indicator would stay up. What `follow_window` has taken out
 * is its own to end (`finish_encoder`)
 */
fn escalate(serial: u64) {
    let watching = thread::Builder::new()
        .name(String::from("capture-record watchdog"))
        .spawn(move || {
            for writer in [false, true] {
                thread::sleep(STOPPING);

                let mut runtime = RUNTIME.locked();

                let Some(held) = runtime.child.as_mut().filter(|held| held.serial == serial) else {
                    return;
                };
                let child = match (writer, held.encoder.as_mut()) {
                    (false, Some(encoder)) => encoder,
                    _ => &mut held.writer,
                };

                eprintln!("capture: the recorder does not end; killing it");
                drop(child.kill());
            }
        });

    if let Err(error) = watching {
        eprintln!("capture: cannot watch the recorder end: {error}");
    }
}

// kills what runs for a segment that cannot go on, and ends its session
fn abandon(mut held: Held) {
    for child in [Some(&mut held.writer), held.encoder.as_mut()]
        .into_iter()
        .flatten()
    {
        drop(child.kill());
        drop(child.wait());
    }

    if let Some(cast) = &held.cast {
        cast.stop();
    }
}

// deletes the file at `path` if it is there and empty
fn discard_empty(path: &Path) {
    if fs::metadata(path).is_ok_and(|file| file.is_file() && file.len() == 0) {
        drop(fs::remove_file(path));
    }
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
 * what the recorder left behind: a file with frames only when it succeeded and wrote one, as one
 * that failed may leave part of one. niri sends a frame only on damage, so a stop on a still screen
 * can come before any
 */
fn classify(status: io::Result<ExitStatus>, written: bool, said: VecDeque<String>) -> Segment {
    match status {
        Ok(status) if status.success() && written => Segment::Written,
        Ok(status) if status.code() == Some(NOT_FOUND) => {
            Segment::Lost(format!("{RECORDER} not found"))
        }
        Ok(status) if status.success() => Segment::Empty,
        // interrupted before it had a handler (a resume stopped at once) and before any frame: it
        // wrote nothing, which is no failure
        Ok(status) if !written && status.signal() == Some(libc::SIGINT) => Segment::Empty,
        Ok(status) => {
            let said = Vec::from(said).join("; ");

            Segment::Lost(match said.is_empty() {
                true => format!("{RECORDER} ended ({status})"),
                false => format!("{RECORDER} ended ({status}): {said}"),
            })
        }
        Err(error) => Segment::Lost(format!("{RECORDER}: {error}")),
    }
}

/*
 * how long a window's encoder has to end of itself after the writer did, then to end when told to,
 * before it is killed
 */
const ENDING: Duration = Duration::from_millis(500);
const ENDING_TOLD: Duration = Duration::from_secs(3);

// how long the encoder's reader has to finish after it is gone; a descendant holding its stderr
// would otherwise hold the follow thread
const SAID_JOINED: Duration = Duration::from_secs(1);

/*
 * a child's stderr, read on a thread of its own: the lines as they were printed, and whether the
 * `watchdog` element (`encode::guard`) ended the chain. The verdict is the reader's, not the lines'
 * after a join: a descendant that inherited the pipe can keep the reader past `SAID_JOINED`, and a
 * stall that a slow join lost would be read as the writer's own failure
 */
struct Reading {
    said: Mutex<VecDeque<String>>,
    watchdog: AtomicBool,
    done: AtomicBool,
}

impl Reading {
    // whether the guard ended the chain, as the reader read it: its own verdict, not a later
    // reading of lines a join may have lost
    fn stalled(&self) -> bool {
        self.watchdog.load(Ordering::Acquire)
    }

    // whether the reader has finished with the pipe, waiting no more than `within` for it
    fn ended_within(&self, within: Duration) -> bool {
        let deadline = Instant::now() + within;

        while !self.done.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                return false;
            }

            thread::sleep(Duration::from_millis(20));
        }

        true
    }

    // what it printed, as the last lines: once it has finished, or as far as it got by then
    fn words(&self) -> VecDeque<String> {
        let _ = self.ended_within(SAID_JOINED);

        self.said.locked().clone()
    }
}

/*
 * reads `said` on a thread of its own, so a full pipe never holds the encoder, keeping the last
 * `SAID` lines as they are printed and whether the guard ended the chain
 */
fn reading(said: ChildStderr) -> io::Result<Arc<Reading>> {
    let read = Arc::new(Reading {
        said: Mutex::new(VecDeque::with_capacity(SAID)),
        watchdog: AtomicBool::new(false),
        done: AtomicBool::new(false),
    });
    let held = Arc::clone(&read);

    thread::Builder::new()
        .name(String::from("capture-record said"))
        .spawn(move || {
            for line in BufReader::new(said).lines().map_while(Result::ok) {
                if stalled_of(&line) {
                    held.watchdog.store(true, Ordering::Release);
                }

                let mut last = held.said.locked();

                if last.len() == SAID {
                    last.pop_front();
                }

                last.push_back(line);
            }

            held.done.store(true, Ordering::Release);
        })?;

    Ok(read)
}

/*
 * starts what records `segment` of a window (ADR 0117), if the recording still waits for it: niri's
 * session for it, the encoder reading the session's PipeWire node, and the writer reading the
 * encoder. All of it is told to the machine as one recorder
 */
fn spawn_window(
    serial: u64,
    segment: u32,
    (id, size): (u64, (u32, u32)),
    audio: Audio,
    file: PathBuf,
    encoding: encode::Encoding,
) {
    let failed = |why: String| {
        feed(Event::Spawned {
            serial,
            started: Err(why),
        })
    };

    let folder = file.parent().map(Path::to_path_buf).unwrap_or_default();

    if let Err(error) = fs::create_dir_all(&folder) {
        return failed(format!("no folder {}: {error}", folder.display()));
    }

    let Some(name) = file.to_str() else {
        return failed(format!("{} is not UTF-8", file.display()));
    };

    let cast = match window::Cast::open(id) {
        Ok(cast) => cast,
        Err(why) => {
            // a resume to a window that closed while paused: what there is of it is saved
            if segment > 1
                && window::open(id) == Some(false)
                && let Some(take) = RUNTIME.locked().take.as_mut()
            {
                take.closed = true;
            }

            return failed(why);
        }
    };

    // a stop or a lock came while niri was asked, and nothing is written yet: nothing is recorded
    let now = RUNTIME.locked().machine.phase();

    match now {
        Phase::Running {
            serial: now,
            stopping: true,
            ..
        } if now == serial => {
            cast.stop();

            return feed(Event::Ended {
                serial,
                segment: Segment::Empty,
            });
        }
        Phase::Running { serial: now, .. } if now == serial => {}
        _ => {
            cast.stop();
            return;
        }
    }

    let (mut encoder, mut writer) = match window_programs(cast.node, size, audio, name, &encoding) {
        Ok(programs) => programs,
        Err(why) => {
            cast.stop();
            return failed(why);
        }
    };

    // read as it is printed, so a full pipe never holds the encoder
    let encoder_said = encoder.stderr.take().map(reading).transpose();
    let writer_said = writer.stderr.take();
    let encoder_said = match encoder_said {
        Ok(said) => said,
        Err(error) => {
            abandon(Held {
                serial,
                writer,
                encoder: Some(encoder),
                cast: Some(cast),
                said: None,
            });
            return failed(format!("cannot read {ENCODER}: {error}"));
        }
    };
    let held = Held {
        serial,
        writer,
        encoder: Some(encoder),
        cast: Some(cast),
        said: encoder_said,
    };
    let ran = Ran::of(&encoding);
    let mut runtime = RUNTIME.locked();
    // ended while it started
    if !matches!(runtime.machine.phase(), Phase::Running { serial: now, .. } if now == serial) {
        drop(runtime);
        return abandon(held);
    }

    let followed = thread::Builder::new()
        .name(String::from("capture-record"))
        .spawn(move || follow_window(writer_said, serial, file, id, ran));
    if let Err(error) = followed {
        drop(runtime);
        abandon(held);

        return failed(format!("cannot follow {MUXER}: {error}"));
    }

    runtime.child = Some(held);
    drop(runtime.feed(Event::Spawned {
        serial,
        started: Ok(()),
    }));
}

// the encoder and the writer that reads it, started; neither is left running if the other cannot
fn window_programs(
    node: u64,
    size: (u32, u32),
    audio: Audio,
    file: &str,
    encoding: &encode::Encoding,
) -> Result<(Child, Child), String> {
    let arguments = plan::encoder(node, size, encoding);
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let mut encoder = wake::hold_source(ENCODER, &arguments)
        .map_err(|error| format!("cannot run {ENCODER}: {error}"))?;

    let Some(output) = encoder.stdout.take() else {
        drop(encoder.kill());
        drop(encoder.wait());

        return Err(format!("{ENCODER} has no output"));
    };

    // the writer's copy is its own; this one is let go at the end, so the encoder alone writes to it
    let input = OwnedFd::from(output);
    let arguments = plan::muxer(audio, file);
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();

    match wake::hold_sink(MUXER, &arguments, &input) {
        Ok(writer) => Ok((encoder, writer)),
        Err(error) => {
            drop(encoder.kill());
            drop(encoder.wait());

            Err(format!("cannot run {MUXER}: {error}"))
        }
    }
}

/*
 * reads what the writer prints until it exits, then ends what is left of the segment: the encoder if
 * it still runs, the niri session always. Only then is the machine told, so a stop is never answered
 * with the session still cast, and a window that closed is a recording that ended, not one that failed
 */
fn follow_window(writer_said: Option<ChildStderr>, serial: u64, file: PathBuf, id: u64, ran: Ran) {
    survive(
        || {
            let writer_said = last_lines(writer_said);

            // out of the runtime before the waits: a stop no longer signals what is taken
            let Some(mut held) = RUNTIME.locked().child.take_if(|held| held.serial == serial)
            else {
                return;
            };

            let writer = held.writer.wait();
            let (encoder, encoder_said) = match held.encoder.take() {
                Some(encoder) => finish_encoder(encoder, held.said.as_deref()),
                None => (Ok(ExitStatus::from_raw(0)), VecDeque::new()),
            };

            if let Some(cast) = &held.cast {
                cast.stop();
            }

            let written = fs::metadata(&file).is_ok_and(|file| file.len() > 0);

            if !written {
                discard_empty(&file);
            }

            /*
             * niri is asked unless both programs ended as they should: a window that closes ends
             * its cast, and whether the encoder then fails or ends of itself, the recording is a
             * saving the Clip is to say closed. Only a stop leaves both ending well with the window
             * there, which is no reason to ask niri at all
             */
            let ok = encoder_ok(&encoder) && writer.as_ref().is_ok_and(|status| status.success());
            let gone = !ok && window::open(id) == Some(false);
            let mut runtime = RUNTIME.locked();
            let segment_number = match runtime.machine.phase() {
                Phase::Running { segment, .. } => segment,
                _ => 1,
            };

            // a stop's own kill of an encoder that did not end is no failure of the encoder's
            let escalated = runtime.escalated.take_if(|escalated| *escalated == serial);
            let escalated = escalated.is_some();
            let blamed = encoder_blamed(&writer, &encoder);

            // what the guard saw, before `classify_window` takes the encoder's words. The reader's
            // own verdict, so a join that ran out of time cannot lose it. A window that closed ends
            // the cast and trips the guard too, which is a recording saved, not a stall
            let quiet = !gone && held.said.as_ref().is_some_and(|said| said.stalled());
            let segment = classify_window(
                [writer, encoder],
                [writer_said, encoder_said],
                written,
                gone.then_some(segment_number),
                escalated,
                quiet,
            );

            if gone
                && segment == Segment::Written
                && let Some(take) = runtime.take.as_mut()
            {
                take.closed = true;
            }

            let fault = fault_of(&segment, quiet, blamed);

            if !gone && let Some(number) = falls_back(&mut runtime, serial, &segment, ran, fault) {
                // the worker starts it, as any recorder: a program's life is tied to its spawning thread
                return runtime.hand(
                    Job::Spawn {
                        serial,
                        segment: number,
                    },
                    |why| Event::Spawned {
                        serial,
                        started: Err(why),
                    },
                );
            }

            drop(runtime.feed(Event::Ended { serial, segment }));
        },
        || {
            let held = RUNTIME.locked().child.take_if(|held| held.serial == serial);

            if let Some(held) = held {
                abandon(held);
            }

            feed(Event::Ended {
                serial,
                segment: Segment::Lost(String::from("internal error")),
            });
        },
    );
}

/*
 * waits for the encoder to end, which it does of itself once the writer has: told to after
 * `ENDING` if it does not (the writer failed, and the encoder waits on a still window), killed
 * after `ENDING_TOLD` more. What it printed, as the last lines
 */
fn finish_encoder(
    mut encoder: Child,
    said: Option<&Reading>,
) -> (io::Result<ExitStatus>, VecDeque<String>) {
    let started = Instant::now();
    let mut told = false;

    let status = loop {
        match encoder.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(error) => break Err(error),
        }

        let waited = started.elapsed();

        if waited >= ENDING + ENDING_TOLD {
            drop(encoder.kill());
            break encoder.wait();
        }

        if waited >= ENDING && !told {
            drop(clock::interrupt(&encoder));
            told = true;
        }

        thread::sleep(Duration::from_millis(20));
    };

    // its pipe is closed now that it is gone; a reader still holding it is let go, not waited on.
    // What it said is kept, though, as far as it got: a stall is the reader's own verdict, which a
    // join that ran out of time cannot lose
    let said = said.map_or_else(VecDeque::new, Reading::words);

    (status, said)
}

// whether the encoder ended as it should: finished, or told to
fn encoder_ok(status: &io::Result<ExitStatus>) -> bool {
    status
        .as_ref()
        .is_ok_and(|status| status.success() || status.signal() == Some(libc::SIGINT))
}

/*
 * whether the encoder is what ended the segment: it did not end as it should, and the writer did.
 * A writer that died first (a muxer of its own fault) leaves the encoder to fail on a broken pipe,
 * and blaming the encoder there would cost a restart that cannot mend it
 */
fn encoder_blamed(writer: &io::Result<ExitStatus>, encoder: &io::Result<ExitStatus>) -> bool {
    !encoder_ok(encoder) && writer.as_ref().is_ok_and(|status| status.success())
}

/*
 * whether a writer that wrote nothing is only saying its stream was short: it succeeded, printed
 * nothing, or said the stream had nothing. Anything else it printed is a real failure, which `Empty`
 * must not hide
 */
fn no_stream(writer: &io::Result<ExitStatus>, said: &VecDeque<String>) -> bool {
    writer.as_ref().is_ok_and(|status| status.success())
        || said.is_empty()
        || said.iter().any(|line| {
            [
                "no stream",
                "does not contain any stream",
                "nothing was encoded",
            ]
            .iter()
            .any(|said| line.contains(said))
        })
}

/*
 * whether one of the encoder's lines says the chain stopped receiving buffers, from the `watchdog`
 * element (`encode::guard`). Its element error is the only thing that names it, so a chain that
 * failed for another reason is left to the usual reading. Both spellings: the element's own name in
 * the error's first line, and the words it carries
 */
fn stalled_of(line: &str) -> bool {
    line.contains("GstWatchdog") || line.contains("Watchdog triggered")
}

/*
 * what made a segment end without a frame, from what the guard saw and what is blamed: the window
 * path's own reading of `Segment`, kept here so a stall cannot be lost to a writer's words
 */
fn fault_of(segment: &Segment, quiet: bool, blamed: bool) -> Fault {
    match segment {
        Segment::Lost(why) if vanished(why) => Fault::Vanished,
        _ if quiet => Fault::Stalled,
        _ if blamed => Fault::Encoder,
        _ => Fault::Muxer,
    }
}

/*
 * whether `why` names a target that is gone rather than an encoder that would not start: the same
 * output or window is gone whichever encoder reads it, so another start cannot mend it
 */
fn vanished(why: &str) -> bool {
    let why = why.to_lowercase();
    let named = ["output", "window", "source", "monitor", "target", "display"]
        .iter()
        .any(|what| why.contains(what));
    let gone = [
        "not found",
        "no such",
        "gone",
        "not available",
        "does not exist",
        "failed to find",
        "could not find",
        "cannot find",
    ]
    .iter()
    .any(|gone| why.contains(gone));

    named && gone
}

/*
 * what the writer and the encoder (in that order) left behind. A window closing ends the encoder,
 * then the writer, with the file whole: that is a segment written, and if no frame was, nothing of
 * this segment (`closed` holds its number when the window closed). `escalated` is whether a stop
 * killed the encoder because it did not end of itself, which is no failure of the encoder's, and
 * `stalled` is the guard's verdict, which the reader holds whether or not its join ran out of time
 */
fn classify_window(
    [writer, encoder]: [io::Result<ExitStatus>; 2],
    [writer_said, encoder_said]: [VecDeque<String>; 2],
    written: bool,
    closed: Option<u32>,
    escalated: bool,
    stalled: bool,
) -> Segment {
    let not_found = |status: &io::Result<ExitStatus>| {
        status
            .as_ref()
            .is_ok_and(|status| status.code() == Some(NOT_FOUND))
    };

    if not_found(&encoder) {
        return Segment::Lost(format!("{ENCODER} not found"));
    }

    if not_found(&writer) {
        return Segment::Lost(format!("{MUXER} not found"));
    }

    let writer_ok = writer.as_ref().is_ok_and(|status| status.success());
    let blamed = encoder_blamed(&writer, &encoder);

    match (writer_ok && written, closed) {
        (true, Some(_)) => return Segment::Written,
        // a stop that killed an encoder which did write frames is a stop, not a failure
        (true, None) if encoder_ok(&encoder) || (escalated && written) => return Segment::Written,
        _ => {}
    }

    match closed {
        // an earlier segment is whole, which `Empty` leaves be
        Some(segment) if !written && segment > 1 => return Segment::Empty,
        Some(_) if !written => {
            return Segment::Lost(String::from("the window closed before a frame was written"));
        }
        _ => {}
    }

    // told to stop before a frame: the encoder ended, and the writer found no stream, which it may
    // call a failure
    if !written && encoder_ok(&encoder) && no_stream(&writer, &writer_said) {
        return Segment::Empty;
    }

    /*
     * the guard ended the chain because no buffer arrived for `QUIET`, and the window is still
     * there: a stall, said as one rather than as an encoder that failed to start. What stalled (the
     * cast, the download, the encoder) is not visible from here, so the words claim none of them.
     * After the closed cases above, because a window that closes ends the cast and so trips the
     * guard too, and that is a recording saved, not a stall
     */
    if closed.is_none() && stalled {
        return Segment::Lost(format!("no picture arrived for {}s", QUIET.as_secs()));
    }

    // the encoder first when it is what failed; a muxer that died first is what ended the segment
    let (program, status, said) = if blamed {
        (ENCODER, encoder, encoder_said)
    } else {
        (MUXER, writer, writer_said)
    };

    if !written
        && status
            .as_ref()
            .is_ok_and(|s| s.signal() == Some(libc::SIGINT))
    {
        return Segment::Empty;
    }

    let said = Vec::from(said).join("; ");

    Segment::Lost(match (status, said.is_empty()) {
        (Err(error), _) => format!("{program}: {error}"),
        (Ok(status), true) => format!("{program} ended ({status})"),
        (Ok(status), false) => format!("{program} ended ({status}): {said}"),
    })
}

/*
 * a window closed, as niri says: a recording of it that is paused, with no program running to find
 * out, is saved as it is. One that runs finds out itself. Called on the niri thread, so it only
 * looks at a flag unless a recording is on
 */
pub fn heard(heard: Heard) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }

    let mut runtime = RUNTIME.locked();

    if !matches!(runtime.machine.phase(), Phase::Paused { .. }) {
        return;
    }

    let Some(Source::Window { id, .. }) =
        runtime.take.as_ref().and_then(|take| take.source.clone())
    else {
        return;
    };

    let gone = match &heard {
        Heard::Closed(closed) => closed.0 == id,
        // the stream began again: every window niri has, which this one is not among
        Heard::All(windows) => !windows.iter().any(|window| window.id.0 == id),
        _ => false,
    };

    if gone {
        if let Some(take) = runtime.take.as_mut() {
            take.closed = true;
        }

        drop(runtime.feed(Event::Stop));
    }
}

// what `kanade doctor` says of the encoder a recording would use (ADR 0116, Update)
pub struct Report {
    // the encoder and its node, none when nothing encodes
    pub encoder: Option<String>,

    // whether it is a GPU's; false for software
    pub gpu: bool,

    // the quantizer the quality is for it, none with no encoder
    pub quantizer: Option<u32>,

    // each way tried or skipped, and what it said
    pub notes: Vec<String>,

    // packages for a GPU encoder that is not in use
    pub hints: Vec<String>,

    // what a trial of the window's GStreamer chain with it said: Err when it fails, none when
    // there is no encoder or `gst-launch-1.0` to ask
    pub window: Option<Result<String, String>>,
}

// tries the encoders as a recording would, one frame each; takes a second or two
pub fn report(recording: &config::Recording) -> Report {
    let nodes = probe::nodes();
    let choice = probe::choice(recording, false, true);
    let kind = choice.encoder.as_ref().map(|encoder| encoder.kind);
    let window = choice.encoder.clone().and_then(|encoder| {
        probe::window_line(
            &encode::Encoding {
                encoder,
                quality: recording.quality,
                threads: probe::threads(),
                download: encode::Download::Gl,
            },
            recording.device.as_deref(),
        )
    });

    Report {
        window,
        encoder: choice.encoder.as_ref().map(ToString::to_string),
        gpu: kind.is_some_and(|kind| kind != encode::Kind::Software),
        quantizer: kind.map(|kind| encode::quantizer(recording.quality, kind)),
        hints: encode::hints(&nodes, &choice),
        notes: choice.notes,
    }
}

// stops the window casts an earlier Kanade left, at start
pub fn reclaim() {
    window::reclaim();

    // a crash after a resume leaves the segments in a hidden folder, which nothing else joins
    let spawned = thread::Builder::new()
        .name(String::from("capture-sweep"))
        .spawn(sweep);

    if let Err(error) = spawned {
        eprintln!("capture: cannot look for recordings an earlier run left: {error}");
    }
}

/*
 * joins the segments an earlier run left in a parts folder, as the end of that recording would
 * have: a crash after a resume leaves the first segment in the file and the rest beside it, and
 * nothing else would ever put them together
 */
fn sweep() {
    let Some(dir) = capture::home(FOLDER) else {
        return;
    };

    for (path, parts, segments) in parts_left(&dir) {
        if segments == 0 {
            continue;
        }

        // a folder that holds only the first segment is that recording's file, put back
        if segments == 1 && !path.exists() {
            let first = parts.join(plan::segment(1));

            if move_new(&first, &path).is_ok() {
                drop(fs::remove_file(parts.join(plan::LIST)));
                drop(fs::remove_file(parts.join(plan::JOINED)));
                drop(fs::remove_dir(&parts));
                eprintln!(
                    "capture: put back the recording {} an earlier run left",
                    path.display()
                );
            }

            continue;
        }

        match saved(&parts, &path, segments) {
            Ok(()) => eprintln!(
                "capture: joined the {segments} segments of {} an earlier run left",
                path.display()
            ),
            Err(why) => eprintln!(
                "capture: cannot join what an earlier run left in {}: {why}",
                parts.display()
            ),
        }
    }
}

/*
 * the parts folders in `dir` that hold footage, with the recording file beside each and the number
 * of its last segment: `.<stem>.parts` beside `<stem>.mp4`, as `parts` names them
 */
fn parts_left(dir: &Path) -> Vec<(PathBuf, PathBuf, u32)> {
    let Ok(listing) = fs::read_dir(dir) else {
        return Vec::new();
    };

    listing
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name();
            let stem = name
                .to_str()?
                .strip_prefix('.')?
                .strip_suffix(".parts")?
                .to_owned();
            let parts = entry.path();
            let path = dir.join(format!("{stem}.mp4"));
            let segments = last_segment(&parts)?;

            Some((path, parts, segments))
        })
        .collect()
}

// the highest numbered segment in `parts`, none when it holds no footage at all
fn last_segment(parts: &Path) -> Option<u32> {
    if !holds_footage(parts) {
        return None;
    }

    let entries = fs::read_dir(parts).ok()?;
    let mut last = 0;

    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(number) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".mp4"))
            .filter(|stem| stem.len() >= 3 && stem.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|stem| stem.parse::<u32>().ok())
        else {
            continue;
        };

        last = last.max(number);
    }

    Some(last)
}

// whether `session` is a window cast this Kanade made, which the privacy dots do not show as another's
pub(crate) fn cast_is_ours(session: u64) -> bool {
    window::ours(session)
}

// pre-warms encoder probing and window pipelines in a background thread so starting a recording does not wait
pub fn prewarm() {
    let _ = std::thread::Builder::new()
        .name(String::from("kanade-record-prewarm"))
        .spawn(|| {
            let recording = config::get().recording.clone();
            if let Ok(encoding) = probe::encoding(&recording, false) {
                let _ = probe::window(encoding, &recording, false);
            } else {
                let _ = probe::choice(&recording, false, false);
            }
        });
}

/*
 * the shell ends: what is held is let go here, as nothing else would stop the cast a window recording
 * has and the next start's reclaim would be the only one to
 */
pub fn shutdown() {
    let held = RUNTIME.locked().child.take();

    if let Some(held) = held {
        abandon(held);
    }
}

// saves the first `segments` segments as the recording's file
fn join(segments: u32) {
    let (parts, path) = {
        let runtime = RUNTIME.locked();

        match &runtime.take {
            Some(take) => (take.parts.clone(), take.path.clone()),
            None => return,
        }
    };

    feed(Event::Joined(saved(&parts, Path::new(&path), segments)));
}

/*
 * the file is the recording once its one segment is written to it. More are joined by copying their
 * streams (no re-encoding): the first, written to the file, is moved to the parts folder to be
 * joined with the rest into one beside them, which is then moved to the file's name. Neither move
 * replaces anything: a file that appeared at the name meanwhile is kept, not overwritten. Only once
 * the file is there are the segments Kanade made deleted, and only those: a failure keeps them, the
 * first segment put back where it was written
 */
fn saved(parts: &Path, path: &Path, segments: u32) -> Result<(), String> {
    if segments == 1 {
        if !fs::metadata(path).is_ok_and(|file| file.is_file() && file.len() > 0) {
            return Err(format!("the recording's file {} is gone", path.display()));
        }

        // nothing is in the folder, but for a segment that was begun and had no frame
        drop(fs::remove_dir(parts));

        return Ok(());
    }

    let first = parts.join(plan::segment(1));

    fs::create_dir_all(parts)
        .map_err(|error| format!("cannot keep the segments in {}: {error}", parts.display()))?;

    /*
     * the file holds the first segment unless it is already in the folder, as an interrupted join
     * leaves it; a recording an earlier run left has both halves where they are
     */
    if path.exists() && !first.exists() {
        move_new(path, &first).map_err(|error| {
            format!("cannot gather the segments in {}: {error}", parts.display())
        })?;
    } else if !first.exists() {
        return Err(format!(
            "neither {} nor {} is there",
            path.display(),
            first.display()
        ));
    }

    join_segments(parts, path, segments).inspect_err(|_| {
        // the first segment is the recording as far as it goes, so it is where it was
        if !path.exists() {
            drop(move_new(&first, path));
        }
    })?;

    for name in (1..=segments)
        .map(plan::segment)
        .chain([String::from(plan::LIST), String::from(plan::JOINED)])
    {
        drop(fs::remove_file(parts.join(name)));
    }

    // only if nothing else is in it
    drop(fs::remove_dir(parts));

    Ok(())
}

// joins the first `segments` segments in the parts folder, then moves the result to `path`
fn join_segments(parts: &Path, path: &Path, segments: u32) -> Result<(), String> {
    let names: Vec<String> = (1..=segments).map(plan::segment).collect();
    let list = parts.join(plan::LIST);
    let joined = parts.join(plan::JOINED);

    fs::write(&list, plan::list(&names))
        .map_err(|error| format!("cannot write {}: {error}", list.display()))?;

    let (Some(list_path), Some(joined_path)) = (list.to_str(), joined.to_str()) else {
        return Err(format!("{} is not UTF-8", parts.display()));
    };

    wake::act_within(JOINER, &plan::joiner(list_path, joined_path), JOINING).map_err(|failed| {
        match failed {
            Failed::Overran(limit) => {
                format!(
                    "{JOINER} did not join the segments within {}s",
                    limit.as_secs()
                )
            }
            Failed::Said(why) => format!("{JOINER} could not join the segments: {why}"),
        }
    })?;

    move_new(&joined, path).map_err(|error| match error.kind() {
        io::ErrorKind::AlreadyExists => {
            String::from("a file appeared at the recording's name and is left as it is")
        }
        _ => format!("cannot save the recording to {}: {error}", path.display()),
    })
}

/*
 * moves `from` to `to`, which must not exist: the kernel refuses a name taken (RENAME_NOREPLACE),
 * checked in its stead where the filesystem has no such rename
 */
fn move_new(from: &Path, to: &Path) -> io::Result<()> {
    match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(Errno::INVAL | Errno::NOSYS | Errno::NOTSUP) if to.symlink_metadata().is_err() => {
            fs::rename(from, to)
        }
        Err(Errno::INVAL | Errno::NOSYS | Errno::NOTSUP) => {
            Err(io::ErrorKind::AlreadyExists.into())
        }
        Err(error) => Err(error.into()),
    }
}

/*
 * the path being recorded to, none while none records. Read from the Activity the island holds, not
 * the recorder, so a view under the island's lock never waits on a recorder that waits on it
 */
pub fn recording(island: &IslandService) -> Option<String> {
    standing(island).path
}

// how the recording the island shows stands, for the Surface that offers to end or pause it
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Standing {
    // none while none records
    pub path: Option<String>,
    pub paused: bool,
    pub pausable: bool,
}

pub fn standing(island: &IslandService) -> Standing {
    match island.activity(&id()).map(Activity::detail) {
        Some(Detail::Recording(Clip::Recording {
            path,
            paused,
            pausable,
            ..
        })) => Standing {
            path: Some(path.clone()),
            paused: *paused,
            pausable: *pausable,
        },
        // joining the segments: still the recording, with nothing left to pause
        Some(Detail::Recording(Clip::Saving { path })) => Standing {
            path: Some(path.clone()),
            paused: false,
            pausable: false,
        },
        _ => Standing::default(),
    }
}

// one recording at a time, its end replacing it
fn id() -> Id {
    Id::new(Kind::Recording, "recording")
}

// Ongoing like a timer: over Media, a Satellite beside a higher primary, on every island
fn running(take: &Take, paused: bool) -> Activity {
    let mut actions = Vec::new();

    if take.pausable {
        actions.push(match paused {
            true => Action {
                key: String::from(RESUME),
                label: String::from("Resume"),
            },
            false => Action {
                key: String::from(PAUSE),
                label: String::from("Pause"),
            },
        });
    }

    actions.push(Action {
        key: String::from(STOP),
        label: String::from("Stop"),
    });

    Activity::new(
        id(),
        Priority::Ongoing,
        Lifetime::Persistent,
        Scope::Global,
        Interrupt::None,
    )
    .expect("a Persistent that does not auto-expand is valid")
    .with_actions(actions)
    .with_detail(Detail::Recording(Clip::Recording {
        path: take.path.clone(),
        output: take.source.as_ref().map(Source::said).unwrap_or_default(),
        audio: take.audio.heard().map(String::from),
        paused,
        pausable: take.pausable,
    }))
}

// the segments being joined, which can take a while: no action, as there is nothing left to do
fn saving(take: &Take) -> Activity {
    Activity::new(
        id(),
        Priority::Ongoing,
        Lifetime::Persistent,
        Scope::Global,
        Interrupt::None,
    )
    .expect("a Persistent that does not auto-expand is valid")
    .with_detail(Detail::Recording(Clip::Saving {
        path: take.path.clone(),
    }))
}

// on the output the user is looking at; a saved one copied or opened, a failed one as nothing else
// says that no recording came of it. `preempt` puts a failed one in front of what is shown: for a
// start the user just asked for, not for a failure that comes later (a join, a recorder that ends),
// which would collapse a Surface they are typing in
fn ended(clip: Clip, preempt: bool) -> Activity {
    let actions = match clip {
        Clip::Saved { .. } => vec![
            Action {
                key: String::from(COPY),
                label: String::from("Copy path"),
            },
            Action {
                key: String::from(SHOW),
                label: String::from("Open"),
            },
        ],
        _ => Vec::new(),
    };

    let interrupt = match (&clip, preempt) {
        (Clip::Failed { .. }, true) => Interrupt::Preempt,
        _ => Interrupt::None,
    };

    Activity::new(
        id(),
        Priority::Actionable,
        Lifetime::Transient(SHOWN),
        Scope::FocusedOutput,
        interrupt,
    )
    .expect("a Transient that does not auto-expand is valid")
    .with_actions(actions)
    .with_detail(Detail::Recording(clip))
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::process::Command;

    use super::*;

    #[test]
    fn a_failed_start_gives_way_once_per_step_and_never_loops() {
        let ran = |software, va| Ran { software, va };
        let given = |no_va, software| Given { no_va, software };

        // a VA-API download goes to EGL first, whatever the encoder, when the encoder failed
        assert_eq!(
            again(ran(false, true), given(false, false), Fault::Encoder),
            Some(Again::Egl)
        );
        assert_eq!(
            again(ran(true, true), given(false, false), Fault::Encoder),
            Some(Again::Egl)
        );

        // not when the muxer did: the download is not to blame, so a hardware encoder goes on
        assert_eq!(
            again(ran(false, true), given(false, false), Fault::Muxer),
            Some(Again::Software)
        );
        assert_eq!(
            again(ran(true, true), given(false, false), Fault::Muxer),
            None
        );

        // software on EGL has nowhere to go
        assert_eq!(
            again(ran(true, false), given(true, true), Fault::Encoder),
            None
        );
        assert_eq!(
            again(ran(true, false), given(false, false), Fault::Encoder),
            None
        );

        // the walk of a window whose encoder fails every time, as the recording keeps it: the most
        // steps, then none. NVENC on VA-API, then on EGL, then software on VA-API (its own failure
        // is not the download's), then on EGL. It walks `step`, which is what `falls_back` walks
        let mut state = given(false, false);
        let mut steps = 0;
        let mut walked = Vec::new();

        loop {
            let ran = ran(state.software, !state.no_va);

            let Some((again, next)) = step(state, ran, Fault::Encoder) else {
                break;
            };

            walked.push(again);
            state = next;
            steps += 1;
            assert!(steps <= 3);
        }

        assert_eq!(steps, 3);
        assert_eq!(
            walked,
            [Again::Egl, Again::Software, Again::Egl],
            "a step that stops clearing no_va loops"
        );
    }

    #[test]
    fn the_verb_reads_its_target_and_audio() {
        let start = |target, audio| Some(Request::Start(Start { target, audio }));
        let area = Rect {
            x: 10,
            y: 20,
            width: 300,
            height: 200,
        };

        assert_eq!(
            Request::parse(&["start"]),
            start(Target::Screen, Audio::None)
        );
        assert_eq!(
            Request::parse(&["start", "mic"]),
            start(Target::Screen, Audio::Mic)
        );
        assert_eq!(
            Request::parse(&["start", "screen", "system"]),
            start(Target::Screen, Audio::System)
        );
        assert_eq!(
            Request::parse(&["start", "area"]),
            start(Target::Area(None), Audio::None)
        );
        // the rectangle in one word or two
        assert_eq!(
            Request::parse(&["start", "area", "10,20 300x200", "none"]),
            start(Target::Area(Some(area)), Audio::None)
        );
        assert_eq!(
            Request::parse(&["start", "area", "10,20", "300x200", "system"]),
            start(Target::Area(Some(area)), Audio::System)
        );

        // the focused window at the start, or one by its id; mic as ever only when asked for
        assert_eq!(
            Request::parse(&["start", "window"]),
            start(Target::Window(None), Audio::None)
        );
        assert_eq!(
            Request::parse(&["start", "window", "149"]),
            start(Target::Window(Some(149)), Audio::None)
        );
        assert_eq!(
            Request::parse(&["start", "window", "149", "system"]),
            start(Target::Window(Some(149)), Audio::System)
        );
        assert_eq!(
            Request::parse(&["start", "window", "mic"]),
            start(Target::Window(None), Audio::Mic)
        );

        for bad in [
            &["start", "window", "kitty"][..],
            &["start", "window", "-3"],
            &["start", "window", "1", "2"],
            &["start", "screen", "window"],
            &["start", "area", "nowhere"],
            &["start", "screen", "area"],
            &["start", "loud"],
            &["pause", "now"],
            &["record"],
        ] {
            assert_eq!(Request::parse(bad), None, "{bad:?}");
        }

        assert_eq!(Request::parse(&["pause"]), Some(Request::Pause));
        assert_eq!(Request::parse(&["resume"]), Some(Request::Resume));
        assert_eq!(Request::parse(&["stop"]), Some(Request::Stop));
        assert_eq!(Request::parse(&["status"]), Some(Request::Status));
    }

    // a recording is started or taken up again only with the session unlocked; ending is always allowed
    #[test]
    fn a_locked_session_starts_and_resumes_nothing() {
        let start = Request::Start(Start {
            target: Target::Screen,
            audio: Audio::None,
        });

        for request in [&start, &Request::Resume] {
            assert!(allowed(request, true).is_err());
            assert!(allowed(request, false).is_ok());
        }

        for request in [Request::Pause, Request::Stop, Request::Status] {
            assert!(allowed(&request, true).is_ok());
        }
    }

    #[test]
    fn the_segments_are_beside_the_file_and_hidden() {
        assert_eq!(
            parts(Path::new(
                "/h/Videos/Screencasts/Screencast from 2026-10-07 15-36-38 (2).mp4"
            )),
            Path::new("/h/Videos/Screencasts/.Screencast from 2026-10-07 15-36-38 (2).parts")
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kanade-recording-{name}-{}", std::process::id()));

        drop(fs::remove_dir_all(&dir));
        fs::create_dir_all(&dir).expect("a scratch folder");
        dir
    }

    fn exited(code: i32) -> io::Result<ExitStatus> {
        Ok(ExitStatus::from_raw(code << 8))
    }

    fn killed(signal: i32) -> io::Result<ExitStatus> {
        Ok(ExitStatus::from_raw(signal))
    }

    // a resume stopped at once ends by SIGINT before the recorder has a handler: that wrote nothing
    #[test]
    fn a_segment_interrupted_before_any_frame_is_empty_not_lost() {
        let none = VecDeque::new();

        assert_eq!(
            classify(killed(libc::SIGINT), false, none.clone()),
            Segment::Empty
        );
        assert_eq!(classify(exited(0), false, none.clone()), Segment::Empty);
        assert_eq!(classify(exited(0), true, none.clone()), Segment::Written);
        // one with frames that died unfinished is no whole file
        assert!(matches!(
            classify(killed(libc::SIGINT), true, none.clone()),
            Segment::Lost(_)
        ));
        assert!(matches!(
            classify(killed(libc::SIGKILL), false, none.clone()),
            Segment::Lost(_)
        ));
        assert!(matches!(classify(exited(1), false, none), Segment::Lost(_)));
    }

    // the recorder's empty file is deleted, a file with frames never
    #[test]
    fn only_an_empty_file_is_discarded() {
        let dir = scratch("discard");
        let (empty, full) = (dir.join("a.mp4"), dir.join("b.mp4"));

        fs::write(&empty, b"").unwrap();
        fs::write(&full, b"frames").unwrap();
        discard_empty(&empty);
        discard_empty(&full);
        discard_empty(&dir.join("missing.mp4"));

        assert!(!empty.exists());
        assert!(full.exists());
        drop(fs::remove_dir_all(&dir));
    }

    // a folder of nothing but empty files of Kanade's is let go and says nothing was kept; one with
    // frames, or a file of someone else's, stays and is named
    #[test]
    fn a_lost_recording_names_only_what_there_is_of_it() {
        let dir = scratch("kept");
        let (path, parts) = (dir.join("a.mp4"), dir.join(".a.parts"));

        fs::create_dir(&parts).unwrap();
        fs::write(parts.join("002.mp4"), b"").unwrap();
        assert_eq!(kept(&path, &parts, String::from("gone")), "gone");
        assert!(!parts.exists());

        fs::create_dir(&parts).unwrap();
        fs::write(parts.join("002.mp4"), b"frames").unwrap();
        assert!(kept(&path, &parts, String::from("gone")).contains(&*parts.to_string_lossy()));

        // the file at the name is Kanade's (the first segment was put back): both are named
        fs::write(&path, b"frames").unwrap();
        assert!(kept(&path, &parts, String::from("gone")).contains(" and "));

        // the first segment still in the folder: the file at the name is someone else's, so the
        // name is said to be taken and only the folder is named as holding the recording
        fs::write(&path, b"theirs").unwrap();
        fs::write(parts.join(plan::segment(1)), b"frames").unwrap();
        let said = kept(&path, &parts, String::from("gone"));
        assert!(said.contains("was taken") && said.ends_with(&*parts.to_string_lossy()));
        assert!(
            !said
                .split("kept in")
                .nth(1)
                .unwrap()
                .contains(&*path.to_string_lossy())
        );

        fs::remove_file(&path).unwrap();
        fs::remove_file(parts.join(plan::segment(1))).unwrap();
        fs::remove_file(parts.join("002.mp4")).unwrap();
        fs::write(parts.join("notes.txt"), b"").unwrap();
        assert!(kept(&path, &parts, String::from("gone")).contains("kept in"));
        assert!(parts.join("notes.txt").exists());
        drop(fs::remove_dir_all(&dir));
    }

    // data integrity: the file a recording is saved to is never replaced by a move
    #[test]
    fn a_move_never_replaces_a_name_taken() {
        let dir = scratch("move");
        let (from, to) = (dir.join("from.mp4"), dir.join("to.mp4"));

        fs::write(&from, b"new").unwrap();
        fs::write(&to, b"taken").unwrap();

        assert_eq!(
            move_new(&from, &to).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&to).unwrap(), b"taken");
        assert_eq!(fs::read(&from).unwrap(), b"new");

        fs::remove_file(&to).unwrap();
        move_new(&from, &to).unwrap();
        assert_eq!(fs::read(&to).unwrap(), b"new");
        drop(fs::remove_dir_all(&dir));
    }

    // a recording never paused is its file already: nothing is moved or joined, and an empty folder goes
    #[test]
    fn a_recording_of_one_segment_is_saved_where_it_was_written() {
        let dir = scratch("one");
        let (path, parts) = (dir.join("a.mp4"), dir.join(".a.parts"));

        assert!(saved(&parts, &path, 1).is_err());

        fs::write(&path, b"frames").unwrap();
        fs::create_dir(&parts).unwrap();
        assert_eq!(saved(&parts, &path, 1), Ok(()));
        assert_eq!(fs::read(&path).unwrap(), b"frames");
        assert!(!parts.exists());
        drop(fs::remove_dir_all(&dir));
    }

    // the picker prints nothing and exits 1 when the user leaves it, which is no failure
    #[test]
    fn a_picker_the_user_leaves_is_no_area() {
        let exited = |code: i32| Ok(ExitStatus::from_raw(code << 8));

        assert_eq!(chosen_area(exited(1), "", ""), Ok(None));
        assert_eq!(
            chosen_area(exited(0), "10,20 301x200\n", ""),
            Ok(Some(Rect {
                x: 10,
                y: 20,
                width: 300,
                height: 200
            }))
        );
        assert!(chosen_area(exited(0), "", "").is_err());
        assert!(chosen_area(exited(0), "10,20 1x200\n", "").is_err());
        assert!(chosen_area(exited(1), "", "compositor does not support layer shell").is_err());
        assert!(chosen_area(exited(127), "", "").is_err());
    }

    fn said(lines: &[&str]) -> VecDeque<String> {
        lines.iter().map(|line| String::from(*line)).collect()
    }

    fn segment(
        writer: io::Result<ExitStatus>,
        encoder: io::Result<ExitStatus>,
        written: bool,
        closed: Option<u32>,
    ) -> Segment {
        segment_said(writer, encoder, &["writer"], &["encoder"], written, closed)
    }

    fn segment_said(
        writer: io::Result<ExitStatus>,
        encoder: io::Result<ExitStatus>,
        writer_said: &[&str],
        encoder_said: &[&str],
        written: bool,
        closed: Option<u32>,
    ) -> Segment {
        classify_window(
            [writer, encoder],
            [said(writer_said), said(encoder_said)],
            written,
            closed,
            false,
            encoder_said.iter().any(|line| stalled_of(line)),
        )
    }

    /*
     * a window that closes ends the encoder, then the writer with its file whole: the recording is
     * saved, as a stop would; one that ends with nothing written has nothing to save
     */
    #[test]
    fn a_window_that_closes_ends_the_segment_not_the_recording() {
        // told to stop: the encoder was interrupted and finished the stream
        assert_eq!(segment(exited(0), exited(0), true, None), Segment::Written);
        assert_eq!(
            segment(exited(0), killed(libc::SIGINT), true, None),
            Segment::Written
        );
        // closed: the encoder failed, the file is whole
        assert_eq!(
            segment(exited(0), exited(1), true, Some(1)),
            Segment::Written
        );
        // the same without the window closing is a failure of the encoder, named
        let Segment::Lost(why) = segment(exited(0), exited(1), true, None) else {
            panic!("saved");
        };
        assert!(why.contains(ENCODER) && why.contains("encoder"), "{why}");

        // closed before a frame: the first segment is nothing, a later one leaves the earlier whole
        assert!(matches!(
            segment(exited(1), exited(1), false, Some(1)),
            Segment::Lost(_)
        ));
        assert_eq!(
            segment(exited(1), exited(1), false, Some(2)),
            Segment::Empty
        );
    }

    #[test]
    fn a_window_stopped_before_a_frame_is_empty_and_a_missing_program_is_said() {
        assert_eq!(segment(exited(0), exited(0), false, None), Segment::Empty);
        // the writer fails on a stream with nothing in it, which is no failure
        assert_eq!(
            segment_said(
                exited(1),
                exited(0),
                &["nothing was encoded"],
                &[],
                false,
                None
            ),
            Segment::Empty
        );
        assert_eq!(
            segment_said(
                exited(1),
                killed(libc::SIGINT),
                &["Output file does not contain any stream"],
                &[],
                false,
                None
            ),
            Segment::Empty
        );
        // a writer that failed for its own reason is named, not hidden as an empty segment
        let Segment::Lost(why) = segment(exited(1), exited(0), false, None) else {
            panic!("empty");
        };
        assert!(why.contains(MUXER) && why.contains("writer"), "{why}");
        // the writer failed: its words, the encoder having only found its pipe closed
        let Segment::Lost(why) = segment(exited(1), exited(0), true, None) else {
            panic!("saved");
        };
        assert!(why.contains(MUXER) && why.contains("writer"), "{why}");
        assert!(matches!(
            segment(exited(127), exited(0), false, None),
            Segment::Lost(why) if why.contains("not found")
        ));
        assert!(matches!(
            segment(exited(0), exited(127), false, None),
            Segment::Lost(why) if why.contains(ENCODER)
        ));
    }

    /*
     * a confirmed defect, once, at the seam it crossed: a window's chain that stopped receiving
     * buffers hung until the user stopped the take, because nothing failed and `falls_back` only
     * walks a `Segment::Lost`. The guard (`encode::guard`) now ends it, and what it says must be
     * read as the stall it is, not as an encoder that would not start
     */
    #[test]
    fn a_chain_the_guard_stopped_is_a_stall_and_walks_the_fallback() {
        let stopped = [
            "ERROR: from element /GstPipeline:pipeline0/GstWatchdog:watchdog0: Watchdog triggered",
        ];

        // nothing written, the window still there: a stall, named as one and not as the encoder
        let Segment::Lost(why) = segment_said(
            exited(234),
            exited(1),
            &["Output file does not contain any stream"],
            &stopped,
            false,
            None,
        ) else {
            panic!("not a stall");
        };

        assert!(why.contains("no picture arrived"), "{why}");
        assert!(why.contains(&QUIET.as_secs().to_string()), "{why}");
        assert!(!why.contains("failed to start"), "{why}");

        // a window that closed ends the cast and trips the guard too, and that is a saving, not a
        // stall: the closed cases are read before it
        assert_eq!(
            segment_said(exited(0), exited(1), &["writer"], &stopped, true, Some(1)),
            Segment::Written
        );

        // a stall walks to EGL and then to software, as a failed encoder does, and stops there
        let ran = Ran {
            software: false,
            va: true,
        };
        let given = Given {
            no_va: false,
            software: false,
        };
        let (again, after) = step(given, ran, Fault::Stalled).expect("nowhere to go");

        assert_eq!(
            (again, after),
            (
                Again::Egl,
                Given {
                    no_va: true,
                    ..given
                }
            )
        );

        let (again, after) = step(after, ran, Fault::Stalled).expect("nowhere to go");

        assert_eq!(
            (again, after),
            (
                Again::Software,
                Given {
                    no_va: false,
                    software: true
                }
            )
        );

        // software tries the VA-API download once more (the stall may have been the encoder's own),
        // then there is nowhere left: three restarts at most, as for a failed encoder
        let (again, after) = step(after, ran, Fault::Stalled).expect("nowhere to go");

        assert_eq!(
            (again, after),
            (
                Again::Egl,
                Given {
                    no_va: true,
                    software: true
                }
            )
        );
        assert_eq!(step(after, ran, Fault::Stalled), None);
    }

    fn window_take() -> Take {
        Take {
            path: String::from("/h/a.mp4"),
            parts: PathBuf::from("/h/.a.parts"),
            source: Some(Source::Window {
                id: 7,
                size: (800, 600),
                app: None,
            }),
            audio: Audio::None,
            pausable: true,
            closed: false,
            software: None,
            no_va: false,
            recording: config::get().recording.clone(),
        }
    }

    // a recording running its first segment, with a window's take
    fn windowed() -> (Runtime, u64) {
        let mut runtime = Runtime::new();

        runtime.machine.step(Event::Start { pick: false });

        let serial = runtime.machine.serial().expect("a recording");

        runtime.take = Some(window_take());

        (runtime, serial)
    }

    /*
     * a confirmed defect: the guard's verdict was read from the encoder's lines once its reader was
     * joined, and a reader a descendant held past `SAID_JOINED` gave none of them back, so a chain
     * that had stopped receiving buffers was read as the writer's own failure: no fallback was
     * walked, and the user got the muxer's "Output file does not contain any stream" for a stall
     */
    #[test]
    fn a_stall_whose_reader_missed_the_deadline_is_still_a_stall() {
        let (mut printed, pipe) = UnixStream::pair().expect("a pipe");
        let reader = reading(ChildStderr::from(OwnedFd::from(pipe))).expect("a reader");

        // the guard's own error, as `encode::guard` prints it, then the pipe held open by a
        // descendant: the reader has its line and never finishes
        printed
            .write_all(
                b"ERROR: from element /GstPipeline:pipeline0/GstWatchdog:watchdog0: Watchdog triggered\n",
            )
            .expect("a line");
        printed.flush().expect("a line");

        let deadline = Instant::now() + Duration::from_secs(5);

        while !reader.stalled() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }

        assert!(reader.stalled());
        assert!(!reader.ended_within(SAID_JOINED));

        // the encoder ended as a guarded one does and the writer is left to say it wrote no
        // stream, so nothing is blamed on the encoder (`encoder_blamed` is false)
        let (_, words) = finish_encoder(ended_child(), Some(&reader));

        // the derivation `follow_window` makes: the segment is classified with the guard's verdict,
        // which the reader holds whether or not a join ran out of time
        let quiet = reader.stalled();
        let segment = classify_window(
            [exited(234), exited(1)],
            [
                said(&["Output file does not contain any stream"]),
                words.clone(),
            ],
            true,
            None,
            false,
            quiet,
        );
        let fault = fault_of(&segment, quiet, false);

        assert_eq!(fault, Fault::Stalled, "{words:?}");

        // and it is said as the stall it is, not as the muxer's missing stream
        let Segment::Lost(why) = &segment else {
            panic!("a stall is lost, not written: {segment:?}");
        };
        assert!(why.contains("no picture arrived"), "{why}");

        // and it is walked as a stall: the download is changed first, the encoder not given up
        let ran = Ran {
            software: false,
            va: true,
        };
        let (mut runtime, serial) = windowed();

        assert_eq!(
            falls_back(&mut runtime, serial, &segment, ran, fault),
            Some(1)
        );

        let take = runtime.take.as_ref().expect("a take");

        assert!(take.no_va);
        assert!(take.software.is_none());
    }

    // an encoder that has ended as it should
    fn ended_child() -> Child {
        Command::new("true").spawn().expect("a child")
    }
    /*
     * a confirmed defect: a stall after the segment had written frames ended the recording as
     * failed instead of walking the encoder fallback, because `falls_back` gave up on any segment
     * that wrote. A stall now walks it, on the segment it is, so its frames stay where they are
     */
    #[test]
    fn a_stall_after_frames_walks_the_fallback_on_the_same_segment() {
        let ran = Ran {
            software: false,
            va: true,
        };
        let (mut runtime, serial) = windowed();
        let stalled = Segment::Lost(format!("no picture arrived for {}s", QUIET.as_secs()));

        // the download is changed first, on the segment the stall happened in
        assert_eq!(
            falls_back(&mut runtime, serial, &stalled, ran, Fault::Stalled),
            Some(1)
        );
        assert!(runtime.take.as_ref().expect("a take").no_va);

        // then the software encoder, as for a failed one
        assert_eq!(
            falls_back(&mut runtime, serial, &stalled, ran, Fault::Stalled),
            Some(1)
        );
        assert!(runtime.take.as_ref().expect("a take").software.is_some());

        // a muxer that failed says nothing of the encoder: the walk is not the muxer's
        let (mut muxer, serial) = windowed();
        let muxed = Segment::Lost(String::from("ffmpeg ended (exit status: 1): Output file"));

        assert_eq!(
            falls_back(&mut muxer, serial, &muxed, ran, Fault::Muxer),
            Some(1)
        );

        // a program that is not there mends nothing, and neither does a segment that is not lost
        let (mut missing, serial) = windowed();

        assert_eq!(
            falls_back(
                &mut missing,
                serial,
                &Segment::Lost(format!("{ENCODER} not found")),
                ran,
                Fault::Encoder
            ),
            None
        );
        assert_eq!(
            falls_back(&mut missing, serial, &Segment::Written, ran, Fault::Encoder),
            None
        );

        // an output that is gone is no encoder's failure
        let (mut gone, serial) = windowed();

        assert_eq!(
            falls_back(
                &mut gone,
                serial,
                &Segment::Lost(String::from("the output Virtual-1 is gone")),
                ran,
                Fault::Vanished
            ),
            None
        );
    }

    /*
     * a confirmed defect: a segment lost on a resume ended the recording as failed and left the
     * earlier segments in the hidden folder, so the user got a split recording and a Failed clip
     * for damage to the last segment only. They are joined into the one file instead
     */
    #[test]
    fn a_lost_segment_on_a_resume_still_joins_what_was_recorded() {
        let dir = scratch("resume-lost");
        let (path, parts) = (dir.join("a.mp4"), dir.join(".a.parts"));

        // a resume writes its segment beside the file, which holds the first
        fs::create_dir(&parts).unwrap();
        fs::write(&path, b"first").unwrap();
        fs::write(parts.join(plan::segment(2)), b"damaged").unwrap();

        // what the machine asks for after a stall on segment 2: one segment, and it is the file.
        // The take is a recording saved, not a failed one, so nothing is joined and the file stays
        assert_eq!(saved(&parts, &path, 1), Ok(()));
        assert_eq!(fs::read(&path).unwrap(), b"first");
        // what the stalled segment wrote is kept, not deleted: Kanade deletes only a join's own
        assert_eq!(fs::read(parts.join(plan::segment(2))).unwrap(), b"damaged");
        drop(fs::remove_dir_all(&dir));
    }

    /*
     * a stop that had to kill an encoder which did write frames saved a recording: the kill is the
     * stop's, not a failure of the encoder's
     */
    #[test]
    fn an_encoder_a_stop_killed_after_writing_is_a_recording() {
        let escalated = |writer, encoder, written, closed| {
            classify_window(
                [writer, encoder],
                [said(&["writer"]), said(&["encoder"])],
                written,
                closed,
                true,
                false,
            )
        };

        assert_eq!(
            escalated(exited(0), killed(libc::SIGKILL), true, None),
            Segment::Written
        );
        // nothing written is still nothing, the kill or not
        assert_ne!(
            escalated(exited(0), killed(libc::SIGKILL), false, None),
            Segment::Written
        );
    }

    /*
     * a muxer that dies first leaves the encoder to fail on a broken pipe, and a vanished output
     * cannot be mended by another encoder: neither is the encoder's fault
     */
    #[test]
    fn only_the_encoders_own_failure_is_another_start() {
        let given = Given {
            no_va: false,
            software: false,
        };
        let va = Ran {
            software: false,
            va: true,
        };

        assert!(encoder_blamed(&exited(0), &exited(1)));
        assert!(!encoder_blamed(&exited(1), &exited(1)));
        assert!(!encoder_blamed(&exited(0), &exited(0)));

        assert_eq!(
            step(given, va, Fault::Encoder),
            Some((
                Again::Egl,
                Given {
                    no_va: true,
                    software: false
                }
            ))
        );
        // the muxer's own failure is no reason to move the download
        assert_eq!(
            step(given, va, Fault::Muxer),
            Some((
                Again::Software,
                Given {
                    no_va: false,
                    software: true
                }
            ))
        );
        // nothing is tried for an output that is gone
        assert_eq!(step(given, va, Fault::Vanished), None);

        assert!(vanished("the output DP-1 is gone"));
        assert!(vanished("wf-recorder: no such window"));
        assert!(!vanished("h264_vaapi: cannot open the device"));
    }
}
