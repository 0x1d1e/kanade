//! A Surface arriving a part at a time: while the body opens, each section of the Surface the
//! crossfade brings in rises into place a step after the one before it. Driven by the crossfade's
//! own progress, like the fade, so it has no clock or timer of its own (`island/fade.rs`).

use std::cell::Cell;

use kanade_runtime::{Rise, Widget};

use crate::config;
use crate::island::motion::Mode;

// how far a section is below its place when it starts to rise
const LIFT: f32 = 10.0;

// how much later each section starts than the one before, as a share of the progress
const STEP: f32 = 0.1;

// sections after this one start with it, so a long Surface does not take longer to arrive
const SECTIONS: usize = 5;

thread_local! {
    // how far the content drawn now has come in; 1 is all of it, and what is drawn outside a form
    static ARRIVED: Cell<f32> = const { Cell::new(1.0) };
}

// the progress a form is drawn at, for the sections drawn while it is held
pub(super) struct Entrance(f32);

impl Entrance {
    pub(super) fn take(arrived: f32) -> Entrance {
        Entrance(ARRIVED.replace(arrived))
    }
}

impl Drop for Entrance {
    fn drop(&mut self) {
        ARRIVED.set(self.0);
    }
}

// how far the `section`th part of a Surface has come in at `arrived`, eased out
fn along(section: usize, arrived: f32) -> f32 {
    let start = section.min(SECTIONS) as f32 * STEP;
    let share = ((arrived - start) / (1.0 - SECTIONS as f32 * STEP)).clamp(0.0, 1.0);

    1.0 - (1.0 - share).powi(3)
}

/*
 * the `section`th part of a Surface, top to bottom, which rises in as the Surface opens. Reduced
 * motion only fades it, with the form
 */
pub(crate) fn rise(section: usize, widget: impl Widget + 'static) -> Rise {
    let arrived = ARRIVED.get();

    if arrived >= 1.0 {
        return Rise::new(widget);
    }

    let along = along(section, arrived);
    let lift = if config::get().island.motion == Mode::Reduced {
        0.0
    } else {
        LIFT * (1.0 - along)
    };

    Rise::new(widget).opacity(along).lift(lift)
}
