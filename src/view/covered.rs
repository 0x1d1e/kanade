//! Whether a fullscreen window covers the Island, and when it shows over one.

use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};

use kanade_runtime::Monitor;

use crate::sources::fullscreen;

// the monitors whose Island shows over a fullscreen window, or morphs back to hide there again
static RAISED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/*
 * whether a fullscreen window covers the Top layer on `monitor`, where the Dock and Banners sit and
 * the Island unless raised; none in niri's overview, which draws the Top layer over every window
 */
pub fn covered(monitor: &Monitor, overview: impl FnOnce() -> bool) -> bool {
    // the Island read only under a fullscreen window, so a view that need not never redraws for it
    fullscreen::covers(monitor) && !overview()
}

/*
 * whether the Island shows over a fullscreen window: from when it is wanted, until the morph back
 * comes to rest, so it hides there whole rather than cut off mid-morph
 */
pub(super) fn raise(monitor: &str, wanted: bool, settled: bool) -> bool {
    let mut raised = RAISED.lock().unwrap_or_else(PoisonError::into_inner);

    if wanted {
        raised.insert(monitor.to_owned());
        true
    } else if settled {
        raised.remove(monitor);
        false
    } else {
        raised.contains(monitor)
    }
}
