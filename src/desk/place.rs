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

use kanade_runtime::Point;
use kanade_runtime::service::Service;

use crate::config::{Corner, Spot};
use crate::island::geometry::Rect;
use crate::settings;

// from an output's edges to a face's box
pub const MARGIN: f32 = 48.0;

// the handle's room at a face's inner edge, and the bar drawn in it
pub const HANDLE: f32 = 24.0;
pub const BAR: f32 = 3.0;

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

    // where in the box the pointer took hold, and where the box is now
    grab: (f32, f32),
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
     * face is drawn in, and the box follows the pointer, kept on its output
     */
    fn drag(
        &mut self,
        monitor: &str,
        face: &'static str,
        drawn: Rect,
        stored: Option<Spot>,
        output: (f32, f32),
        point: Point,
    ) {
        let at = match self.moving(monitor) {
            Some(at) if at.face == face => at.clone(),
            _ => Moving {
                face,
                stored,
                from: drawn,
                held: true,
                grab: (point.x - drawn.x, point.y - drawn.y),
                to: drawn,
            },
        };
        let to = moved(at.from, at.grab, point, output);

        self.moving.insert(
            monitor.to_owned(),
            Moving {
                held: true,
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

// the box `from` once the pointer, holding it `grab` from its corner, is at `point`
pub fn moved(from: Rect, grab: (f32, f32), point: Point, output: (f32, f32)) -> Rect {
    on_output(
        Rect {
            x: point.x - grab.0,
            y: point.y - grab.1,
            ..from
        },
        output,
    )
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
    output: (f32, f32),
    point: Point,
) {
    Places::write().drag(monitor, face, drawn, stored, output, point);
}

/*
 * the pointer let go: the face stays where it was let go, and its place is kept in the config on a
 * worker. A face the pointer left where it stood writes nothing, and a write that fails puts the
 * face back and says why once
 */
pub fn release(monitor: &str) {
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

        places.drag(MONITOR, "clock", drawn, stored, OUTPUT, hold);
        places.drag(
            MONITOR,
            "clock",
            drawn,
            stored,
            OUTPUT,
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

    // the box follows the pointer from where it was taken hold of, kept whole on the output
    #[test]
    fn a_drag_carries_the_box_with_the_pointer_and_keeps_it_on_the_output() {
        let from = boxes(Corner::BottomRight)[0];
        let grab = (10.0, 10.0);

        let moved_on = moved(from, grab, Point { x: 900.0, y: 500.0 }, OUTPUT);

        assert_eq!((moved_on.x, moved_on.y), (890.0, 490.0));

        // past the output's own edges, the box stops with the margin still clear
        for (point, at) in [
            (
                Point {
                    x: -500.0,
                    y: -500.0,
                },
                (MARGIN, MARGIN),
            ),
            (
                Point {
                    x: 5000.0,
                    y: 5000.0,
                },
                (OUTPUT.0 - MARGIN - WIDTH, OUTPUT.1 - MARGIN - 150.0),
            ),
        ] {
            let held = moved(from, grab, point, OUTPUT);

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
            OUTPUT,
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
