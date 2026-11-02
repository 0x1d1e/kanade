//! Floating OSD capsules are suppressed over active Island states.

use std::time::Instant;

use kanade_runtime::{Monitor, Rectangle};

use crate::island::geometry;
use crate::island::service::IslandService;

// Normal OSD feedback remains part of the Rest Island content; there is no floating HUD.
pub(super) fn hud(
    _island: &IslandService,
    _monitor: &Monitor,
    _body: geometry::Rect,
    _hang: geometry::Hang,
    _now: Instant,
) -> Option<Rectangle> {
    None
}
