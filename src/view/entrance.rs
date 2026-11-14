//! A Surface arriving a part at a time: while the body opens, each section of the Surface the
//! crossfade brings in rises into place a step after the one before it. Driven by the crossfade's
//! own progress, like the fade, so it has no clock or timer of its own (`island/fade.rs`).

use std::cell::Cell;

use kanade_runtime::{Rise, Widget};

use crate::config;
use crate::island::fade::section_pose;

thread_local! {
    // Section arrival and outer opacity of the form being built; outside a form both are whole.
    static ARRIVED: Cell<(f32, f32)> = const { Cell::new((1.0, 1.0)) };
}

// Restore the enclosing form's section pose and opacity after building this one.
pub(super) struct Entrance((f32, f32));

impl Entrance {
    pub(super) fn take(arrived: f32, opacity: f32) -> Entrance {
        Entrance(ARRIVED.replace((arrived, opacity)))
    }
}

impl Drop for Entrance {
    fn drop(&mut self) {
        ARRIVED.set(self.0);
    }
}

/*
 * the `section`th part of a Surface, top to bottom, which rises in as the Surface opens. Reduced
 * motion only fades it, with the form
 */
pub(crate) fn rise(section: usize, widget: impl Widget + 'static) -> Rise {
    let (arrived, opacity) = ARRIVED.get();
    let (along, lift) = section_pose(section, arrived, config::get().island.motion);
    Rise::new(widget)
        .opacity(along)
        .lift(lift)
        .inert(opacity * along < 0.5)
}
