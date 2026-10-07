// view code for the full interactive Surfaces; read-only, never write() a Service

use amane::{Button, Rectangle};

use crate::theme;

pub mod controls;
mod grid;
pub mod launcher;
pub mod media;
pub mod notifications;
mod slider;
pub mod tray;

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
            self.border(RING, theme::ISLAND.on_surface)
        } else {
            self
        }
    }
}
