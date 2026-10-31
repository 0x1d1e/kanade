//! Kanade's own niri EventStream, tracking the overview and casts too. It writes what
//! niri says as the `Niri` Service: a plain focused output, whether the overview is open and how
//! many times it opened, the focused workspace, whether anything casts and the window each output
//! shows. It knows none of what reads it: a Module `watch`es `Niri` in its own start. Each window
//! niri opens, closes or focuses and each keyboard layout switch are events, not state, so a
//! Module takes those by `on_windows` and `on_layout`, a screenshot niri saved by `on_screenshot`,
//! a touchpad toggle by `on_touchpad`, and a failed load of niri's config by
//! `on_config_failed`. `ask` sends
//! niri one request on a connection of its own.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::io::{self, BufRead, BufReader, Write as _};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::{MutexExt, service::Service};

use super::json::{self, Json};
use crate::backoff::Backoff;
use crate::config;
use crate::island::activity::Workspace;
use crate::supervise;

// what the core gets from niri; the default is also what a lost socket degrades to
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seen {
    // none while unknown, which counts every monitor as focused
    pub focused_output: Option<String>,

    pub overview: bool,

    // how many times the overview has opened, so one opened and closed between two looks still shows
    pub overviews: u32,

    // none while unknown, like focus
    pub workspace: Option<Focused>,

    // what the casts niri has capture, paused ones included, each once; empty while none cast
    pub casts: BTreeSet<Target>,

    // the sessions of those PipeWire carries, which niri can stop; a wlr-screencopy cast has none
    pub sessions: BTreeSet<u64>,

    // some cast has a dynamic target, which `retarget` changes
    pub dynamic: bool,

    // casts run and every one has stopped streaming frames, as OBS does on another scene
    pub paused: bool,

    // the window each output shows, by output; empty while unknown
    pub shown: BTreeMap<String, Showing>,

    // the workspaces niri has, ordered by output and index
    pub workspaces: Vec<WorkspaceInfo>,

    // the focused window, or the last focused one across a layer surface focus
    pub focused_window: Option<FocusedWindow>,

    // the windows tracked across workspaces for strip minimap
    pub windows: Vec<StripWindow>,

    // urgent workspaces whose output is known; one with none is left out
    pub urgent: Vec<UrgentWorkspace>,
}

// a workspace niri marked urgent, on a known output
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrgentWorkspace {
    pub id: u64,
    pub output: String,
    pub index: u32,
    pub count: u32,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StripWindow {
    pub id: u64,
    pub title: String,
    pub app_id: Option<String>,
    pub workspace_id: Option<u64>,
    pub is_floating: bool,
    pub is_focused: bool,
    pub is_urgent: bool,
    pub col: Option<u64>,
    pub row: Option<u64>,
    pub tile_width: Option<f64>,
    pub tile_height: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FocusedWindow {
    pub id: u64,
    pub title: String,
    pub app_id: Option<String>,
    pub is_floating: bool,
    pub tile_count: usize, // number of windows in this column
    pub column_width: Option<f64>,
    pub workspace_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceInfo {
    pub id: u64,
    pub index: u32,
    pub name: Option<String>,
    pub output: Option<String>,
    pub is_active: bool,
    pub is_focused: bool,
}

// what a cast captures; a window is no more than that here, whose title and app stay with niri
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    Screen,
    Window,

    // not set yet or cleared, or one niri has since added
    Unknown,
}

// what niri last said, written only when it changes; a lost stream writes the default again
#[derive(Debug, Default)]
pub struct Niri {
    pub seen: Seen,
    pub connected: bool,
}

impl Niri {
    pub fn is_connected(&self) -> bool {
        self.connected
    }
}

/*
 * the one `Niri` is the process's, so a test that writes it is every other test's input: one that
 * reads it between a write and its undo sees a niri that is gone, or one casting nothing. Every
 * test that reads or writes it takes this turn, so the writes land whole in tests that mean them
 */
#[cfg(test)]
pub static NIRI_TURN: Mutex<()> = Mutex::new(());

impl Service for Niri {
    fn new() -> Self {
        Self::default()
    }

    // written only by `follow`
    fn listen() {}
}

// the window an output shows, as niri says: its active workspace's active window
#[derive(Debug, Clone, PartialEq)]
pub struct Showing {
    pub app_id: Option<String>,

    // logical, as an output's size is
    pub tile: (f64, f64),
}

// niri's window id, which stays with the window for as long as it is open
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub id: WindowId,

    // the Wayland app id; none until the app sets one, and some never do
    pub app_id: Option<String>,

    pub focused: bool,

    // asks for attention
    pub urgent: bool,
}

// what niri said of its windows
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Heard {
    // every window, replacing those known
    All(Vec<Window>),

    // one window, new or changed; one that is focused takes the focus from every other
    Opened(Window),

    Closed(WindowId),

    // none: no window has the focus
    Focused(Option<WindowId>),

    Urgent(WindowId, bool),

    // the stream ended, so no window is known
    Lost,
}

// the focused workspace, as niri last said
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Focused {
    // niri's id, which stays with the workspace when it moves or another one comes or goes
    pub id: u64,

    pub output: Option<String>,

    pub workspace: Workspace,
}

// a workspace as niri lists it
#[derive(Debug)]
struct Listed {
    output: Option<String>,

    // 1 based, on its output
    index: u32,

    name: Option<String>,

    // the one its output shows
    active: bool,

    // the window it shows
    window: Option<u64>,

    urgent: bool,
}

// niri's events only say what changed, so the rest is remembered here
#[derive(Debug, Default)]
struct Known {
    // every workspace by id
    workspaces: HashMap<u64, Listed>,

    focused: Option<u64>,

    overview: bool,

    overviews: u32,

    // every cast by stream id
    casts: HashMap<u64, Cast>,

    // every window by id: which app's, and its tile
    windows: HashMap<u64, Showing>,

    last_focused_window: Option<u64>,

    tracked_windows: HashMap<u64, TrackedWindow>,
}

#[derive(Debug, Clone)]
struct TrackedWindow {
    id: u64,
    title: String,
    app_id: Option<String>,
    is_floating: bool,
    is_focused: bool,
    is_urgent: bool,
    workspace_id: Option<u64>,
    col: Option<u64>,
    row: Option<u64>,
    tile_width: Option<f64>,
    tile_height: Option<f64>,
}

impl Known {
    // anything niri sends that is not about workspaces, windows, focus, the overview or casts changes
    // nothing
    fn apply(&mut self, event: &Json) {
        if let Some(changed) = event.get("WorkspacesChanged") {
            let workspaces = changed.get("workspaces").and_then(json::array);

            self.workspaces.clear();
            self.focused = None;

            for workspace in workspaces.unwrap_or_default() {
                let Some(id) = workspace.get("id").and_then(Json::as_u64) else {
                    continue;
                };

                let text = |key| workspace.get(key).and_then(Json::as_str).map(String::from);
                let index = workspace.get("idx").and_then(Json::as_u64);

                self.workspaces.insert(
                    id,
                    Listed {
                        output: text("output"),
                        index: index.and_then(|index| index.try_into().ok()).unwrap_or(0),
                        name: text("name"),
                        active: workspace.get("is_active").and_then(Json::as_bool) == Some(true),
                        window: workspace.get("active_window_id").and_then(Json::as_u64),
                        urgent: workspace.get("is_urgent").and_then(Json::as_bool) == Some(true),
                    },
                );

                if workspace.get("is_focused").and_then(Json::as_bool) == Some(true) {
                    self.focused = Some(id);
                }
            }
        } else if let Some(activated) = event.get("WorkspaceActivated") {
            let id = activated.get("id").and_then(Json::as_u64);

            // not focused: it only became the one shown on its own output
            if activated.get("focused").and_then(Json::as_bool) == Some(true) {
                self.focused = id;
            }

            if let Some(output) = id
                .and_then(|id| self.workspaces.get(&id))
                .map(|listed| listed.output.clone())
            {
                for (other, listed) in &mut self.workspaces {
                    if listed.output == output {
                        listed.active = Some(*other) == id;
                    }
                }
            }
        } else if let Some(changed) = event.get("WorkspaceActiveWindowChanged") {
            if let Some(listed) = changed
                .get("workspace_id")
                .and_then(Json::as_u64)
                .and_then(|id| self.workspaces.get_mut(&id))
            {
                listed.window = changed.get("active_window_id").and_then(Json::as_u64);
            }
        } else if let Some(changed) = event.get("WindowsChanged") {
            let windows = changed.get("windows").and_then(json::array);

            self.windows = windows
                .unwrap_or_default()
                .iter()
                .filter_map(shown)
                .collect();

            self.tracked_windows.clear();
            let mut focused_id = None;

            for window in windows.unwrap_or_default() {
                if let Some(tracked) = tracked_window(window) {
                    if tracked.is_focused {
                        focused_id = Some(tracked.id);
                    }
                    self.tracked_windows.insert(tracked.id, tracked);
                }
            }

            if let Some(id) = focused_id {
                self.last_focused_window = Some(id);
            } else if let Some(last) = self.last_focused_window
                && !self.tracked_windows.contains_key(&last)
            {
                self.last_focused_window = None;
            }
        } else if let Some(opened) = event.get("WindowOpenedOrChanged") {
            self.windows.extend(opened.get("window").and_then(shown));
            if let Some(window) = opened.get("window").and_then(tracked_window) {
                if window.is_focused {
                    self.last_focused_window = Some(window.id);
                    for (id, other) in &mut self.tracked_windows {
                        if *id != window.id {
                            other.is_focused = false;
                        }
                    }
                }
                self.tracked_windows.insert(window.id, window);
            }
        } else if let Some(closed) = event.get("WindowClosed") {
            if let Some(id) = closed.get("id").and_then(Json::as_u64) {
                self.windows.remove(&id);
                self.tracked_windows.remove(&id);
                if self.last_focused_window == Some(id) {
                    self.last_focused_window = None;
                }
            }
        } else if let Some(focus) = event.get("WindowFocusChanged") {
            let id = focus.get("id").and_then(Json::as_u64);
            if let Some(id) = id {
                self.last_focused_window = Some(id);
                for (w_id, w) in &mut self.tracked_windows {
                    w.is_focused = *w_id == id;
                }
            } else {
                for w in self.tracked_windows.values_mut() {
                    w.is_focused = false;
                }
            }
        } else if let Some(changed) = event.get("WindowLayoutsChanged") {
            for change in changed
                .get("changes")
                .and_then(json::array)
                .unwrap_or_default()
            {
                if let Some([id, layout]) = json::array(change)
                    && let Some(id) = id.as_u64()
                {
                    if let Some(window) = self.windows.get_mut(&id)
                        && let Some(tile) = self::tile(layout)
                    {
                        window.tile = tile;
                    }
                    if let Some(tracked) = self.tracked_windows.get_mut(&id) {
                        if let Some(tile) = self::tile(layout) {
                            tracked.tile_width = Some(tile.0);
                            tracked.tile_height = Some(tile.1);
                        }
                        if let Some(col) = self::col(layout) {
                            tracked.col = Some(col);
                        }
                        if let Some(row) = self::row(layout) {
                            tracked.row = Some(row);
                        }
                    }
                }
            }
        } else if let Some(urgency) = event.get("WorkspaceUrgencyChanged") {
            if let Some(id) = urgency.get("id").and_then(Json::as_u64)
                && let Some(urgent) = urgency.get("urgent").and_then(Json::as_bool)
                && let Some(listed) = self.workspaces.get_mut(&id)
            {
                listed.urgent = urgent;
            }
        } else if let Some(urgency) = event.get("WindowUrgencyChanged") {
            if let Some(id) = urgency.get("id").and_then(Json::as_u64)
                && let Some(urgent) = urgency.get("urgent").and_then(Json::as_bool)
                && let Some(tracked) = self.tracked_windows.get_mut(&id)
            {
                tracked.is_urgent = urgent;
            }
        } else if let Some(overview) = event.get("OverviewOpenedOrClosed")
            && let Some(open) = overview.get("is_open").and_then(Json::as_bool)
        {
            if open && !self.overview {
                self.overviews = self.overviews.wrapping_add(1);
            }

            self.overview = open;
        } else if let Some(changed) = event.get("CastsChanged") {
            let casts = changed.get("casts").and_then(json::array);

            self.casts = casts.unwrap_or_default().iter().filter_map(cast).collect();
        } else if let Some(started) = event.get("CastStartedOrChanged") {
            self.casts.extend(started.get("cast").and_then(cast));
        } else if let Some(stopped) = event.get("CastStopped")
            && let Some(id) = stopped.get("stream_id").and_then(Json::as_u64)
        {
            self.casts.remove(&id);
        }
    }

    fn seen(&self) -> Seen {
        let focused = self
            .focused
            .and_then(|id| Some((id, self.workspaces.get(&id)?)));

        let workspace = focused.map(|(id, listed)| Focused {
            id,
            output: listed.output.clone(),
            workspace: Workspace {
                index: listed.index,
                count: self.on(&listed.output),
                name: listed.name.clone(),
            },
        });

        let shown = self
            .workspaces
            .values()
            .filter(|listed| listed.active)
            .filter_map(|listed| {
                let window = self.windows.get(&listed.window?)?;

                Some((listed.output.clone()?, window.clone()))
            })
            .collect();

        let focused_id = self.last_focused_window.or_else(|| {
            self.tracked_windows
                .values()
                .find(|w| w.is_focused)
                .map(|w| w.id)
        });

        let focused_window = focused_id
            .and_then(|id| self.tracked_windows.get(&id))
            .map(|w| {
                let tile_count = match (w.workspace_id, w.col) {
                    (Some(ws_id), Some(col)) if !w.is_floating => self
                        .tracked_windows
                        .values()
                        .filter(|other| {
                            !other.is_floating
                                && other.workspace_id == Some(ws_id)
                                && other.col == Some(col)
                        })
                        .count()
                        .max(1),
                    _ => 1,
                };

                let column_width = if w.is_floating { None } else { w.tile_width };

                FocusedWindow {
                    id: w.id,
                    title: w.title.clone(),
                    app_id: w.app_id.clone(),
                    is_floating: w.is_floating,
                    tile_count,
                    column_width,
                    workspace_id: w.workspace_id,
                }
            });

        let mut workspaces: Vec<WorkspaceInfo> = self
            .workspaces
            .iter()
            .map(|(&id, listed)| WorkspaceInfo {
                id,
                index: listed.index,
                name: listed.name.clone(),
                output: listed.output.clone(),
                is_active: listed.active,
                is_focused: self.focused == Some(id),
            })
            .collect();
        workspaces.sort_by_key(|w| (w.output.clone(), w.index, w.id));

        let mut windows: Vec<StripWindow> = self
            .tracked_windows
            .values()
            .map(|w| StripWindow {
                id: w.id,
                title: w.title.clone(),
                app_id: w.app_id.clone(),
                workspace_id: w.workspace_id,
                is_floating: w.is_floating,
                is_focused: w.is_focused,
                is_urgent: w.is_urgent,
                col: w.col,
                row: w.row,
                tile_width: w.tile_width,
                tile_height: w.tile_height,
            })
            .collect();
        windows.sort_by_key(|w| (w.workspace_id, w.col, w.row, w.id));

        Seen {
            focused_output: focused.and_then(|(_, listed)| listed.output.clone()),
            overview: self.overview,
            overviews: self.overviews,
            workspace,
            casts: self.casts.values().map(|cast| cast.target).collect(),
            sessions: self
                .casts
                .values()
                .filter_map(|cast| cast.session)
                .collect(),
            dynamic: self.casts.values().any(|cast| cast.dynamic),
            paused: !self.casts.is_empty() && self.casts.values().all(|cast| !cast.active),
            shown,
            workspaces,
            focused_window,
            windows,
            urgent: self.urgent(),
        }
    }

    // urgent workspaces with a known output, by id, so a list that only reorders posts nothing
    fn urgent(&self) -> Vec<UrgentWorkspace> {
        let mut rows: Vec<_> = self
            .workspaces
            .iter()
            .filter(|(_, listed)| listed.urgent)
            .filter_map(|(&id, listed)| {
                Some((
                    id,
                    listed.output.clone()?,
                    listed.index,
                    listed.name.clone(),
                ))
            })
            .collect();

        rows.sort_by_key(|row| row.0);
        rows.into_iter()
            .map(|(id, output, index, name)| UrgentWorkspace {
                count: self.on(&Some(output.clone())),
                id,
                output,
                index,
                name,
            })
            .collect()
    }

    // how many workspaces `output` has
    fn on(&self, output: &Option<String>) -> u32 {
        let count = self
            .workspaces
            .values()
            .filter(|listed| listed.output == *output)
            .count();

        count.try_into().unwrap_or(u32::MAX)
    }
}

// a window as `fullscreen` matches it, by id; none without an id or a tile
fn shown(window: &Json) -> Option<(u64, Showing)> {
    let id = window.get("id").and_then(Json::as_u64)?;
    let app_id = window
        .get("app_id")
        .and_then(Json::as_str)
        .map(String::from);
    let tile = tile(window.get("layout")?)?;

    Some((id, Showing { app_id, tile }))
}

fn tracked_window(window: &Json) -> Option<TrackedWindow> {
    let id = window.get("id").and_then(Json::as_u64)?;
    let title = window
        .get("title")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string();
    let app_id = window
        .get("app_id")
        .and_then(Json::as_str)
        .map(String::from);
    let is_floating = window
        .get("is_floating")
        .and_then(Json::as_bool)
        .unwrap_or(false);
    let is_focused = window
        .get("is_focused")
        .and_then(Json::as_bool)
        .unwrap_or(false);
    let is_urgent = window
        .get("is_urgent")
        .and_then(Json::as_bool)
        .unwrap_or(false);
    let workspace_id = window.get("workspace_id").and_then(Json::as_u64);

    let layout = window.get("layout");
    let col = layout.and_then(self::col);
    let row = layout.and_then(self::row);
    let tile_size = layout.and_then(self::tile);
    let tile_width = tile_size.map(|t| t.0);
    let tile_height = tile_size.map(|t| t.1);

    Some(TrackedWindow {
        id,
        title,
        app_id,
        is_floating,
        is_focused,
        is_urgent,
        workspace_id,
        col,
        row,
        tile_width,
        tile_height,
    })
}

fn col(layout: &Json) -> Option<u64> {
    let pos = layout.get("pos_in_scrolling_layout")?;
    match json::array(pos)? {
        [col, ..] => col.as_u64(),
        _ => None,
    }
}

fn row(layout: &Json) -> Option<u64> {
    let pos = layout.get("pos_in_scrolling_layout")?;
    match json::array(pos)? {
        [_, row, ..] => row.as_u64(),
        _ => None,
    }
}

fn tile(layout: &Json) -> Option<(f64, f64)> {
    match json::array(layout.get("tile_size")?)? {
        [width, height] => Some((width.as_f64()?, height.as_f64()?)),
        _ => None,
    }
}

// a cast as far as it is followed
#[derive(Debug, Clone, Copy)]
struct Cast {
    target: Target,

    // its session, only for a PipeWire cast, the one kind `stop` can end
    session: Option<u64>,

    // whether `retarget` acts on it
    dynamic: bool,

    // streaming frames; a cast niri does not say is taken to be
    active: bool,
}

// a cast's stream and what it captures; none for Kanade's own capture
fn cast(cast: &Json) -> Option<(u64, Cast)> {
    cast_with(cast, |session| {
        crate::sources::recording::cast_is_ours(session)
    })
}

/*
 * `cast` as a cast, unless it is Kanade's own: the recorder's own pid says so, and a window's cast
 * has no pid behind it (a D-Bus session), so the ledger of the casts this Kanade made says it. `ours`
 * is asked only for such a cast, off the draw thread
 */
fn cast_with(cast: &Json, ours: impl Fn(u64) -> bool) -> Option<(u64, Cast)> {
    let own = cast.get("pid").and_then(Json::as_u64) == Some(u64::from(std::process::id()));

    if own {
        return None;
    }

    // `{"Window": {"id": 1}}`, `{"Output": {"name": "eDP-1"}}` or `{"Nothing": {}}`
    let target = match cast.get("target") {
        Some(target) if target.get("Window").is_some() => Target::Window,
        Some(target) if target.get("Output").is_some() => Target::Screen,
        _ => Target::Unknown,
    };

    let session = match cast.get("kind").and_then(Json::as_str) {
        Some("WlrScreencopy") => None,
        _ => cast.get("session_id").and_then(Json::as_u64),
    };

    // a window cast Kanade made, whose pid says nothing: its own, not another program's
    if cast.get("pid").is_none_or(Json::is_null)
        && target == Target::Window
        && let Some(session) = cast.get("session_id").and_then(Json::as_u64)
        && ours(session)
    {
        return None;
    }

    let flag = |key, default| cast.get(key).and_then(Json::as_bool).unwrap_or(default);

    Some((
        cast.get("stream_id")
            .or_else(|| cast.get("session_id"))
            .and_then(Json::as_u64)?,
        Cast {
            target,
            session,
            dynamic: flag("is_dynamic_target", false),
            active: flag("is_active", true),
        },
    ))
}

/*
 * asks niri to stop the PipeWire cast of `session`, every stream of it; one niri cannot stop, or an
 * older niri without the action, answers an error, and a missing or unclear answer is `Unknown`,
 * the cast maybe stopped, so never asked again for it. Blocks up to `PATIENCE`, so never on the
 * draw thread
 */
pub fn stop(session: u64) -> io::Result<Acted> {
    act(&format!(
        r#"{{"Action":{{"StopCast":{{"session_id":{session}}}}}}}"#
    ))
}

pub fn consume_window_into_column() -> io::Result<Acted> {
    act(r#"{"Action":"ConsumeWindowIntoColumn"}"#)
}

pub fn expel_window_from_column() -> io::Result<Acted> {
    act(r#"{"Action":"ExpelWindowFromColumn"}"#)
}

pub fn set_column_width(proportion: f64) -> io::Result<Acted> {
    act(&format!(
        r#"{{"Action":{{"SetColumnWidth":{{"change":{{"SetProportion":{proportion}}}}}}}}}"#
    ))
}

pub fn switch_preset_column_width() -> io::Result<Acted> {
    act(r#"{"Action":"SwitchPresetColumnWidth"}"#)
}

pub fn toggle_window_floating() -> io::Result<Acted> {
    act(r#"{"Action":"ToggleWindowFloating"}"#)
}

pub fn maximize_column() -> io::Result<Acted> {
    act(r#"{"Action":"MaximizeColumn"}"#)
}

pub fn move_window_to_workspace(window_id: u64, workspace_id: u64) -> io::Result<Acted> {
    act(&format!(
        r#"{{"Action":{{"MoveWindowToWorkspace":{{"window_id":{window_id},"reference":{{"Id":{workspace_id}}},"focus":false}}}}}}"#
    ))
}

pub fn focus_window(id: u64) -> io::Result<Acted> {
    act(&format!(r#"{{"Action":{{"FocusWindow":{{"id":{id}}}}}}}"#))
}

/*
 * what a dynamic cast target is set to: a window by id, none for the focused one; an output by
 * name, none for the focused one; or nothing. An older niri without the actions answers an error,
 * and the same as for `stop` otherwise
 */
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Retarget {
    Window(Option<u64>),
    Output(Option<String>),
    Clear,
}

pub fn retarget(to: &Retarget) -> io::Result<Acted> {
    let action = match to {
        Retarget::Window(id) => {
            let id = id.map_or_else(|| String::from("null"), |id| id.to_string());

            format!(r#""SetDynamicCastWindow":{{"id":{id}}}"#)
        }
        Retarget::Output(name) => {
            let name = name
                .as_deref()
                .map_or_else(|| String::from("null"), json::quote);

            format!(r#""SetDynamicCastMonitor":{{"output":{name}}}"#)
        }
        Retarget::Clear => String::from(r#""ClearDynamicCastTarget":{}"#),
    };

    act(&format!(r#"{{"Action":{{{action}}}}}"#))
}

// what a window event says; none for another event, and for those `windows` ignores, like layouts
fn windowed(event: &Json) -> Option<Heard> {
    if let Some(changed) = event.get("WindowsChanged") {
        let windows = changed.get("windows").and_then(json::array);

        Some(Heard::All(
            windows
                .unwrap_or_default()
                .iter()
                .filter_map(window)
                .collect(),
        ))
    } else if let Some(opened) = event.get("WindowOpenedOrChanged") {
        opened.get("window").and_then(window).map(Heard::Opened)
    } else if let Some(closed) = event.get("WindowClosed") {
        closed
            .get("id")
            .and_then(Json::as_u64)
            .map(|id| Heard::Closed(WindowId(id)))
    } else if let Some(focus) = event.get("WindowFocusChanged") {
        // a null id: no window has the focus
        Some(Heard::Focused(
            focus.get("id").and_then(Json::as_u64).map(WindowId),
        ))
    } else if let Some(urgency) = event.get("WindowUrgencyChanged") {
        let id = urgency.get("id").and_then(Json::as_u64)?;
        let urgent = urgency.get("urgent").and_then(Json::as_bool)?;

        Some(Heard::Urgent(WindowId(id), urgent))
    } else {
        None
    }
}

// one of niri's windows as `windows` keeps it; none without an id
fn window(window: &Json) -> Option<Window> {
    let flag = |key| window.get(key).and_then(Json::as_bool).unwrap_or(false);

    Some(Window {
        id: WindowId(window.get("id").and_then(Json::as_u64)?),
        app_id: window
            .get("app_id")
            .and_then(Json::as_str)
            .map(String::from),
        focused: flag("is_focused"),
        urgent: flag("is_urgent"),
    })
}

// the keyboard layouts niri has, as it last listed them, and which is in use
#[derive(Debug, Default)]
struct Layouts {
    names: Vec<String>,
    current: u64,
}

impl Layouts {
    /*
     * the name of the layout a switch went to; none for another event, for the list niri gives
     * (first on connecting, and on a change of config), for a switch to the one in use, and with
     * fewer than two layouts, where there is nothing to switch between
     */
    fn switch(&mut self, event: &Json) -> Option<String> {
        if let Some(changed) = event.get("KeyboardLayoutsChanged") {
            let layouts = changed.get("keyboard_layouts");
            let names = layouts
                .and_then(|layouts| layouts.get("names"))
                .and_then(json::array)
                .unwrap_or_default();

            self.names = names
                .iter()
                .filter_map(|name| name.as_str().map(String::from))
                .collect();
            self.current = layouts
                .and_then(|layouts| layouts.get("current_idx"))
                .and_then(Json::as_u64)
                .unwrap_or(0);

            return None;
        }

        let index = event
            .get("KeyboardLayoutSwitched")?
            .get("idx")
            .and_then(Json::as_u64)?;

        if index == self.current {
            return None;
        }

        self.current = index;

        if self.names.len() < 2 {
            return None;
        }

        self.names.get(usize::try_from(index).ok()?).cloned()
    }

    // the name of the layout in use; none with fewer than two, where there is nothing to switch
    // between, and for an index the list does not have
    fn in_use(&self) -> Option<String> {
        if self.names.len() < 2 {
            return None;
        }

        self.names.get(usize::try_from(self.current).ok()?).cloned()
    }
}

// what a Module asked to hear, in the order asked; kept as `fn`s so a Module takes it in its start
static WINDOWS: Mutex<Vec<fn(Heard)>> = Mutex::new(Vec::new());
static LAYOUTS: Mutex<Vec<fn(String)>> = Mutex::new(Vec::new());
type Shot = fn(Option<String>);
static SCREENSHOTS: Mutex<Vec<Shot>> = Mutex::new(Vec::new());
static TOUCHPADS: Mutex<Vec<fn(bool)>> = Mutex::new(Vec::new());
static CONFIGS: Mutex<Vec<fn()>> = Mutex::new(Vec::new());
static KEYBOARDS: Mutex<Vec<InUse>> = Mutex::new(Vec::new());

// what hears the layout in use, none when there is no choice of one
type InUse = fn(Option<String>);

// set once `follow` runs, after which a Module can no longer ask to hear
static FOLLOWING: AtomicBool = AtomicBool::new(false);

// every ConfigLoaded niri sends, successful or not; the high bit is whether the last one failed.
// Read by the settings worker to know when niri has reloaded the config after a Keep.
static CONFIG_LOADS: AtomicU64 = AtomicU64::new(0);

// `then` hears every window event, on the niri thread, so it must not block. Taken before `follow`
// runs, or the windows niri lists first are missed
/// A window as screen time attributes it. The title is absent unless that switch is on, and
/// is never logged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppWindow {
    pub id: u64,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub focused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppHeard {
    All(Vec<AppWindow>),
    Opened(AppWindow),
    Closed(u64),
    Focused(Option<u64>),
    Lost,
}

static APPS_ON: AtomicBool = AtomicBool::new(false);
static APPS: Mutex<Option<fn(AppHeard)>> = Mutex::new(None);

// once, before the stream, and only while the screen-time Module is on
pub fn on_apps(then: fn(AppHeard)) {
    assert!(
        !APPS_ON.swap(true, Ordering::Relaxed),
        "the screen-time listener is registered once, before niri's stream"
    );
    *APPS.locked() = Some(then);
}

fn tell_apps(heard: AppHeard) {
    if !APPS_ON.load(Ordering::Relaxed) {
        return;
    }

    if let Some(then) = *APPS.locked() {
        then(heard);
    }
}

fn apps(event: &Json) {
    if !APPS_ON.load(Ordering::Relaxed) {
        return;
    }

    if let Some(heard) = app_heard(event, config::get().screen_time_titles) {
        tell_apps(heard);
    }
}

fn app_heard(event: &Json, titles: bool) -> Option<AppHeard> {
    if let Some(changed) = event.get("WindowsChanged") {
        let windows = changed.get("windows").and_then(json::array);

        Some(AppHeard::All(
            windows
                .unwrap_or_default()
                .iter()
                .filter_map(|window| app_window(window, titles))
                .collect(),
        ))
    } else if let Some(opened) = event.get("WindowOpenedOrChanged") {
        opened
            .get("window")
            .and_then(|window| app_window(window, titles))
            .map(AppHeard::Opened)
    } else if let Some(closed) = event.get("WindowClosed") {
        closed
            .get("id")
            .and_then(Json::as_u64)
            .map(AppHeard::Closed)
    } else {
        event
            .get("WindowFocusChanged")
            .map(|focus| AppHeard::Focused(focus.get("id").and_then(Json::as_u64)))
    }
}

fn app_window(window: &Json, titles: bool) -> Option<AppWindow> {
    let text = |key, most| {
        window.get(key).and_then(Json::as_str).map(|text| {
            text.chars()
                .filter(|character| !character.is_control())
                .take(most)
                .collect::<String>()
        })
    };

    Some(AppWindow {
        id: window.get("id").and_then(Json::as_u64)?,
        app_id: text("app_id", 128),
        title: titles
            .then(|| text("title", 80))
            .flatten()
            .filter(|title| !title.is_empty()),
        focused: window
            .get("is_focused")
            .and_then(Json::as_bool)
            .unwrap_or(false),
    })
}

pub fn on_windows(then: fn(Heard)) {
    assert!(
        !FOLLOWING.load(Ordering::Relaxed),
        "niri is already followed"
    );

    WINDOWS.locked().push(then);
}

/*
 * `then` hears the name XKB gives each keyboard layout niri switches to, like "English (US)", only
 * on a switch and with two or more layouts, on the niri thread, so it must not block
 */
pub fn on_layout(then: fn(String)) {
    assert!(
        !FOLLOWING.load(Ordering::Relaxed),
        "niri is already followed"
    );

    LAYOUTS.locked().push(then);
}

/*
 * `then` hears a screenshot niri saved, the path or none when it only copied, on the niri thread,
 * so it must not block. The path is not logged
 */
pub fn on_screenshot(then: fn(Option<String>)) {
    assert!(
        !FOLLOWING.load(Ordering::Relaxed),
        "niri is already followed"
    );

    SCREENSHOTS.locked().push(then);
}

// `then` hears niri's touchpad toggle, on or off, on the niri thread, so it must not block
pub fn on_touchpad(then: fn(bool)) {
    assert!(
        !FOLLOWING.load(Ordering::Relaxed),
        "niri is already followed"
    );

    TOUCHPADS.locked().push(then);
}

/*
 * `then` hears the layout in use, by the name XKB gives it, when it changes: as niri lists the
 * layouts on connecting and on a change of config, and on each switch. None with fewer than two
 * layouts. On the niri thread, so it must not block. Unlike `on_layout` it says what is in use
 * before any switch, for what shows the layout rather than the change
 */
pub fn on_keyboard(then: InUse) {
    assert!(
        !FOLLOWING.load(Ordering::Relaxed),
        "niri is already followed"
    );

    KEYBOARDS.locked().push(then);
}

/*
 * `then` hears each time niri says a load of its config failed, on the niri thread, so it must
 * not block. That includes the failure of the last load when the stream connects, which a shell
 * started after it would otherwise never hear of; a load that succeeded says nothing
 */
pub fn on_config_failed(then: fn()) {
    assert!(
        !FOLLOWING.load(Ordering::Relaxed),
        "niri is already followed"
    );

    CONFIGS.locked().push(then);
}

fn record_config_load(failed: bool) {
    let failed_bit = if failed { 1u64 << 63 } else { 0 };
    // increment the counter in the low 63 bits, set the high bit from this load
    let prev = CONFIG_LOADS.load(Ordering::Relaxed);
    let count = (prev & !(1u64 << 63)).wrapping_add(1);
    CONFIG_LOADS.store(count | failed_bit, Ordering::Relaxed);
}

/// A snapshot of niri's config-load counter: how many `ConfigLoaded` events have been seen and
/// whether the last one failed. The settings worker reads this before and after a Keep to know
/// when niri has reloaded the written config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigLoadState {
    pub count: u64,
    pub last_failed: bool,
}

pub fn config_load_state() -> ConfigLoadState {
    let raw = CONFIG_LOADS.load(Ordering::Relaxed);
    ConfigLoadState {
        count: raw & !(1u64 << 63),
        last_failed: raw & (1u64 << 63) != 0,
    }
}

// how long to wait before reconnecting; a long stream starts the backoff over
const FIRST_RETRY: Duration = Duration::from_secs(1);
const LAST_RETRY: Duration = Duration::from_secs(60);
const HEALTHY: Duration = Duration::from_secs(60);

// runs on its own thread for good; without niri, or once its socket is lost, every monitor is focused
pub fn follow() {
    FOLLOWING.store(true, Ordering::Relaxed);

    // It outlives a stream: a reconnect starts with fresh Known state and diffs against the state
    // already posted, while the disconnect path withdraws anything that no longer exists.
    let mut posted = Seen::default();

    supervise::run("niri", || {
        let mut backoff = Backoff::new(FIRST_RETRY, LAST_RETRY);
        // one line while niri is unreachable, not one per backoff attempt
        let mut said_unreachable = false;

        loop {
            let started = Instant::now();
            let reached = run(
                connect().map(BufReader::new),
                &mut posted,
                |_, seen| {
                    let mut niri = Niri::write();
                    niri.seen = seen.clone();
                    niri.connected = true;
                },
                |heard| {
                    let hearing = WINDOWS.locked().clone();

                    for then in hearing {
                        then(heard.clone());
                    }
                },
                |name| {
                    let hearing = LAYOUTS.locked().clone();

                    for then in hearing {
                        then(name.clone());
                    }
                },
                || {
                    let hearing = CONFIGS.locked().clone();

                    for then in hearing {
                        then();
                    }
                },
            );

            let wait = backoff.after(started.elapsed(), HEALTHY);
            if reached {
                said_unreachable = false;
                eprintln!("kanade: niri event stream lost; connecting again in {wait:?}");
            } else if !said_unreachable {
                eprintln!("kanade: niri is not reachable; connecting again in {wait:?}");
                said_unreachable = true;
            }
            thread::sleep(wait);
        }
    });
}

// watches one connection until it ends; whether niri accepted it
fn run(
    stream: io::Result<impl BufRead>,
    posted: &mut Seen,
    mut post: impl FnMut(&Seen, &Seen),
    mut hear: impl FnMut(Heard),
    mut switched: impl FnMut(String),
    mut failed: impl FnMut(),
) -> bool {
    let mut reached = false;

    let _lost = match stream {
        Ok(stream) => watch_accepted(
            stream,
            posted,
            &mut post,
            &mut hear,
            &mut switched,
            &mut failed,
            || reached = true,
        ),
        Err(error) => error,
    };

    // `follow` retries, and says so once; stream errors can include peer text, so they are not
    // logged here

    let had_state = *posted != Seen::default();
    if reached || had_state {
        post(posted, &Seen::default());
        *posted = Seen::default();

        // the windows niri had may be gone by the time it is back; it lists them again then
        hear(Heard::Lost);
        tell_apps(AppHeard::Lost);
    }

    Niri::write().connected = false;

    reached
}

fn connect() -> io::Result<UnixStream> {
    let mut stream = UnixStream::connect(socket()?)?;

    stream.write_all(b"\"EventStream\"\n")?;

    Ok(stream)
}

fn socket() -> io::Result<String> {
    env::var("NIRI_SOCKET").map_err(|_| io::Error::other("NIRI_SOCKET is not set"))
}

// how long niri may take to answer `ask` or `act`
const PATIENCE: Duration = Duration::from_secs(2);

// how long the user may take to pick a color
const PICK: Duration = Duration::from_secs(60);

// how a request niri was sent came out
#[derive(Debug)]
enum Exchange {
    Answered(Json),
    Refused(io::Error),

    // sent but not answered, or answered with what is neither: niri may have carried it out
    Unclear(io::Error),
}

// what an action niri was sent came to
#[derive(Debug)]
pub enum Acted {
    Done,

    // niri took it but gave no clear answer, so it may or may not be carried out; why
    Unknown(String),
}

/*
 * what niri answers `request`, a query like `"Version"`, on a connection of its own; what it
 * refuses, or does not answer, is an error
 */
pub fn ask(request: &str) -> io::Result<Json> {
    match exchange(UnixStream::connect(socket()?)?, request, PATIENCE)? {
        Exchange::Answered(answer) => Ok(answer),
        Exchange::Refused(error) | Exchange::Unclear(error) => Err(error),
    }
}

/*
 * what niri answers `"PickColor"`: its picker is on screen until the user clicks a pixel or leaves
 * it, and only then does it answer, so this waits up to `PICK`, never on the draw thread
 */
pub fn pick() -> io::Result<Json> {
    match exchange(UnixStream::connect(socket()?)?, "\"PickColor\"", PICK)? {
        Exchange::Answered(answer) => Ok(answer),
        Exchange::Refused(error) | Exchange::Unclear(error) => Err(error),
    }
}

/*
 * asks niri to carry out `action`, a request with an effect like `{"Action":...}`; an error is a
 * definite no, while a missing or unclear answer is told apart, so the action is not repeated as if
 * it failed
 */
pub fn act(action: &str) -> io::Result<Acted> {
    match exchange(UnixStream::connect(socket()?)?, action, PATIENCE)? {
        Exchange::Answered(_) => Ok(Acted::Done),
        Exchange::Refused(error) => Err(error),
        Exchange::Unclear(error) => Ok(Acted::Unknown(error.to_string())),
    }
}

/*
 * one `patience` for sending and answering both; an error before niri has the whole request means
 * it was not sent
 */
fn exchange(stream: UnixStream, request: &str, patience: Duration) -> io::Result<Exchange> {
    let deadline = Instant::now() + patience;

    stream.set_write_timeout(Some(patience))?;
    (&stream).write_all(format!("{request}\n").as_bytes())?;

    let mut reply = String::new();

    // a timeout of zero is refused, so a request that took all the time waits a moment more
    let left = deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(1));
    let read = stream
        .set_read_timeout(Some(left))
        .and_then(|()| BufReader::new(stream).read_line(&mut reply));

    let unanswered = match read {
        Ok(0) => Some(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "closed without answering",
        )),
        Ok(_) => None,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            Some(io::Error::new(
                error.kind(),
                format!("did not answer within {patience:?}"),
            ))
        }
        Err(error) => Some(error),
    };

    if let Some(error) = unanswered {
        return Ok(Exchange::Unclear(error));
    }

    let parsed = json::parse(&reply);

    if let Some(answer) = parsed.as_ref().and_then(|parsed| parsed.get("Ok")) {
        return Ok(Exchange::Answered(answer.clone()));
    }

    if let Some(refused) = parsed
        .as_ref()
        .and_then(|parsed| parsed.get("Err"))
        .and_then(Json::as_str)
    {
        return Ok(Exchange::Refused(io::Error::other(refused.to_owned())));
    }

    Ok(Exchange::Unclear(io::Error::other(format!(
        "answered unexpectedly: {}",
        reply.trim_end()
    ))))
}

/*
 * follows niri until the stream ends, posting what changes as it was and is now, so an event that
 * changes nothing the core follows wakes nothing; window events go to `hear`, which sorts out its
 * own. Returns why it ended
 */
#[cfg(test)]
fn watch(
    lines: impl BufRead,
    posted: &mut Seen,
    post: &mut impl FnMut(&Seen, &Seen),
    hear: &mut impl FnMut(Heard),
    switched: &mut impl FnMut(String),
    failed: &mut impl FnMut(),
) -> io::Error {
    watch_accepted(lines, posted, post, hear, switched, failed, || {})
}

// as `watch`, with `accepted` run once niri has accepted the stream
fn watch_accepted(
    lines: impl BufRead,
    posted: &mut Seen,
    post: &mut impl FnMut(&Seen, &Seen),
    hear: &mut impl FnMut(Heard),
    switched: &mut impl FnMut(String),
    failed: &mut impl FnMut(),
    mut accepted: impl FnMut(),
) -> io::Error {
    let mut lines = lines.lines();
    let mut niri = Known::default();

    // per stream: a new one lists the layouts first, which shows nothing
    let mut layouts = Layouts::default();

    // niri answers the request first, events follow on the same connection
    match lines.next() {
        Some(Ok(reply)) if json::parse(&reply).is_some_and(|reply| reply.get("Ok").is_some()) => {}
        Some(Ok(reply)) => return io::Error::other(format!("niri refused: {reply}")),
        Some(Err(error)) => return error,
        None => return io::ErrorKind::UnexpectedEof.into(),
    }

    struct ConnectedGuard;
    impl Drop for ConnectedGuard {
        fn drop(&mut self) {
            Niri::write().connected = false;
        }
    }

    Niri::write().connected = true;
    eprintln!("kanade: niri event stream connected");
    accepted();
    let _guard = ConnectedGuard;

    for line in lines {
        let line = match line {
            Ok(line) => line,
            Err(error) => return error,
        };

        /*
         * a line this reader cannot follow is skipped, the next event still counts. Only its
         * length is said: a niri event carries window titles, and nothing of niri leaves
         * `windows` but window ids
         */
        let Some(event) = json::parse(&line) else {
            eprintln!(
                "kanade: skipped a niri event that is not JSON ({} bytes)",
                line.len()
            );
            continue;
        };

        if let Some(heard) = windowed(&event) {
            hear(heard);
        }

        // no-op until screen time registers, so an off Module reads no titles
        apps(&event);

        let before = layouts.in_use();
        if let Some(name) = layouts.switch(&event) {
            switched(name);
        }
        let after = layouts.in_use();
        if after != before {
            let hearing = KEYBOARDS.locked().clone();

            for then in hearing {
                then(after.clone());
            }
        }

        if let Some(path) = shot(&event) {
            let hearing = SCREENSHOTS.locked().clone();

            for then in hearing {
                then(path.clone());
            }
        }

        if let Some(on) = touchpad(&event) {
            let hearing = TOUCHPADS.locked().clone();

            for then in hearing {
                then(on);
            }
        }

        if let Some(loaded) = event.get("ConfigLoaded") {
            let is_failed = loaded.get("failed").and_then(Json::as_bool) == Some(true);

            record_config_load(is_failed);

            if is_failed {
                failed();
            }
        }

        niri.apply(&event);

        let seen = niri.seen();

        if seen != *posted {
            post(posted, &seen);
            *posted = seen;
        }
    }

    io::ErrorKind::UnexpectedEof.into()
}

// the path niri saved, or none when it only copied; none for another event. Never logged
fn shot(event: &Json) -> Option<Option<String>> {
    let captured = event.get("ScreenshotCaptured")?;

    Some(
        captured
            .get("path")
            .and_then(Json::as_str)
            .map(str::to_owned),
    )
}

fn touchpad(event: &Json) -> Option<bool> {
    event
        .get("TouchpadToggled")?
        .get("on")
        .and_then(Json::as_bool)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"{"Ok":"Handled"}"#;

    // two outputs, workspace 1 focused on eDP-1, workspace 3 shown on HDMI-A-1
    const WORKSPACES: &str = r#"{"WorkspacesChanged":{"workspaces":[{"id":1,"idx":1,"name":null,"output":"eDP-1","is_urgent":false,"is_active":true,"is_focused":true,"active_window_id":2},{"id":2,"idx":2,"name":null,"output":"eDP-1","is_urgent":false,"is_active":false,"is_focused":false,"active_window_id":null},{"id":3,"idx":1,"name":"web","output":"HDMI-A-1","is_urgent":false,"is_active":true,"is_focused":false,"active_window_id":null}]}}"#;

    fn activated(id: u64, focused: bool) -> String {
        format!(r#"{{"WorkspaceActivated":{{"id":{id},"focused":{focused}}}}}"#)
    }

    fn overview(open: bool) -> String {
        format!(r#"{{"OverviewOpenedOrClosed":{{"is_open":{open}}}}}"#)
    }

    // every post for this stream, and why it ended
    fn posts(lines: &[&str]) -> (Vec<Seen>, io::ErrorKind) {
        // `watch` writes the one `Niri` every test shares, so the tests that run it take turns
        let _turn = NIRI_TURN.locked();

        let mut posts = Vec::new();
        let mut posted = Seen::default();

        let text = lines.join("\n");
        let lost = watch(
            text.as_bytes(),
            &mut posted,
            &mut |before, seen| {
                assert_eq!(before, posts.last().unwrap_or(&Seen::default()));
                posts.push(seen.clone())
            },
            &mut drop,
            &mut drop,
            &mut || {},
        );

        assert_eq!(posts.last().cloned().unwrap_or_default(), posted);

        (posts, lost.kind())
    }

    // a watcher that looks once sees only the last post
    #[test]
    fn an_overview_opened_and_closed_is_counted() {
        let (posts, _) = posts(&[
            OK,
            WORKSPACES,
            &overview(true),
            &activated(2, true),
            &overview(false),
        ]);
        let last = posts.last().expect("the stream changed things");

        assert!(!last.overview);
        assert_eq!(last.overviews, 1);
        assert_eq!(last.workspace.as_ref().map(|focused| focused.id), Some(2));
    }

    #[test]
    fn urgency_names_the_output_and_drops_one_without() {
        let read = posts;
        let (posts, _) = read(&[
            OK,
            WORKSPACES,
            r#"{"WorkspaceUrgencyChanged":{"id":2,"urgent":true}}"#,
            r#"{"WorkspaceUrgencyChanged":{"id":9,"urgent":true}}"#,
            r#"{"WorkspaceUrgencyChanged":{"id":2,"urgent":false}}"#,
        ]);
        let marked = &posts[1];

        assert_eq!(marked.urgent.len(), 1);
        assert_eq!(marked.urgent[0].id, 2);
        assert_eq!(marked.urgent[0].output, "eDP-1");
        assert!(posts.last().unwrap().urgent.is_empty());

        let (unknown, _) = read(&[
            OK,
            r#"{"WorkspacesChanged":{"workspaces":[{"id":4,"idx":1,"name":null,"output":null,"is_urgent":true,"is_active":true,"is_focused":false}]}}"#,
        ]);

        assert!(unknown.last().unwrap().urgent.is_empty());
    }

    #[test]
    fn a_screenshot_event_is_a_path_or_copied() {
        let saved = json::parse(r#"{"ScreenshotCaptured":{"path":"/tmp/a.png"}}"#).unwrap();
        let copied = json::parse(r#"{"ScreenshotCaptured":{"path":null}}"#).unwrap();

        assert_eq!(shot(&saved).unwrap().as_deref(), Some("/tmp/a.png"));
        assert_eq!(shot(&copied).unwrap(), None);
        assert!(shot(&json::parse(OK).unwrap()).is_none());
        assert_eq!(
            touchpad(&json::parse(r#"{"TouchpadToggled":{"on":false}}"#).unwrap()),
            Some(false)
        );
    }

    #[test]
    fn other_events_and_junk_post_nothing() {
        let (posts, _) = posts(&[
            OK,
            r#"{"WindowsChanged":{"windows":[]}}"#,
            r#"{"KeyboardLayoutsChanged":{"keyboard_layouts":{"names":["English (US)"],"current_idx":0}}}"#,
            "not json",
            &activated(9, true),
            r#"{"CastsChanged":{"casts":[]}}"#,
        ]);

        // workspace 9 is unknown, so its output is too, which is the default
        assert_eq!(posts, []);
    }

    // what `windows` hears of this stream
    fn heard(lines: &[&str]) -> Vec<Heard> {
        let _turn = NIRI_TURN.locked();
        let text = [&[OK], lines].concat().join("\n");
        let mut heard = Vec::new();

        watch(
            text.as_bytes(),
            &mut Seen::default(),
            &mut |_, _| {},
            &mut |told| heard.push(told),
            &mut drop,
            &mut || {},
        );

        heard
    }

    fn window(id: u64, app_id: Option<&str>, focused: bool, urgent: bool) -> Window {
        Window {
            id: WindowId(id),
            app_id: app_id.map(String::from),
            focused,
            urgent,
        }
    }

    #[test]
    fn window_events_are_heard_as_windows() {
        let heard = heard(&[
            r#"{"WindowsChanged":{"windows":[{"id":1,"title":"~","app_id":"kitty","pid":7,"workspace_id":1,"is_focused":true,"is_floating":false,"is_urgent":false,"layout":{},"focus_timestamp":null},{"id":2,"title":null,"app_id":null,"pid":null,"workspace_id":null,"is_focused":false,"is_floating":true,"is_urgent":true}]}}"#,
            r#"{"WindowOpenedOrChanged":{"window":{"id":3,"title":"a","app_id":"firefox","is_focused":true,"is_urgent":false}}}"#,
            r#"{"WindowFocusChanged":{"id":1}}"#,
            r#"{"WindowFocusChanged":{"id":null}}"#,
            r#"{"WindowUrgencyChanged":{"id":3,"urgent":true}}"#,
            r#"{"WindowClosed":{"id":3}}"#,
            r#"{"WindowLayoutsChanged":{"changes":[]}}"#,
            r#"{"WindowFocusTimestampChanged":{"id":1,"focus_timestamp":null}}"#,
            r#"{"WindowOpenedOrChanged":{"window":{"title":"no id"}}}"#,
            &overview(true),
        ]);

        assert_eq!(
            heard,
            [
                Heard::All(vec![
                    window(1, Some("kitty"), true, false),
                    window(2, None, false, true),
                ]),
                Heard::Opened(window(3, Some("firefox"), true, false)),
                Heard::Focused(Some(WindowId(1))),
                Heard::Focused(None),
                Heard::Urgent(WindowId(3), true),
                Heard::Closed(WindowId(3)),
            ]
        );
    }

    fn cast(stream: u64) -> String {
        format!(
            r#"{{"CastStartedOrChanged":{{"cast":{{"stream_id":{stream},"session_id":1,"kind":"PipeWire","target":{{"Output":{{"name":"eDP-1"}}}},"is_dynamic_target":false,"is_active":true,"pid":null,"pw_node_id":null}}}}}}"#
        )
    }

    fn stopped(stream: u64) -> String {
        format!(r#"{{"CastStopped":{{"stream_id":{stream}}}}}"#)
    }

    // whether each post says something casts
    fn casting(posts: &[Seen]) -> Vec<bool> {
        posts.iter().map(|seen| !seen.casts.is_empty()).collect()
    }

    // a cast that retargets, or has a target niri may add later, posts what it captures now
    #[test]
    fn a_pid_null_cast_in_our_ledger_is_dropped() {
        let cast_json = json::parse(
            r#"{"stream_id":10,"session_id":42,"kind":"PipeWire","target":{"Window":{"id":1}},"pid":null}"#,
        )
        .unwrap();

        // when the session is in Kanade's ledger, it is treated as Kanade's own and dropped
        assert!(cast_with(&cast_json, |session| session == 42).is_none());

        // when the session is someone else's, it is reported
        assert!(cast_with(&cast_json, |_| false).is_some());
    }

    #[test]
    fn a_cast_says_what_it_captures() {
        let with = |stream: u64, target: &str| {
            format!(
                r#"{{"CastStartedOrChanged":{{"cast":{{"stream_id":{stream},"kind":"Future","target":{target},"extra":1}}}}}}"#
            )
        };
        let (posts, _) = posts(&[
            OK,
            &with(1, r#"{"Nothing":{}}"#),
            &with(1, r#"{"Window":{"id":4}}"#),
            &with(2, r#"{"Output":{"name":"eDP-1"}}"#),
            &with(3, r#"{"Region":{}}"#),
            &stopped(1),
        ]);
        let casts: Vec<Vec<Target>> = posts
            .iter()
            .map(|seen| seen.casts.iter().copied().collect())
            .collect();

        assert_eq!(
            casts,
            [
                vec![Target::Unknown],
                vec![Target::Window],
                vec![Target::Screen, Target::Window],
                vec![Target::Screen, Target::Window, Target::Unknown],
                vec![Target::Screen, Target::Unknown],
            ]
        );
    }

    // a paused cast still counts as a cast, and a dynamic one can be retargeted
    #[test]
    fn a_cast_says_whether_it_is_paused_or_dynamic() {
        let with = |stream: u64, dynamic: bool, active: bool| {
            format!(
                r#"{{"CastStartedOrChanged":{{"cast":{{"stream_id":{stream},"session_id":{stream},"kind":"PipeWire","target":{{"Nothing":{{}}}},"is_dynamic_target":{dynamic},"is_active":{active},"pid":null,"pw_node_id":null}}}}}}"#
            )
        };
        let (posts, _) = posts(&[
            OK,
            &with(1, true, true),
            &with(1, true, false),
            &with(2, false, true),
            &with(2, false, false),
            &stopped(1),
        ]);
        let states: Vec<(bool, bool, bool)> = posts
            .iter()
            .map(|seen| (!seen.casts.is_empty(), seen.dynamic, seen.paused))
            .collect();

        assert_eq!(
            states,
            [
                (true, true, false),
                (true, true, true),
                (true, true, false),
                (true, true, true),
                (true, false, true),
            ]
        );
    }

    // only a PipeWire cast has a session niri can stop; a recording's wlr-screencopy one has none
    #[test]
    fn only_a_pipewire_cast_has_a_session_to_stop() {
        let wlr = r#"{"CastStartedOrChanged":{"cast":{"stream_id":9,"session_id":9,"kind":"WlrScreencopy","target":{"Output":{"name":"eDP-1"}},"pid":1}}}"#;
        let (posts, _) = posts(&[OK, wlr, &cast(1), &stopped(1)]);
        let sessions: Vec<Vec<u64>> = posts
            .iter()
            .map(|seen| seen.sessions.iter().copied().collect())
            .collect();

        assert_eq!(sessions, [vec![], vec![1], vec![]]);
    }

    // a share and a recording at once are one capture, until both stop
    #[test]
    fn casting_lasts_from_the_first_cast_to_the_last() {
        let (posts, _) = posts(&[OK, &cast(1), &cast(2), &stopped(1), &cast(2), &stopped(2)]);

        assert_eq!(casting(&posts), [true, false]);
    }

    // Kanade's own capture records nothing
    #[test]
    fn kanade_s_own_capture_is_no_cast() {
        let own = format!(
            r#"{{"CastStartedOrChanged":{{"cast":{{"stream_id":7,"session_id":7,"kind":"WlrScreencopy","target":{{"Output":{{"name":"eDP-1"}}}},"is_dynamic_target":false,"is_active":true,"pid":{},"pw_node_id":null}}}}}}"#,
            std::process::id()
        );
        let list = format!(
            r#"{{"CastsChanged":{{"casts":[{{"stream_id":8,"pid":{}}}]}}}}"#,
            std::process::id()
        );

        let (own_only, _) = posts(&[OK, &own, &list]);
        assert!(!casting(&own_only).contains(&true));

        // and hides no other
        let (beside, _) = posts(&[OK, &own, &cast(1)]);
        assert_eq!(casting(&beside), [true]);
    }

    #[test]
    fn a_cast_list_replaces_the_casts_known() {
        let list = |streams: &str| {
            format!(
                r#"{{"CastsChanged":{{"casts":[{}]}}}}"#,
                streams
                    .split(',')
                    .filter(|stream| !stream.is_empty())
                    .map(|stream| {
                        format!(
                            r#"{{"stream_id":{stream},"target":{{"Output":{{"name":"eDP-1"}}}}}}"#
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };

        let replaced = posts(&[OK, &list("3"), &stopped(9), &list("4,5"), &list("")]).0;

        assert_eq!(casting(&replaced), [true, false]);

        // a stop for a cast the list no longer has changes nothing; the list posts once, as the
        // session of the PipeWire cast it replaces is gone
        let forgotten = posts(&[OK, &cast(1), &list("2"), &stopped(1)]).0;

        assert_eq!(casting(&forgotten), [true, true]);
    }

    // how many failed loads of its config this stream tells of
    fn failures(lines: &[&str]) -> usize {
        let _turn = NIRI_TURN.locked();
        let text = [&[OK], lines].concat().join("\n");
        let mut failures = 0;

        watch(
            text.as_bytes(),
            &mut Seen::default(),
            &mut |_, _| {},
            &mut drop,
            &mut drop,
            &mut || failures += 1,
        );

        failures
    }

    #[test]
    fn only_a_failed_config_load_is_told() {
        let loaded = |failed| format!(r#"{{"ConfigLoaded":{{"failed":{failed}}}}}"#);

        assert_eq!(failures(&[&loaded(false), WORKSPACES]), 0);
        assert_eq!(failures(&[&loaded(true)]), 1);
        assert_eq!(failures(&[&loaded(false), &loaded(true), &loaded(true)]), 2);
        assert_eq!(failures(&[r#"{"ConfigLoaded":{}}"#]), 0);
    }

    // the layouts each switch of this stream goes to
    fn switches(lines: &[&str]) -> Vec<String> {
        let _turn = NIRI_TURN.locked();
        let text = [&[OK], lines].concat().join("\n");
        let mut switches = Vec::new();

        watch(
            text.as_bytes(),
            &mut Seen::default(),
            &mut |_, _| {},
            &mut drop,
            &mut |name| switches.push(name),
            &mut || {},
        );

        switches
    }

    fn layouts(names: &[&str], current: u8) -> String {
        let names: Vec<String> = names.iter().map(|name| format!("{name:?}")).collect();

        format!(
            r#"{{"KeyboardLayoutsChanged":{{"keyboard_layouts":{{"names":[{}],"current_idx":{current}}}}}}}"#,
            names.join(",")
        )
    }

    fn switched(index: u8) -> String {
        format!(r#"{{"KeyboardLayoutSwitched":{{"idx":{index}}}}}"#)
    }

    // the list niri gives on connecting or on a change of config is no switch, nor is one to the
    // layout in use
    #[test]
    fn only_a_switch_between_layouts_is_heard() {
        let two = layouts(&["English (US)", "Russian"], 0);

        assert_eq!(
            switches(&[&two, &switched(1), &switched(1), &switched(0)]),
            ["Russian", "English (US)"]
        );
        assert_eq!(
            switches(&[&two, &layouts(&["German", "Russian"], 1)]),
            Vec::<String>::new()
        );

        // the list again, as on a change of config, starts from the layout it says is in use
        assert_eq!(
            switches(&[&two, &layouts(&["English (US)", "German"], 1), &switched(0)]),
            ["English (US)"]
        );
    }

    // with one layout there is nothing to switch between; a switch to one not listed says nothing
    #[test]
    fn a_switch_with_one_layout_or_none_listed_is_not_heard() {
        assert_eq!(
            switches(&[&layouts(&["English (US)"], 0), &switched(1)]),
            Vec::<String>::new()
        );
        assert_eq!(switches(&[&switched(1)]), Vec::<String>::new());
        assert_eq!(
            switches(&[&layouts(&["English (US)", "German"], 0), &switched(5)]),
            Vec::<String>::new()
        );
    }

    // what the lock screen shows is the layout in use, known from the list, before any switch
    #[test]
    fn the_layout_in_use_is_known_from_the_list_and_only_with_two() {
        let mut layouts = Layouts::default();
        let mut feed = |event: String| {
            layouts.switch(&json::parse(&event).unwrap());
            layouts.in_use()
        };

        assert_eq!(feed(switched(1)), None);
        assert_eq!(
            feed(self::layouts(&["English (US)", "Russian"], 1)).as_deref(),
            Some("Russian")
        );
        assert_eq!(feed(switched(0)).as_deref(), Some("English (US)"));
        assert_eq!(feed(switched(5)), None);
        assert_eq!(feed(self::layouts(&["English (US)"], 0)), None);
    }

    #[test]
    fn a_refused_request_ends_the_stream() {
        let (posts, lost) = posts(&[r#"{"Err":"nope"}"#, WORKSPACES]);

        assert_eq!(posts, []);
        assert_eq!(lost, io::ErrorKind::Other);
    }

    // the core is told once, then niri is gone for good
    #[test]
    fn losing_the_stream_degrades_to_every_monitor_focused() {
        let _turn = NIRI_TURN.locked();

        let text = [OK, WORKSPACES, &overview(true), &cast(1)].join("\n");
        let mut posts = Vec::new();

        run(
            Ok(text.as_bytes()),
            &mut Seen::default(),
            |_, seen| posts.push(seen.clone()),
            drop,
            drop,
            || {},
        );

        // nobody can say a cast still runs, so its capture goes too
        assert_eq!(posts.last(), Some(&Seen::default()));
        assert_eq!(posts.len(), 4);

        // a failed connection does nothing; an accepted empty stream still posts its default
        // state once when it ends
        let mut posts = Vec::new();

        run(
            Err::<&[u8], _>(io::ErrorKind::NotFound.into()),
            &mut Seen::default(),
            |_, seen| posts.push(seen.clone()),
            drop,
            drop,
            || {},
        );
        run(
            Ok(OK.as_bytes()),
            &mut Seen::default(),
            |_, seen| posts.push(seen.clone()),
            drop,
            drop,
            || {},
        );

        assert_eq!(posts, [Seen::default()]);
    }

    #[test]
    fn losing_the_stream_marks_niri_disconnected() {
        let _turn = NIRI_TURN.locked();

        let text = [OK, WORKSPACES].join("\n");
        let mut posted = Seen::default();

        run(
            Ok(text.as_bytes()),
            &mut posted,
            |_, seen| {
                let mut niri = Niri::write();
                niri.seen = seen.clone();
                niri.connected = true;
            },
            drop,
            drop,
            || {},
        );

        assert!(!Niri::read().is_connected());
    }

    #[test]
    fn window_layout_and_column_tracking() {
        let mut known = Known {
            workspaces: HashMap::new(),
            focused: Some(1),
            overview: false,
            overviews: 0,
            casts: HashMap::new(),
            windows: HashMap::new(),
            last_focused_window: None,
            tracked_windows: HashMap::new(),
        };

        let workspaces_event = json::parse(
            r#"{"WorkspacesChanged":{"workspaces":[{"id":1,"idx":1,"name":null,"output":"eDP-1","is_active":true,"is_focused":true,"active_window_id":10}]}}"#,
        )
        .unwrap();
        known.apply(&workspaces_event);

        let windows_event = json::parse(
            r#"{"WindowsChanged":{"windows":[
                {
                    "id": 10,
                    "title": "Terminal 1",
                    "app_id": "alacritty",
                    "is_floating": false,
                    "is_focused": true,
                    "workspace_id": 1,
                    "layout": {"pos_in_scrolling_layout": [0, 0], "tile_size": [640.0, 480.0]}
                },
                {
                    "id": 11,
                    "title": "Terminal 2",
                    "app_id": "alacritty",
                    "is_floating": false,
                    "is_focused": false,
                    "workspace_id": 1,
                    "layout": {"pos_in_scrolling_layout": [0, 1], "tile_size": [640.0, 480.0]}
                },
                {
                    "id": 12,
                    "title": "Browser",
                    "app_id": "firefox",
                    "is_floating": false,
                    "is_focused": false,
                    "workspace_id": 1,
                    "layout": {"pos_in_scrolling_layout": [1, 0], "tile_size": [800.0, 960.0]}
                }
            ]}}"#,
        )
        .unwrap();
        known.apply(&windows_event);

        let seen = known.seen();
        assert_eq!(seen.workspaces.len(), 1);
        assert_eq!(seen.workspaces[0].id, 1);
        assert_eq!(seen.workspaces[0].index, 1);
        assert!(seen.workspaces[0].is_active);
        assert!(seen.workspaces[0].is_focused);

        let focused = seen.focused_window.expect("focused window present");
        assert_eq!(focused.id, 10);
        assert_eq!(focused.title, "Terminal 1");
        assert_eq!(focused.app_id.as_deref(), Some("alacritty"));
        assert!(!focused.is_floating);
        // Column 0 has two windows (10 and 11)
        assert_eq!(focused.tile_count, 2);
        assert_eq!(focused.column_width, Some(640.0));
        assert_eq!(focused.workspace_id, Some(1));

        // When layer surface takes focus (WindowFocusChanged { id: None }), last focused window is held
        let unfocus_event = json::parse(r#"{"WindowFocusChanged":{"id":null}}"#).unwrap();
        known.apply(&unfocus_event);
        let seen = known.seen();
        let held = seen.focused_window.expect("last focused window held");
        assert_eq!(held.id, 10);
        assert_eq!(held.tile_count, 2);

        // When focus moves to window 12 (column 1 with single window)
        let focus_12 = json::parse(r#"{"WindowFocusChanged":{"id":12}}"#).unwrap();
        known.apply(&focus_12);
        let seen = known.seen();
        let focused_12 = seen.focused_window.expect("window 12 focused");
        assert_eq!(focused_12.id, 12);
        assert_eq!(focused_12.tile_count, 1);
        assert_eq!(focused_12.column_width, Some(800.0));

        // WindowLayoutsChanged updates width
        let layout_change = json::parse(
            r#"{"WindowLayoutsChanged":{"changes":[[12, {"pos_in_scrolling_layout": [1, 0], "tile_size": [1000.0, 960.0]}]]}}"#,
        )
        .unwrap();
        known.apply(&layout_change);
        let seen = known.seen();
        let updated = seen.focused_window.expect("window 12 updated");
        assert_eq!(updated.column_width, Some(1000.0));

        // Closing focused window clears it
        let close_12 = json::parse(r#"{"WindowClosed":{"id":12}}"#).unwrap();
        known.apply(&close_12);
        let seen = known.seen();
        assert_eq!(seen.focused_window, None);
    }

    #[test]
    fn strip_window_tracking_and_deterministic_order() {
        let mut known = Known {
            workspaces: HashMap::new(),
            focused: Some(1),
            overview: false,
            overviews: 0,
            casts: HashMap::new(),
            windows: HashMap::new(),
            last_focused_window: None,
            tracked_windows: HashMap::new(),
        };

        // Two workspaces
        let workspaces_event = json::parse(
            r#"{"WorkspacesChanged":{"workspaces":[
                {"id":1,"idx":1,"name":null,"output":"eDP-1","is_active":true,"is_focused":true,"active_window_id":20},
                {"id":2,"idx":2,"name":null,"output":"eDP-1","is_active":false,"is_focused":false,"active_window_id":null}
            ]}}"#,
        )
        .unwrap();
        known.apply(&workspaces_event);

        // Windows intentionally out of order across workspaces, columns, and rows
        let windows_event = json::parse(
            r#"{"WindowsChanged":{"windows":[
                {
                    "id": 30,
                    "title": "Editor",
                    "app_id": "code",
                    "is_floating": false,
                    "is_focused": false,
                    "workspace_id": 2,
                    "layout": {"pos_in_scrolling_layout": [0, 0], "tile_size": [1000.0, 900.0]}
                },
                {
                    "id": 21,
                    "title": "Sub Terminal",
                    "app_id": "foot",
                    "is_floating": false,
                    "is_focused": false,
                    "workspace_id": 1,
                    "layout": {"pos_in_scrolling_layout": [0, 1], "tile_size": [700.0, 400.0]}
                },
                {
                    "id": 20,
                    "title": "Main Terminal",
                    "app_id": "foot",
                    "is_floating": false,
                    "is_focused": true,
                    "workspace_id": 1,
                    "layout": {"pos_in_scrolling_layout": [0, 0], "tile_size": [700.0, 600.0]}
                },
                {
                    "id": 22,
                    "title": "Web Browser",
                    "app_id": "firefox",
                    "is_floating": false,
                    "is_focused": false,
                    "workspace_id": 1,
                    "layout": {"pos_in_scrolling_layout": [1, 0], "tile_size": [900.0, 1000.0]}
                },
                {
                    "id": 40,
                    "title": "Floating Note",
                    "app_id": "notes",
                    "is_floating": true,
                    "is_focused": false,
                    "workspace_id": 1,
                    "layout": null
                }
            ]}}"#,
        )
        .unwrap();
        known.apply(&windows_event);

        let seen = known.seen();
        assert_eq!(seen.windows.len(), 5);

        // Deterministic sort: (workspace_id, col, row, id)
        // 1. Floating window (col: None, row: None) on ws 1
        assert_eq!(seen.windows[0].id, 40);
        assert!(seen.windows[0].is_floating);
        assert_eq!(seen.windows[0].col, None);
        assert_eq!(seen.windows[0].row, None);

        // 2. ws 1, col 0, row 0 -> id 20
        assert_eq!(seen.windows[1].id, 20);
        assert_eq!(seen.windows[1].title, "Main Terminal");
        assert_eq!(seen.windows[1].app_id.as_deref(), Some("foot"));
        assert_eq!(seen.windows[1].workspace_id, Some(1));
        assert_eq!(seen.windows[1].col, Some(0));
        assert_eq!(seen.windows[1].row, Some(0));
        assert_eq!(seen.windows[1].tile_width, Some(700.0));
        assert_eq!(seen.windows[1].tile_height, Some(600.0));
        assert!(seen.windows[1].is_focused);

        // 3. ws 1, col 0, row 1 -> id 21
        assert_eq!(seen.windows[2].id, 21);
        assert_eq!(seen.windows[2].col, Some(0));
        assert_eq!(seen.windows[2].row, Some(1));
        assert_eq!(seen.windows[2].tile_width, Some(700.0));
        assert_eq!(seen.windows[2].tile_height, Some(400.0));
        assert!(!seen.windows[2].is_focused);

        // 4. ws 1, col 1, row 0 -> id 22
        assert_eq!(seen.windows[3].id, 22);
        assert_eq!(seen.windows[3].col, Some(1));
        assert_eq!(seen.windows[3].row, Some(0));

        // 5. ws 2, col 0, row 0 -> id 30
        assert_eq!(seen.windows[4].id, 30);
        assert_eq!(seen.windows[4].workspace_id, Some(2));
        assert_eq!(seen.windows[4].col, Some(0));
        assert_eq!(seen.windows[4].row, Some(0));

        // WindowLayoutsChanged updates col, row, width, height
        let layout_event = json::parse(
            r#"{"WindowLayoutsChanged":{"changes":[
                [20, {"pos_in_scrolling_layout": [2, 0], "tile_size": [850.0, 950.0]}]
            ]}}"#,
        )
        .unwrap();
        known.apply(&layout_event);

        let seen = known.seen();
        let w20 = seen.windows.iter().find(|w| w.id == 20).unwrap();
        assert_eq!(w20.col, Some(2));
        assert_eq!(w20.row, Some(0));
        assert_eq!(w20.tile_width, Some(850.0));
        assert_eq!(w20.tile_height, Some(950.0));

        // WindowFocusChanged updates is_focused
        let focus_event = json::parse(r#"{"WindowFocusChanged":{"id":22}}"#).unwrap();
        known.apply(&focus_event);
        let seen = known.seen();
        assert!(!seen.windows.iter().find(|w| w.id == 20).unwrap().is_focused);
        assert!(seen.windows.iter().find(|w| w.id == 22).unwrap().is_focused);

        // WindowUrgencyChanged updates is_urgent
        let urgency_event =
            json::parse(r#"{"WindowUrgencyChanged":{"id":20,"urgent":true}}"#).unwrap();
        known.apply(&urgency_event);
        let seen = known.seen();
        assert!(seen.windows.iter().find(|w| w.id == 20).unwrap().is_urgent);

        // WindowOpenedOrChanged adds new window with focus
        let open_event = json::parse(
            r#"{"WindowOpenedOrChanged":{"window":{
                "id": 50,
                "title": "Music",
                "app_id": "spotify",
                "is_floating": false,
                "is_focused": true,
                "workspace_id": 1,
                "layout": {"pos_in_scrolling_layout": [0, 0], "tile_size": [600.0, 800.0]}
            }}}"#,
        )
        .unwrap();
        known.apply(&open_event);
        let seen = known.seen();
        let w50 = seen.windows.iter().find(|w| w.id == 50).unwrap();
        assert!(w50.is_focused);
        assert!(!seen.windows.iter().find(|w| w.id == 22).unwrap().is_focused);

        // WindowClosed removes the window
        let close_event = json::parse(r#"{"WindowClosed":{"id":50}}"#).unwrap();
        known.apply(&close_event);
        let seen = known.seen();
        assert!(seen.windows.iter().find(|w| w.id == 50).is_none());
    }

    #[test]
    fn app_focus_carries_no_title_unless_asked() {
        let changed = json::parse(
            r#"{"WindowsChanged":{"windows":[{"id":1,"title":"secret title","app_id":"kitty","is_focused":true}]}}"#,
        )
        .unwrap();
        let focus = json::parse(r#"{"WindowFocusChanged":{"id":2}}"#).unwrap();
        let none = json::parse(r#"{"WindowFocusChanged":{"id":null}}"#).unwrap();

        let heard = app_heard(&changed, false).unwrap();
        assert!(!format!("{heard:?}").contains("secret"));
        assert!(format!("{heard:?}").contains("kitty"));

        let heard = app_heard(&changed, true).unwrap();
        assert!(
            matches!(heard, AppHeard::All(windows) if windows[0].title.as_deref() == Some("secret title"))
        );
        assert_eq!(app_heard(&focus, false), Some(AppHeard::Focused(Some(2))));
        assert_eq!(app_heard(&none, false), Some(AppHeard::Focused(None)));
        assert!(!APPS_ON.load(Ordering::Relaxed));
    }
}
