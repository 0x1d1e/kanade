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
pub mod controls;
mod grid;
pub mod keys;
pub mod launcher;
pub mod media;
pub mod notifications;
mod pad;
mod ring;
mod search;
pub mod session;
mod slider;
pub mod timer;
pub mod tray;
mod visit;
pub mod wallpapers;
pub mod weather;

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
