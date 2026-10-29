// view code for the full interactive Surfaces; read-only, never write() a Service (only a watcher posts a size ask)

use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use kanade_runtime::{Button, Rectangle};
use kanade_runtime::{MutexExt, service::Service};

use crate::island::presentation::Surface;
use crate::island::service::IslandService;
use crate::theme;

pub mod calendar;
pub mod capture;
pub mod clipboard;
pub mod confirm;
pub mod controls;
pub mod emoji;
pub mod files;
mod grid;
pub mod keys;
pub mod launcher;
pub mod media;
pub mod notifications;
mod pad;
mod ring;
pub mod screen_time;
mod search;
pub mod session;
pub(crate) mod slider;
pub mod strip;
pub mod timer;
pub mod tray;
mod visit;
pub mod wallpapers;
pub mod weather;
pub mod welcome;
pub mod window;

/*
 * the face every pill-shaped control shares, the Notifications footer's, the Calendar's Add and
 * an empty state's Open Settings: one lifted fill and one outline, so no pill reads as another's
 * kind
 */
fn pill_face(pill: Rectangle) -> Rectangle {
    pill.fill(theme::island().surface_container)
        .border(1.0, theme::island().outline)
}

/*
 * how much of a list's row shows, as the opacity it is drawn at: whole while it is inside the
 * `room` the list scrolls in, fading as an edge cuts it, so a row never ends half drawn at full
 * strength. `top` is the row's place in the list, `offset` how far the list scrolled
 */
fn edge_fade(top: f32, height: f32, offset: f32, room: f32) -> f32 {
    let shown = (top + height).min(offset + room) - top.max(offset);
    let part = (shown / height.max(1.0)).clamp(0.0, 1.0);

    if part >= 1.0 { 1.0 } else { part * part * 0.8 }
}

/*
 * held through a Surface's `fit`, which reads what it asks for and then posts it. Besides the
 * runtime's derive, `fit` is called by a handler or a start, so two at once would post out of
 * order, leaving the body at the older ask until the next change
 */
fn fitting() -> MutexGuard<'static, ()> {
    static FIT: Mutex<()> = Mutex::new(());

    FIT.locked()
}

// posts what a Surface asks for, drawing nothing again if it asks what it did
fn ask(surface: Surface, fresh: f32, visit: Option<(u64, f32)>) {
    let mut island = IslandService::write();

    if !island.fit(surface, fresh, visit, Instant::now()) {
        island.quiet();
    }
}

/*
 * a Surface target pressed with the left button. Only the topmost target gets a click, so a right
 * click on it would otherwise never reach the island beneath: it pins the island there too
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

// a write wakes the window even when nothing changed, so only write a real change
fn store<S: Service + PartialEq>(next: S) {
    if *S::read() != next {
        *S::write() = next;
    }
}

// the keyboard focus, thick enough to see on any part
const RING: f32 = 2.0;

// the one focus mark every Surface draws, so the keyboard reads the same everywhere
trait Outline {
    fn border_if(self, ring: bool) -> Self;
}

impl Outline for Rectangle {
    fn border_if(self, ring: bool) -> Self {
        if ring {
            self.border(RING, theme::island().on_surface)
        } else {
            self
        }
    }
}
