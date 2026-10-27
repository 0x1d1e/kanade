//! The niri page's Outputs section (ADR 0119, slice 2): each connected monitor's mode, scale,
//! transform, variable refresh rate, position and whether it is on, tried live and then kept or
//! reverted. This file is the logic and has no thread, window or clock of its own: what niri says
//! (`Output`, read from the `Outputs` IPC request), what a change comes to (`Edit`), and the
//! `Controller` that holds the one trial at a time. `Port` is everything the controller does to
//! the world (ask niri, send it `niri msg output`, write the config, tell the time), so the tests
//! give it fakes. `work` has the real `Port`, the worker thread and the state the page draws.
//!
//! A trial is niri's own memory only: `niri msg output <name> ...` changes the output until niri
//! reloads its config. Before the first change of a trial the output's state as niri reports it
//! (`Previous`) is written to a pending file, so a Kanade that dies mid-trial puts it back when it
//! next starts (`Controller::recover`). Keep writes every edit of the trial to the config in one
//! validated change; Revert (the button, Esc, the 15 s running out, the page or window going away,
//! Kanade exiting) puts `Previous` back from niri's side, never from the config, which may not hold
//! what the output showed before.

pub mod canvas;
mod work;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::niri::format_number;
use crate::atomic::{self, Mode as FileMode};
use crate::niri_config::{Op, OutputId, Value};
use crate::sources::json::Json;
use crate::sources::niri as ipc;

pub use canvas::{BoundingBox, CanvasFit, Rect, SNAP_THRESHOLD, snap};
pub use work::{
    Disconnected, Monitor, State, drag_end, drag_start, drag_update, edit, input, keep,
    recover_at_start, refresh, reset, revert, revert_on_exit, select, sync_inputs, typed,
    typed_position,
};
/// How long a trial waits for Keep before it reverts, and starts over at each further edit.
pub const TRIAL_MS: u64 = 15_000;

/// Grace window for worker queue latency when processing a Keep request.
pub const KEEP_GRACE_MS: u64 = 1_000;

/// How old a pending file may be and still be put back at start: an older one is deleted unread.
/// A trial is over in 15 s, so a file this old is not from a crash during one but from a Kanade
/// that died and left niri's state to someone else; applying it could undo what the user did since.
pub const RECOVERY_MS: u64 = 5 * 60 * 1000;

/// How long a failed revert waits before it tries again.
const RETRY_MS: u64 = 5_000;

/// The scales the page offers; niri rounds a scale to one the output can show.
pub const SCALES: (f64, f64, f64) = (0.5, 4.0, 0.25);

// ---- what niri says ----

/// A mode of a monitor: size and refresh rate in millihertz, as niri lists them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mode {
    pub width: u16,
    pub height: u16,
    pub refresh: u32,
    pub preferred: bool,
}

impl Mode {
    /// As niri's `mode` takes it, in the CLI and the config: `1920x1080@144.001`.
    pub fn text(&self) -> String {
        format!(
            "{}x{}@{:.3}",
            self.width,
            self.height,
            f64::from(self.refresh) / 1000.0
        )
    }

    /// As the page lists it: `1920 × 1080 @ 144.001 Hz`.
    pub fn label(&self) -> String {
        format!(
            "{} \u{d7} {} @ {:.3} Hz",
            self.width,
            self.height,
            f64::from(self.refresh) / 1000.0
        )
    }

    fn same(&self, other: &Mode) -> bool {
        (self.width, self.height, self.refresh) == (other.width, other.height, other.refresh)
    }
}

/// How an output is turned, counter-clockwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Transform {
    Normal,
    R90,
    R180,
    R270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl Transform {
    pub const ALL: [Transform; 8] = [
        Transform::Normal,
        Transform::R90,
        Transform::R180,
        Transform::R270,
        Transform::Flipped,
        Transform::Flipped90,
        Transform::Flipped180,
        Transform::Flipped270,
    ];

    // the turn in degrees; written as numbers, as a quoted "180" would read as a color to the boundary check
    fn degrees(self) -> u16 {
        match self {
            Transform::Normal | Transform::Flipped => 0,
            Transform::R90 | Transform::Flipped90 => 90,
            Transform::R180 | Transform::Flipped180 => 180,
            Transform::R270 | Transform::Flipped270 => 270,
        }
    }

    /// As the CLI and the config write it.
    pub fn name(self) -> String {
        let flipped = matches!(
            self,
            Transform::Flipped
                | Transform::Flipped90
                | Transform::Flipped180
                | Transform::Flipped270
        );

        match (flipped, self.degrees()) {
            (false, 0) => String::from("normal"),
            (true, 0) => String::from("flipped"),
            (false, degrees) => degrees.to_string(),
            (true, degrees) => format!("flipped-{degrees}"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Transform::Normal => "Normal",
            Transform::R90 => "90\u{b0}",
            Transform::R180 => "180\u{b0}",
            Transform::R270 => "270\u{b0}",
            Transform::Flipped => "Flipped",
            Transform::Flipped90 => "Flipped 90\u{b0}",
            Transform::Flipped180 => "Flipped 180\u{b0}",
            Transform::Flipped270 => "Flipped 270\u{b0}",
        }
    }

    // as the IPC's JSON names it ("Normal", "90", "Flipped90")
    fn from_ipc(name: &str) -> Option<Transform> {
        Transform::ALL
            .into_iter()
            .find(|transform| transform.name().replace('-', "").eq_ignore_ascii_case(name))
    }

    fn from_name(name: &str) -> Option<Transform> {
        Transform::ALL
            .into_iter()
            .find(|transform| transform.name() == name)
    }
}

/// Variable refresh rate: niri's IPC says whether it is on, not whether on demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Vrr {
    Off,
    On,
    OnDemand,
}

impl Vrr {
    pub const ALL: [Vrr; 3] = [Vrr::Off, Vrr::On, Vrr::OnDemand];

    pub fn label(self) -> &'static str {
        match self {
            Vrr::Off => "Off",
            Vrr::On => "On",
            Vrr::OnDemand => "On demand",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Vrr::Off => "off",
            Vrr::On => "on",
            Vrr::OnDemand => "on-demand",
        }
    }

    fn from_name(name: &str) -> Option<Vrr> {
        Vrr::ALL.into_iter().find(|vrr| vrr.name() == name)
    }

    /// What the output does, given what niri reports of it and what the config asks of it: on
    /// demand is seen only in the config, as niri reports it on only while a window wants it.
    pub fn of(enabled: bool, hint: Option<Vrr>) -> Vrr {
        match (enabled, hint) {
            (_, Some(Vrr::OnDemand)) => Vrr::OnDemand,
            (true, _) => Vrr::On,
            (false, _) => Vrr::Off,
        }
    }
}

/// Where and how an enabled output is placed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Logical {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub transform: Transform,
}

impl Logical {
    pub fn rect(&self) -> canvas::Rect {
        canvas::Rect::new(self.x, self.y, self.width, self.height)
    }
}

/// A connected monitor as niri reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub name: String,
    pub make: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub modes: Vec<Mode>,

    // the index in `modes` of the one in use; none while off or on a custom mode
    pub current: Option<usize>,
    pub vrr_supported: bool,
    pub vrr_enabled: bool,

    // none while the output is off
    pub logical: Option<Logical>,
}

impl Output {
    pub fn enabled(&self) -> bool {
        self.logical.is_some()
    }

    pub fn current_mode(&self) -> Option<Mode> {
        self.current.and_then(|at| self.modes.get(at)).copied()
    }

    /// Who niri matches a block to: the connector, and what the EDID gives.
    pub fn id(&self) -> OutputId {
        OutputId {
            connector: self.name.clone(),
            make: self.make.clone(),
            model: self.model.clone(),
            serial: self.serial.clone(),
        }
    }

    /// The make and model, as the page names the monitor under its connector.
    pub fn product(&self) -> String {
        let known = |part: &Option<String>| part.clone().filter(|part| !part.is_empty());

        match (known(&self.make), known(&self.model)) {
            (Some(make), Some(model)) if model.starts_with(&make) => model,
            (Some(make), Some(model)) => format!("{make} {model}"),
            (Some(part), None) | (None, Some(part)) => part,
            (None, None) => String::from("Unknown monitor"),
        }
    }
}

/// The outputs in an answer to the `Outputs` request (`{"Outputs": {name: {...}}}`), by name.
pub fn parse_outputs(reply: &Json) -> Vec<Output> {
    let listed = reply.get("Outputs").unwrap_or(reply);
    let Some(listed) = listed.as_object() else {
        return Vec::new();
    };
    let mut outputs: Vec<Output> = listed
        .iter()
        .filter_map(|(name, output)| parse_output(name, output))
        .collect();

    outputs.sort_by(|a, b| a.name.cmp(&b.name));
    outputs
}

fn parse_output(name: &str, output: &Json) -> Option<Output> {
    // what the EDID lacks niri says as "Unknown", which a block's name says the same way
    let part = |key: &str| {
        output
            .get(key)
            .and_then(Json::as_str)
            .filter(|part| !part.is_empty() && *part != "Unknown")
            .map(str::to_owned)
    };
    let modes = output
        .get("modes")?
        .as_array()?
        .iter()
        .filter_map(|mode| {
            Some(Mode {
                width: u16::try_from(mode.get("width")?.as_u64()?).ok()?,
                height: u16::try_from(mode.get("height")?.as_u64()?).ok()?,
                refresh: u32::try_from(mode.get("refresh_rate")?.as_u64()?).ok()?,
                preferred: mode.get("is_preferred").and_then(Json::as_bool) == Some(true),
            })
        })
        .collect();
    let logical = output
        .get("logical")
        .filter(|logical| logical.is_object())
        .and_then(|logical| {
            Some(Logical {
                x: i32::try_from(logical.get("x")?.as_i64()?).ok()?,
                y: i32::try_from(logical.get("y")?.as_i64()?).ok()?,
                width: u32::try_from(logical.get("width")?.as_u64()?).ok()?,
                height: u32::try_from(logical.get("height")?.as_u64()?).ok()?,
                scale: logical.get("scale")?.as_f64()?,
                transform: Transform::from_ipc(logical.get("transform")?.as_str()?)?,
            })
        });
    let flag = |key: &str| output.get(key).and_then(Json::as_bool) == Some(true);

    Some(Output {
        name: name.to_owned(),
        make: part("make"),
        model: part("model"),
        serial: part("serial"),
        modes,
        current: output
            .get("current_mode")
            .and_then(Json::as_u64)
            .and_then(|at| usize::try_from(at).ok())
            .filter(|_| !flag("is_custom_mode")),
        vrr_supported: flag("vrr_supported"),
        vrr_enabled: flag("vrr_enabled"),
        logical,
    })
}

// ---- what a change comes to ----

/// A part of an output a change is to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Field {
    Enabled,
    Mode,
    Scale,
    Transform,
    Vrr,
    Position,
}

/// One change to an output.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    Enabled(bool),
    Mode(Mode),
    Scale(f64),
    Transform(Transform),
    Vrr(Vrr),
    Position(i32, i32),
}

impl Edit {
    pub fn field(&self) -> Field {
        match self {
            Edit::Enabled(_) => Field::Enabled,
            Edit::Mode(_) => Field::Mode,
            Edit::Scale(_) => Field::Scale,
            Edit::Transform(_) => Field::Transform,
            Edit::Vrr(_) => Field::Vrr,
            Edit::Position(..) => Field::Position,
        }
    }

    /// The words after `niri msg output <name>` that make it.
    pub fn args(&self) -> Vec<String> {
        let words: Vec<String> = match self {
            Edit::Enabled(true) => vec!["on".into()],
            Edit::Enabled(false) => vec!["off".into()],
            Edit::Mode(mode) => vec!["mode".into(), mode.text()],
            Edit::Scale(scale) => vec!["scale".into(), scale_text(*scale)],
            Edit::Transform(transform) => vec!["transform".into(), transform.name()],
            Edit::Vrr(Vrr::Off) => vec!["vrr".into(), "off".into()],
            Edit::Vrr(Vrr::On) => vec!["vrr".into(), "on".into()],
            Edit::Vrr(Vrr::OnDemand) => vec!["vrr".into(), "--on-demand".into(), "on".into()],
            Edit::Position(x, y) => position_words(*x, *y),
        };

        words
    }

    /// What the config's block takes for it: the keys of `OUTPUT_KEYS` set or removed.
    pub fn ops(&self) -> Vec<(&'static str, Op)> {
        match self {
            Edit::Enabled(true) => vec![("off", Op::Remove)],
            Edit::Enabled(false) => vec![("off", Op::Set(Value::Bool(true)))],
            Edit::Mode(mode) => vec![("mode", Op::Set(Value::Str(mode.text())))],
            Edit::Scale(scale) => vec![(
                "scale",
                Op::Set(Value::parse(&scale_text(*scale)).unwrap_or(Value::Float(*scale))),
            )],
            Edit::Transform(transform) => vec![(
                "transform",
                Op::Set(Value::Str(transform.name().to_owned())),
            )],
            Edit::Vrr(Vrr::Off) => vec![("variable-refresh-rate", Op::Remove)],
            Edit::Vrr(Vrr::On) => vec![("variable-refresh-rate", Op::Set(Value::Props(vec![])))],
            Edit::Vrr(Vrr::OnDemand) => vec![(
                "variable-refresh-rate",
                Op::Set(Value::Props(vec![("on-demand".into(), Value::Bool(true))])),
            )],
            Edit::Position(x, y) => vec![(
                "position",
                Op::Set(Value::Props(vec![
                    ("x".into(), Value::Int(i64::from(*x))),
                    ("y".into(), Value::Int(i64::from(*y))),
                ])),
            )],
        }
    }

    pub fn to_json(&self) -> Json {
        match self {
            Edit::Enabled(enabled) => serde_json::json!({
                "type": "enabled",
                "value": enabled,
            }),
            Edit::Mode(mode) => serde_json::json!({
                "type": "mode",
                "width": mode.width,
                "height": mode.height,
                "refresh": mode.refresh,
                "preferred": mode.preferred,
            }),
            Edit::Scale(scale) => serde_json::json!({
                "type": "scale",
                "value": scale,
            }),
            Edit::Transform(transform) => serde_json::json!({
                "type": "transform",
                "value": transform.name(),
            }),
            Edit::Vrr(vrr) => serde_json::json!({
                "type": "vrr",
                "value": vrr.name(),
            }),
            Edit::Position(x, y) => serde_json::json!({
                "type": "position",
                "x": x,
                "y": y,
            }),
        }
    }

    pub fn from_json(json: &Json) -> Option<Edit> {
        let kind = json.get("type")?.as_str()?;
        match kind {
            "enabled" => Some(Edit::Enabled(json.get("value")?.as_bool()?)),
            "mode" => Some(Edit::Mode(Mode {
                width: u16::try_from(json.get("width")?.as_u64()?).ok()?,
                height: u16::try_from(json.get("height")?.as_u64()?).ok()?,
                refresh: u32::try_from(
                    json.get("refresh")
                        .or_else(|| json.get("refresh_rate"))?
                        .as_u64()?,
                )
                .ok()?,
                preferred: json
                    .get("preferred")
                    .and_then(Json::as_bool)
                    .unwrap_or(false),
            })),
            "scale" => Some(Edit::Scale(
                json.get("value")?
                    .as_f64()
                    .filter(|scale| scale.is_finite())?,
            )),
            "transform" => Some(Edit::Transform(Transform::from_name(
                json.get("value")?.as_str()?,
            )?)),
            "vrr" => Some(Edit::Vrr(Vrr::from_name(json.get("value")?.as_str()?)?)),
            "position" => Some(Edit::Position(
                i32::try_from(json.get("x")?.as_i64()?).ok()?,
                i32::try_from(json.get("y")?.as_i64()?).ok()?,
            )),
            _ => None,
        }
    }
}

// `position set -- X Y`: niri's CLI parses a negative coordinate as a flag unless `--` goes before
// it, and Scale, mode and the rest never take a negative number
fn position_words(x: i32, y: i32) -> Vec<String> {
    ["position", "set", "--"]
        .into_iter()
        .map(str::to_owned)
        .chain([x.to_string(), y.to_string()])
        .collect()
}

// a scale as written: 1.25, 2
fn scale_text(scale: f64) -> String {
    format_number(scale, 4)
}

/// Whether `edit` may be tried on `output`, `outputs` being all that are connected. The reason
/// when it may not.
pub fn check(edit: &Edit, output: &Output, outputs: &[Output]) -> Result<(), String> {
    if !output.enabled() && *edit != Edit::Enabled(true) {
        return Err(format!("{} is off: turn it on first", output.name));
    }

    match edit {
        Edit::Enabled(false) => {
            let others = outputs
                .iter()
                .filter(|other| other.name != output.name && other.enabled())
                .count();

            // the only rule about what stays on: a screen must be left to see Keep and Revert on
            if others == 0 {
                return Err(String::from(
                    "It is the only output that is on; turning it off would leave no screen",
                ));
            }
        }
        Edit::Enabled(true) => {}
        Edit::Mode(mode) => {
            // a custom mode is not in the list, so a revert could not name it
            if output.current_mode().is_none() {
                return Err(format!(
                    "{} is on a custom mode, which Kanade cannot put back; change it in the config",
                    output.name
                ));
            }

            if !output.modes.iter().any(|listed| listed.same(mode)) {
                return Err(format!(
                    "{}: {} is not one of its modes",
                    output.name,
                    mode.text()
                ));
            }
        }
        Edit::Scale(scale) => {
            let (least, most, _) = SCALES;

            if !scale.is_finite() || *scale < least || *scale > most {
                return Err(format!(
                    "A scale is between {} and {}; niri rounds it to one the screen can show",
                    format_number(least, 2),
                    format_number(most, 2)
                ));
            }
        }
        Edit::Vrr(vrr) => {
            if *vrr != Vrr::Off && !output.vrr_supported {
                return Err(format!(
                    "{} does not support variable refresh rate",
                    output.name
                ));
            }
        }
        Edit::Transform(_) | Edit::Position(..) => {}
    }

    Ok(())
}

// ---- the state to come back to ----

/// An output as niri reported it before a trial, to put back.
#[derive(Debug, Clone, PartialEq)]
pub struct Previous {
    pub enabled: bool,

    // none for a custom mode, which is left as it is
    pub mode: Option<String>,
    pub scale: f64,
    pub transform: Transform,
    pub x: i32,
    pub y: i32,
    pub vrr: Vrr,
}

impl Previous {
    pub fn of(output: &Output, hint: Option<Vrr>) -> Previous {
        let logical = output.logical;

        Previous {
            enabled: output.enabled(),
            mode: output.current_mode().map(|mode| mode.text()),
            scale: logical.map_or(1.0, |logical| logical.scale),
            transform: logical.map_or(Transform::Normal, |logical| logical.transform),
            x: logical.map_or(0, |logical| logical.x),
            y: logical.map_or(0, |logical| logical.y),
            vrr: Vrr::of(output.vrr_enabled, hint),
        }
    }

    fn to_json(&self) -> Json {
        serde_json::json!({
            "enabled": self.enabled,
            "mode": self.mode,
            "scale": self.scale,
            "transform": self.transform.name(),
            "x": self.x,
            "y": self.y,
            "vrr": self.vrr.name(),
        })
    }

    fn from_json(json: &Json) -> Option<Previous> {
        Some(Previous {
            enabled: json.get("enabled")?.as_bool()?,
            mode: match json.get("mode")? {
                Json::Null => None,
                mode => Some(mode.as_str()?.to_owned()),
            },
            scale: json
                .get("scale")?
                .as_f64()
                .filter(|scale| scale.is_finite())?,
            transform: Transform::from_name(json.get("transform")?.as_str()?)?,
            x: i32::try_from(json.get("x")?.as_i64()?).ok()?,
            y: i32::try_from(json.get("y")?.as_i64()?).ok()?,
            vrr: Vrr::from_name(json.get("vrr")?.as_str()?)?,
        })
    }
}

/// Whether an output may be turned off: another one that niri lists is on. A list niri did not
/// give, or an empty one, is not known to leave a screen.
pub fn leaves_a_screen(outputs: Option<&[Output]>, name: &str) -> bool {
    outputs.is_some_and(|outputs| {
        outputs
            .iter()
            .any(|other| other.name != name && other.enabled())
    })
}

/// What puts `previous` back, as the words after `niri msg output <name>`, one command each. An
/// output niri shows as it was needs nothing but the fields in `force` (those a trial changed:
/// on demand cannot be told from on, so it is not compared); `now` unknown, or an output that is
/// off, needs all of it. An output that was off is turned off only if `may_turn_off`: putting a
/// monitor back must never take the last screen away.
pub fn restore_plan(
    previous: &Previous,
    now: Option<&Output>,
    force: &[Field],
    may_turn_off: bool,
) -> Vec<Vec<String>> {
    let words = |words: &[&str]| {
        words
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>()
    };
    let is_on = now.is_some_and(Output::enabled);

    if !previous.enabled {
        return if (now.is_none() || is_on) && may_turn_off {
            vec![words(&["off"])]
        } else {
            Vec::new()
        };
    }

    let shown = now.filter(|_| is_on);
    let logical = shown.and_then(|output| output.logical);
    let wanted = |field: Field, differs: bool| shown.is_none() || differs || force.contains(&field);
    let mut plan = Vec::new();

    if !is_on {
        plan.push(words(&["on"]));
    }

    if let Some(mode) = &previous.mode
        && wanted(
            Field::Mode,
            shown
                .and_then(|output| output.current_mode())
                .map(|mode| mode.text())
                .as_ref()
                != Some(mode),
        )
    {
        plan.push(words(&["mode", mode]));
    }

    if wanted(
        Field::Scale,
        logical.is_none_or(|logical| (logical.scale - previous.scale).abs() > 1e-3),
    ) {
        plan.push(vec!["scale".into(), scale_text(previous.scale)]);
    }

    if wanted(
        Field::Transform,
        logical.is_none_or(|logical| logical.transform != previous.transform),
    ) {
        plan.push(words(&["transform", &previous.transform.name()]));
    }

    if wanted(
        Field::Position,
        logical.is_none_or(|logical| (logical.x, logical.y) != (previous.x, previous.y)),
    ) {
        plan.push(position_words(previous.x, previous.y));
    }

    // on-demand VRR looks off to niri, so `vrr_enabled` cannot detect it. Restore it only when the
    // trial changed VRR (`force`); otherwise preserve niri's change. Off and on remain visible.
    let vrr_changed = force.contains(&Field::Vrr);
    let vrr_wanted = if previous.vrr == Vrr::OnDemand {
        vrr_changed
    } else {
        match shown {
            Some(output) => {
                output.vrr_supported
                    && (vrr_changed || output.vrr_enabled != (previous.vrr != Vrr::Off))
            }
            None => vrr_changed || previous.vrr != Vrr::Off,
        }
    };

    if vrr_wanted {
        plan.push(match previous.vrr {
            Vrr::Off => words(&["vrr", "off"]),
            Vrr::On => words(&["vrr", "on"]),
            Vrr::OnDemand => words(&["vrr", "--on-demand", "on"]),
        });
    }

    plan
}

/// Where the config and niri disagree about what was kept, in words: niri rounds a scale, or an
/// output may not take what the file now says.
pub fn disagreements(edits: &[&Edit], output: &Output) -> Vec<String> {
    let logical = output.logical;
    let mut said = Vec::new();

    for edit in edits {
        let differs = match edit {
            Edit::Enabled(on) => output.enabled() != *on,
            Edit::Mode(mode) => output
                .current_mode()
                .is_none_or(|current| !current.same(mode)),
            Edit::Scale(scale) => {
                logical.is_none_or(|logical| (logical.scale - scale).abs() > 1e-3)
            }
            Edit::Transform(transform) => {
                logical.is_none_or(|logical| logical.transform != *transform)
            }
            Edit::Position(x, y) => {
                logical.is_none_or(|logical| (logical.x, logical.y) != (*x, *y))
            }
            Edit::Vrr(Vrr::Off) => output.vrr_enabled,
            Edit::Vrr(Vrr::On) => !output.vrr_enabled,

            // niri shows it on only while a window wants it
            Edit::Vrr(Vrr::OnDemand) => false,
        };

        if !differs {
            continue;
        }

        said.push(match edit {
            Edit::Enabled(on) => format!(
                "the file turns {} {} but niri shows it {}",
                output.name,
                if *on { "on" } else { "off" },
                if output.enabled() { "on" } else { "off" }
            ),
            Edit::Mode(mode) => format!(
                "the file has mode {} but niri uses {}",
                mode.text(),
                output
                    .current_mode()
                    .map_or_else(|| String::from("another"), |mode| mode.text())
            ),
            Edit::Scale(scale) => format!(
                "the file has scale {} but niri uses {}",
                format_number(*scale, 4),
                logical.map_or_else(|| String::from("none"), |l| format_number(l.scale, 4))
            ),
            Edit::Transform(transform) => format!(
                "the file has transform {} but niri uses {}",
                transform.name(),
                logical.map_or_else(|| String::from("none"), |logical| logical.transform.name())
            ),
            Edit::Position(x, y) => format!(
                "the file has position {x}, {y} but niri uses {}",
                logical.map_or_else(|| String::from("none"), |l| format!("{}, {}", l.x, l.y))
            ),
            Edit::Vrr(vrr) => format!(
                "the file has variable refresh rate {} but niri shows it {}",
                vrr.name(),
                if output.vrr_enabled { "on" } else { "off" }
            ),
        });
    }

    said
}

/// Whether an output currently matches what it was before the trial.
pub fn matches_previous(output: &Output, previous: &Previous) -> bool {
    if output.enabled() != previous.enabled {
        return false;
    }
    if !previous.enabled {
        return true;
    }
    let current_mode = output.current_mode().map(|m| m.text());
    if previous.mode.is_some() && current_mode != previous.mode {
        return false;
    }
    let Some(logical) = output.logical else {
        return false;
    };
    if (logical.scale - previous.scale).abs() > 1e-3 {
        return false;
    }
    if logical.transform != previous.transform {
        return false;
    }
    if (logical.x, logical.y) != (previous.x, previous.y) {
        return false;
    }
    match previous.vrr {
        Vrr::Off => {
            if output.vrr_enabled {
                return false;
            }
        }
        Vrr::On => {
            if !output.vrr_enabled {
                return false;
            }
        }
        Vrr::OnDemand => {}
    }
    true
}

/// Whether an output currently matches the edits applied during the trial, with all other fields
/// matching `previous`.
pub fn matches_applied(output: &Output, applied: &[Edit], previous: &Previous) -> bool {
    if applied.is_empty() {
        return false;
    }
    let refs: Vec<&Edit> = applied.iter().collect();
    if !disagreements(&refs, output).is_empty() {
        return false;
    }
    let covered_fields: Vec<Field> = applied.iter().map(Edit::field).collect();
    if !covered_fields.contains(&Field::Enabled) && output.enabled() != previous.enabled {
        return false;
    }
    if !output.enabled() {
        return true;
    }
    if !covered_fields.contains(&Field::Mode) {
        let current_mode = output.current_mode().map(|m| m.text());
        if previous.mode.is_some() && current_mode != previous.mode {
            return false;
        }
    }
    let Some(logical) = output.logical else {
        return false;
    };
    if !covered_fields.contains(&Field::Scale) && (logical.scale - previous.scale).abs() > 1e-3 {
        return false;
    }
    if !covered_fields.contains(&Field::Transform) && logical.transform != previous.transform {
        return false;
    }
    if !covered_fields.contains(&Field::Position)
        && (logical.x, logical.y) != (previous.x, previous.y)
    {
        return false;
    }
    if !covered_fields.contains(&Field::Vrr) {
        match previous.vrr {
            Vrr::Off => {
                if output.vrr_enabled {
                    return false;
                }
            }
            Vrr::On => {
                if !output.vrr_enabled {
                    return false;
                }
            }
            Vrr::OnDemand => {}
        }
    }
    true
}

// ---- the trial ----

/// What the controller does to the world.
pub trait Port {
    /// The connected outputs, as niri reports them now.
    fn outputs(&self) -> Result<Vec<Output>, String>;

    /// `niri msg output <output> <args>`: `Refused` with niri's message when it says no,
    /// `Unanswered` when the command timed out or could not be checked (niri may have applied it).
    fn run(&self, output: &str, args: &[String]) -> Result<(), RunError>;

    /// Writes `edits` to the output's block in one validated change: what to say of it, and the
    /// hash the change left, which the page's Undo goes by (`Editor::last_change_hash`).
    fn write(&self, output: &Output, edits: &[&Edit]) -> Result<(String, Option<String>), String>;

    /// Restores what the last `write` replaced.
    fn undo_write(&self) -> Result<String, String>;

    /// Milliseconds on a clock that only goes forward.
    fn now(&self) -> u64;

    /// Milliseconds since the epoch, to date the pending file.
    fn unix_ms(&self) -> u64;
}

/// Why a `Port::run` failed: a definite refusal, or a command that never answered.
#[derive(Debug, Clone, PartialEq)]
pub enum RunError {
    /// niri said no; the message.
    Refused(String),

    /// The command timed out or could not be checked: niri may have applied it.
    Unanswered(String),
}

impl RunError {
    pub fn message(&self) -> String {
        match self {
            RunError::Refused(msg) | RunError::Unanswered(msg) => msg.clone(),
        }
    }
}

/// The trial in progress: one output, the changes made to it so far, and when they revert.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: u64,
    pub output: String,
    pub previous: Previous,
    pub edits: BTreeMap<Field, Edit>,

    // when the user's window to Keep ends, and restarts at each change; it never moves back
    pub deadline: u64,

    // when a revert that failed may be tried again; the deadline itself while no revert failed,
    // so `revert_when_due` and the trial's clock need no other rule
    pub retry_at: u64,

    // the config holds this trial (a Keep was written): from then it is niri's reload, not a
    // deadline, that decides between confirming and abandoning it, so no clock reverts it
    pub kept: bool,

    // why the last revert failed, which the page says while it retries
    pub stuck: Option<String>,
    pub taken: u64,
}

/// How a trial ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Ended {
    // what the write said, and the hash it left: what the page's Undo must match (`undo_only`)
    Kept(String, Option<String>),
    Reverted,

    // put back, but for an output left on because no other would be
    LeftOn(String),
}

/// What a Keep came to once niri said its word on the reload (`Controller::resolve_keep_reload`).
#[derive(Debug, Clone, PartialEq)]
pub enum Kept {
    // niri reloaded the config: the trial is over
    Confirmed,

    // niri rejected it: the config is back, and what the revert of the output came to
    Undone(Result<Ended, String>),

    // niri rejected it and the config could not be put back: why, and what the revert came to
    NotUndone(String, Result<(), String>),

    // niri has not said: the trial is over, its file kept as a record of a change already written
    Unconfirmed,
}

pub struct Controller {
    file: PathBuf,
    pub session: Option<Session>,
    next_session_id: u64,
}

impl Controller {
    pub fn new(file: PathBuf) -> Controller {
        Controller {
            file,
            session: None,
            next_session_id: 0,
        }
    }

    /// Where the pending file is kept: `outputs-pending.json` in Kanade's state directory.
    pub fn file() -> Option<PathBuf> {
        let home = std::env::var("HOME").ok();

        crate::config::base("XDG_STATE_HOME", ".local/state", home.as_deref())
            .map(|dir| dir.join("kanade/outputs-pending.json"))
    }

    /// Milliseconds left to Keep, none outside a trial.
    pub fn remaining(&self, now: u64) -> Option<u64> {
        self.session
            .as_ref()
            .map(|session| session.deadline.saturating_sub(now))
    }

    /// Milliseconds until the trial next needs its clock: `retry_at`, which is the deadline while no
    /// revert has failed. None outside a trial, or once the config holds it. The trial's clock waits
    /// on this rather than on a tick, so a deadline that passed while the worker was panicking, or
    /// blocked in niri, is still due when the clock next looks.
    pub fn wait(&self, now: u64) -> Option<u64> {
        self.unkept()
            .map(|session| session.retry_at.saturating_sub(now))
    }

    /// The trial as niri's still to put back: none once the config holds it (`Session::kept`), and
    /// none outside a trial.
    fn unkept(&self) -> Option<&Session> {
        self.session.as_ref().filter(|session| !session.kept)
    }

    /// Tries `edit` on the output `name` in niri's memory. The first change of a trial writes the
    /// pending file first and does nothing if that fails; a change niri refuses leaves the trial as
    /// it was (and ends it, deleting the file, when it was the first). `hint` is what the config
    /// says of the variable refresh rate (`Vrr::of`).
    pub fn try_edit(
        &mut self,
        port: &dyn Port,
        name: &str,
        edit: Edit,
        hint: Option<Vrr>,
    ) -> Result<(), String> {
        if let Some(session) = &self.session
            && session.output != name
        {
            return Err(format!(
                "Keep or revert the change to {} first",
                session.output
            ));
        }

        let outputs = port.outputs()?;
        let output = outputs
            .iter()
            .find(|output| output.name == name)
            .ok_or_else(|| format!("{name} is not connected"))?;

        check(&edit, output, &outputs)?;

        let now = port.now();

        if self.session.is_none() {
            // a fresh, unsaved file is another Kanade's unfinished trial: writing over it would
            // drop the only record that puts that monitor back, so the change is refused, not made
            if is_foreign(&self.file, port.unix_ms()) {
                return Err(String::from(
                    "Nothing was tried: a change another Kanade left unfinished is on file, and is \
                     put back the next time Kanade starts",
                ));
            }

            let previous = Previous::of(output, hint);
            let taken = port.unix_ms();

            write_pending(&self.file, name, &previous, &[], taken, false)?;
            self.next_session_id += 1;
            let id = self.next_session_id;
            self.session = Some(Session {
                id,
                output: name.to_owned(),
                previous,
                edits: BTreeMap::new(),
                deadline: now + TRIAL_MS,
                retry_at: now + TRIAL_MS,
                kept: false,
                stuck: None,
                taken,
            });
        }

        match port.run(name, &edit.args()) {
            Ok(()) => {}
            Err(RunError::Refused(why)) => {
                if self.session.as_ref().is_some_and(|s| s.edits.is_empty()) {
                    self.session = None;
                    delete_pending(&self.file);
                }

                return Err(why);
            }
            Err(RunError::Unanswered(why)) => {
                // niri may have applied it: keep the session and pending file so the
                // trial's deadline can revert, rather than discarding state for a change
                // that may be live
                if let Some(session) = &mut self.session {
                    session.edits.insert(edit.field(), edit);
                    session.deadline = now + TRIAL_MS;
                    session.retry_at = session.retry_at.max(session.deadline);
                    let applied: Vec<Edit> = session.edits.values().cloned().collect();
                    let _ = write_pending(
                        &self.file,
                        name,
                        &session.previous,
                        &applied,
                        session.taken,
                        false,
                    );
                }

                return Err(why);
            }
        }

        if let Some(session) = &mut self.session {
            session.edits.insert(edit.field(), edit);
            session.deadline = now + TRIAL_MS;
            session.retry_at = session.retry_at.max(session.deadline);
            // niri accepted a change, so the last revert's error is stale: leaving the page then
            // reverts at once, not after the failed revert's own deadline
            session.stuck = None;
            let applied: Vec<Edit> = session.edits.values().cloned().collect();
            let _ = write_pending(
                &self.file,
                name,
                &session.previous,
                &applied,
                session.taken,
                false,
            );
        }

        Ok(())
    }

    /// Writes the trial's changes to the config if the session matches and the request is timely.
    pub fn keep_request(
        &mut self,
        port: &dyn Port,
        id: u64,
        requested_at: u64,
    ) -> Result<Ended, String> {
        let Some(session) = &self.session else {
            return Err(String::from("The trial has ended"));
        };
        if session.id != id {
            return Err(String::from("The trial has ended"));
        }

        let now = port.now();

        // a request made after the deadline arrived too late, whether or not a tick has run since:
        // the window the user was given is over, so the change goes back instead of being written.
        // The grace is the worker's queue only, so a press made inside the window still counts.
        let late = requested_at > session.deadline;

        if late || now > session.deadline + KEEP_GRACE_MS {
            let _ = self.revert(port);

            return Err(if late {
                String::from("The trial has expired")
            } else {
                String::from("The trial has expired: Keep came too late")
            });
        }

        let outputs = port.outputs()?;
        let output = outputs
            .iter()
            .find(|output| output.name == session.output)
            .ok_or_else(|| format!("{} is no longer connected", session.output))?;
        let edits: Vec<&Edit> = session.edits.values().collect();
        let (said, after) = port.write(output, &edits)?;

        // Once config is written, prevent startup recovery from reverting it; if saving the record
        // fails, clear the pending file. The borrow of `session` ends here.
        let previous = session.previous.clone();
        let name = session.output.clone();
        let taken = session.taken;
        let applied: Vec<Edit> = session.edits.values().cloned().collect();

        // the config holds the trial now: the deadline no longer puts the output back, and the
        // reload's answer does (`confirm_keep`, `abandon_keep`)
        if let Some(session) = &mut self.session {
            session.kept = true;
        }

        if write_pending(&self.file, &name, &previous, &applied, taken, true).is_err() {
            delete_pending(&self.file);
        }

        Ok(Ended::Kept(said, after))
    }

    /// The reload succeeded and niri agrees: delete the pending file and end the trial.
    pub fn confirm_keep(&mut self) {
        self.session = None;
        delete_pending(&self.file);
    }

    /// The reload failed or niri disagrees: the session is cleared and the pending file is left.
    /// The config is already written, so the file is marked saved and a start clears it without
    /// touching niri; it is kept only as a record of the change, not to put the monitor back.
    pub fn abandon_keep(&mut self) {
        self.session = None;
    }

    /// Resolves keep outcome after reload: rejected configs undo the write and revert compositor
    /// state; accepted reloads confirm keep and remove the pending file; unconfirmed reloads
    /// abandon keep while preserving the record.
    pub fn resolve_keep_reload(
        &mut self,
        port: &dyn Port,
        loaded: Option<ipc::ConfigLoadState>,
    ) -> Kept {
        match loaded {
            Some(state) if state.last_failed => match port.undo_write() {
                Ok(_) => Kept::Undone(self.revert(port)),
                Err(why) => {
                    let reverted = self.revert_compositor(port);
                    self.abandon_keep();

                    Kept::NotUndone(why, reverted)
                }
            },
            Some(_) => {
                self.confirm_keep();

                Kept::Confirmed
            }
            None => {
                self.abandon_keep();

                Kept::Unconfirmed
            }
        }
    }

    /// Puts the output back as niri showed it before the trial, without clearing the session
    /// or deleting the pending file.
    pub fn revert_compositor(&self, port: &dyn Port) -> Result<(), String> {
        let Some(session) = &self.session else {
            return Ok(());
        };
        let force: Vec<Field> = session.edits.keys().copied().collect();

        let shown = port.outputs().ok().filter(|outputs| !outputs.is_empty());
        let gone = shown
            .as_ref()
            .is_some_and(|outputs| outputs.iter().all(|output| output.name != session.output));
        let now = shown
            .as_ref()
            .and_then(|outputs| outputs.iter().find(|output| output.name == session.output));
        let may_turn_off = leaves_a_screen(shown.as_deref(), &session.output);
        let plan = if gone {
            Vec::new()
        } else {
            restore_plan(&session.previous, now, &force, may_turn_off)
        };
        let mut failed = None;

        if shown.is_none() && !session.previous.enabled {
            failed = Some(String::from("niri did not list its outputs"));
        }

        for args in plan {
            if let Err(error) = port.run(&session.output, &args) {
                failed.get_or_insert(error.message());
            }
        }

        if let Some(why) = failed {
            Err(why)
        } else {
            Ok(())
        }
    }

    /// Puts the output back as niri showed it before the trial. When it cannot (niri refuses),
    /// the trial and the file stay and it is tried again in a few seconds.
    pub fn revert(&mut self, port: &dyn Port) -> Result<Ended, String> {
        let Some(mut session) = self.session.take() else {
            return Ok(Ended::Reverted);
        };
        let force: Vec<Field> = session.edits.keys().copied().collect();

        // an output unplugged since is nothing to put back; one niri cannot be asked about, or an
        // answer with no output in it at all, which is not that every output is gone, is given all
        // of it
        let shown = port.outputs().ok().filter(|outputs| !outputs.is_empty());
        let gone = shown
            .as_ref()
            .is_some_and(|outputs| outputs.iter().all(|output| output.name != session.output));
        let now = shown
            .as_ref()
            .and_then(|outputs| outputs.iter().find(|output| output.name == session.output));
        let may_turn_off = leaves_a_screen(shown.as_deref(), &session.output);
        let plan = if gone {
            Vec::new()
        } else {
            restore_plan(&session.previous, now, &force, may_turn_off)
        };
        let left_on =
            !session.previous.enabled && !may_turn_off && !gone && now.is_none_or(Output::enabled);
        let mut failed = None;

        // turning an output off needs to know another is on: without an answer, try again later
        if shown.is_none() && !session.previous.enabled {
            failed = Some(String::from("niri did not list its outputs"));
        }

        for args in plan {
            if let Err(error) = port.run(&session.output, &args) {
                failed.get_or_insert(error.message());
            }
        }

        if let Some(why) = failed {
            session.retry_at = port.now() + RETRY_MS;
            session.stuck = Some(why.clone());
            self.session = Some(session);

            return Err(why);
        }

        delete_pending(&self.file);

        if left_on {
            return Ok(Ended::LeftOn(left_on_words(&session.output)));
        }

        Ok(Ended::Reverted)
    }

    /// Reverts an unkept trial when its deadline passes or immediately if its page is gone.
    /// Returns `None` before the deadline or once the trial has already been kept in config.
    pub fn revert_when_due(
        &mut self,
        port: &dyn Port,
        gone: bool,
    ) -> Option<Result<Ended, String>> {
        let session = self.unkept()?;
        let due = (gone && session.stuck.is_none()) || port.now() >= session.retry_at;

        due.then(|| self.revert(port))
    }

    /// Reverts an active trial only while unkept by config, preventing external exits or worker
    /// shutdowns from rolling back changes the user has already kept.
    pub fn revert_unkept(&mut self, port: &dyn Port) -> Option<Result<Ended, String>> {
        self.unkept()?;

        Some(self.revert(port))
    }

    /// The trial-timeout case of `revert_when_due`: the tests drive the clock to a deadline with it.
    #[cfg(test)]
    pub fn expire(&mut self, port: &dyn Port) -> Option<Result<Ended, String>> {
        self.revert_when_due(port, false)
    }

    /// Recovers leftover trial state on startup without clobbering active trial ownership.
    /// In-flight sessions leave the pending file untouched; stale or unconfirmed files are reverted
    /// or cleaned up according to saved state and live compositor outputs.
    pub fn recover(&mut self, port: &dyn Port) -> Result<Option<String>, String> {
        // a file that is this shell's own trial's is not left over: taking it would put back the
        // trial's output while the page still shows the trial, and drop the file that reverts it
        if let Some(session) = &self.session {
            return Ok(Some(format!(
                "Left the pending-output file alone: {} is being changed now",
                session.output
            )));
        }

        let Some(text) = fs::read_to_string(&self.file).ok() else {
            return Ok(None);
        };
        let parsed = crate::sources::json::parse(&text).and_then(|json| {
            let output = json.get("output")?.as_str()?.to_owned();
            let taken = json.get("taken_ms")?.as_u64()?;
            let previous = Previous::from_json(json.get("previous")?)?;
            let applied: Vec<Edit> = json
                .get("applied")
                .and_then(Json::as_array)
                .map(|arr| arr.iter().filter_map(Edit::from_json).collect())
                .unwrap_or_default();
            let saved = json.get("saved").and_then(Json::as_bool).unwrap_or(false);
            Some((output, taken, previous, applied, saved))
        });
        let Some((name, taken, previous, applied, saved)) = parsed else {
            delete_pending(&self.file);

            return Ok(Some(String::from(
                "Dropped an unreadable pending-output file",
            )));
        };

        // a saved file is a record of a change already in the config, not a trial to put back
        if saved {
            delete_pending(&self.file);

            return Ok(Some(String::from(
                "Cleared the pending record of a saved output change",
            )));
        }

        // what a trial changed, which niri's own report cannot tell (on-demand reads as off), so
        // restoring the output needs those fields forced
        let force: Vec<Field> = applied.iter().map(Edit::field).collect();

        // no output at all is niri not ready, not every output gone: the file stays for later
        let outputs = port.outputs()?;

        if outputs.is_empty() {
            return Err(String::from("niri lists no outputs yet"));
        }

        let is_stale = port.unix_ms().saturating_sub(taken) > RECOVERY_MS;
        let found = outputs.iter().find(|output| output.name == name);

        if is_stale {
            let matches_app =
                found.is_some_and(|output| matches_applied(output, &applied, &previous));
            let matches_prev = found.is_some_and(|output| matches_previous(output, &previous));

            if matches_app {
                // The unconfirmed trial is STILL actively running on niri: DO NOT drop it!
                // Restore the monitor to previous and then delete the pending file.
                let may_turn_off = leaves_a_screen(Some(&outputs), &name);
                let plan = found
                    .map(|output| restore_plan(&previous, Some(output), &force, may_turn_off))
                    .unwrap_or_default();
                let left_on =
                    !previous.enabled && !may_turn_off && found.is_some_and(Output::enabled);

                for args in plan {
                    port.run(&name, &args).map_err(|e| e.message())?;
                }

                delete_pending(&self.file);

                if left_on {
                    return Ok(Some(left_on_words(&name)));
                }

                return Ok(Some(format!(
                    "Put {name} back as it was before an unfinished change"
                )));
            } else if matches_prev {
                delete_pending(&self.file);

                return Ok(Some(format!(
                    "{name} is already as it was before an unfinished change"
                )));
            } else {
                delete_pending(&self.file);

                return Ok(Some(format!(
                    "Dropped a pending-output file for {name} older than {} minutes",
                    RECOVERY_MS / 60_000
                )));
            }
        }

        let may_turn_off = leaves_a_screen(Some(&outputs), &name);
        let plan = found
            .map(|output| restore_plan(&previous, Some(output), &force, may_turn_off))
            .unwrap_or_default();
        let left_on = !previous.enabled && !may_turn_off && found.is_some_and(Output::enabled);

        for args in plan {
            port.run(&name, &args).map_err(|e| e.message())?;
        }

        delete_pending(&self.file);

        if left_on {
            return Ok(Some(left_on_words(&name)));
        }

        Ok(Some(format!(
            "Put {name} back as it was before an unfinished change"
        )))
    }
}

fn left_on_words(name: &str) -> String {
    format!("{name} was left on: turning it off would leave no screen on")
}

fn write_pending(
    file: &Path,
    name: &str,
    previous: &Previous,
    applied: &[Edit],
    taken: u64,
    saved: bool,
) -> Result<(), String> {
    let json = serde_json::json!({
        "output": name,
        "taken_ms": taken,
        "previous": previous.to_json(),
        "applied": applied.iter().map(Edit::to_json).collect::<Vec<_>>(),
        "saved": saved,
    });

    atomic::write(file, json.to_string().as_bytes(), FileMode::Private).map_err(|error| {
        format!(
            "Not tried: the file that puts {name} back after a crash could not be written: {error}"
        )
    })
}

/// Whether the pending file here is one a new trial must not write over. A fresh, unsaved file is
/// the record of a trial another Kanade was in when it ended: only recovery may put that monitor
/// back or judge the file stale, and `RECOVERY_MS` is that same judgement here. A `saved` file is a
/// record of a change already in the config (recovery deletes it), and an old one is a file recovery
/// would delete unread, so a new trial may replace both without losing a trial.
fn is_foreign(file: &Path, now: u64) -> bool {
    let Ok(text) = fs::read_to_string(file) else {
        return false;
    };
    let Some(json) = crate::sources::json::parse(&text) else {
        return false;
    };
    let saved = json.get("saved").and_then(Json::as_bool).unwrap_or(false);
    let Some(taken) = json.get("taken_ms").and_then(Json::as_u64) else {
        return false;
    };

    !saved && now.saturating_sub(taken) <= RECOVERY_MS
}

fn delete_pending(file: &Path) {
    if fs::remove_file(file).is_err() && file.exists() {
        #[cfg(unix)]
        if let Ok(metadata) = fs::metadata(file) {
            use std::os::unix::fs::PermissionsExt;

            let mut perms = metadata.permissions();
            if perms.mode() & 0o200 == 0 {
                perms.set_mode(0o600);
                let _ = fs::set_permissions(file, perms);
            }
        }
        let _ = fs::OpenOptions::new().write(true).truncate(true).open(file);
    }
}

#[cfg(test)]
mod tests;
