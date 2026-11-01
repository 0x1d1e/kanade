//! Umbriel compositor source and IPC connection.
//!
//! Subscribes to events from Umbriel (overview, windows, workspaces, keyboard_layout)
//! over its unix socket and tracks status in `Umbriel` Service.

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
use crate::backoff::Backoff;
use crate::supervise;

/// What the core tracks from Umbriel compositor.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seen {
    pub overview: bool,
    pub focused_output: Option<String>,
    pub focused_window: Option<String>,
    pub focused_workspace: Option<String>,
    pub layout: Option<String>,
    pub windows_count: usize,
    pub workspaces_count: usize,
}

/// Umbriel Service state.
#[derive(Debug, Default)]
pub struct Umbriel {
    pub seen: Seen,
    pub connected: bool,
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
    true
}

pub(crate) fn handle_event(event: &Json) {
    let Some(event_type) = event.get("event").and_then(Json::as_str) else {
        return;
    };

    let data = event.get("data").unwrap_or(event);

    let mut state = Umbriel::write();
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
                state.seen.windows_count = windows.len();
                for w in windows {
                    if w.get("focused").and_then(Json::as_bool) == Some(true) {
                        state.seen.focused_window =
                            w.get("title").and_then(Json::as_str).map(String::from);
                        if let Some(output) = w.get("output").and_then(Json::as_str) {
                            state.seen.focused_output = Some(output.to_string());
                        }
                    }
                }
            }
        }
        "workspaces" => {
            let workspaces = data
                .as_array()
                .or_else(|| event.get("workspaces").and_then(Json::as_array));
            if let Some(workspaces) = workspaces {
                state.seen.workspaces_count = workspaces.len();
                for ws in workspaces {
                    if ws.get("is_active").and_then(Json::as_bool) == Some(true)
                        || ws.get("is_focused").and_then(Json::as_bool) == Some(true)
                        || ws.get("focused").and_then(Json::as_bool) == Some(true)
                    {
                        state.seen.focused_workspace =
                            ws.get("name").and_then(Json::as_str).map(String::from);
                    }
                }
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
                let mut focused = None;
                let mut first = None;
                for out in outputs {
                    if let Some(name) = out.get("name").and_then(Json::as_str) {
                        if first.is_none() {
                            first = Some(name.to_string());
                        }
                        if out.get("focused").and_then(Json::as_bool) == Some(true) {
                            focused = Some(name.to_string());
                            break;
                        }
                    }
                }
                if let Some(out) = focused {
                    state.seen.focused_output = Some(out);
                } else if state.seen.focused_output.is_none() {
                    state.seen.focused_output = first;
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
    fn socket_path_resolution() {
        // Without env vars or socket files, returns None
        let original_socket = env::var_os("UMBRIEL_SOCKET");
        unsafe {
            env::remove_var("UMBRIEL_SOCKET");
        }
        let _ = socket_path();

        if let Some(orig) = original_socket {
            unsafe {
                env::set_var("UMBRIEL_SOCKET", orig);
            }
        }
    }

    #[test]
    fn event_handling_updates_service_state() {
        let overview_json = json::parse(r#"{"event":"overview","open":true}"#).unwrap();
        handle_event(&overview_json);
        assert!(Umbriel::read().seen.overview);

        let layout_json = json::parse(r#"{"event":"keyboard_layout","layout":"de"}"#).unwrap();
        handle_event(&layout_json);
        assert_eq!(Umbriel::read().seen.layout.as_deref(), Some("de"));

        let windows_json = json::parse(
            r#"{"event":"windows","windows":[{"title":"Terminal","focused":true},{"title":"Browser","focused":false}]}"#,
        )
        .unwrap();
        handle_event(&windows_json);
        assert_eq!(Umbriel::read().seen.windows_count, 2);
        assert_eq!(
            Umbriel::read().seen.focused_window.as_deref(),
            Some("Terminal")
        );

        let outputs_json = json::parse(
            r#"{"event":"outputs","data":[{"name":"eDP-1","focused":true},{"name":"HDMI-A-1","focused":false}]}"#,
        )
        .unwrap();
        handle_event(&outputs_json);
        assert_eq!(
            Umbriel::read().seen.focused_output.as_deref(),
            Some("eDP-1")
        );

        // When no output has focus, existing focused_output is preserved
        let outputs_unfocused =
            json::parse(r#"{"event":"outputs","data":[{"name":"DP-2","focused":false}]}"#).unwrap();
        handle_event(&outputs_unfocused);
        assert_eq!(
            Umbriel::read().seen.focused_output.as_deref(),
            Some("eDP-1")
        );

        // When no output has focus and focused_output is None, fallback to first output
        Umbriel::write().seen.focused_output = None;
        handle_event(&outputs_unfocused);
        assert_eq!(Umbriel::read().seen.focused_output.as_deref(), Some("DP-2"));
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
}
