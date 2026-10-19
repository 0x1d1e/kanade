use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::allocator;
use crate::changes;

// a collapse has faded; the next frames drop what the visit cached, then one trim
static AFTER_FRAME: AtomicBool = AtomicBool::new(false);

// set by that frame; the event loop trims once, not per frame
static TRIM: AtomicBool = AtomicBool::new(false);

// one redraw so a settled island still drops the visit before the trim
static REDRAW: AtomicBool = AtomicBool::new(false);

// windows drop glyph pictures from a visit when this moves, once, not on every frame
static GENERATION: AtomicU64 = AtomicU64::new(0);

/*
 * a Surface visit ended at Rest and has faded out. the next frame drops vello
 * encodings and glyph pictures that visit left; the event loop then trims once.
 * the Rest clock's letters stay in the atlas
 */
pub fn release_visit() {
    GENERATION.fetch_add(1, Ordering::Release);

    vello::low_level::vello_encoding::release_cold();

    AFTER_FRAME.store(true, Ordering::Release);
    REDRAW.store(true, Ordering::Release);

    changes::wake();
}

pub(crate) fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

// the wake asks every window for one frame, so a settled island still draws the drop
pub(crate) fn take_redraw() -> bool {
    REDRAW.swap(false, Ordering::AcqRel)
}

// called after a frame, once the visit's pictures and the view's tree are dropped
pub(crate) fn note_drawn() {
    if !AFTER_FRAME.swap(false, Ordering::AcqRel) {
        return;
    }

    TRIM.store(true, Ordering::Release);

    changes::wake();
}

// true the first time a visit's trim is taken, so a later wake does not trim again
pub(crate) fn trim_if_due() -> bool {
    if !TRIM.swap(false, Ordering::AcqRel) {
        return false;
    }

    allocator::trim_heap();

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_visit_trims_once_after_its_frame() {
        release_visit();
        assert!(take_redraw());

        assert!(!trim_if_due());

        note_drawn();
        note_drawn();

        assert!(trim_if_due());
        assert!(!trim_if_due());
    }
}
