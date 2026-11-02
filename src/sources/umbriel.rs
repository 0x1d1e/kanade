//! Umbriel compositor source and IPC connection.
//!
//! Subscribes to events from Umbriel (overview, windows, workspaces, keyboard_layout, outputs)
//! over its unix socket and tracks status in `Umbriel` Service.

use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use kanade_runtime::service::Service;

use super::json::{self, Json};
use super::niri::Showing;
use crate::backoff::Backoff;
use crate::sources::niri::{FocusedWindow, StripWindow, WorkspaceInfo};
use crate::supervise;

/// Bidirectional bridge between Umbriel 128-bit hex UUID strings and sequential `u64` IDs.
#[derive(Debug)]
struct WindowBridge {
    next_id: u64,
    hex_to_id: HashMap<String, u64>,
    id_to_hex: HashMap<u64, String>,
}

impl Default for WindowBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowBridge {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            hex_to_id: HashMap::new(),
            id_to_hex: HashMap::new(),
        }
    }

    pub fn get_or_assign(&mut self, hex: &str) -> u64 {
        if let Some(&id) = self.hex_to_id.get(hex) {
            return id;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.hex_to_id.insert(hex.to_owned(), id);
        self.id_to_hex.insert(id, hex.to_owned());
        id
    }

    pub fn hex(&self, id: u64) -> Option<String> {
        self.id_to_hex.get(&id).cloned()
    }

    #[cfg(test)]
    fn id(&self, hex: &str) -> Option<u64> {
        self.hex_to_id.get(hex).copied()
    }
}

/// Saved workspace target details for resolving a bridged `u64` ID back to Umbriel selector.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceTarget {
    pub output: String,
    pub index: u32,
    pub name: Option<String>,
}

/// Bidirectional bridge between Umbriel workspace ID strings (`"<output>:<serial>"`) and `u64` IDs.
#[derive(Debug)]
struct WorkspaceBridge {
    next_id: u64,
    str_to_id: HashMap<String, u64>,
    id_to_str: HashMap<u64, String>,
    targets: HashMap<u64, WorkspaceTarget>,
}

impl Default for WorkspaceBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceBridge {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            str_to_id: HashMap::new(),
            id_to_str: HashMap::new(),
            targets: HashMap::new(),
        }
    }

    pub fn get_or_assign_str(&mut self, str_id: &str) -> u64 {
        if let Some(&id) = self.str_to_id.get(str_id) {
            return id;
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.str_to_id.insert(str_id.to_owned(), id);
        self.id_to_str.insert(id, str_id.to_owned());
        id
    }

    pub fn get_or_assign(
        &mut self,
        str_id: &str,
        output: String,
        index: u32,
        name: Option<String>,
    ) -> u64 {
        let id = self.get_or_assign_str(str_id);
        self.targets.insert(
            id,
            WorkspaceTarget {
                output,
                index,
                name,
            },
        );
        id
    }

    pub fn target(&self, id: u64) -> Option<WorkspaceTarget> {
        self.targets.get(&id).cloned()
    }

    #[cfg(test)]
    fn str_id(&self, id: u64) -> Option<String> {
        self.id_to_str.get(&id).cloned()
    }

    #[cfg(test)]
    fn id(&self, str_id: &str) -> Option<u64> {
        self.str_to_id.get(str_id).copied()
    }
}

/// Umbriel's hexadecimal UUID for a window shown by a Surface.
pub fn window_hex(id: u64) -> Option<String> {
    Umbriel::read().window_bridge.hex(id)
}

/// What the core tracks from Umbriel compositor.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seen {
    pub overview: bool,
    pub focused_output: Option<String>,
    pub focused_window: Option<FocusedWindow>,
    pub focused_maximized: bool,
    pub focused_workspace: Option<String>,
    // the window each output shows, its active workspace's remembered focus; empty while unknown.
    // `fullscreen` is Umbriel's own bit, never `maximized`, and the tile is the committed content size
    pub shown: BTreeMap<String, Showing>,
    pub layout: Option<String>,
    pub windows_count: usize,
    pub workspaces_count: usize,
    pub workspaces: Vec<WorkspaceInfo>,
    pub windows: Vec<StripWindow>,
}

/// Umbriel Service state.
#[derive(Debug, Default)]
pub struct Umbriel {
    pub seen: Seen,
    pub connected: bool,
    window_bridge: WindowBridge,
    workspace_bridge: WorkspaceBridge,
    raw_windows: Vec<Json>,
    raw_workspaces: Vec<Json>,
    output_widths: HashMap<String, f64>,
}

impl Service for Umbriel {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {}
}

/// Finds Umbriel's IPC socket: `$UMBRIEL_SOCKET` if set, else `$XDG_RUNTIME_DIR/umbriel-$WAYLAND_DISPLAY.sock`.
pub fn socket_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("UMBRIEL_SOCKET").filter(|p| !p.is_empty()) {
        let path = PathBuf::from(path);
        if is_socket(&path) {
            return Some(path);
        }
    }

    let runtime = env::var_os("XDG_RUNTIME_DIR")?;
    let display = env::var_os("WAYLAND_DISPLAY")?;
    let path =
        PathBuf::from(runtime).join(format!("umbriel-{}.sock", Path::new(&display).display()));
    if is_socket(&path) { Some(path) } else { None }
}

fn is_socket(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        fs::metadata(path).is_ok_and(|m| m.file_type().is_socket())
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Whether Umbriel IPC is active and responsive.
pub fn is_active() -> bool {
    Umbriel::read().connected || socket_path().is_some_and(|p| UnixStream::connect(p).is_ok())
}

/// Encodes a spawn command request as Umbriel IPC JSON action.
pub fn spawn_action(command: &str) -> String {
    format!(
        r#"{{"cmd":"action","action":"spawn","command":{}}}"#,
        json::quote(command)
    )
}

/// Spawns a shell command via Umbriel compositor IPC over its Unix socket.
pub fn spawn(command: &str) -> Result<(), String> {
    let reply = request(&spawn_action(command))?;
    decode_response(&reply.to_string())
}

/// Reads open windows from Umbriel's IPC query.
pub fn windows() -> Result<Json, String> {
    request(r#"{"cmd":"windows"}"#)
}

/// Reads the connected monitors from Umbriel.
pub fn outputs() -> Result<Json, String> {
    let reply = request(r#"{"cmd":"outputs"}"#)?;
    decode_response(&reply.to_string())?;
    Ok(reply)
}

/// Encodes output properties for a live trial, without changing the config.
pub fn set_output_action(output: &str, properties: &serde_json::Map<String, Json>) -> String {
    let mut action = properties.clone();
    action.insert("cmd".into(), Json::from("action"));
    action.insert("action".into(), Json::from("output-set"));
    action.insert("output".into(), Json::from(output));
    Json::Object(action).to_string()
}

pub fn set_output(output: &str, properties: &serde_json::Map<String, Json>) -> Result<(), String> {
    let reply = request(&set_output_action(output, properties))?;
    decode_response(&reply.to_string())
}

/// Converts Umbriel's outputs reply to the monitor model shared by both Settings pages.
pub fn parse_outputs(reply: &Json) -> Vec<crate::settings::outputs::Output> {
    use crate::settings::outputs::{Logical, Mode, Output, Transform};
    let Some(listed) = reply.get("ok").and_then(Json::as_array) else {
        return Vec::new();
    };
    let mut outputs: Vec<_> = listed
        .iter()
        .filter_map(|output| {
            let name = output.get("name")?.as_str()?.to_owned();
            let mut current = None;
            let modes = output
                .get("modes")?
                .as_array()?
                .iter()
                .filter_map(|mode| {
                    let parsed = Mode {
                        width: mode.get("width")?.as_u64()?.try_into().ok()?,
                        height: mode.get("height")?.as_u64()?.try_into().ok()?,
                        refresh: mode.get("refresh_mhz")?.as_u64()?.try_into().ok()?,
                        preferred: mode.get("preferred").and_then(Json::as_bool) == Some(true),
                    };
                    Some((
                        parsed,
                        mode.get("current").and_then(Json::as_bool) == Some(true),
                    ))
                })
                .enumerate()
                .map(|(at, (mode, active))| {
                    if active {
                        current = Some(at);
                    }
                    mode
                })
                .collect();
            let logical = (output.get("enabled").and_then(Json::as_bool) == Some(true))
                .then(|| {
                    let size = output.get("logical_size")?;
                    let position = output.get("position")?;
                    let scale = output
                        .get("scale")?
                        .as_f64()
                        .filter(|scale| scale.is_finite() && *scale > 0.0)?;
                    Some(Logical {
                        x: position.get("x")?.as_i64()?.try_into().ok()?,
                        y: position.get("y")?.as_i64()?.try_into().ok()?,
                        width: size.get("width")?.as_u64()?.try_into().ok()?,
                        height: size.get("height")?.as_u64()?.try_into().ok()?,
                        scale,
                        transform: Transform::from_name(output.get("transform")?.as_str()?)?,
                    })
                })
                .flatten();
            let text = |key| {
                output
                    .get(key)
                    .and_then(Json::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            };
            let vrr_enabled = output
                .get("vrr_enabled")
                .or_else(|| output.get("adaptive_sync"))
                .and_then(Json::as_bool)
                == Some(true);
            Some(Output {
                name,
                make: text("make"),
                model: text("model"),
                serial: text("serial"),
                modes,
                current: logical.and(current),
                logical,
                // An active adaptive-sync state proves support; a false state alone does not.
                vrr_supported: output.get("vrr_supported").and_then(Json::as_bool) == Some(true)
                    || vrr_enabled,
                vrr_enabled,
            })
        })
        .collect();
    outputs.sort_by(|a, b| a.name.cmp(&b.name));
    outputs
}

/// Encodes a window-focus action using Umbriel's hexadecimal window id.
pub fn focus_window_action(id: &str) -> String {
    format!(
        r#"{{"cmd":"action","action":"window-focus","window":{}}}"#,
        json::quote(id)
    )
}

/// Focuses a window by Umbriel's hexadecimal window id.
pub fn focus_window(id: &str) -> Result<(), String> {
    let reply = request(&focus_window_action(id))?;
    decode_response(&reply.to_string())
}

/// Encodes a window-consume-right action request.
pub fn consume_window_right_action() -> String {
    r#"{"cmd":"action","action":"window-consume-right"}"#.to_string()
}

/// Stacks the focused window into the column to the right.
pub fn consume_window_right() -> Result<(), String> {
    let reply = request(&consume_window_right_action())?;
    decode_response(&reply.to_string())
}

/// Encodes a window-consume-or-expel-right action request.
pub fn expel_window_bottom_action() -> String {
    r#"{"cmd":"action","action":"window-consume-or-expel-right"}"#.to_string()
}

/// Expels the focused window into a separate column.
pub fn expel_window_bottom() -> Result<(), String> {
    let reply = request(&expel_window_bottom_action())?;
    decode_response(&reply.to_string())
}

/// Encodes a window-toggle-maximize action request.
pub fn toggle_maximize_action() -> String {
    r#"{"cmd":"action","action":"window-toggle-maximize"}"#.to_string()
}

/// Toggles maximized state for the focused window.
pub fn toggle_maximize() -> Result<(), String> {
    let reply = request(&toggle_maximize_action())?;
    decode_response(&reply.to_string())
}

/// Encodes a window-toggle-floating action request.
pub fn toggle_floating_action() -> String {
    r#"{"cmd":"action","action":"window-toggle-floating"}"#.to_string()
}

/// Toggles floating state for the focused window.
pub fn toggle_floating() -> Result<(), String> {
    let reply = request(&toggle_floating_action())?;
    decode_response(&reply.to_string())
}

/// Encodes a window-set-primary-extent action request.
pub fn set_primary_extent_action(fraction: f64) -> String {
    format!(r#"{{"cmd":"action","action":"window-set-primary-extent","fraction":{fraction}}}"#)
}

/// Sets the focused window's primary extent fraction.
pub fn set_primary_extent(fraction: f64) -> Result<(), String> {
    let reply = request(&set_primary_extent_action(fraction))?;
    decode_response(&reply.to_string())
}

/// Encodes a window-cycle-primary-extent action request.
pub fn cycle_primary_extent_action() -> String {
    r#"{"cmd":"action","action":"window-cycle-primary-extent"}"#.to_string()
}

/// Cycles the primary extent of the focused window through presets.
pub fn cycle_primary_extent() -> Result<(), String> {
    let reply = request(&cycle_primary_extent_action())?;
    decode_response(&reply.to_string())
}

/// Encodes a window-move-to-workspace action request using index and output.
pub fn move_window_to_workspace_with_output_action(index: u32, output: &str) -> String {
    format!(
        r#"{{"cmd":"action","action":"window-move-to-workspace","workspace":{index},"output":{}}}"#,
        json::quote(output)
    )
}

/// Encodes a window-move-to-workspace action request from a bridged `workspace_id`.
pub fn move_window_to_workspace_action(workspace_id: u64) -> Result<String, String> {
    let target = Umbriel::read()
        .workspace_bridge
        .target(workspace_id)
        .ok_or_else(|| format!("unknown workspace id: {workspace_id}"))?;

    Ok(move_window_to_workspace_with_output_action(
        target.index,
        &target.output,
    ))
}

/// Moves the focused window to the target workspace identified by bridged `workspace_id`.
pub fn move_window_to_workspace(workspace_id: u64) -> Result<(), String> {
    let action = move_window_to_workspace_action(workspace_id)?;
    let reply = request(&action)?;
    decode_response(&reply.to_string())
}

fn request(request: &str) -> Result<Json, String> {
    let socket = socket_path().ok_or_else(|| String::from("umbriel socket not found"))?;
    let mut stream =
        UnixStream::connect(&socket).map_err(|e| format!("connecting to umbriel socket: {e}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| e.to_string())?;
    stream
        .write_all(format!("{request}\n").as_bytes())
        .map_err(|e| format!("sending request to umbriel: {e}"))?;

    let mut reply = String::new();
    let bytes = BufReader::new(stream)
        .read_line(&mut reply)
        .map_err(|e| format!("reading umbriel response: {e}"))?;
    if bytes == 0 {
        return Err(String::from("umbriel closed socket without answering"));
    }

    json::parse(&reply).ok_or_else(|| format!("invalid JSON in umbriel response: {reply:?}"))
}

pub(crate) fn decode_response(reply: &str) -> Result<(), String> {
    let response =
        json::parse(reply).ok_or_else(|| format!("invalid JSON in umbriel response: {reply:?}"))?;

    if let Some(err) = response.get("err").filter(|v| !v.is_null()) {
        if let Some(msg) = err.as_str() {
            return Err(format!("umbriel rejected request: {msg}"));
        }
        return Err(format!("umbriel rejected request: {err}"));
    }

    if response.get("ok").is_some() {
        return Ok(());
    }

    Err(format!("unrecognized umbriel response: {reply}"))
}

static FOLLOWING: AtomicBool = AtomicBool::new(false);
const FIRST_RETRY: Duration = Duration::from_millis(500);
const LAST_RETRY: Duration = Duration::from_secs(5);
const HEALTHY: Duration = Duration::from_secs(60);

/// Starts following Umbriel's event stream on a background worker thread.
pub fn follow() {
    if FOLLOWING.swap(true, Ordering::Relaxed) {
        return;
    }

    supervise::run("umbriel", || {
        let mut backoff = Backoff::new(FIRST_RETRY, LAST_RETRY);

        loop {
            if socket_path().is_none() {
                return;
            }
            let started = std::time::Instant::now();
            let reached = run_stream();

            let wait = backoff.after(started.elapsed(), HEALTHY);
            if reached {
                eprintln!("kanade: umbriel event stream lost; connecting again in {wait:?}");
            }
            thread::sleep(wait);
        }
    });
}

fn run_stream() -> bool {
    let Some(socket) = socket_path() else {
        return false;
    };

    let Ok(mut stream) = UnixStream::connect(&socket) else {
        return false;
    };

    let sub_req = b"{\"cmd\":\"subscribe\",\"events\":[\"overview\",\"windows\",\"workspaces\",\"keyboard_layout\",\"outputs\"]}\n";
    if stream.write_all(sub_req).is_err() {
        return false;
    }

    Umbriel::write().connected = true;

    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else {
            break;
        };
        if let Some(event) = json::parse(&line) {
            handle_event(&event);
        }
    }

    let mut state = Umbriel::write();
    state.connected = false;
    state.seen = Seen::default();
    state.raw_windows.clear();
    state.raw_workspaces.clear();
    state.output_widths.clear();
    state.workspace_bridge.targets.clear();
    true
}

struct ParsedWindow {
    hex_id: String,
    title: String,
    app_id: Option<String>,
    floating: bool,
    maximized: bool,
    fullscreen: bool,
    workspace: String,
    column: i64,
    row: i64,
    w: f64,
    h: f64,
    active: bool,
    focused: bool,
    urgent: bool,
    output: Option<String>,
}

fn update_windows_from_raw(state: &mut Umbriel) {
    let mut parsed_windows = Vec::with_capacity(state.raw_windows.len());
    for w in &state.raw_windows {
        let Some(hex_id) = w.get("id").and_then(Json::as_str).filter(|s| !s.is_empty()) else {
            continue;
        };
        let hex_id = hex_id.to_owned();
        let title = w
            .get("title")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let app_id = w
            .get("app_id")
            .and_then(Json::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from);
        let floating = w.get("floating").and_then(Json::as_bool).unwrap_or(false);
        let workspace = w
            .get("workspace")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let column = w.get("column").and_then(Json::as_i64).unwrap_or(-1);
        let row = w.get("row").and_then(Json::as_i64).unwrap_or(-1);
        let w_px = w.get("w").and_then(Json::as_f64).unwrap_or(0.0);
        let h_px = w.get("h").and_then(Json::as_f64).unwrap_or(0.0);
        let active = w.get("active").and_then(Json::as_bool).unwrap_or(false);
        let focused = w.get("focused").and_then(Json::as_bool).unwrap_or(false);
        let urgent = w.get("urgent").and_then(Json::as_bool).unwrap_or(false);
        let output = w.get("output").and_then(Json::as_str).map(String::from);

        parsed_windows.push(ParsedWindow {
            hex_id,
            title,
            app_id,
            floating,
            maximized: w.get("maximized").and_then(Json::as_bool).unwrap_or(false),
            fullscreen: w.get("fullscreen").and_then(Json::as_bool).unwrap_or(false),
            workspace,
            column,
            row,
            w: w_px,
            h: h_px,
            active,
            focused,
            urgent,
            output,
        });
    }

    // Focus determination:
    // 1. Active seat window (active == true)
    let mut focused_win = parsed_windows.iter().find(|w| w.active);

    // 2. Fallback to workspace's remembered workspace["focused_window"] (a hex string)
    if focused_win.is_none() {
        let ws_focused_hex = state
            .raw_workspaces
            .iter()
            .find(|ws| {
                ws.get("is_focused").and_then(Json::as_bool) == Some(true)
                    || ws.get("focused").and_then(Json::as_bool) == Some(true)
            })
            .or_else(|| {
                state.raw_workspaces.iter().find(|ws| {
                    let is_active = ws.get("is_active").and_then(Json::as_bool) == Some(true)
                        || ws.get("active").and_then(Json::as_bool) == Some(true);
                    let on_focused_output =
                        state.seen.focused_output.as_deref().is_some_and(|f_out| {
                            ws.get("output").and_then(Json::as_str) == Some(f_out)
                        });
                    is_active && on_focused_output
                })
            })
            .or_else(|| {
                state.raw_workspaces.iter().find(|ws| {
                    ws.get("is_active").and_then(Json::as_bool) == Some(true)
                        || ws.get("active").and_then(Json::as_bool) == Some(true)
                })
            })
            .and_then(|ws| ws.get("focused_window").and_then(Json::as_str))
            .filter(|hex| !hex.is_empty());

        if let Some(hex) = ws_focused_hex {
            focused_win = parsed_windows.iter().find(|w| w.hex_id == hex);
        }
    }

    // Before the workspace snapshot arrives, the window snapshot is all we have.
    if focused_win.is_none() && state.raw_workspaces.is_empty() {
        focused_win = parsed_windows.iter().find(|w| w.focused);
    }

    if let Some(fw) = focused_win {
        if let Some(ref out) = fw.output {
            state.seen.focused_output = Some(out.clone());
        }
        let id = state.window_bridge.get_or_assign(&fw.hex_id);
        let workspace_id = if !fw.workspace.is_empty() {
            Some(state.workspace_bridge.get_or_assign_str(&fw.workspace))
        } else {
            None
        };

        let tile_count = if !fw.floating && fw.column >= 0 {
            parsed_windows
                .iter()
                .filter(|other| {
                    !other.floating && other.workspace == fw.workspace && other.column == fw.column
                })
                .count()
                .max(1)
        } else {
            1
        };

        let column_width = if fw.floating {
            None
        } else {
            let out_name = fw
                .output
                .as_deref()
                .or(state.seen.focused_output.as_deref());
            let output_width = out_name
                .and_then(|name| state.output_widths.get(name).copied())
                .or_else(|| {
                    kanade_runtime::Monitors::read()
                        .all()
                        .iter()
                        .find(|m| out_name == Some(&m.name))
                        .map(|m| m.width as f64)
                })
                .filter(|width| *width > 0.0);
            output_width.map(|width| fw.w / width)
        };

        state.seen.focused_maximized = fw.maximized;
        state.seen.focused_window = Some(FocusedWindow {
            id,
            title: fw.title.clone(),
            app_id: fw.app_id.clone(),
            is_floating: fw.floating,
            tile_count,
            column_width,
            workspace_id,
        });
    } else {
        state.seen.focused_window = None;
        state.seen.focused_maximized = false;
    }

    state.seen.shown = shown_windows(state, &parsed_windows);

    // Sequential fallback for non-floating windows with column < 0
    let mut max_cols: HashMap<String, u64> = HashMap::new();
    for w in &parsed_windows {
        if !w.floating && w.column >= 0 {
            let entry = max_cols.entry(w.workspace.clone()).or_insert(0);
            *entry = (*entry).max(w.column as u64);
        }
    }
    let mut next_fallback_cols: HashMap<String, u64> = max_cols
        .into_iter()
        .map(|(ws, max_c)| (ws, max_c + 1))
        .collect();

    let focused_hex = focused_win.map(|f| &f.hex_id);

    let mut strip_windows = Vec::with_capacity(parsed_windows.len());
    for w in &parsed_windows {
        let id = state.window_bridge.get_or_assign(&w.hex_id);
        let workspace_id = if !w.workspace.is_empty() {
            Some(state.workspace_bridge.get_or_assign_str(&w.workspace))
        } else {
            None
        };

        let col = if w.column >= 0 {
            Some(w.column as u64)
        } else if !w.floating {
            let entry = next_fallback_cols.entry(w.workspace.clone()).or_insert(0);
            let val = *entry;
            *entry += 1;
            Some(val)
        } else {
            None
        };

        let row = if w.row >= 0 { Some(w.row as u64) } else { None };
        let is_focused = focused_hex == Some(&w.hex_id);

        strip_windows.push(StripWindow {
            id,
            title: w.title.clone(),
            app_id: w.app_id.clone(),
            workspace_id,
            is_floating: w.floating,
            is_focused,
            is_urgent: w.urgent,
            col,
            row,
            tile_width: Some(w.w),
            tile_height: Some(w.h),
        });
    }

    strip_windows.sort_by_key(|w| (w.workspace_id, w.col, w.row, w.id));
    state.seen.windows = strip_windows;
    state.seen.windows_count = parsed_windows.len();
}

// the window each output shows: its active workspace's remembered focus, the same window the
// Surfaces keep while a layer holds the keyboard. One with no remembered window shows nothing.
// A missing `fullscreen` bit is not fullscreen.
fn shown_windows(state: &Umbriel, windows: &[ParsedWindow]) -> BTreeMap<String, Showing> {
    let mut shown = BTreeMap::new();

    for workspace in &state.raw_workspaces {
        let active = workspace
            .get("is_active")
            .or_else(|| workspace.get("active"))
            .and_then(Json::as_bool)
            == Some(true);
        if !active {
            continue;
        }

        let Some(output) = workspace.get("output").and_then(Json::as_str) else {
            continue;
        };
        let Some(id) = workspace
            .get("focused_window")
            .and_then(Json::as_str)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        let Some(window) = windows.iter().find(|window| window.hex_id == id) else {
            continue;
        };

        shown.insert(
            output.to_owned(),
            Showing {
                app_id: window.app_id.clone(),
                tile: (window.w, window.h),
                fullscreen: window.fullscreen,
            },
        );
    }

    shown
}

fn update_workspaces(state: &mut Umbriel, workspaces: &[Json]) {
    state.raw_workspaces = workspaces.to_vec();
    state.workspace_bridge.targets.clear();
    state.seen.workspaces_count = workspaces.len();

    let mut parsed_workspaces = Vec::with_capacity(workspaces.len());
    let mut explicit_focused_name = None;
    let mut explicit_focused_output = None;
    let mut active_for_current_output = None;
    let mut first_active_name = None;
    let mut first_active_output = None;

    for ws in workspaces {
        let output = ws.get("output").and_then(Json::as_str).map(String::from);
        let index = ws.get("index").and_then(Json::as_u64).unwrap_or(1) as u32;
        let str_id = ws
            .get("id")
            .and_then(Json::as_str)
            .map(String::from)
            .unwrap_or_else(|| format!("{}:{}", output.as_deref().unwrap_or(""), index));

        let named = ws.get("named").and_then(Json::as_bool).unwrap_or(false);
        let raw_name = ws.get("name").and_then(Json::as_str);
        let name = raw_name
            .filter(|n| named || *n != index.to_string())
            .map(String::from);

        let is_active = ws
            .get("is_active")
            .or_else(|| ws.get("active"))
            .and_then(Json::as_bool)
            .unwrap_or(false);
        let is_focused = ws
            .get("is_focused")
            .or_else(|| ws.get("focused"))
            .and_then(Json::as_bool)
            .unwrap_or(false);

        let id = state.workspace_bridge.get_or_assign(
            &str_id,
            output.clone().unwrap_or_default(),
            index,
            name.clone(),
        );

        if is_focused {
            explicit_focused_name = raw_name.map(String::from);
            explicit_focused_output = output.clone();
        } else if is_active {
            if first_active_name.is_none() {
                first_active_name = raw_name.map(String::from);
                first_active_output = output.clone();
            }
            if output.as_ref() == state.seen.focused_output.as_ref() {
                active_for_current_output = raw_name.map(String::from);
            }
        }

        parsed_workspaces.push(WorkspaceInfo {
            id,
            index,
            name,
            output,
            is_active,
            is_focused,
        });
    }

    if let Some(out) = explicit_focused_output {
        state.seen.focused_output = Some(out);
        state.seen.focused_workspace = explicit_focused_name;
    } else {
        if state.seen.focused_output.is_none() {
            state.seen.focused_output = first_active_output;
        }
        state.seen.focused_workspace = active_for_current_output.or(first_active_name);
    }

    parsed_workspaces.sort_by_key(|w| (w.output.clone(), w.index, w.id));
    state.seen.workspaces = parsed_workspaces;

    if !state.raw_windows.is_empty() {
        update_windows_from_raw(state);
    }
}

pub(crate) fn handle_event(event: &Json) {
    apply_event(&mut Umbriel::write(), event);
}

fn apply_event(state: &mut Umbriel, event: &Json) {
    let Some(event_type) = event.get("event").and_then(Json::as_str) else {
        return;
    };

    let data = event.get("data").unwrap_or(event);

    match event_type {
        "overview" => {
            if let Some(open) = data
                .get("open")
                .or_else(|| event.get("open"))
                .and_then(Json::as_bool)
            {
                state.seen.overview = open;
            }
        }
        "windows" => {
            let windows = data
                .as_array()
                .or_else(|| event.get("windows").and_then(Json::as_array));
            if let Some(windows) = windows {
                state.raw_windows = windows.to_vec();
                update_windows_from_raw(state);
            }
        }
        "workspaces" => {
            let workspaces = data
                .as_array()
                .or_else(|| event.get("workspaces").and_then(Json::as_array));
            if let Some(workspaces) = workspaces {
                update_workspaces(state, workspaces);
            }
        }
        "keyboard_layout" => {
            if let Some(layout) = data
                .get("layout")
                .or_else(|| event.get("layout"))
                .and_then(Json::as_str)
            {
                state.seen.layout = Some(layout.to_owned());
            } else if let Some(name) = data
                .get("names")
                .and_then(Json::as_array)
                .zip(data.get("current_index").and_then(Json::as_u64))
                .and_then(|(names, idx)| names.get(idx as usize))
                .and_then(Json::as_str)
            {
                state.seen.layout = Some(name.to_owned());
            }
        }
        "outputs" => {
            let outputs = data
                .as_array()
                .or_else(|| event.get("outputs").and_then(Json::as_array));
            if let Some(outputs) = outputs {
                state.output_widths.clear();
                let mut focused = None;
                let mut first = None;
                for out in outputs {
                    if let Some(name) = out.get("name").and_then(Json::as_str) {
                        if first.is_none() {
                            first = Some(name.to_string());
                        }
                        if out.get("focused").and_then(Json::as_bool) == Some(true) {
                            focused = Some(name.to_string());
                        }
                        let width = out
                            .get("logical_size")
                            .and_then(|size| size.get("width"))
                            .and_then(Json::as_f64)
                            .or_else(|| {
                                out.get("modes")
                                    .and_then(Json::as_array)
                                    .and_then(|modes| {
                                        modes
                                            .iter()
                                            .find(|m| {
                                                m.get("current").and_then(Json::as_bool)
                                                    == Some(true)
                                            })
                                            .or_else(|| modes.first())
                                    })
                                    .and_then(|m| m.get("width").and_then(Json::as_f64))
                                    .or_else(|| out.get("width").and_then(Json::as_f64))
                            });
                        let width = width.filter(|width| *width > 0.0);
                        if let Some(w) = width {
                            state.output_widths.insert(name.to_string(), w);
                        }
                    }
                }
                if let Some(out) = focused {
                    state.seen.focused_output = Some(out);
                } else if state.seen.focused_output.is_none() {
                    state.seen.focused_output = first;
                }
                if !state.raw_windows.is_empty() {
                    update_windows_from_raw(state);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outputs_parse_live_schema_and_skip_bad_modes_without_shifting_current() {
        let reply = serde_json::json!({"ok": [{
            "name": "DP-1", "make": "Vendor", "model": "Panel", "serial": "123",
            "enabled": true, "adaptive_sync": true,
            "logical_size": {"width": 1080, "height": 1920},
            "position": {"x": -1080, "y": 20}, "scale": 1.0, "transform": "90",
            "modes": [{"width": 999999}, {"width": 1920, "height": 1080, "refresh_mhz": 59940, "current": true, "preferred": true}]
        }, {
            "name": "HDMI-A-1", "enabled": false, "modes": []
        }, {"modes": []}]});
        let outputs = parse_outputs(&reply);
        assert_eq!(outputs.len(), 2);
        let first = &outputs[0];
        assert_eq!(first.current, Some(0));
        assert_eq!(first.current_mode().unwrap().refresh, 59940);
        assert_eq!(first.logical.unwrap().x, -1080);
        assert_eq!(
            first.logical.unwrap().transform,
            crate::settings::outputs::Transform::R90
        );
        assert!(first.vrr_enabled);
        assert!(first.vrr_supported);
        assert!(!outputs[1].enabled());
        assert!(!outputs[1].vrr_enabled);
        assert!(!outputs[1].vrr_supported);
        assert!(parse_outputs(&serde_json::json!({"err":"no outputs"})).is_empty());
    }

    #[test]
    fn output_action_quotes_names_and_preserves_property_types() {
        let properties =
            serde_json::json!({"position": [-1920, 10], "scale": 1.25, "enabled": true});
        let encoded = set_output_action("DP.1\"", properties.as_object().unwrap());
        let action: Json = serde_json::from_str(&encoded).unwrap();
        assert_eq!(action["cmd"], "action");
        assert_eq!(action["action"], "output-set");
        assert_eq!(action["output"], "DP.1\"");
        assert_eq!(action["position"], properties["position"]);
        assert_eq!(action["scale"], properties["scale"]);
        assert_eq!(action["enabled"], true);
    }

    #[test]
    fn event_handling_updates_service_state() {
        let mut state = Umbriel::default();
        let overview_json = json::parse(r#"{"event":"overview","open":true}"#).unwrap();
        apply_event(&mut state, &overview_json);
        assert!(state.seen.overview);

        let layout_json = json::parse(r#"{"event":"keyboard_layout","layout":"de"}"#).unwrap();
        apply_event(&mut state, &layout_json);
        assert_eq!(state.seen.layout.as_deref(), Some("de"));

        let windows_json = json::parse(
            r#"{"event":"windows","windows":[{"id":"58f26771340d94c9fd41b7841be9e3e0","title":"Terminal","focused":true},{"id":"c3a1b2c3d4e5f60123456789abcdef01","title":"Browser","focused":false}]}"#,
        )
        .unwrap();
        apply_event(&mut state, &windows_json);
        assert_eq!(state.seen.windows_count, 2);
        assert_eq!(
            state.seen.focused_window.as_ref().map(|w| w.title.as_str()),
            Some("Terminal")
        );

        let outputs_json = json::parse(
            r#"{"event":"outputs","data":[{"name":"eDP-1","focused":true},{"name":"HDMI-A-1","focused":false}]}"#,
        )
        .unwrap();
        apply_event(&mut state, &outputs_json);
        assert_eq!(state.seen.focused_output.as_deref(), Some("eDP-1"));

        // When no output has focus, existing focused_output is preserved
        let outputs_unfocused =
            json::parse(r#"{"event":"outputs","data":[{"name":"DP-2","focused":false}]}"#).unwrap();
        apply_event(&mut state, &outputs_unfocused);
        assert_eq!(state.seen.focused_output.as_deref(), Some("eDP-1"));

        // When no output has focus and focused_output is None, fallback to first output
        state.seen.focused_output = None;
        apply_event(&mut state, &outputs_unfocused);
        assert_eq!(state.seen.focused_output.as_deref(), Some("DP-2"));
    }

    #[test]
    fn spawn_action_encodes_json_request() {
        assert_eq!(
            spawn_action("exec 'firefox'"),
            r#"{"cmd":"action","action":"spawn","command":"exec 'firefox'"}"#
        );
        assert_eq!(
            spawn_action(r#"cd -- '/tmp' && exec 'foo "bar"'"#),
            r#"{"cmd":"action","action":"spawn","command":"cd -- '/tmp' && exec 'foo \"bar\"'"}"#
        );
        assert_eq!(
            focus_window_action("58f26771340d94c9fd41b7841be9e3e0"),
            r#"{"cmd":"action","action":"window-focus","window":"58f26771340d94c9fd41b7841be9e3e0"}"#
        );
    }

    #[test]
    fn decode_response_parses_ok_and_err() {
        assert_eq!(decode_response(r#"{"ok":null}"#), Ok(()));
        assert_eq!(decode_response(r#"{"ok":true}"#), Ok(()));
        assert_eq!(decode_response(r#"{"ok":[{"id":"1"}]}"#), Ok(()));
        assert_eq!(
            decode_response(r#"{"err":"executable not found"}"#),
            Err(String::from(
                "umbriel rejected request: executable not found"
            ))
        );
        assert!(decode_response(r#"{"invalid":123}"#).is_err());
        assert!(decode_response("not valid json").is_err());
    }

    #[test]
    fn window_bridge_maps_hex_uuids_to_sequential_ids_bidirectionally() {
        let mut bridge = WindowBridge::new();
        let hex1 = "58f26771340d94c9fd41b7841be9e3e0";
        let hex2 = "c3a1b2c3d4e5f60123456789abcdef01";

        let id1 = bridge.get_or_assign(hex1);
        let id2 = bridge.get_or_assign(hex2);
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);

        // Repeated assignment preserves existing ID
        assert_eq!(bridge.get_or_assign(hex1), 1);
        assert_eq!(bridge.hex(1).as_deref(), Some(hex1));
        assert_eq!(bridge.hex(2).as_deref(), Some(hex2));
        assert_eq!(bridge.id(hex1), Some(1));
        assert_eq!(bridge.id(hex2), Some(2));
        assert_eq!(bridge.hex(99), None);
    }

    #[test]
    fn workspace_bridge_maps_strings_and_resolves_targets() {
        let mut bridge = WorkspaceBridge::new();
        let str1 = "eDP-1:0";
        let str2 = "HDMI-A-1:1";

        let id1 = bridge.get_or_assign(str1, "eDP-1".into(), 1, None);
        let id2 = bridge.get_or_assign(str2, "HDMI-A-1".into(), 2, Some("dev".into()));

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(bridge.get_or_assign_str(str1), 1);
        assert_eq!(bridge.id(str1), Some(id1));
        assert_eq!(bridge.str_id(id2).as_deref(), Some(str2));

        let target1 = bridge.target(id1).expect("target 1");
        assert_eq!(target1.output, "eDP-1");
        assert_eq!(target1.index, 1);
        assert_eq!(target1.name, None);

        let target2 = bridge.target(id2).expect("target 2");
        assert_eq!(target2.output, "HDMI-A-1");
        assert_eq!(target2.index, 2);
        assert_eq!(target2.name.as_deref(), Some("dev"));
    }

    #[test]
    fn action_json_serializations_match_umbriel_ipc_schema() {
        let mut state = Umbriel::default();
        assert_eq!(
            consume_window_right_action(),
            r#"{"cmd":"action","action":"window-consume-right"}"#
        );
        assert_eq!(
            expel_window_bottom_action(),
            r#"{"cmd":"action","action":"window-consume-or-expel-right"}"#
        );
        assert_eq!(
            toggle_maximize_action(),
            r#"{"cmd":"action","action":"window-toggle-maximize"}"#
        );
        assert_eq!(
            toggle_floating_action(),
            r#"{"cmd":"action","action":"window-toggle-floating"}"#
        );
        assert_eq!(
            set_primary_extent_action(0.5),
            r#"{"cmd":"action","action":"window-set-primary-extent","fraction":0.5}"#
        );
        assert_eq!(
            cycle_primary_extent_action(),
            r#"{"cmd":"action","action":"window-cycle-primary-extent"}"#
        );

        // Workspace move action lookup
        let ws_id = state
            .workspace_bridge
            .get_or_assign("eDP-1:0", "eDP-1".into(), 1, None);
        let target = state.workspace_bridge.target(ws_id).unwrap();
        let move_act = move_window_to_workspace_with_output_action(target.index, &target.output);
        assert_eq!(
            move_act,
            r#"{"cmd":"action","action":"window-move-to-workspace","workspace":1,"output":"eDP-1"}"#
        );

        assert!(state.workspace_bridge.target(999).is_none());
    }

    #[test]
    fn full_workspaces_and_windows_snapshot_parsing() {
        let mut state = Umbriel::default();

        let outputs_json = json::parse(
            r#"{"event":"outputs","data":[{"name":"eDP-1","focused":true,"modes":[{"width":1920.0,"height":1080.0,"current":true}]}]}"#,
        )
        .unwrap();
        apply_event(&mut state, &outputs_json);

        let ws_json = json::parse(
            r#"{"event":"workspaces","data":[
                {"id":"eDP-1:0","index":1,"name":"1","named":false,"output":"eDP-1","active":true,"focused":true,"focused_window":"win-hex-1"},
                {"id":"eDP-1:1","index":2,"name":"code","named":true,"output":"eDP-1","active":false,"focused":false,"focused_window":""}
            ]}"#,
        )
        .unwrap();
        apply_event(&mut state, &ws_json);

        let win_json = json::parse(
            r#"{"event":"windows","data":[
                {"id":"win-hex-1","title":"Editor","app_id":"zed","floating":false,"maximized":true,"workspace":"eDP-1:0","column":0,"row":0,"w":960.0,"h":1080.0,"active":true,"focused":true,"output":"eDP-1"},
                {"id":"win-hex-2","title":"Terminal","app_id":"foot","floating":false,"workspace":"eDP-1:0","column":0,"row":1,"w":960.0,"h":540.0,"active":false,"focused":false,"output":"eDP-1"},
                {"id":"win-hex-3","title":"Music","app_id":"","floating":true,"workspace":"eDP-1:0","column":-1,"row":-1,"w":400.0,"h":300.0,"active":false,"focused":false,"output":"eDP-1"}
            ]}"#,
        )
        .unwrap();
        apply_event(&mut state, &win_json);

        let seen = state.seen.clone();
        assert_eq!(seen.workspaces.len(), 2);
        assert_eq!(seen.workspaces[0].index, 1);
        assert_eq!(seen.workspaces[0].name, None); // Unnamed with index matching
        assert!(seen.workspaces[0].is_active);
        assert!(seen.workspaces[0].is_focused);
        assert_eq!(seen.workspaces[1].index, 2);
        assert_eq!(seen.workspaces[1].name.as_deref(), Some("code"));

        // Focused window check
        let fw = seen.focused_window.expect("focused window");
        assert_eq!(fw.title, "Editor");
        assert_eq!(fw.app_id.as_deref(), Some("zed"));
        assert!(!fw.is_floating);
        assert!(seen.focused_maximized);
        assert_eq!(fw.tile_count, 2); // win-hex-1 and win-hex-2 in column 0
        assert_eq!(fw.column_width, Some(960.0 / 1920.0)); // 0.5

        // Windows strip check
        assert_eq!(seen.windows.len(), 3);
        let w_ed = seen.windows.iter().find(|w| w.title == "Editor").unwrap();
        assert!(w_ed.is_focused);
        assert_eq!(w_ed.col, Some(0));
        assert_eq!(w_ed.row, Some(0));
        assert_eq!(w_ed.tile_width, Some(960.0));

        let w_music = seen.windows.iter().find(|w| w.title == "Music").unwrap();
        assert_eq!(w_music.app_id, None); // empty app_id mapped to None
        assert!(w_music.is_floating);
        assert!(!w_music.is_focused);

        // Check window_hex lookup
        assert_eq!(state.window_bridge.hex(fw.id).as_deref(), Some("win-hex-1"));
        // maximized, and short of the output: the active workspace shows it, not as fullscreen
        let shown = seen.shown.get("eDP-1").expect("shown window");
        assert_eq!(shown.app_id.as_deref(), Some("zed"));
        assert_eq!(shown.tile, (960.0, 1080.0));
        assert!(!shown.fullscreen);
    }

    #[test]
    fn a_fullscreen_window_on_the_active_workspace_is_the_one_its_output_shows() {
        let mut state = Umbriel::default();
        let workspaces = json::parse(
            r#"{"event":"workspaces","data":[
                {"id":"eDP-1:0","output":"eDP-1","active":true,"focused":false,"focused_window":"game"},
                {"id":"HDMI-A-1:0","output":"HDMI-A-1","active":true,"focused":true,"focused_window":"notes"}
            ]}"#,
        )
        .unwrap();
        let windows = json::parse(
            r#"{"event":"windows","data":[
                {"id":"game","app_id":"steam","fullscreen":true,"maximized":false,"workspace":"eDP-1:0","w":1920.0,"h":1080.0,"output":"eDP-1"},
                {"id":"notes","app_id":"zed","maximized":true,"workspace":"HDMI-A-1:0","w":1920.0,"h":1080.0,"output":"HDMI-A-1"},
                {"id":"away","app_id":"foot","fullscreen":true,"workspace":"eDP-1:1","w":1920.0,"h":1080.0,"output":"eDP-1"}
            ]}"#,
        )
        .unwrap();
        apply_event(&mut state, &workspaces);
        apply_event(&mut state, &windows);

        let game = state.seen.shown.get("eDP-1").expect("eDP-1");
        assert!(game.fullscreen);
        assert_eq!(game.tile, (1920.0, 1080.0));
        let notes = state.seen.shown.get("HDMI-A-1").expect("HDMI-A-1");
        assert!(!notes.fullscreen);
        assert!(state.seen.shown.len() == 2);
    }

    #[test]
    fn focus_falls_back_to_workspace_remembered_focus_when_seat_inactive() {
        let mut state = Umbriel::default();

        let ws_json = json::parse(
            r#"{"event":"workspaces","data":[
                {"id":"eDP-1:0","index":1,"name":"1","output":"eDP-1","active":true,"focused":true,"focused_window":"win-hex-2"}
            ]}"#,
        )
        .unwrap();
        apply_event(&mut state, &ws_json);

        // Neither window has active == true (e.g. Island holds seat focus)
        let win_json = json::parse(
            r#"{"event":"windows","data":[
                {"id":"win-hex-1","title":"Editor","floating":false,"workspace":"eDP-1:0","column":0,"row":0,"w":960.0,"h":1080.0,"active":false,"focused":false,"output":"eDP-1"},
                {"id":"win-hex-2","title":"Terminal","floating":false,"workspace":"eDP-1:0","column":1,"row":0,"w":960.0,"h":1080.0,"active":false,"focused":false,"output":"eDP-1"}
            ]}"#,
        )
        .unwrap();
        apply_event(&mut state, &win_json);

        let seen = state.seen.clone();
        let fw = seen.focused_window.expect("fallback focused window");
        assert_eq!(fw.title, "Terminal"); // Fell back to win-hex-2 from workspace focused_window
    }
    #[test]
    fn snapshots_link_in_either_order_and_logical_width_updates_presets() {
        let windows = json::parse(r#"{"event":"windows","data":[
            {"id":"58f26771340d94c9fd41b7841be9e3e0","workspace":"eDP-1:3","output":"eDP-1","column":0,"row":0,"w":960,"h":1000,"active":true}
        ]}"#).unwrap();
        let workspaces = json::parse(r#"{"event":"workspaces","data":[
            {"id":"eDP-1:3","index":2,"name":"code","named":true,"output":"eDP-1","active":true,"focused":true,"focused_window":"58f26771340d94c9fd41b7841be9e3e0"}
        ]}"#).unwrap();
        let outputs = json::parse(r#"{"event":"outputs","data":[
            {"name":"eDP-1","focused":true,"logical_size":{"width":1920,"height":1080},"scale":2,"modes":[{"width":3840,"current":true}]}
        ]}"#).unwrap();
        for ordered in [[&windows, &workspaces], [&workspaces, &windows]] {
            let mut state = Umbriel::default();
            for event in ordered {
                apply_event(&mut state, event);
            }
            let focused = state.seen.focused_window.as_ref().unwrap();
            let id = focused.id;
            let workspace_id = state.seen.workspaces[0].id;
            assert_eq!(focused.workspace_id, Some(workspace_id));
            assert_eq!(state.seen.windows[0].workspace_id, Some(workspace_id));
            apply_event(&mut state, &outputs);
            let focused = state.seen.focused_window.as_ref().unwrap();
            assert_eq!(focused.id, id);
            assert_eq!(focused.column_width, Some(0.5));
            let target = state.workspace_bridge.target(workspace_id).unwrap();
            assert_eq!((target.output.as_str(), target.index), ("eDP-1", 2));
            apply_event(
                &mut state,
                &json::parse(r#"{"event":"windows","data":[]}"#).unwrap(),
            );
            assert!(state.seen.focused_window.is_none());
            assert!(state.seen.windows.is_empty());
            apply_event(
                &mut state,
                &json::parse(r#"{"event":"workspaces","data":[]}"#).unwrap(),
            );
            assert!(state.seen.workspaces.is_empty());
            assert!(state.workspace_bridge.target(workspace_id).is_none());
        }
    }

    #[test]
    fn remembered_focus_stays_on_focused_workspace_and_active_seat_wins() {
        let mut state = Umbriel::default();
        let workspaces = json::parse(r#"{"event":"workspaces","data":[
            {"id":"DP-1:1","index":1,"output":"DP-1","active":true,"focused":false,"focused_window":"other"},
            {"id":"eDP-1:1","index":1,"output":"eDP-1","active":true,"focused":true,"focused_window":"mine"}
        ]}"#).unwrap();
        let mut windows = json::parse(
            r#"{"event":"windows","data":[
            {"id":"other","workspace":"DP-1:1","output":"DP-1","focused":true,"floating":true},
            {"id":"mine","workspace":"eDP-1:1","output":"eDP-1","focused":true,"column":-1},
            {"id":"extra","workspace":"eDP-1:1","output":"eDP-1","column":-1}
        ]}"#,
        )
        .unwrap();
        apply_event(&mut state, &workspaces);
        apply_event(&mut state, &windows);
        assert_eq!(state.seen.focused_output.as_deref(), Some("eDP-1"));
        let mine = state.seen.focused_window.as_ref().unwrap();
        assert_eq!(state.window_bridge.hex(mine.id).as_deref(), Some("mine"));
        let columns: Vec<_> = state
            .seen
            .windows
            .iter()
            .filter(|w| !w.is_floating)
            .map(|w| w.col)
            .collect();
        assert_eq!(columns, vec![Some(0), Some(1)]);
        windows["data"][0]["active"] = Json::Bool(true);
        apply_event(&mut state, &windows);
        let other = state.seen.focused_window.as_ref().unwrap();
        assert_eq!(state.window_bridge.hex(other.id).as_deref(), Some("other"));
        assert_eq!(other.column_width, None);
        assert_eq!(other.tile_count, 1);
        windows["data"][0]["active"] = Json::Bool(false);
        let mut empty_focus = workspaces.clone();
        empty_focus["data"][1]["focused_window"] = Json::from("");
        apply_event(&mut state, &empty_focus);
        apply_event(&mut state, &windows);
        assert!(state.seen.focused_window.is_none());
    }

    #[test]
    fn multi_monitor_remembered_focus_preservation_when_seat_inactive() {
        let mut state = Umbriel::default();

        let initial_ws = json::parse(
            r#"{"event":"workspaces","data":[
                {"id":"DP-1:1","index":1,"name":"ws-dp","named":true,"output":"DP-1","active":true,"focused":false},
                {"id":"HDMI-A-1:1","index":1,"name":"ws-hdmi","named":true,"output":"HDMI-A-1","active":true,"focused":true}
            ]}"#,
        )
        .unwrap();
        apply_event(&mut state, &initial_ws);
        assert_eq!(state.seen.focused_output.as_deref(), Some("HDMI-A-1"));
        assert_eq!(state.seen.focused_workspace.as_deref(), Some("ws-hdmi"));

        // Island takes keyboard focus: no workspace is focused, but both monitors remain active.
        // DP-1 comes first in the array. Preserved focused_output should not be clobbered.
        let unfocused_ws = json::parse(
            r#"{"event":"workspaces","data":[
                {"id":"DP-1:1","index":1,"name":"ws-dp","named":true,"output":"DP-1","active":true,"focused":false},
                {"id":"HDMI-A-1:1","index":1,"name":"ws-hdmi","named":true,"output":"HDMI-A-1","active":true,"focused":false}
            ]}"#,
        )
        .unwrap();
        apply_event(&mut state, &unfocused_ws);
        assert_eq!(state.seen.focused_output.as_deref(), Some("HDMI-A-1"));
        assert_eq!(state.seen.focused_workspace.as_deref(), Some("ws-hdmi"));

        // If focused_output was previously None, fallback selects the first active output.
        let mut fresh_state = Umbriel::default();
        apply_event(&mut fresh_state, &unfocused_ws);
        assert_eq!(fresh_state.seen.focused_output.as_deref(), Some("DP-1"));
        assert_eq!(fresh_state.seen.focused_workspace.as_deref(), Some("ws-dp"));
    }

    #[test]
    fn fallback_monitor_lookup_when_focused_window_lacks_explicit_output() {
        let mut state = Umbriel::default();
        state.seen.focused_output = Some("DP-FALLBACK".to_string());

        kanade_runtime::Monitors::publish(vec![kanade_runtime::Monitor {
            name: "DP-FALLBACK".to_string(),
            width: 2560,
            height: 1440,
        }]);

        let win_json = json::parse(
            r#"{"event":"windows","data":[
                {"id":"fallback-win","title":"App","floating":false,"workspace":"ws1","column":0,"row":0,"w":1280.0,"h":1440.0,"active":true,"focused":true}
            ]}"#,
        )
        .unwrap();
        apply_event(&mut state, &win_json);

        let fw = state.seen.focused_window.expect("focused window");
        assert_eq!(fw.column_width, Some(1280.0 / 2560.0));

        kanade_runtime::Monitors::publish(vec![]);
    }
}
