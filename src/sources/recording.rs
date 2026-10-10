//! Screen recording (#140, docs/design.md Capture, ADR 0013): `kanade capture record start` records
//! the focused output with wf-recorder, encoded on the GPU through VA-API, to `~/Videos/Screencasts`;
//! `stop` finishes the file and `status` says how the recording stands. wf-recorder captures
//! through niri's wlr-screencopy, which niri counts as a cast, so the privacy cluster shows it
//! without asking this Module. While it records, a Persistent Ongoing Recording Activity offers
//! Stop; once saved, a Transient one copies its path or opens it, as a screenshot's does.
//!
//! niri hands a recorder a frame only once the screen changes, and on a still screen that is often
//! the island the draw thread, which answers IPC, would draw, so neither `start` nor `stop` waits on
//! the recorder: each answers at once, and `kanade` waits on `status`.
//!
//! wf-recorder is a holder (ADR 0011): one recording runs while it does. Ending on its own, it ended
//! the recording, which is never started again.

use std::collections::VecDeque;
use std::fmt;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, ChildStderr};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::service::Service;

use super::capture::{self, COPY, SHOW, SHOWN};
use super::json::Json;
use super::niri;
use super::wake::{self, NOT_FOUND};
use crate::clock;
use crate::island::activity::{
    Action, Activity, Clip, Detail, Id, Interrupt, Kind, Lifetime, Priority, Scope,
};
use crate::island::service::IslandService;

pub const RECORDER: &str = "wf-recorder";

// the recording's action that ends it
pub const STOP: &str = "stop";

// H.264 on the GPU through VA-API (ADR 0013)
const CODEC: &str = "h264_vaapi";

// the last lines the recorder printed, which say why it ended
const SAID: usize = 4;

static RECORDER_STATE: Mutex<Recorder> = Mutex::new(Recorder {
    running: None,
    ended: None,
    issued: Vec::new(),
});

#[derive(Default)]
struct Recorder {
    // the one recording, none while none records
    running: Option<Running>,

    // the last recording that ended, until another starts
    ended: Option<Ended>,

    /*
     * every path a recording was given since Kanade started, never given again, so a path names
     * one recording: a stale Stop or `kanade` waiting on an older one never mistakes a newer one
     */
    issued: Vec<String>,
}

struct Running {
    path: String,
    output: String,

    // reaped only once taken out of `running`, so while there its pid is its own
    child: Child,

    // asked to finish, so its end is no surprise
    stopping: bool,
}

struct Ended {
    path: String,

    // none once saved, else why nothing was
    lost: Option<String>,
}

// what `kanade capture record` asks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Start,
    Stop,
    Status,
}

impl Request {
    pub fn parse(word: &str) -> Option<Request> {
        match word {
            "start" => Some(Request::Start),
            "stop" => Some(Request::Stop),
            "status" => Some(Request::Status),
            _ => None,
        }
    }
}

/*
 * how the recording stands, as `status` says it: a line a person reads, and `kanade` parses back
 * to wait on `start` and `stop`. An output's name holds no space
 */
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    // none since Kanade started
    Idle,

    // the recorder runs but wrote no frame yet
    Starting { output: String, path: String },

    Recording { output: String, path: String },

    // asked to finish the file
    Finishing { path: String },

    Saved { path: String },

    Failed { path: String, why: String },
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Status::Idle => write!(f, "not recording"),
            Status::Starting { output, path } => write!(f, "starting to record {output} to {path}"),
            Status::Recording { output, path } => write!(f, "recording {output} to {path}"),
            Status::Finishing { path } => write!(f, "finishing {path}"),
            Status::Saved { path } => write!(f, "saved {path}"),
            Status::Failed { path, why } => write!(f, "failed to record to {path}\n{why}"),
        }
    }
}

impl Status {
    pub fn parse(text: &str) -> Option<Status> {
        let recording = |rest: &str| {
            rest.split_once(" to ")
                .map(|(output, path)| (output.to_owned(), path.to_owned()))
        };

        if text == "not recording" {
            Some(Status::Idle)
        } else if let Some(rest) = text.strip_prefix("starting to record ") {
            recording(rest).map(|(output, path)| Status::Starting { output, path })
        } else if let Some(rest) = text.strip_prefix("recording ") {
            recording(rest).map(|(output, path)| Status::Recording { output, path })
        } else if let Some(path) = text.strip_prefix("finishing ") {
            Some(Status::Finishing {
                path: path.to_owned(),
            })
        } else if let Some(path) = text.strip_prefix("saved ") {
            Some(Status::Saved {
                path: path.to_owned(),
            })
        } else {
            let (path, why) = text
                .strip_prefix("failed to record to ")?
                .split_once('\n')?;

            Some(Status::Failed {
                path: path.to_owned(),
                why: why.to_owned(),
            })
        }
    }
}

// where a request has left the recording at its path, as `status` says
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    // not there yet
    Waiting,

    Done,

    // the recording failed, as the recorder said
    Failed(String),

    // the shell says how another recording stands, or none, so what became of this one is unknown
    Lost(String),
}

impl Request {
    /*
     * whether the recording at `path` is where this request leaves it: started once its first
     * frame is written, stopped once its file is saved. Only a status of `path` settles it
     */
    pub fn settled(self, path: &str, status: &Status) -> Settled {
        match (self, status) {
            (Request::Status, _) => Settled::Done,
            (_, Status::Failed { path: failed, why }) if failed == path => {
                Settled::Failed(why.clone())
            }
            (
                Request::Start,
                Status::Starting { path: mine, .. } | Status::Finishing { path: mine },
            )
            | (Request::Stop, Status::Finishing { path: mine })
                if mine == path =>
            {
                Settled::Waiting
            }
            (
                Request::Start,
                Status::Recording { path: mine, .. } | Status::Saved { path: mine },
            )
            | (Request::Stop, Status::Saved { path: mine })
                if mine == path =>
            {
                Settled::Done
            }
            _ => Settled::Lost(format!(
                "the shell no longer follows {path}; it says \"{status}\""
            )),
        }
    }

    // why a request was not settled after waiting `waited` on it
    pub fn unsettled(self, path: &str, waited: Duration) -> String {
        let waited = waited.as_secs();

        match self {
            Request::Stop => format!(
                "{RECORDER} did not finish within {waited}s; the recording may still be saved at {path}"
            ),
            _ => {
                format!("{RECORDER} wrote no frame within {waited}s; it may still record to {path}")
            }
        }
    }
}

fn lock() -> MutexGuard<'static, Recorder> {
    RECORDER_STATE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/*
 * starts recording the focused output, to the path returned; `status` says once the first frame
 * is written. Call it from the draw thread, which lives as long as Kanade: the recorder is told to
 * finish when the thread that started it ends
 */
pub fn start() -> Result<String, String> {
    let mut recorder = lock();

    if let Some(running) = &recorder.running {
        return Err(format!("already recording to {}", running.path));
    }

    let output = focused_output()
        .map_err(|error| format!("niri: {error}"))?
        .ok_or("no output to record")?;

    let dir = capture::home("Videos/Screencasts").ok_or("no recording folder: HOME is not set")?;

    fs::create_dir_all(&dir)
        .map_err(|error| format!("no recording folder {}: {error}", dir.display()))?;

    let path = capture::free(
        &dir,
        &format!("Screencast from {}", clock::stamp()),
        "mp4",
        |path| {
            path.exists()
                || recorder
                    .issued
                    .iter()
                    .any(|issued| path == Path::new(issued))
        },
    );
    let path = path
        .to_str()
        .ok_or_else(|| format!("{} is not UTF-8", path.display()))?
        .to_owned();

    let mut child = wake::hold_file(RECORDER, &arguments(&output, &path))
        .map_err(|error| format!("cannot run {RECORDER}: {error}"))?;

    let said = child.stderr.take();
    let followed = {
        let path = path.clone();

        thread::Builder::new()
            .name(String::from("capture-record"))
            .spawn(move || follow(said, path))
    };

    if let Err(error) = followed {
        drop(child.kill());
        drop(child.wait());

        return Err(format!("cannot follow {RECORDER}: {error}"));
    }

    // under the lock, so the recording's end, which takes it, posts after
    IslandService::write().post(running(&path, &output), Instant::now());

    recorder.issued.push(path.clone());
    recorder.ended = None;
    recorder.running = Some(Running {
        path: path.clone(),
        output,
        child,
        stopping: false,
    });

    Ok(path)
}

// asks the recorder to finish the file, at the path returned; `status` says once it is saved
pub fn stop() -> Result<String, String> {
    let mut recorder = lock();
    let running = recorder.running.as_mut().ok_or("not recording")?;

    interrupt(running)?;

    Ok(running.path.clone())
}

// once, so a second stop does not cut the recorder's finishing short
fn interrupt(running: &mut Running) -> Result<(), String> {
    if !running.stopping {
        clock::interrupt(&running.child)
            .map_err(|error| format!("cannot stop {RECORDER}: {error}"))?;

        running.stopping = true;
    }

    Ok(())
}

pub fn status() -> Status {
    let recorder = lock();

    match (&recorder.running, &recorder.ended) {
        (Some(running), _) if running.stopping => Status::Finishing {
            path: running.path.clone(),
        },
        (Some(running), _) if Path::new(&running.path).exists() => Status::Recording {
            output: running.output.clone(),
            path: running.path.clone(),
        },
        (Some(running), _) => Status::Starting {
            output: running.output.clone(),
            path: running.path.clone(),
        },
        (None, Some(Ended { path, lost: None })) => Status::Saved { path: path.clone() },
        (
            None,
            Some(Ended {
                path,
                lost: Some(why),
            }),
        ) => Status::Failed {
            path: path.clone(),
            why: why.clone(),
        },
        (None, None) => Status::Idle,
    }
}

// ends the recording at `path`, as its Activity's Stop does
pub fn stop_at(path: &str) {
    if let Err(error) = stop_recording(&mut lock(), path) {
        eprintln!("capture: stopping the recording: {error}");
    }
}

// under one lock with the check, so a Stop for an ended recording never reaches a newer one
fn stop_recording(recorder: &mut Recorder, path: &str) -> Result<(), String> {
    match recorder.running.as_mut() {
        Some(running) if running.path == path => interrupt(running),
        _ => Ok(()),
    }
}

/*
 * reads what the recorder prints until it exits, then shows the recording saved, or why nothing
 * was
 */
fn follow(said: Option<ChildStderr>, path: String) {
    let said = last_lines(said);
    let mut recorder = lock();

    // under the lock, so a newer recording's start posts after
    if let Some(clip) = end(&mut recorder, &path, said) {
        IslandService::write().post(ended(clip), Instant::now());
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
 * reaps the recorder at `path`, done printing `said`, and how its recording ended; none once
 * another recording took its place. Saved only when the recorder succeeded and wrote a file: one
 * that failed may leave part of one
 */
fn end(recorder: &mut Recorder, path: &str, said: VecDeque<String>) -> Option<Clip> {
    let mut running = recorder.running.take_if(|running| running.path == path)?;

    let status = running.child.wait();
    let written = fs::metadata(path).is_ok_and(|file| file.len() > 0);

    let lost = match status {
        Ok(status) if status.success() && written => None,
        Ok(status) if status.code() == Some(NOT_FOUND) => Some(format!("{RECORDER} not found")),
        // niri sends a frame only on damage, so a stop on a still screen can come before any
        Ok(status) if status.success() && running.stopping => {
            Some(String::from("stopped before the screen gave a frame"))
        }
        Ok(status) => {
            let said = Vec::from(said).join("; ");

            Some(match said.is_empty() {
                true => format!("{RECORDER} ended ({status})"),
                false => format!("{RECORDER} ended ({status}): {said}"),
            })
        }
        Err(error) => Some(format!("{RECORDER}: {error}")),
    };

    if !running.stopping {
        eprintln!(
            "capture: the recording ended on its own: {}",
            lost.as_deref().unwrap_or("saved")
        );
    }

    let clip = match &lost {
        None => Clip::Saved {
            path: path.to_owned(),
            copied: false,
        },
        Some(why) => Clip::Failed {
            path: path.to_owned(),
            why: why.clone(),
        },
    };

    recorder.ended = Some(Ended {
        path: path.to_owned(),
        lost,
    });

    Some(clip)
}

// the recording at `path` copied, shown so unless a newer recording took its place
pub fn copied(path: &str) {
    let recorder = lock();

    let last = matches!(
        &recorder.ended,
        Some(Ended { path: saved, lost: None }) if saved == path
    );

    if recorder.running.is_none() && last {
        IslandService::write().post(
            ended(Clip::Saved {
                path: path.to_owned(),
                copied: true,
            }),
            Instant::now(),
        );
    }
}

// the focused output's name, none while niri focuses none
fn focused_output() -> std::io::Result<Option<String>> {
    Ok(output_name(&niri::ask("\"FocusedOutput\"")?))
}

fn output_name(reply: &Json) -> Option<String> {
    reply
        .get("FocusedOutput")?
        .get("name")?
        .as_str()
        .map(String::from)
}

// damage tracked, as only that is a cast to niri, so the privacy cluster shows it
fn arguments<'a>(output: &'a str, path: &'a str) -> [&'a str; 6] {
    ["-c", CODEC, "-o", output, "-f", path]
}

// one recording at a time, its end replacing it
fn id() -> Id {
    Id::new(Kind::Recording, "recording")
}

// Ongoing like a timer: over Media, a Satellite beside a higher primary, on every island
fn running(path: &str, output: &str) -> Activity {
    Activity::new(
        id(),
        Priority::Ongoing,
        Lifetime::Persistent,
        Scope::Global,
        Interrupt::None,
    )
    .expect("a Persistent that does not auto-expand is valid")
    .with_actions(vec![Action {
        key: String::from(STOP),
        label: String::from("Stop"),
    }])
    .with_detail(Detail::Recording(Clip::Recording {
        path: path.to_owned(),
        output: output.to_owned(),
    }))
}

// as a screenshot shows, on the output the user is looking at; a saved one copied or opened
fn ended(clip: Clip) -> Activity {
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

    Activity::new(
        id(),
        Priority::Actionable,
        Lifetime::Transient(SHOWN),
        Scope::FocusedOutput,
        Interrupt::None,
    )
    .expect("a Transient that does not auto-expand is valid")
    .with_actions(actions)
    .with_detail(Detail::Recording(clip))
}
