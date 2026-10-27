//! Screenshots (docs/archived/design.md Capture): niri takes them, as its own screenshot actions are
//! all an area, a window or an output needs, so there is no other backend to choose. `kanade
//! capture screenshot` asks niri to save one under `~/Pictures/Screenshots`, named as niri names its
//! own. niri announces each screenshot it saves by its own notification, whoever asked for it; the
//! Island shows none. The actions here pause, resume or stop a recording, copy its path or open it.

use std::collections::BTreeSet;
use std::env;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use kanade_runtime::service::Service;
use kanade_runtime::{IpcReply, MutexExt};

use super::clipboard;
use super::json::{self, Json};
use super::live;
use super::niri::{self, Acted, Niri, Retarget};
use super::recording;
use crate::cli::Reply;
use crate::clock;

// what opens a recording, in the user's viewer
pub const OPEN: &str = "xdg-open";

pub const COPY: &str = "copy";
pub const SHOW: &str = "open";

// how long a recording's Activity stays after it was saved, long enough to reach its actions
pub const SHOWN: Duration = Duration::from_secs(10);

/*
 * the paths handed to niri this second, under that second's stamp: niri saves after it answers,
 * so a second screenshot the same second would otherwise find the first's name still free. A new
 * second's stamp names other paths, so the last second's are let go
 */
static NAMED: Mutex<(String, BTreeSet<PathBuf>)> = Mutex::new((String::new(), BTreeSet::new()));

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

// whether niri's picker is on screen: it takes one pick at a time
static PICKING: AtomicBool = AtomicBool::new(false);

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
        .map_err(|error| format!("no screenshot folder ({error}): {}", dir.display()))?;

    let window = match mode {
        Mode::Window => Some(window().map_err(backend)?.ok_or("no window to capture")?),
        _ => None,
    };

    let path = name(&dir, clock::stamp());
    let path = path
        .to_str()
        .ok_or_else(|| format!("path is not UTF-8: {}", path.display()))?;

    match niri::act(&action(mode, path, window)).map_err(backend)? {
        Acted::Done => Ok(Asked::Taken(path.to_owned())),
        Acted::Unknown(why) => Ok(Asked::Unknown(format!(
            "niri got the request, but {why}; the screenshot may still be saved at {path}"
        ))),
    }
}

/*
 * lets the user pick a pixel of the screen with niri's own picker, which this waits on, so never
 * on the draw thread. The color goes on the clipboard as `#rrggbb` and is shown by a Transient
 * Activity; none when the user leaves the picker. Plain text, unlike a password manager's copy, so
 * ADR 0019's sensitive hint is not set and the clipboard history may keep it
 */
pub fn color() -> Result<Option<String>, String> {
    if PICKING.swap(true, Ordering::AcqRel) {
        return Err(String::from("a color is already being picked"));
    }

    let picked = niri::pick().map_err(backend);

    PICKING.store(false, Ordering::Release);

    let Some(hex) = picked.and_then(|answer| hex(&answer))? else {
        return Ok(None);
    };

    clipboard::put(&hex).map_err(|error| format!("copying the color: {error}"))?;
    live::color_picked(&hex);

    Ok(Some(hex))
}

/*
 * `#rrggbb` of what niri answers a pick with: `{"PickedColor":{"rgb":[r,g,b]}}`, each from 0 to
 * 1, or null for a picker the user left; an error for any other answer
 */
fn hex(answer: &Json) -> Result<Option<String>, String> {
    let unexpected = || String::from("niri answered the color pick unexpectedly");
    let picked = answer.get("PickedColor").ok_or_else(unexpected)?;

    if picked.is_null() {
        return Ok(None);
    }

    let rgb = picked
        .get("rgb")
        .and_then(json::array)
        .filter(|rgb| rgb.len() == 3)
        .ok_or_else(unexpected)?;

    let mut text = String::from("#");

    for channel in rgb {
        let value = channel.as_f64().ok_or_else(unexpected)?;

        // rounded to the nearest of 256, as an 8-bit pixel was read
        text.push_str(&format!(
            "{:02x}",
            (value.clamp(0.0, 1.0) * 255.0).round() as u8
        ));
    }

    Ok(Some(text))
}

/*
 * `kanade capture color`: answers on the user's pick, so the client waits on them; the color, or
 * a refusal when they leave the picker
 */
pub fn pick(reply: IpcReply) {
    let spawned = thread::Builder::new()
        .name(String::from("capture-color"))
        .spawn(move || {
            let answer = match color() {
                Ok(Some(hex)) => Reply::Done(hex),
                Ok(None) => Reply::Refused(String::from("no color picked")),
                Err(why) => Reply::Refused(why),
            };

            reply.send(&answer.encode());
        });

    if let Err(error) = spawned {
        eprintln!("capture: {error}");
    }
}

// a free path this second, kept from the next screenshot this second
fn name(dir: &Path, stamp: String) -> PathBuf {
    let mut named = NAMED.locked();
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
 * `capture cast window [<id>]|monitor [<output>]|clear`: what a screen share that picks its target
 * as it runs (a dynamic cast target, as a portal share of niri's "dynamic target" is) shows; none
 * for the focused window or output
 */
pub fn parse_cast(words: &[&str]) -> Option<Retarget> {
    match words {
        ["window"] => Some(Retarget::Window(None)),
        ["window", id] => Some(Retarget::Window(Some(id.parse().ok()?))),
        ["monitor"] => Some(Retarget::Output(None)),
        ["monitor", output] => Some(Retarget::Output(Some((*output).to_owned()))),
        ["clear"] => Some(Retarget::Clear),
        _ => None,
    }
}

/*
 * changes what the dynamic-target cast shows. Refused while no cast has a dynamic target, as niri
 * would do nothing; a niri without the actions says so. Blocks up to niri's patience, so never on
 * the draw thread
 */
pub fn cast(to: &Retarget) -> Result<Acted, String> {
    if !Niri::read().seen.dynamic {
        return Err(String::from(
            "no screen cast with a dynamic target is running",
        ));
    }

    niri::retarget(to).map_err(backend)
}

/*
 * the focused workspace's active window, which niri captures for no id even when no window has
 * focus; asked by id so a window that is not there is said rather than silently missed
 */
pub(super) fn window() -> io::Result<Option<u64>> {
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

/*
 * runs an action of the recording at `path`, off the draw thread: a copy says it is done in the
 * Activity, unless a newer recording took its place
 */
pub fn act(key: &str, path: &str) {
    let path = path.to_owned();

    let spawned = match key {
        recording::STOP => thread::Builder::new()
            .name(String::from("capture-stop"))
            .spawn(move || recording::stop_at(&path)),
        recording::PAUSE => thread::Builder::new()
            .name(String::from("capture-pause"))
            .spawn(move || recording::pause_at(&path)),
        recording::RESUME => thread::Builder::new()
            .name(String::from("capture-resume"))
            .spawn(move || recording::resume_at(&path)),
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
    match clipboard::put(&path) {
        Ok(()) => recording::copied(&path),
        Err(error) => eprintln!("capture: copying the path: {error}"),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn picked(text: &str) -> Result<Option<String>, String> {
        hex(&json::parse(text).expect("a fixture is JSON"))
    }

    #[test]
    fn a_pick_reads_as_hex_and_a_cancel_as_none() {
        assert_eq!(
            picked(r#"{"PickedColor":{"rgb":[1.0,0.0,0.5]}}"#),
            Ok(Some(String::from("#ff0080")))
        );
        // out of range is held to it, as a float error could push a channel past 1
        assert_eq!(
            picked(r#"{"PickedColor":{"rgb":[1.0000001,-0.1,0.2]}}"#),
            Ok(Some(String::from("#ff0033")))
        );
        assert_eq!(picked(r#"{"PickedColor":null}"#), Ok(None));
    }

    #[test]
    fn an_answer_that_is_no_pick_is_an_error() {
        for text in [
            r#"{"Handled":null}"#,
            r#"{"PickedColor":{"rgb":[1.0,0.0]}}"#,
            r#"{"PickedColor":{"rgb":[1.0,0.0,"x"]}}"#,
            r#"{"PickedColor":{}}"#,
        ] {
            assert!(picked(text).is_err(), "{text}");
        }
    }
}
