//! The UI scale: one multiplier on top of each output's own scale (ADR 0091).
//!
//! Views, layouts and drawing work in a virtual space, the compositor's logical pixels divided
//! by it. The runtime multiplies back wherever the compositor is spoken to (surface sizes,
//! margins, regions, the pointer, the caret) and draws at the output's scale times it, so text,
//! icons, bodies, input regions and hit tests all scale together and no view knows. The buffer
//! stays at the output's real scale.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::changes;

// the least and the most a scale may be; a caller's own range lies inside it
const LEAST: f32 = 0.5;
const MOST: f32 = 4.0;

// what was asked, from any thread
static WANTED: AtomicU32 = AtomicU32::new(1.0f32.to_bits());

// what the event loop has applied; all drawing and talking to the compositor reads this one, so a
// frame never mixes two scales
static APPLIED: AtomicU32 = AtomicU32::new(1.0f32.to_bits());

/// The scale the shell is shown at now, 1.0 unless set. The virtual space's `monitor.width` times
/// it is the output's logical width, which is what niri's own sizes are in.
pub fn ui_scale() -> f32 {
    f32::from_bits(APPLIED.load(Ordering::Relaxed))
}

/// Asks for a scale, from any thread, and draws every window again at it. Before the runtime
/// starts it is the scale the first window opens at. A value outside `LEAST` to `MOST`, or not a
/// number, is ignored.
pub fn set_ui_scale(scale: f32) {
    if !(LEAST..=MOST).contains(&scale) {
        return;
    }

    if WANTED.swap(scale.to_bits(), Ordering::Relaxed) == scale.to_bits() {
        return;
    }

    changes::mark_all();
}

// the event loop takes what was asked; whether it differs from what was applied
pub(crate) fn adopt() -> bool {
    let wanted = WANTED.load(Ordering::Relaxed);

    APPLIED.swap(wanted, Ordering::Relaxed) != wanted
}
