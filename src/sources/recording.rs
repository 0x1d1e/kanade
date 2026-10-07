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
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, ChildStderr};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use amane::Service;

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
});

struct Recorder {
    // the one recording, none while none records
    running: Option<Running>,

    // the last recording that ended, until another starts
    ended: Option<Ended>,
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

impl Request {
    /*
     * whether the recording at `path` is where this request leaves it, as `status` says: started
     * once its first frame is written, stopped once its file is saved. None while it gets there
     */
    pub fn settled(self, path: &str, status: &Status) -> Option<Result<(), String>> {
        match (self, status) {
            (_, Status::Failed { path: failed, why }) if failed == path => Some(Err(why.clone())),
            (Request::Start, Status::Starting { path: starting, .. }) if starting == path => None,
            (Request::Stop, Status::Finishing { path: finishing }) if finishing == path => None,
            _ => Some(Ok(())),
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
        Path::exists,
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

    if !running.stopping {
        clock::interrupt(&running.child)
            .map_err(|error| format!("cannot stop {RECORDER}: {error}"))?;

        running.stopping = true;
    }

    Ok(running.path.clone())
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
    let recording = lock()
        .running
        .as_ref()
        .is_some_and(|running| running.path == path);

    if !recording {
        return;
    }

    if let Err(error) = stop() {
        eprintln!("capture: stopping the recording: {error}");
    }
}

/*
 * reads what the recorder prints until it exits, then shows the recording saved, or why nothing
 * was
 */
fn follow(said: Option<ChildStderr>, path: String) {
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

    let mut recorder = lock();

    let Some(mut running) = recorder.running.take_if(|running| running.path == path) else {
        return;
    };

    let status = running.child.wait();
    let saved = fs::metadata(&path).is_ok_and(|file| file.len() > 0);

    let why = match status {
        Ok(status) if status.code() == Some(NOT_FOUND) => format!("{RECORDER} not found"),
        // niri sends a frame only on damage, so a stop on a still screen can come before any
        Ok(status) if status.success() && running.stopping => {
            "stopped before the screen gave a frame".to_owned()
        }
        Ok(status) => {
            let said = Vec::from(last).join("; ");

            match said.is_empty() {
                true => format!("{RECORDER} ended ({status})"),
                false => format!("{RECORDER} ended ({status}): {said}"),
            }
        }
        Err(error) => format!("{RECORDER}: {error}"),
    };

    if !running.stopping {
        eprintln!("capture: the recording ended on its own: {why}");
    }

    let clip = match saved {
        true => Clip::Saved {
            path: path.clone(),
            copied: false,
        },
        false => Clip::Failed {
            path: path.clone(),
            why: why.clone(),
        },
    };

    IslandService::write().post(ended(clip), Instant::now());

    recorder.ended = Some(Ended {
        path,
        lost: (!saved).then_some(why),
    });
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

// damage tracked, as only that is a cast to niri, so the privacy cluster sees it
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_request_is_named() {
        assert_eq!(Request::parse("start"), Some(Request::Start));
        assert_eq!(Request::parse("stop"), Some(Request::Stop));
        assert_eq!(Request::parse("status"), Some(Request::Status));
        assert_eq!(Request::parse("pause"), None);
    }

    #[test]
    fn the_output_is_the_focused_one() {
        let reply = |text: &str| Json::parse(text).expect("valid JSON");

        assert_eq!(
            output_name(&reply(
                r#"{"FocusedOutput":{"name":"eDP-1","make":"BOE","model":"0x0BCA"}}"#
            )),
            Some(String::from("eDP-1"))
        );
        assert_eq!(output_name(&reply(r#"{"FocusedOutput":null}"#)), None);
    }

    #[test]
    fn the_recorder_encodes_the_output_on_the_gpu() {
        assert_eq!(
            arguments("eDP-1", "/v/a.mp4"),
            ["-c", "h264_vaapi", "-o", "eDP-1", "-f", "/v/a.mp4"]
        );
    }

    #[test]
    fn a_recording_is_ongoing_until_saved_then_actionable_for_a_while() {
        let recording = running("/v/a.mp4", "eDP-1");

        assert_eq!(recording.priority(), Priority::Ongoing);
        assert_eq!(recording.lifetime(), Lifetime::Persistent);
        assert_eq!(recording.scope(), Scope::Global);
        assert_eq!(recording.actions()[0].key, STOP);

        let saved = ended(Clip::Saved {
            path: String::from("/v/a.mp4"),
            copied: false,
        });

        assert_eq!(saved.id(), recording.id());
        assert_eq!(saved.priority(), Priority::Actionable);
        assert_eq!(saved.lifetime(), Lifetime::Transient(SHOWN));
        assert_eq!(saved.scope(), Scope::FocusedOutput);
        assert_eq!(
            saved
                .actions()
                .iter()
                .map(|action| action.key.as_str())
                .collect::<Vec<_>>(),
            [COPY, SHOW]
        );

        let failed = ended(Clip::Failed {
            path: String::from("/v/a.mp4"),
            why: String::from("wf-recorder not found"),
        });

        assert_eq!(failed.id(), recording.id());
        assert!(failed.actions().is_empty());
    }

    #[test]
    fn start_settles_on_the_first_frame_and_stop_on_the_saved_file() {
        let path = "/v/a.mp4";
        let output = String::from("eDP-1");
        let starting = Status::Starting {
            output: output.clone(),
            path: String::from(path),
        };
        let finishing = Status::Finishing {
            path: String::from(path),
        };
        let failed = Status::Failed {
            path: String::from(path),
            why: String::from("wf-recorder not found"),
        };

        assert_eq!(Request::Start.settled(path, &starting), None);
        assert_eq!(
            Request::Start.settled(
                path,
                &Status::Recording {
                    output,
                    path: String::from(path)
                }
            ),
            Some(Ok(()))
        );
        assert_eq!(
            Request::Start.settled(path, &failed),
            Some(Err(String::from("wf-recorder not found")))
        );

        assert_eq!(Request::Stop.settled(path, &finishing), None);
        assert_eq!(
            Request::Stop.settled(
                path,
                &Status::Saved {
                    path: String::from(path)
                }
            ),
            Some(Ok(()))
        );
        assert_eq!(
            Request::Stop.settled(path, &failed),
            Some(Err(String::from("wf-recorder not found")))
        );
    }

    #[test]
    fn a_status_reads_back_as_said() {
        let path = String::from("/v/Screencast from 2026-10-08 00-00-32 (2).mp4");

        for status in [
            Status::Idle,
            Status::Starting {
                output: String::from("eDP-1"),
                path: path.clone(),
            },
            Status::Recording {
                output: String::from("HDMI-A-1"),
                path: path.clone(),
            },
            Status::Finishing { path: path.clone() },
            Status::Saved { path: path.clone() },
            Status::Failed {
                path: path.clone(),
                why: String::from(
                    "wf-recorder ended (exit status: 255): Failed to find the given codec: h264_vaapi",
                ),
            },
        ] {
            assert_eq!(Status::parse(&status.to_string()), Some(status));
        }

        assert_eq!(Status::parse("paused"), None);
    }
}
