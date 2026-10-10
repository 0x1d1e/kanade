//! Which outputs a fullscreen window covers (ADR 0021). A fullscreen window covers the Island's Top
//! layer, and niri sends a covered window frames only about once a second, so the OSD would show
//! late; over such an output the Island moves to the Overlay layer, hidden but for the OSD, as a
//! hidden window draws at once.
//!
//! Two sources meet here: the runtime's `Toplevels` hears from the compositor which windows are
//! fullscreen (ADR 0034), and niri's IPC which window each output shows and how big its tile is. A window covers its output
//! only while both hold: one fullscreen in a tile as big as the output. One fullscreen in a column,
//! as niri's windowed fullscreen keeps it, covers nothing; nor does any, whoever has the keyboard,
//! on a workspace not shown.

use std::collections::BTreeMap;

use kanade_runtime::service::Service;
use kanade_runtime::{FullscreenWindow, Monitor, Toplevels};

use super::niri::Niri;

// the window an output shows, as niri says: its active workspace's active window
#[derive(Debug, Clone, PartialEq)]
pub struct Showing {
    pub app_id: Option<String>,

    // logical, as an output's size is
    pub tile: (f64, f64),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fullscreen {
    // by output, empty while unknown
    pub shown: BTreeMap<String, Showing>,
}

// a watcher of `Niri`: the window each output shows; the same again changes nothing
pub fn follow_shown() {
    let niri = Niri::read();
    let mut fullscreen = Fullscreen::write();

    if fullscreen.shown == niri.seen.shown {
        fullscreen.quiet();
    } else {
        fullscreen.shown.clone_from(&niri.seen.shown);
    }
}

// whether a fullscreen window covers `monitor`
pub fn covers(monitor: &Monitor) -> bool {
    Fullscreen::read().covers(Toplevels::read().fullscreen(), monitor)
}

// why fullscreen windows are not followed, when the compositor cannot say; the Island then stays on
// Top, and the OSD over a fullscreen window shows late or not at all
pub fn status() -> Option<String> {
    let toplevels = Toplevels::read();

    toplevels
        .unavailable()
        .map(|why| format!("fullscreen windows are not followed: {why}"))
}

impl Fullscreen {
    // `windows` are the fullscreen ones the compositor lists
    fn covers(&self, windows: &[FullscreenWindow], monitor: &Monitor) -> bool {
        let Some(shown) = self.shown.get(&monitor.name) else {
            return false;
        };

        // a logical size in whole pixels, a tile's rounded
        let fills = shown.tile.0 >= f64::from(monitor.width) - 1.0
            && shown.tile.1 >= f64::from(monitor.height) - 1.0;

        // a window without an app id cannot be told from another such
        fills
            && shown.app_id.is_some()
            && windows
                .iter()
                .any(|window| window.output == monitor.name && window.app_id == shown.app_id)
    }
}

impl Service for Fullscreen {
    fn new() -> Self {
        Fullscreen::default()
    }

    fn listen() {}
}
