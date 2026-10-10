//! Screenshots (#139, docs/design.md Capture): niri takes them, as its own screenshot actions are
//! all an area, a window or an output needs, so there is no other backend to choose. `kanade
//! capture screenshot` asks niri to save one under `~/Pictures/Screenshots`, named as niri names its
//! own. niri announces each screenshot it saves, whoever asked for it, and each becomes a Transient
//! Activity whose actions copy its path or open it.

use std::collections::BTreeSet;
use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::service::Service;

use super::clipboard;
use super::json::{self, Json};
use super::niri::{self, Acted};
use super::recording;
use crate::clock;
use crate::island::activity::{
    Action, Activity, Detail, Id, Interrupt, Kind, Lifetime, Priority, Scope, Shot,
};
use crate::island::service::IslandService;

// what opens a screenshot or a recording, in the user's viewer
pub const OPEN: &str = "xdg-open";

pub const COPY: &str = "copy";
pub const SHOW: &str = "open";

// how long a screenshot's or a recording's Activity stays after it was saved, long enough to reach
// its actions
pub const SHOWN: Duration = Duration::from_secs(10);

/*
 * the paths handed to niri this second, under that second's stamp: niri saves after it answers,
 * so a second screenshot the same second would otherwise find the first's name still free. A new
 * second's stamp names other paths, so the last second's are let go
 */
static NAMED: Mutex<(String, BTreeSet<PathBuf>)> = Mutex::new((String::new(), BTreeSet::new()));

// the screenshot the Activity shows, so a copy finishing after a newer one leaves that one alone
static LAST: Mutex<Option<String>> = Mutex::new(None);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    // the one the user picks in niri's screenshot UI
    Area,

    // the focused workspace's active window
    Window,

    // the focused output
    Output,
}

impl Mode {
    pub fn parse(word: &str) -> Option<Mode> {
        match word {
            "area" => Some(Mode::Area),
            "window" => Some(Mode::Window),
            "output" => Some(Mode::Output),
            _ => None,
        }
    }
}

// what came of asking niri for a screenshot
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    // niri took it, to be saved at this path
    Taken(String),

    // niri may or may not take it; why, with the path it would be saved at
    Unknown(String),
}

/*
 * asks niri for a screenshot, saved to the path returned. An area is saved once the user confirms
 * it in niri's screenshot UI, and not at all if they leave it
 */
pub fn screenshot(mode: Mode) -> Result<Asked, String> {
    let dir = directory().ok_or("no screenshot folder: HOME is not set")?;

    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("no screenshot folder {}: {error}", dir.display()))?;

    let window = match mode {
        Mode::Window => Some(window().map_err(backend)?.ok_or("no window to capture")?),
        _ => None,
    };

    let path = name(&dir, clock::stamp());
    let path = path
        .to_str()
        .ok_or_else(|| format!("{} is not UTF-8", path.display()))?;

    match niri::act(&action(mode, path, window)).map_err(backend)? {
        Acted::Done => Ok(Asked::Taken(path.to_owned())),
        Acted::Unknown(why) => Ok(Asked::Unknown(format!(
            "niri got the request, but {why}; the screenshot may still be saved at {path}"
        ))),
    }
}

// a free path this second, kept from the next screenshot this second
fn name(dir: &Path, stamp: String) -> PathBuf {
    let mut named = NAMED.lock().unwrap_or_else(PoisonError::into_inner);
    let (named_stamp, named) = &mut *named;

    if *named_stamp != stamp {
        *named_stamp = stamp;
        named.clear();
    }

    let path = free(
        dir,
        &format!("Screenshot from {named_stamp}"),
        "png",
        |path| path.exists() || named.contains(path),
    );

    named.insert(path.clone());
    path
}

fn backend(error: io::Error) -> String {
    format!("niri: {error}")
}

// niri's own default, `~/Pictures/Screenshots`
fn directory() -> Option<PathBuf> {
    home("Pictures/Screenshots")
}

// `folder` under the user's home
pub fn home(folder: &str) -> Option<PathBuf> {
    Some(PathBuf::from(env::var_os("HOME")?).join(folder))
}

// named as niri names its own, with " (2)" and up when one taken the same second is there
pub fn free(dir: &Path, stem: &str, extension: &str, taken: impl Fn(&Path) -> bool) -> PathBuf {
    (1..)
        .map(|count| match count {
            1 => dir.join(format!("{stem}.{extension}")),
            _ => dir.join(format!("{stem} ({count}).{extension}")),
        })
        .find(|path| !taken(path))
        .expect("an unbounded count finds a free name")
}

/*
 * the focused workspace's active window, which niri captures for no id even when no window has
 * focus; asked by id so a window that is not there is said rather than silently missed
 */
fn window() -> io::Result<Option<u64>> {
    Ok(active_window(&niri::ask("\"Workspaces\"")?))
}

fn active_window(reply: &Json) -> Option<u64> {
    reply
        .get("Workspaces")?
        .as_array()?
        .iter()
        .find(|workspace| workspace.get("is_focused").and_then(Json::as_bool) == Some(true))?
        .get("active_window_id")?
        .as_u64()
}

// niri's request, its pointer shown as its own binds show it
fn action(mode: Mode, path: &str, window: Option<u64>) -> String {
    let path = json::quote(path);

    let action = match (mode, window) {
        (Mode::Area, _) => format!(r#""Screenshot":{{"show_pointer":true,"path":{path}}}"#),
        (Mode::Window, Some(id)) => format!(
            r#""ScreenshotWindow":{{"id":{id},"write_to_disk":true,"show_pointer":false,"path":{path}}}"#
        ),
        (Mode::Window, None) | (Mode::Output, _) => format!(
            r#""ScreenshotScreen":{{"write_to_disk":true,"show_pointer":true,"path":{path}}}"#
        ),
    };

    format!(r#"{{"Action":{{{action}}}}}"#)
}

// niri saved a screenshot at `path`
pub fn captured(path: String) {
    *LAST.lock().unwrap_or_else(PoisonError::into_inner) = Some(path.clone());

    show(Shot {
        path,
        copied: false,
    });
}

fn show(shot: Shot) {
    IslandService::write().post(activity(shot), Instant::now());
}

// one at a time, so a new screenshot replaces the last; on the output the user is looking at
fn activity(shot: Shot) -> Activity {
    Activity::new(
        Id::new(Kind::Screenshot, "screenshot"),
        Priority::Actionable,
        Lifetime::Transient(SHOWN),
        Scope::FocusedOutput,
        Interrupt::None,
    )
    .expect("a Transient that does not auto-expand is valid")
    .with_actions(vec![
        Action {
            key: String::from(COPY),
            label: String::from("Copy path"),
        },
        Action {
            key: String::from(SHOW),
            label: String::from("Open"),
        },
    ])
    .with_detail(Detail::Screenshot(shot))
}

/*
 * runs an action of the screenshot or recording at `path`, off the draw thread: a copy says it is
 * done in the Activity, unless a newer one took its place
 */
pub fn act(key: &str, path: &str) {
    let path = path.to_owned();

    let spawned = match key {
        recording::STOP => thread::Builder::new()
            .name(String::from("capture-stop"))
            .spawn(move || recording::stop_at(&path)),
        COPY => thread::Builder::new()
            .name(String::from("capture-copy"))
            .spawn(move || copy(path)),
        SHOW => thread::Builder::new()
            .name(String::from("capture-open"))
            .spawn(move || open(&path)),
        _ => return,
    };

    if let Err(error) = spawned {
        eprintln!("capture: {key}: {error}");
    }
}

fn copy(path: String) {
    if let Err(error) = clipboard::put(&path) {
        eprintln!("capture: copying the path: {error}");
        return;
    }

    let last = LAST.lock().unwrap_or_else(PoisonError::into_inner);

    if last.as_deref() == Some(path.as_str()) {
        drop(last);
        show(Shot { path, copied: true });
    } else {
        drop(last);
        recording::copied(&path);
    }
}

// the viewer outlives Kanade, so it is not one of Kanade's children that die with it; only reaped
fn open(path: &str) {
    let opened = Command::new(OPEN)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .and_then(|mut child| child.wait());

    match opened {
        Ok(status) if !status.success() => eprintln!("capture: {OPEN} failed ({status})"),
        Ok(_) => {}
        Err(error) => eprintln!("capture: {OPEN}: {error}"),
    }
}
