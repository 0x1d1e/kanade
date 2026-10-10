// view code for the full interactive Surfaces; read-only, never write() a Service

use std::sync::{Mutex, MutexGuard, PoisonError};

use kanade_runtime::{Button, Rectangle};

use crate::theme;

pub mod calendar;
pub mod clipboard;
pub mod controls;
mod grid;
pub mod launcher;
pub mod media;
pub mod notifications;
pub mod session;
mod slider;
pub mod tray;
pub mod weather;

/*
 * held through a Surface's `fit`, which reads what it asks for and then posts it. Besides the
 * runtime's derive, `fit` is called by a handler or a start, so two at once would post out of
 * order, leaving the body at the older ask until the next change
 */
fn fitting() -> MutexGuard<'static, ()> {
    static FIT: Mutex<()> = Mutex::new(());

    FIT.lock().unwrap_or_else(PoisonError::into_inner)
}

/*
 * a Surface target pressed with the left button. Only the topmost target gets a click, so a right
 * click on it would otherwise never reach the island beneath: it pins the island there too (#31)
 */
fn on_left(press: impl Fn() + 'static) -> impl Fn(Button) + 'static {
    move |button| match button {
        Button::Left => {
            crate::view::claim();
            press();
        }
        Button::Right => crate::view::pin(),
        _ => {}
    }
}

// the keyboard focus, thick enough to see on any part
const RING: f32 = 2.0;

// the one focus mark every Surface draws, so the keyboard reads the same everywhere
trait Ring {
    fn border_if(self, ring: bool) -> Self;
}

impl Ring for Rectangle {
    fn border_if(self, ring: bool) -> Self {
        if ring {
            self.border(RING, theme::island().on_surface)
        } else {
            self
        }
    }
}
