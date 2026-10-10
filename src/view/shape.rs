//! The sizes of the Island's body, as the config, the output and the cluster's room make them.

use std::cell::Cell;

use kanade_runtime::service::Service;
use kanade_runtime::{Monitor, Monitors};

use crate::cluster;
use crate::config;
use crate::island::geometry::{self, Canvas, Hang, Shape, TimePeek};
use crate::island::presentation::Presentation;
use crate::modules;

use super::rest::has_battery;

/*
 * the largest body, as `island.width` and `island.height` ask, made smaller where an output has no
 * room for it: the one size every body, list and canvas follows (ADR 0030)
 */
pub(crate) fn largest() -> Shape {
    let monitors = Monitors::read();
    let outputs = monitors
        .all()
        .iter()
        .map(|monitor| (monitor.width as f32, monitor.height as f32));

    geometry::fitted(config::get().largest.shape(), outputs)
}

/*
 * the Island's canvas on `monitor`: around the largest body, no larger than the output, so it
 * resizes only with them (ADR 0030)
 */
pub(crate) fn canvas(monitor: &Monitor) -> Canvas {
    let canvas = Canvas::around(largest(), hang());

    if monitor.width == 0 || monitor.height == 0 {
        return canvas;
    }

    canvas.within(monitor.width as f32, monitor.height as f32)
}

/*
 * what the time's Peek holds on any output, as the settings ask and the machine has: the room the
 * Tray strip gives it
 */
pub(crate) fn peek() -> TimePeek {
    let battery = has_battery();

    config::get()
        .rests()
        .fold(TimePeek::default(), |peek, rest| TimePeek {
            battery: peek.battery || rest.peek_battery && battery,
            weather: peek.weather || rest.weather && modules::on("weather"),
            agenda: peek.agenda || rest.agenda && modules::on("calendar"),
        })
}

/*
 * where the Island's content lays out in this Presentation, at the largest body, as the Island
 * hangs: the body itself, less what the cluster takes from a small form's trailing end while
 * the form being drawn makes room for it
 */
pub(crate) fn shape(presentation: Presentation) -> Shape {
    let shape = geometry::shape(presentation, largest(), hang(), peek());
    let room = if cluster::beside(presentation) {
        Room::taken()
    } else {
        0.0
    };

    if upright() {
        Shape {
            height: shape.height - room,
            ..shape
        }
    } else {
        Shape {
            width: shape.width - room,
            ..shape
        }
    }
}

thread_local! {
    // what the cluster takes from the small forms drawn now
    static ROOM: Cell<f32> = const { Cell::new(0.0) };
}

// the room the cluster takes while it is held: the Island's own forms, not a HUD capsule's
pub(super) struct Room(f32);

impl Room {
    pub(super) fn take(room: f32) -> Room {
        Room(ROOM.replace(room))
    }

    pub(super) fn taken() -> f32 {
        ROOM.get()
    }
}

impl Drop for Room {
    fn drop(&mut self) {
        ROOM.set(self.0);
    }
}

// where the Island hangs, as `island.edge` and `island.align` say
pub(crate) fn hang() -> Hang {
    config::get().island_place.anchor.hang()
}

// along a side edge the small forms stand upright, their content stacked
pub(crate) fn upright() -> bool {
    hang().sideways()
}
