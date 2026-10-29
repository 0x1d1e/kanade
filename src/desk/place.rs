//! Where each Desk face stands on its output, and the drag that moves it (ADR 0143). A face the
//! config gives no place stands in the corner stack, one under the other from the corner
//! `desk.clock_corner` names, exactly as the Desk has always stood. A face given one stands there,
//! kept on the output.
//!
//! A face moves by the handle at its inner edge, never by its text or the media buttons: a press on
//! the handle takes hold of the face's box, every move after it carries the box with the pointer,
//! and the release keeps the place in the settings file, on a worker, since the draw thread waits on
//! no disk. Escape, or the keyboard leaving the window, puts the face back where it stood. Nothing
//! is written while the pointer is still down.
//!
//! The state is the `Places` Service, read by the Desk's windows and written by the handles' pointer
//! handlers; the worker that keeps a place clears it when the file refused it.

use std::collections::{BTreeMap, BTreeSet};
use std::thread;

use kanade_runtime::service::Service;
use kanade_runtime::{Monitors, Point};

use crate::config::{Corner, Spot};
use crate::island::geometry::Rect;
use crate::settings;

// from an output's edges to a face's box
pub const MARGIN: f32 = 48.0;

// the handle's room at a face's inner edge, and the bar drawn in it
pub const HANDLE: f32 = 24.0;
pub const BAR: f32 = 3.0;

// what a drag is kept within: the output, and the Island's area on it while it shows there
#[derive(Debug, Clone, Copy)]
pub struct Room {
    pub output: (f32, f32),
    pub island: Option<Rect>,
}

// one face being moved, or moved and just let go
#[derive(Debug, Clone, PartialEq)]
struct Moving {
    face: &'static str,

    // what the config held for the face when the press began, and the box it was drawn in then: a
    // place the config no longer gives is one no drag is about to keep
    stored: Option<Spot>,
    from: Rect,

    // whether the pointer is still down on it
    held: bool,

    // where the pointer last was on the handle, as the handle reports it, and where the box is now:
    // a move carries the box by how far that point has moved, never by where it is from the box
    last: Point,
    to: Rect,
}

// the faces a pointer is moving, one per output; a Service, so a move on the wayland thread draws
// the window that shows it, and a place the file refused puts the face back
#[derive(Debug, Default, PartialEq)]
pub struct Places {
    moving: BTreeMap<String, Moving>,

    // the outputs whose pointer is on a handle, so the window asks for the keyboard before the press
    // that takes hold, as the notes card does (ADR 0140)
    over: BTreeSet<String>,
}

impl Service for Places {
    fn new() -> Self {
        Places::default()
    }

    fn listen() {}
}

impl Places {
    fn moving(&self, monitor: &str) -> Option<&Moving> {
        self.moving.get(monitor)
    }

    /*
     * a press on a face's handle, and every move after it: the first call takes hold of the box the
     * face is drawn in, and each move after carries the box by how far the handle reports the
     * pointer has moved, kept on its output and out of the Island. A face let go is no drag to go
     * on with: the next press takes hold of the box the face is drawn in now
     */
    fn drag(
        &mut self,
        monitor: &str,
        face: &'static str,
        drawn: Rect,
        stored: Option<Spot>,
        room: Room,
        point: Point,
    ) {
        let held = self
            .moving(monitor)
            .filter(|at| at.face == face && at.held)
            .cloned();

        let Some(at) = held else {
            self.moving.insert(
                monitor.to_owned(),
                Moving {
                    face,
                    stored,
                    from: drawn,
                    held: true,
                    last: point,
                    to: drawn,
                },
            );

            return;
        };

        // the handle reports its point in its own space, so only its move since the last report
        // says how far the face travels, not where it stands in the box
        let delta = (point.x - at.last.x, point.y - at.last.y);
        let to = clear(shifted(at.to, delta, room.output), room.island, room.output);

        self.moving.insert(
            monitor.to_owned(),
            Moving {
                held: true,
                last: point,
                to,
                ..at
            },
        );
    }

    // the pointer let go: the face stays where it is, and a drag it started is over
    fn let_go(&mut self, monitor: &str) -> Option<(&'static str, Spot, Spot)> {
        let at = self.moving.get_mut(monitor)?;

        at.held = false;

        Some((at.face, kept(at.from), kept(at.to)))
    }

    // the face goes back to where it stood before the drag
    fn back(&mut self, monitor: &str) {
        self.moving.remove(monitor);
    }

    // the Desk window went away: its face goes with it, and no handle of it is hovered
    fn clear(&mut self, monitor: &str) {
        self.moving.remove(monitor);
        self.over.remove(monitor);
    }
}

/*
 * the corner stack: one box per face, in the order they are drawn, one under the other from the
 * corner's top. The whole stands `MARGIN` from the output's edges, as it always has
 */
pub fn stack(corner: Corner, heights: &[f32], output: (f32, f32), width: f32) -> Vec<Rect> {
    let total: f32 = heights.iter().sum();
    let x = edge(corner, output, width);
    let mut y = if corner.top() {
        MARGIN
    } else {
        output.1 - MARGIN - total
    };

    heights
        .iter()
        .map(|height| {
            let rect = Rect {
                x,
                y,
                width,
                height: *height,
            };

            y += height;

            rect
        })
        .collect()
}

// where a box of the Desk's width stands against the corner's side of the output
fn edge(corner: Corner, output: (f32, f32), width: f32) -> f32 {
    if corner.left() {
        MARGIN
    } else {
        (output.0 - MARGIN - width).max(MARGIN)
    }
}

/*
 * a box moved in until all of it lies on the output, a margin from its edges: what a drag may reach,
 * and what a place the config gives is kept to
 */
pub fn on_output(rect: Rect, output: (f32, f32)) -> Rect {
    let keep = |at: f32, size: f32, room: f32| match size + 2.0 * MARGIN <= room {
        true => at.clamp(MARGIN, room - MARGIN - size),
        // a face too big for the margin still starts at it, as the stack does
        false => MARGIN,
    };

    Rect {
        x: keep(rect.x, rect.width, output.0),
        y: keep(rect.y, rect.height, output.1),
        ..rect
    }
}

// the box `from` moved by `delta`, kept whole on the output
pub fn shifted(from: Rect, delta: (f32, f32), output: (f32, f32)) -> Rect {
    on_output(
        Rect {
            x: from.x + delta.0,
            y: from.y + delta.1,
            ..from
        },
        output,
    )
}

/*
 * `rect` kept out of the Island's `area`, when it would lie over it: the box moves the shortest way
 * to one of the Island's sides, and stays on the output. With no side free, as an Island grown past
 * the whole output, it stays where the pointer carries it
 */
fn clear(rect: Rect, area: Option<Rect>, output: (f32, f32)) -> Rect {
    let Some(area) = area.filter(|area| over(rect, *area)) else {
        return rect;
    };

    let ways = [
        Rect {
            x: area.x - rect.width,
            ..rect
        },
        Rect {
            x: area.right(),
            ..rect
        },
        Rect {
            y: area.y - rect.height,
            ..rect
        },
        Rect {
            y: area.bottom(),
            ..rect
        },
    ];

    let mut best: Option<(f32, Rect)> = None;

    for way in ways {
        let distance = (way.x - rect.x).abs() + (way.y - rect.y).abs();
        let kept = on_output(way, output);

        if over(kept, area) || best.is_some_and(|(least, _)| least <= distance) {
            continue;
        }

        best = Some((distance, kept));
    }

    best.map_or(rect, |(_, kept)| kept)
}

// whether two boxes share any part of the output
fn over(a: Rect, b: Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

/*
 * where one face is drawn: where a drag carries it, else where the config puts it, else the corner
 * stack's place. A drag's own place is drawn until the config gives another, so a face the file kept
 * stays where it was let go
 */
pub fn at(
    places: &Places,
    monitor: &str,
    face: &str,
    stored: Option<Spot>,
    stack: Rect,
    output: (f32, f32),
) -> Rect {
    match places.moving(monitor) {
        Some(at) if at.face == face && at.stored == stored => at.to,
        _ => match stored {
            Some(spot) => on_output(
                Rect {
                    x: spot.x as f32,
                    y: spot.y as f32,
                    ..stack
                },
                output,
            ),
            None => stack,
        },
    }
}

// the handle at a face's inner edge, in the window's own space: the side away from the corner, where
// the face draws nothing
pub fn handle(rect: Rect, corner: Corner) -> Rect {
    let (width, height) = (HANDLE.min(rect.width), HANDLE.min(rect.height));

    Rect {
        x: match corner.left() {
            true => rect.x + rect.width - width,
            false => rect.x,
        },
        y: rect.y + (rect.height - height) / 2.0,
        width,
        height,
    }
}

// a press on a handle, and every move after it, on this output
pub fn drag(
    monitor: &str,
    face: &'static str,
    drawn: Rect,
    stored: Option<Spot>,
    room: Room,
    point: Point,
) {
    Places::write().drag(monitor, face, drawn, stored, room, point);
}

/*
 * the pointer let go: the face stays where it was let go, and its place is kept in the config on a
 * worker. A face the pointer left where it stood writes nothing, and a write that fails puts the
 * face back and says why once. A drag that ended otherwise, as the pointer left the window or the
 * target went away, and one on an output that is gone, writes nothing: the face goes back
 */
pub fn release(monitor: &str, released: bool) {
    if !released || !Monitors::read().has(monitor) {
        Places::write().back(monitor);

        return;
    }

    let Some((face, stood, spot)) = Places::write().let_go(monitor) else {
        return;
    };

    if stood == spot {
        Places::write().back(monitor);

        return;
    }

    let name = monitor.to_owned();

    thread::spawn(move || {
        let path = ["output", &name, "desk", "place", face];

        if let Err(why) = settings::set(&path, Some(spot.value())) {
            eprintln!("kanade: the {face} on {name} stays where it was: {why}");

            Places::write().back(&name);
        }
    });
}

/*
 * the Desk window went away, hidden or off: a face being moved goes with it and a hovered handle is
 * no longer hovered. The write runs on a worker, since a view writes no Service, and only when
 * there is something to clear
 */
pub fn hide(monitor: &str) {
    {
        let places = Places::read();

        if places.moving(monitor).is_none() && !places.over.contains(monitor) {
            return;
        }
    }

    let name = monitor.to_owned();

    thread::spawn(move || Places::write().clear(&name));
}

// a box's corner as the config keeps it, in whole px
fn kept(rect: Rect) -> Spot {
    Spot::new(rect.x.round() as i32, rect.y.round() as i32)
}

// the face goes back to where it stood before the drag: Escape, or the keyboard leaving the window
pub fn back(monitor: &str) {
    Places::write().back(monitor);
}

// whether the pointer is on a handle or holding one, so the window asks for the keyboard, for Escape
// alone, before the press that would otherwise focus nothing
pub fn keyboard(monitor: &str) -> bool {
    let places = Places::read();

    places.over.contains(monitor) || places.moving(monitor).is_some_and(|at| at.held)
}

// whether a face of this output is under the pointer's hold, which is the only drag Escape cancels
pub fn dragging(monitor: &str) -> bool {
    Places::read().moving(monitor).is_some_and(|at| at.held)
}

// the pointer came on a handle of this output, or left it
pub fn hover(monitor: &str, inside: bool) {
    let mut places = Places::write();
    let changed = match inside {
        true => places.over.insert(monitor.to_owned()),
        false => places.over.remove(monitor),
    };

    if !changed {
        places.quiet();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUTPUT: (f32, f32) = (1920.0, 1080.0);
    const WIDTH: f32 = 420.0;
    const MONITOR: &str = "eDP-1";
    const OTHER: &str = "HDMI-A-1";

    fn room(island: Option<Rect>) -> Room {
        Room {
            output: OUTPUT,
            island,
        }
    }

    fn boxes(corner: Corner) -> Vec<Rect> {
        stack(corner, &[150.0, 28.0, 28.0, 140.0, 56.0], OUTPUT, WIDTH)
    }

    // a press on the clock's handle, then a move to `point`
    fn drag_to(places: &mut Places, stored: Option<Spot>, point: (f32, f32)) {
        let drawn = at(
            places,
            MONITOR,
            "clock",
            stored,
            boxes(Corner::BottomRight)[0],
            OUTPUT,
        );
        let hold = Point {
            x: drawn.x + 10.0,
            y: drawn.y + 10.0,
        };

        places.drag(MONITOR, "clock", drawn, stored, room(None), hold);
        places.drag(
            MONITOR,
            "clock",
            drawn,
            stored,
            room(None),
            Point {
                x: point.0,
                y: point.1,
            },
        );
    }

    // the Desk as it has always stood: the faces one under the other from the corner's top, the
    // whole the margin from the edges
    #[test]
    fn the_stack_is_where_it_has_always_been() {
        let down = boxes(Corner::BottomRight);

        assert_eq!(down[0].x, OUTPUT.0 - MARGIN - WIDTH);
        assert_eq!(down[0].y, OUTPUT.1 - MARGIN - 402.0);
        assert_eq!(down[1].y, down[0].y + 150.0);
        assert_eq!(down[4].y + down[4].height, OUTPUT.1 - MARGIN);

        let up = boxes(Corner::TopLeft);

        assert_eq!(up[0].x, MARGIN);
        assert_eq!(up[0].y, MARGIN);
        assert_eq!(up[4].y + up[4].height, MARGIN + 402.0);

        // an output too narrow for the margin keeps the box at it rather than off its edge
        assert_eq!(
            stack(Corner::BottomRight, &[1.0], (WIDTH, 800.0), WIDTH)[0].x,
            MARGIN
        );
    }

    // a face with no place stands in the stack; one with a place stands there, moved in only when it
    // would hang over an edge
    #[test]
    fn a_place_from_the_config_stands_where_it_says_and_no_further() {
        let places = Places::default();
        let stack = boxes(Corner::BottomRight)[0];

        assert_eq!(at(&places, MONITOR, "clock", None, stack, OUTPUT), stack);

        let placed = at(
            &places,
            MONITOR,
            "clock",
            Some(Spot::new(100, 200)),
            stack,
            OUTPUT,
        );

        assert_eq!((placed.x, placed.y), (100.0, 200.0));

        // the far corner of the output, part of it over the edge
        let kept = at(
            &places,
            MONITOR,
            "clock",
            Some(Spot::new(3000, 3000)),
            stack,
            OUTPUT,
        );

        assert_eq!(
            (kept.x, kept.y),
            (OUTPUT.0 - MARGIN - WIDTH, OUTPUT.1 - MARGIN - 150.0)
        );

        // and never nearer an edge than the margin
        let near = at(
            &places,
            MONITOR,
            "clock",
            Some(Spot::new(-40, -40)),
            stack,
            OUTPUT,
        );

        assert_eq!((near.x, near.y), (MARGIN, MARGIN));
    }

    // a box carried by the pointer's own move, kept whole on the output
    #[test]
    fn a_drag_carries_the_box_with_the_pointer_and_keeps_it_on_the_output() {
        let from = boxes(Corner::BottomRight)[0];

        let there = shifted(from, (-40.0, -30.0), OUTPUT);

        assert_eq!((there.x, there.y), (from.x - 40.0, from.y - 30.0));

        // past the output's own edges, the box stops with the margin still clear
        for (delta, at) in [
            ((-5000.0, -5000.0), (MARGIN, MARGIN)),
            (
                (5000.0, 5000.0),
                (OUTPUT.0 - MARGIN - WIDTH, OUTPUT.1 - MARGIN - 150.0),
            ),
        ] {
            let held = shifted(from, delta, OUTPUT);

            assert_eq!((held.x, held.y), at);
        }
    }

    // a drag draws its own place, and only its own: the other faces, the other outputs and a place
    // the config gives all stand where they say
    #[test]
    fn a_drags_place_is_drawn_until_the_config_gives_another() {
        let mut places = Places::default();
        let stack = boxes(Corner::BottomRight)[0];

        drag_to(&mut places, None, (700.0, 400.0));

        let dragged = at(&places, MONITOR, "clock", None, stack, OUTPUT);

        assert_eq!((dragged.x, dragged.y), (690.0, 390.0));

        assert_eq!(at(&places, OTHER, "clock", None, stack, OUTPUT), stack);
        assert_eq!(at(&places, MONITOR, "agenda", None, stack, OUTPUT), stack);

        // the config keeps another place, as the worker writing it does: the drag is no longer about
        // this face, and the config's place is drawn
        assert_eq!(
            at(
                &places,
                MONITOR,
                "clock",
                Some(Spot::new(60, 60)),
                stack,
                OUTPUT
            )
            .y,
            60.0
        );

        // Escape, or the window losing the keyboard, puts the face back
        places.back(MONITOR);

        assert_eq!(places.moving(MONITOR), None);
        assert_eq!(at(&places, MONITOR, "clock", None, stack, OUTPUT), stack);
    }

    // a drag that would end over the Island stops against its nearest side; one that starts and
    // stays clear of it is not touched, and a place the config already gives stays where it says
    #[test]
    fn a_drag_keeps_out_of_the_islands_area() {
        let mut places = Places::default();
        let stack = boxes(Corner::BottomRight)[0];
        let island = Rect {
            x: 700.0,
            y: 8.0,
            width: 520.0,
            height: 60.0,
        };
        let hold = Point {
            x: stack.x + 10.0,
            y: stack.y + 10.0,
        };
        // the pointer carries the box's top left to here, which lies over the Island
        let under = Point { x: 800.0, y: 40.0 };

        places.drag(MONITOR, "clock", stack, None, room(Some(island)), hold);
        places.drag(MONITOR, "clock", stack, None, room(Some(island)), under);

        let dragged = at(&places, MONITOR, "clock", None, stack, OUTPUT);

        // the shortest way out is under the Island, its top against the Island's bottom
        assert_eq!((dragged.x, dragged.y), (790.0, island.bottom()));
        assert!(!over(dragged, island));

        // carried clear of it, the box stands where the pointer is
        places.drag(
            MONITOR,
            "clock",
            stack,
            None,
            room(Some(island)),
            Point { x: 400.0, y: 500.0 },
        );

        let free = at(&places, MONITOR, "clock", None, stack, OUTPUT);

        assert_eq!((free.x, free.y), (390.0, 528.0));

        // with no Island there, nor is there a side to keep out of
        assert_eq!(clear(dragged, None, OUTPUT), dragged);

        // a place the config gives is drawn as it is, even under the Island
        let placed = at(
            &Places::default(),
            MONITOR,
            "clock",
            Some(Spot::new(800, 50)),
            stack,
            OUTPUT,
        );

        assert_eq!((placed.x, placed.y), (800.0, 50.0));
    }

    // a release is only a place to keep when the pointer carried the face somewhere else
    #[test]
    fn a_place_is_kept_only_once_the_face_has_moved() {
        let mut places = Places::default();
        let stack = boxes(Corner::BottomRight)[0];

        // a press on the handle that moves nothing, as a click on it does
        places.drag(
            MONITOR,
            "clock",
            stack,
            None,
            room(None),
            Point {
                x: stack.x + 10.0,
                y: stack.y + 10.0,
            },
        );

        assert_eq!(
            kept(at(&places, MONITOR, "clock", None, stack, OUTPUT)),
            kept(stack)
        );

        drag_to(&mut places, None, (700.0, 400.0));

        let spot = kept(at(&places, MONITOR, "clock", None, stack, OUTPUT));

        assert_eq!(spot, Spot::new(690, 390));
        assert_ne!(spot, kept(stack));
    }

    // each report carries the box by how far the handle says the pointer moved, never by where the
    // pointer stands in the box, and a press after a release takes hold where the face now stands
    #[test]
    fn a_move_carries_the_box_by_the_pointers_own_move() {
        let mut places = Places::default();
        let stack = boxes(Corner::BottomRight)[0];

        places.drag(
            MONITOR,
            "clock",
            stack,
            None,
            room(None),
            Point { x: 34.0, y: 10.0 },
        );
        places.drag(
            MONITOR,
            "clock",
            stack,
            None,
            room(None),
            Point { x: 22.0, y: 10.0 },
        );

        let first = at(&places, MONITOR, "clock", None, stack, OUTPUT);

        places.drag(
            MONITOR,
            "clock",
            stack,
            None,
            room(None),
            Point { x: 10.0, y: 10.0 },
        );

        let second = at(&places, MONITOR, "clock", None, stack, OUTPUT);

        assert_eq!((first.x, first.y), (stack.x - 12.0, stack.y));
        assert_eq!((second.x, second.y), (stack.x - 24.0, stack.y));

        // the face is let go where it stands; the next press takes hold of that box, not the press
        // before it, so nothing jumps back
        places.let_go(MONITOR);

        let drawn = at(&places, MONITOR, "clock", None, stack, OUTPUT);

        places.drag(
            MONITOR,
            "clock",
            drawn,
            None,
            room(None),
            Point { x: 5.0, y: 5.0 },
        );

        assert_eq!(at(&places, MONITOR, "clock", None, stack, OUTPUT), drawn);
    }

    // a handle sits at the face's inner edge, inside its box, on the side the face draws nothing
    #[test]
    fn the_handle_is_inside_the_box_away_from_the_corner() {
        let down = boxes(Corner::BottomRight)[0];
        let at = handle(down, Corner::BottomRight);

        assert_eq!((at.x, at.width), (down.x, HANDLE));
        assert!(at.y > down.y && at.y + at.height < down.y + down.height);

        let up = handle(down, Corner::TopLeft);

        assert_eq!(up.x + up.width, down.x + WIDTH);
        assert_eq!(up.width, HANDLE);
    }
}
