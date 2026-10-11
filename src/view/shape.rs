//! The sizes of the Island's body, as the config, the output and the cluster's room make them.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use kanade_runtime::service::Service;
use kanade_runtime::{Monitor, Monitors};

use crate::cluster;
use crate::config::{self, Rest};
use crate::island::geometry::{self, Canvas, Hang, Shape, TimePeek};
use crate::island::presentation::Presentation;
use crate::island::service::IslandService;
use crate::modules;

use super::rest::{Agenda, has_battery};

fn size(monitor: &Monitor) -> (f32, f32) {
    (monitor.width as f32, monitor.height as f32)
}

// the largest body `monitor` holds
fn largest_on(monitor: &Monitor) -> Shape {
    geometry::fitted(config::get().largest.shape(), [size(monitor)].into_iter())
}

/*
 * the largest body, as `island.width` and `island.height` ask, made smaller where an output has no
 * room for it: the size the bodies, lists and canvas of that output follow (ADR 0030). The output
 * is the one whose Island is drawn (`On`), else where a Surface is open, else the least of them
 */
pub(crate) fn largest() -> Shape {
    let on = ON
        .with_borrow(Clone::clone)
        .or_else(|| IslandService::read().expanded_on().map(str::to_owned));
    let monitors = Monitors::read();
    let named = on.and_then(|on| monitors.all().iter().find(|monitor| monitor.name == on));

    match named {
        Some(monitor) => largest_on(monitor),
        None => geometry::fitted(
            config::get().largest.shape(),
            monitors.all().iter().map(size),
        ),
    }
}

// the largest body asked for, and what each connected output holds of it, for the Island service
pub(crate) fn largest_of_each() -> (Shape, HashMap<String, Shape>) {
    let wanted = config::get().largest.shape();
    let fits = Monitors::read()
        .all()
        .iter()
        .map(|monitor| (monitor.name.clone(), largest_on(monitor)))
        .collect();

    (wanted, fits)
}

/*
 * the Island's canvas on `monitor`: around the largest body, no larger than the output, so it
 * resizes only with them (ADR 0030)
 */
pub(crate) fn canvas(monitor: &Monitor) -> Canvas {
    let canvas = Canvas::around(largest_on(monitor), hang());

    if monitor.width == 0 || monitor.height == 0 {
        return canvas;
    }

    canvas.within(monitor.width as f32, monitor.height as f32)
}

// what Rest and its Peek show on each connected output
pub(super) fn rests() -> Vec<Rest> {
    let monitors = Monitors::read();

    config::get().rests(monitors.all().iter().map(|monitor| monitor.name.as_str()))
}

/*
 * what the time's Peek holds on any output, as the settings ask and the machine has: the room the
 * Tray strip gives it
 */
pub(crate) fn peek() -> TimePeek {
    let battery = has_battery();

    let peek = rests()
        .into_iter()
        .fold(TimePeek::default(), |peek, rest| TimePeek {
            battery: peek.battery || rest.peek_battery && battery,
            weather: peek.weather || rest.weather && modules::on("weather"),
            agenda: 0,
        });

    TimePeek {
        agenda: Agenda::read().0,
        ..peek
    }
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
    // the output whose Island is being drawn
    static ON: RefCell<Option<String>> = const { RefCell::new(None) };

    // what the cluster takes from the small forms drawn now
    static ROOM: Cell<f32> = const { Cell::new(0.0) };
}

// the output whose Island is drawn while it is held, which `largest()` and `shape()` follow
pub(super) struct On(Option<String>);

impl On {
    pub(super) fn take(monitor: &Monitor) -> On {
        On(ON.replace(Some(monitor.name.clone())))
    }
}

impl Drop for On {
    fn drop(&mut self) {
        ON.set(self.0.take());
    }
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
