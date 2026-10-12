//! The sizes of the Island's body, as the config, the output and the cluster's room make them.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use kanade_runtime::service::Service;
use kanade_runtime::{Monitor, Monitors};

use crate::cluster;
use crate::config::{self, Rest};
use crate::island::geometry::{self, Canvas, Hang, Holds, Shape, TimePeek};
use crate::island::presentation::Presentation;
use crate::island::service::IslandService;
use crate::modules;

use super::rest::{self, Agenda, has_battery};

fn size(monitor: &Monitor) -> (f32, f32) {
    (monitor.width as f32, monitor.height as f32)
}

// the largest body `monitor` holds
fn largest_on(monitor: &Monitor) -> Shape {
    geometry::fitted(config::get().largest.shape(), [size(monitor)].into_iter())
}

// the output whose Island is drawn (`On`), else where a Surface is open
fn drawn(monitors: &Monitors) -> Option<&Monitor> {
    let on = ON
        .with_borrow(Clone::clone)
        .or_else(|| IslandService::read().expanded_on().map(str::to_owned))?;

    monitors.all().iter().find(|monitor| monitor.name == on)
}

/*
 * the largest body, as `island.width` and `island.height` ask, made smaller where an output has no
 * room for it: the size the bodies, lists and canvas of that output follow (ADR 0030). The output
 * is the one whose Island is drawn, else where a Surface is open, else the least of them
 */
pub(crate) fn largest() -> Shape {
    let monitors = Monitors::read();

    match drawn(&monitors) {
        Some(monitor) => largest_on(monitor),
        None => geometry::fitted(
            config::get().largest.shape(),
            monitors.all().iter().map(size),
        ),
    }
}

/*
 * what the Island service sizes by: the largest body asked for with what Rest's Peek holds on any
 * output, and what each connected output holds of them
 */
pub(crate) fn holds() -> ((Shape, TimePeek), HashMap<String, Holds>) {
    let wanted = config::get().largest.shape();
    let fits: HashMap<String, Shape> = Monitors::read()
        .all()
        .iter()
        .map(|monitor| (monitor.name.clone(), largest_on(monitor)))
        .collect();

    rest::measure_agenda(&fits);

    let agenda = Agenda::read();
    let rooms: HashMap<String, Holds> = fits
        .into_iter()
        .map(|(name, largest)| {
            let peek = peek_of(config::on(&name).rest, agenda.on(&name));

            (name, Holds { largest, peek })
        })
        .collect();

    // what an output not named yet holds: the most any does, or the global setting with none
    let any = if rooms.is_empty() {
        peek_of(config::get().rest, 0)
    } else {
        rooms
            .values()
            .fold(TimePeek::default(), |any, room| TimePeek {
                battery: any.battery || room.peek.battery,
                weather: any.weather || room.peek.weather,
                agenda: any.agenda.max(room.peek.agenda),
                dots: any.dots.max(room.peek.dots),
            })
    };

    ((wanted, any), rooms)
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

// what Rest's Peek holds, as `rest` asks and the machine has, with an agenda line `agenda` wide
fn peek_of(rest: Rest, agenda: u16) -> TimePeek {
    TimePeek {
        battery: rest.peek_battery && has_battery(),
        weather: rest.weather && modules::on("weather"),
        agenda,
        dots: cluster::count(),
    }
}

/*
 * what the time's Peek holds on the output drawn, the room its Tray strip gives it; on none, the
 * most it holds on any
 */
pub(crate) fn peek() -> TimePeek {
    let monitors = Monitors::read();

    match drawn(&monitors) {
        Some(monitor) => peek_of(
            config::on(&monitor.name).rest,
            Agenda::read().on(&monitor.name),
        ),
        None => IslandService::read().peek(),
    }
}

// how many of a Tray strip's `slots` fit the output drawn
pub(crate) fn tray_slots(slots: u8) -> usize {
    geometry::tray_slots(slots, largest(), hang(), peek())
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
    config::get().island_placement().anchor.hang()
}

// along a side edge the small forms stand upright, their content stacked
pub(crate) fn upright() -> bool {
    hang().sideways()
}
