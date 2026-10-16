//! Screen recording (docs/design.md Capture, ADR 0013, ADR 0116): `kanade capture record start`
//! records the focused output or an area with wf-recorder, encoded on the GPU through VA-API, with
//! no sound, the speakers' or the microphone's, to `~/Videos/Screencasts`; `pause` and `resume` end
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
//! thread that started it.

mod machine;
mod plan;

use std::collections::VecDeque;
use std::fmt;
use std::fs;
use std::io::{self, BufRead, BufReader, Read};
use std::os::unix::process::ExitStatusExt;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ExitStatus};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::{MutexExt, service::Service};
use rustix::fs::{CWD, RenameFlags, renameat_with};
use rustix::io::Errno;

use self::machine::{Effect, Event, Machine, Phase, Pick, Segment};
use self::plan::Source;
pub use self::plan::{Audio, JOINER, PICKER, RECORDER, Rect};
use super::capture::{self, COPY, SHOW, SHOWN};
use super::json::Json;
use super::niri;
use super::wake::{self, Failed, NOT_FOUND};
use crate::clock;
use crate::island::activity::{
    Action, Activity, Clip, Detail, Id, Interrupt, Kind, Lifetime, Priority, Scope,
};
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

    // the recorder that runs, reaped only once taken out, so while here its pid is its own
    child: Option<(u64, Child)>,

    // the area picker's pid while it runs, for the one who ends it; reaped by the worker
    picker: Option<u32>,

    // the `kanade` calls waiting on something of the recording
    waiting: Vec<(Wait, Later)>,

    // the last recording that ended, until another starts
    ended: Option<Ended>,

    /*
     * every path a recording was given since Kanade started, never given again, so a path names
     * one recording: a stale Stop or `kanade` waiting on an older one never mistakes a newer one
     */
    issued: Vec<String>,
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
                if let Some(pid) = self.picker.take() {
                    // SIGTERM: the picker inherits Kanade's ignored SIGINT
                    drop(clock::terminate_pid(pid));
                }
            }
            Effect::Spawn { serial, segment } => {
                if let Some(take) = &self.take {
                    post(running(take, false));
                }

                self.hand(Job::Spawn { serial, segment }, |why| Event::Spawned {
                    serial,
                    started: Err(why),
                });
            }
            Effect::Interrupt => {
                if let Some((_, child)) = &self.child
                    && let Err(error) = clock::interrupt(child)
                {
                    eprintln!("capture: cannot stop {RECORDER}: {error}");
                }
            }
            Effect::Join { segments } => {
                // one segment is the file already; joining more takes a while, which is shown
                if segments > 1
                    && let Some(take) = &self.take
                {
                    post(saving(take));
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

                if let Some(take) = &self.take {
                    post(running(take, true));
                }

                for later in self
                    .answer(Wait::Paused)
                    .into_iter()
                    .chain(self.answer(Wait::End))
                {
                    later.refused(why.clone());
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
            },
            Some(why) => Clip::Failed {
                path: take.path.clone(),
                why: why.clone(),
            },
        };

        self.ended = Some(Ended {
            path: take.path,
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
}

impl Request {
    /*
     * `start [screen|area [<x,y WxH>]] [none|system|mic]`, `pause`, `resume`, `stop` or `status`,
     * the words after `record`. The rectangle is one word or two
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
    });

    if pick {
        // the user takes their time, which `later` would not wait for
        runtime.feed(Event::Start { pick })?;
        later.done("choose the area on screen; recording starts when you have");
        return Ok(());
    }

    later.unsettled(format!(
        "{RECORDER} wrote no frame within {}s; it may still record to {path}",
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
        "{RECORDER} did not finish the segment within {}s; the recording may still pause",
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
        "{RECORDER} wrote no frame within {}s; it may still record to {}",
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

        match take.audio.heard() {
            Some(heard) => format!("{what}, {heard}"),
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
            Some(Ended { path, lost: None }) => Status::Saved { path: path.clone() },
            Some(Ended {
                path,
                lost: Some(why),
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

    let last = matches!(
        &runtime.ended,
        Some(Ended { path: saved, lost: None }) if saved == path
    );

    if !runtime.machine.busy() && last {
        post(ended(
            Clip::Saved {
                path: path.to_owned(),
                copied: true,
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

        if runtime.machine.serial() == Some(serial) {
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
                )
            }
            _ => return,
        }
    };

    let (source, audio, phase, file) = chosen;

    // a stop or a lock came while it was handed over: nothing is recorded, not even a microphone
    if matches!(phase, Phase::Running { stopping: true, .. }) {
        return feed(Event::Ended {
            serial,
            segment: Segment::Empty,
        });
    }

    let folder = file.parent().map(Path::to_path_buf).unwrap_or_default();
    let started = fs::create_dir_all(&folder)
        .map_err(|error| format!("no folder {}: {error}", folder.display()))
        .and_then(|()| {
            let file = file
                .to_str()
                .ok_or_else(|| format!("{} is not UTF-8", file.display()))?;
            let arguments = plan::recorder(&source, audio, file);
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
        .spawn(move || follow(said, serial, file));

    if let Err(error) = followed {
        drop(child.kill());
        drop(child.wait());

        return drop(runtime.feed(Event::Spawned {
            serial,
            started: Err(format!("cannot follow {RECORDER}: {error}")),
        }));
    }

    runtime.child = Some((serial, child));
    drop(runtime.feed(Event::Spawned {
        serial,
        started: Ok(()),
    }));
}

// reads what the recorder prints until it exits, then tells the machine how its segment ended
fn follow(said: Option<ChildStderr>, serial: u64, file: PathBuf) {
    survive(
        || {
            let said = last_lines(said);
            let mut runtime = RUNTIME.locked();

            // under the lock, so a stop that interrupts it never signals a pid that is reaped
            let Some((_, mut child)) = runtime.child.take_if(|(now, _)| *now == serial) else {
                return;
            };

            let status = child.wait();
            let written = fs::metadata(&file).is_ok_and(|file| file.len() > 0);

            // an empty file is one Kanade's recorder made, and is no recording
            if !written {
                discard_empty(&file);
            }

            drop(runtime.feed(Event::Ended {
                serial,
                segment: classify(status, written, said),
            }));
        },
        || {
            let mut runtime = RUNTIME.locked();

            if let Some((_, mut child)) = runtime.child.take_if(|(now, _)| *now == serial) {
                drop(child.kill());
                drop(child.wait());
            }

            drop(runtime.feed(Event::Ended {
                serial,
                segment: Segment::Lost(String::from("internal error")),
            }));
        },
    );
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
    move_new(path, &first)
        .map_err(|error| format!("cannot gather the segments in {}: {error}", parts.display()))?;

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
    use super::*;

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

        for bad in [
            &["start", "window"][..],
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
}
