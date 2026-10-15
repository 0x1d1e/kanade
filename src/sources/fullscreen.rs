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

use super::niri::{Niri, Showing};

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

        // niri's sizes are the output's logical pixels, the monitor's are in the UI scale's space
        let ui = f64::from(kanade_runtime::ui_scale());

        // a logical size in whole pixels, a tile's rounded
        let fills = shown.tile.0 >= f64::from(monitor.width) * ui - 1.0
            && shown.tile.1 >= f64::from(monitor.height) * ui - 1.0;

        // The sources can be joined only by app id; output and size do not distinguish a
        // fullscreen window from an unrelated maximized-to-edges window.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unidentified_niri_window_does_not_match_an_unrelated_fullscreen_window() {
        let monitor = Monitor {
            name: "eDP-1".into(),
            width: 1920,
            height: 1080,
        };
        let fullscreen = Fullscreen {
            shown: BTreeMap::from([(
                monitor.name.clone(),
                Showing {
                    app_id: None,
                    tile: (1920.0, 1080.0),
                },
            )]),
        };
        let window = FullscreenWindow {
            output: monitor.name.clone(),
            app_id: Some("browser".into()),
        };

        assert!(!fullscreen.covers(&[window], &monitor));
    }

    #[test]
    fn two_unidentified_windows_do_not_match_by_output_alone() {
        let monitor = Monitor {
            name: "eDP-1".into(),
            width: 1920,
            height: 1080,
        };
        let fullscreen = Fullscreen {
            shown: BTreeMap::from([(
                monitor.name.clone(),
                Showing {
                    app_id: None,
                    tile: (1920.0, 1080.0),
                },
            )]),
        };
        let window = FullscreenWindow {
            output: monitor.name.clone(),
            app_id: None,
        };

        assert!(!fullscreen.covers(&[window], &monitor));
    }
}
