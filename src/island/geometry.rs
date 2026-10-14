//! Where the island body sits inside its canvas, and which part of the canvas takes the pointer.
//! The window resizes only with the largest body or the output (ADR 0030), never in a morph: only
//! the body moves inside it. The canvas hangs from an output's top or bottom edge, at its left,
//! middle or right, or from its left or right edge, in the middle (`Hang`), and the body sits
//! against that edge and side of it.

use std::collections::HashMap;

use super::arbiter::SATELLITES;
use super::presentation::{Presentation, Segment, Surface};

/*
 * the layer window around the largest body: room on both sides for its shadow and for a bouncy
 * spring to carry it past its target and back (`bounded`), the same at every edge. The room is what
 * the privacy cluster's pill once needed (ADR 0033), kept so no canvas, merge or Satellite moves
 */
pub const ROOM_WIDTH: f32 = 2.0 * ROOM;
const ROOM_HEIGHT: f32 = ROOM_WIDTH;
const ROOM: f32 = 86.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Canvas {
    pub width: f32,
    pub height: f32,
}

impl Canvas {
    /*
     * along a side edge also room above and below the middle for the longest upright small form,
     * its Satellites below it and the gap to the canvas's end
     */
    pub const fn around(largest: Shape, hang: Hang) -> Self {
        let height = largest.height + ROOM_HEIGHT;
        let upright = UPRIGHT_TRAY.height + 2.0 * (SATELLITES_REACH + TOP);

        Canvas {
            width: largest.width + ROOM_WIDTH,
            height: match hang.edge {
                Edge::Left | Edge::Right if upright > height => upright,
                _ => height,
            },
        }
    }

    // no larger than an output this size, in whole pixels
    pub fn within(self, width: f32, height: f32) -> Self {
        Canvas {
            width: self.width.min(width).floor(),
            height: self.height.min(height).floor(),
        }
    }
}

// gap between the edge of the screen the body hangs from and the body, and beside it at a side
pub const TOP: f32 = 8.0;

// where along a top or bottom edge the canvas and the body in it sit
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Side {
    Start,
    #[default]
    Middle,
    End,
}

// the edge of the output the canvas hangs from
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Edge {
    #[default]
    Top,
    Bottom,
    Left,
    Right,
}

// which edge and where along it; at the left or right the side is the middle. The default hangs
// from the top, in the middle
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Hang {
    pub edge: Edge,
    pub side: Side,
}

impl Hang {
    // where across the canvas the body sits: at its edge's side, else as the side says
    pub fn across(self) -> Side {
        match self.edge {
            Edge::Left => Side::Start,
            Edge::Right => Side::End,
            Edge::Top | Edge::Bottom => self.side,
        }
    }

    // from a left or right edge, so it slides and reserves sideways
    pub fn sideways(self) -> bool {
        matches!(self.edge, Edge::Left | Edge::Right)
    }
}

/*
 * a small form as it hangs: along a left or right edge it stands upright, as long as it is wide
 * across the top or bottom, its content stacked. A Peek, which carries a line of text, and the
 * Surfaces stay as they are, growing out of the edge
 */
pub const fn upright(shape: Shape, hang: Hang) -> Shape {
    match hang.edge {
        Edge::Left | Edge::Right => Shape {
            width: shape.height,
            height: shape.width,
            radius: shape.radius,
        },
        Edge::Top | Edge::Bottom => shape,
    }
}

// the pointer counts as on the body this far outside it, so grazing an edge does not flicker
pub const HOVER_PADDING: f32 = 8.0;

// starting values; the small forms are pills, radius half the height
pub const REST: Shape = Shape {
    width: 150.0,
    height: 32.0,
    radius: 16.0,
};

pub const COMPACT: Shape = Shape {
    width: 220.0,
    height: 38.0,
    radius: 19.0,
};

/*
 * Compact and the top Satellite's segment after it, as wide as a Peek, so a Peek out of it only
 * grows down and the Satellites beside it stay where they are
 */
pub const SPLIT: Shape = Shape {
    width: 300.0,
    height: 38.0,
    radius: 19.0,
};

// the trailing segment, inset in the body's round end like a Satellite grown into it
pub const SEGMENT_INSET: f32 = 4.0;

pub const PEEK: Shape = Shape {
    width: 300.0,
    height: 52.0,
    radius: 26.0,
};

/*
 * the Tray strip: the time's Peek, the time over a line with the date, then a slot for each tray
 * item, as tall as a Peek for the two lines. Past `TRAY_SLOTS` items the last slot opens the Tray
 * Surface with them all
 */
pub const TRAY_SLOTS: usize = 6;
pub const TRAY_SLOT: f32 = 32.0;

// the time's Peek from the round end, and the room after the last slot
pub const TIME_INSET: f32 = 24.0;
pub const TRAY_END: f32 = 16.0;

/*
 * the privacy cluster's dots (ADR 0033): each one's size and the gap between, and from the body's
 * trailing end to the last dot and from the first dot to what the form draws
 */
pub const DOT: f32 = 8.0;
pub const DOT_GAP: f32 = 5.0;
pub const DOT_INSET: f32 = 12.0;
pub const DOT_CLEAR: f32 = 10.0;

// what Rest keeps clear of its content at each end, before its battery is dropped to fit
pub const REST_MARGIN: f32 = 6.0;

// the most dots at once: a microphone, a camera and a screen cast
pub const DOTS: u8 = 3;

// what `dots` dots take from a body's trailing end, none for none
pub const fn dots_room(dots: u8) -> f32 {
    if dots == 0 {
        0.0
    } else {
        DOT_INSET + DOT_CLEAR + dots as f32 * DOT + (dots - 1) as f32 * DOT_GAP
    }
}

// from the last slot to a strip's end: the slots' trailing space, or the dots' room if that is more
const fn tray_end(slots: usize, dots: u8) -> f32 {
    let end = if slots == 0 { TIME_INSET } else { TRAY_END };
    let room = dots_room(dots);

    if room > end { room } else { end }
}

// between the time's Peek and the first slot
pub const TIME_GAP: f32 = 12.0;

/*
 * what the time's Peek holds beside the clock and the date, as the settings ask and the day has: it
 * sizes the Tray strip's lead to its content, so no room is left between it and the slots. The
 * strip's end holds the privacy cluster's dots, as many as capture
 */
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TimePeek {
    pub battery: bool,
    pub weather: bool,

    // the width in pixels of today's next event, 0 for none or for the setting off
    pub agenda: u16,

    // the dots at the strip's end
    pub dots: u8,
}

impl TimePeek {
    pub const ALL: TimePeek = TimePeek {
        battery: true,
        weather: true,
        agenda: AGENDA_MOST,
        dots: DOTS,
    };

    // across the top: the widest of its two lines
    pub const fn width(self) -> f32 {
        let clock = CLOCK + if self.battery { BATTERY } else { 0.0 };
        let date = DATE
            + if self.weather { WEATHER } else { 0.0 }
            + if self.agenda > 0 {
                LINE_GAP + self.agenda as f32
            } else {
                0.0
            };

        if clock > date { clock } else { date }
    }

    // along a side edge: the stacked time, then what is set
    pub const fn height(self) -> f32 {
        UPRIGHT_CLOCK
            + if self.weather { UPRIGHT_WEATHER } else { 0.0 }
            + if self.battery { UPRIGHT_BATTERY } else { 0.0 }
    }
}

/*
 * the lines of the time's Peek, the widest they lay out, as a view test holds: the clock, and the
 * battery after it; the date, the weather after it and the next event's line, each with the gap
 * before it
 */
pub const HEAD_GAP: f32 = 10.0;

// the bolt before the battery's percent while it charges, and the gap after it
pub const BOLT: f32 = 12.0;
pub const LINE_GAP: f32 = 8.0;
pub const CLOCK: f32 = 57.0;
pub const READOUT: f32 = 55.0;
pub const BATTERY: f32 = HEAD_GAP + BOLT + 4.0 + READOUT;
pub const DATE: f32 = 70.0;
pub const WEATHER: f32 = LINE_GAP + 41.0;

/*
 * the most room the next event's line takes in a Tray strip no wider than a body `body` wide, with
 * every slot and the rest of the time's Peek: none where even those do not fit
 */
pub const fn agenda_most(body: f32) -> u16 {
    let room = body
        - TIME_INSET
        - TIME_GAP
        - TRAY_SLOTS as f32 * TRAY_SLOT
        - tray_end(TRAY_SLOTS, DOTS)
        - DATE
        - WEATHER
        - LINE_GAP;

    if room > 0.0 { room as u16 } else { 0 }
}

// at the least the largest body may be, so the widest strip fits any
pub const AGENDA_MOST: u16 = agenda_most(CALENDAR.width);

pub const UPRIGHT_CLOCK: f32 = 64.0;
pub const UPRIGHT_WEATHER: f32 = 34.0;
pub const UPRIGHT_BATTERY: f32 = 44.0 + BOLT + 3.0;

const fn tray(slots: usize, peek: TimePeek) -> Shape {
    // with no item the strip ends as it began, round
    let after = if slots == 0 {
        tray_end(0, peek.dots)
    } else {
        TIME_GAP + slots as f32 * TRAY_SLOT + tray_end(slots, peek.dots)
    };

    Shape {
        width: TIME_INSET + peek.width() + after,
        height: PEEK.height,
        radius: PEEK.radius,
    }
}

// the widest, with everything the time's Peek can hold and every slot
pub const TRAY: Shape = tray(TRAY_SLOTS, TimePeek::ALL);

const _: () = assert!(TRAY.width <= CALENDAR.width);

/*
 * the Tray strip along a side edge: the hours over the minutes, the weather and the battery, then
 * the slots down the edge, a slot wider than Compact stands
 */
pub const UPRIGHT_TRAY_WIDTH: f32 = 44.0;

const fn upright_tray(slots: usize, peek: TimePeek) -> Shape {
    Shape {
        width: UPRIGHT_TRAY_WIDTH,
        height: peek.height() + slots as f32 * TRAY_SLOT + tray_end(slots, peek.dots),
        radius: UPRIGHT_TRAY_WIDTH / 2.0,
    }
}

pub const UPRIGHT_TRAY: Shape = upright_tray(TRAY_SLOTS, TimePeek::ALL);

pub const CONTROLS: Shape = Shape {
    width: 440.0,
    height: 330.0,
    radius: 32.0,
};

// the Tray Surface's body: items and menus are short lines, so it is narrower than Controls
pub const TRAY_LIST: Shape = Shape {
    width: 320.0,
    height: CONTROLS.height,
    radius: CONTROLS.radius,
};

// as wide as Controls, so the two Surfaces line up when one replaces the other
pub const MEDIA: Shape = Shape {
    width: 440.0,
    height: 216.0,
    radius: 32.0,
};

// the Calendar's body, and the least the largest body may be, so every Surface fits it
pub const CALENDAR: Shape = Shape {
    width: 520.0,
    height: 330.0,
    radius: 32.0,
};

// the largest body, `island.width` by `island.height`, which the Surfaces with a list grow to
pub const fn largest(width: f32, height: f32) -> Shape {
    Shape {
        width,
        height,
        radius: CALENDAR.radius,
    }
}

// as large as the Calendar, a little taller for a list
pub const DEFAULT_LARGEST: Shape = largest(520.0, 400.0);

/*
 * how large each body grows: the largest, which the canvas is around, and how tall the content of
 * each Surface with a list asks its body to be, up to that (ADR 0030)
 */
#[derive(Debug, Clone, PartialEq)]
pub struct Sizes {
    // the largest body asked for, which an output not in `fits` holds whole
    pub largest: Shape,

    // what each output holds, by name: the largest body, smaller where it has no room for `largest`,
    // and what its own time's Peek has
    pub fits: HashMap<String, Holds>,

    // where the Island hangs, which stands its small forms upright along a side edge
    pub hang: Hang,

    // what the time's Peek holds on an output not in `fits`, which sizes its Tray strip
    pub peek: TimePeek,
    asks: Vec<(Surface, Asks)>,
}

// a list Surface's content height as a new visit opens it, and in the visit open now
#[derive(Debug, Clone, Copy, PartialEq)]
struct Asks {
    fresh: f32,
    visit: Option<(u64, f32)>,
}

impl Default for Sizes {
    fn default() -> Self {
        Sizes {
            largest: DEFAULT_LARGEST,
            fits: HashMap::new(),
            hang: Hang::default(),
            peek: TimePeek::ALL,
            asks: Vec::new(),
        }
    }
}

// what one output's Island is sized by
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Holds {
    // the largest body, which its canvas is around
    pub largest: Shape,

    // what its time's Peek holds, which sizes its Tray strip
    pub peek: TimePeek,
}

impl Sizes {
    // what this output's Island is sized by
    pub fn holds(&self, monitor: &str) -> Holds {
        self.fits.get(monitor).copied().unwrap_or(Holds {
            largest: self.largest,
            peek: self.peek,
        })
    }

    // the largest body on this output, which its Island's canvas is around
    pub fn largest_on(&self, monitor: &str) -> Shape {
        self.holds(monitor).largest
    }

    // what `surface`'s content asks now; false if it asked that already
    pub fn ask(&mut self, surface: Surface, fresh: f32, visit: Option<(u64, f32)>) -> bool {
        let asks = Asks { fresh, visit };

        match self.asks.iter_mut().find(|(each, _)| *each == surface) {
            Some((_, old)) if *old == asks => false,
            Some((_, old)) => {
                *old = asks;
                true
            }
            None => {
                self.asks.push((surface, asks));
                true
            }
        }
    }

    // where a body in this Presentation morphs to in `visit`, within the room of its output; a list
    // Surface's as its content asks
    pub fn target(&self, presentation: Presentation, visit: u64, holds: Holds) -> Shape {
        let shape = shape(presentation, holds.largest, self.hang, holds.peek);
        let Presentation::Expanded(surface) = presentation else {
            return shape;
        };

        let Some((_, asks)) = self.asks.iter().find(|(each, _)| *each == surface) else {
            return shape;
        };

        let asks = match asks.visit {
            Some((asked, height)) if asked == visit => height,
            _ => asks.fresh,
        };

        Shape {
            height: asks.clamp(PEEK.height, shape.height).round(),
            ..shape
        }
    }
}

// every shape a body morphs between; a bouncy spring overshoots them, `bounded` keeps it in
pub const SHAPES: [Shape; 10] = [
    REST,
    COMPACT,
    SPLIT,
    PEEK,
    TRAY,
    UPRIGHT_TRAY,
    CONTROLS,
    TRAY_LIST,
    MEDIA,
    CALENDAR,
];

// a Satellite's diameter, and the gap before each one
pub const SATELLITE: f32 = 28.0;
pub const SATELLITE_GAP: f32 = 6.0;

// how far past the body's end the Satellites reach, the overflow's "+N" included
const SATELLITES_REACH: f32 = (SATELLITES + 1) as f32 * (SATELLITE + SATELLITE_GAP);

/*
 * how many of `slots` a Tray strip shows in a body no larger than `largest`: all, fewer on an
 * output too small for them, one at the least; the last then says how many more
 */
pub fn tray_slots(slots: u8, largest: Shape, hang: Hang, peek: TimePeek) -> usize {
    let fits = |count: usize| {
        if hang.sideways() {
            upright_tray(count, peek).height <= largest.height
        } else {
            tray(count, peek).width <= largest.width
        }
    };
    let slots = usize::from(slots);

    (1..=slots)
        .rev()
        .find(|&count| fits(count))
        .unwrap_or(slots.min(1))
}

/*
 * where the body of an island in this Presentation morphs to, hanging so, the lists' as large as
 * `largest`
 */
pub fn shape(presentation: Presentation, largest: Shape, hang: Hang, peek: TimePeek) -> Shape {
    match presentation {
        Presentation::Rest => upright(REST, hang),
        Presentation::Compact => upright(COMPACT, hang),
        Presentation::Split => upright(SPLIT, hang),
        Presentation::Peek => PEEK,
        Presentation::Tray(slots) => {
            let slots = tray_slots(slots, largest, hang, peek);

            if hang.sideways() {
                upright_tray(slots, peek)
            } else {
                tray(slots, peek)
            }
        }
        Presentation::Expanded(
            Surface::Controls
            | Surface::Weather
            | Surface::Session
            | Surface::Timer
            | Surface::Capture,
        ) => CONTROLS,
        Presentation::Expanded(Surface::Tray) => TRAY_LIST,
        Presentation::Expanded(Surface::Media) => MEDIA,
        Presentation::Expanded(Surface::Calendar) => CALENDAR,
        Presentation::Expanded(
            Surface::Notifications
            | Surface::Launcher
            | Surface::Clipboard
            | Surface::Wallpapers
            | Surface::Keys,
        ) => largest,
    }
}

// the body's size and corner radius, animated as one value so they always move together
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shape {
    pub width: f32,
    pub height: f32,
    pub radius: f32,
}

// as one spring group, see motion
impl From<Shape> for [f32; 3] {
    fn from(shape: Shape) -> Self {
        [shape.width, shape.height, shape.radius]
    }
}

impl From<[f32; 3]> for Shape {
    fn from([width, height, radius]: [f32; 3]) -> Self {
        Self {
            width,
            height,
            radius,
        }
    }
}

// in canvas coordinates, from its top left corner
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn right(self) -> f32 {
        self.x + self.width
    }

    pub fn bottom(self) -> f32 {
        self.y + self.height
    }

    pub fn holds(self, x: f32, y: f32) -> bool {
        (self.x..self.right()).contains(&x) && (self.y..self.bottom()).contains(&y)
    }

    // the least rectangle around both
    pub fn around(self, other: Rect) -> Self {
        Self::from_edges(
            self.x.min(other.x),
            self.y.min(other.y),
            self.right().max(other.right()),
            self.bottom().max(other.bottom()),
        )
    }

    fn from_edges(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            x: left,
            y: top,
            width: right - left,
            height: bottom - top,
        }
    }
}

/*
 * a shape a bouncy spring carries past its target kept a body: never so large it leaves the canvas
 * with its padding, never squeezed thinner than three quarters of Rest, its corners never rounder
 * than a pill
 */
pub fn bounded(shape: Shape, canvas: Canvas) -> Shape {
    // the least wins on a canvas too small for both, which a tiny output may give
    let width = shape
        .width
        .min(canvas.width - 2.0 * (TOP + HOVER_PADDING))
        .max(REST.height);
    let height = shape
        .height
        .min(canvas.height - 2.0 * (TOP + HOVER_PADDING))
        .max(REST.height * 0.75);

    Shape {
        width,
        height,
        radius: shape.radius.clamp(0.0, width.min(height) / 2.0),
    }
}

/*
 * the largest body every output, `width` by `height`, holds in the canvas it leaves: the one
 * asked for, smaller where an output is too small for it. Outputs of no size yet hold any
 */
pub fn fitted(largest: Shape, outputs: impl Iterator<Item = (f32, f32)>) -> Shape {
    outputs
        .filter(|&(width, height)| width > 0.0 && height > 0.0)
        .map(|(width, height)| {
            bounded(
                largest,
                Canvas::around(largest, Hang::default()).within(width, height),
            )
        })
        .fold(largest, |most, fits| Shape {
            width: most.width.min(fits.width),
            height: most.height.min(fits.height),
            radius: most.radius.min(fits.radius),
        })
}

/*
 * how far off its edge the Island reaches while small: a Split and `slots` Satellites beside it,
 * which along a side edge line up below it. The same in any canvas
 */
pub fn rest_reach(hang: Hang, slots: usize) -> f32 {
    let canvas = Canvas::around(DEFAULT_LARGEST, hang);
    let body = body(upright(SPLIT, hang), hang, canvas);
    let far = (0..slots)
        .map(|slot| satellite(body, hang, canvas, slot as f32, 1.0))
        .fold(body, |far, dot| {
            Rect::from_edges(
                far.x.min(dot.x),
                far.y.min(dot.y),
                far.right().max(dot.right()),
                far.bottom().max(dot.bottom()),
            )
        });

    match hang.edge {
        Edge::Top => far.bottom(),
        Edge::Bottom => canvas.height - far.y,
        Edge::Left => far.right(),
        Edge::Right => canvas.width - far.x,
    }
}

/*
 * against the edge it hangs from, `TOP` off it, and at its side, `TOP` in from a corner; from the
 * left or right edge, in the middle of it
 */
pub fn body(shape: Shape, hang: Hang, canvas: Canvas) -> Rect {
    let x = match hang.across() {
        Side::Start => TOP,
        Side::Middle => (canvas.width - shape.width) / 2.0,
        Side::End => canvas.width - TOP - shape.width,
    };

    let y = match hang.edge {
        Edge::Top => TOP,
        Edge::Bottom => canvas.height - TOP - shape.height,
        Edge::Left | Edge::Right => (canvas.height - shape.height) / 2.0,
    };

    Rect {
        x,
        y,
        width: shape.width,
        height: shape.height,
    }
}

/*
 * the body plus hover padding, grown to whole pixels and kept inside the canvas;
 * whole pixels so the hover target and the Wayland input region are exactly the same area
 */
pub fn input_area(body: Rect, canvas: Canvas) -> Rect {
    let left = (body.x - HOVER_PADDING).floor().max(0.0);
    let top = (body.y - HOVER_PADDING).floor().max(0.0);
    let right = (body.right() + HOVER_PADDING).ceil().min(canvas.width);
    let bottom = (body.bottom() + HOVER_PADDING).ceil().min(canvas.height);

    Rect::from_edges(left, top, right, bottom)
}

/*
 * the input region around `area`, the body's, and the small form a morph took from under a still
 * pointer (`IslandService::under`), so the pointer is never left out until it moves on
 */
pub fn holding(area: Rect, under: Option<Shape>, hang: Hang, canvas: Canvas) -> Rect {
    under.map_or(area, |under| {
        let under = body(bounded(under, canvas), hang, canvas);

        area.around(input_area(under, canvas))
    })
}

/*
 * the segment of a Split body under the pointer at `x`, `y`, in canvas coordinates: the primary
 * takes Compact's length, the top Satellite the rest, below it along a side edge. Measured on the
 * Split body whatever shows, so a still pointer is on the right segment when the body becomes
 * Split under it; any other body is the primary's alone, wherever the pointer is. `room` is what
 * the privacy cluster takes from the trailing end, which moves the segments' boundary back by it
 */
pub fn segment(x: f32, y: f32, room: f32, hang: Hang, canvas: Canvas) -> Segment {
    let split = body(upright(SPLIT, hang), hang, canvas);
    let primary = match hang.sideways() {
        true => y < split.y + COMPACT.width - room,
        false => x < split.x + COMPACT.width - room,
    };

    if primary {
        Segment::Primary
    } else {
        Segment::Satellite
    }
}

// where the trailing segment of a Split body draws, from the body's own top left corner
pub fn trailing(hang: Hang) -> Rect {
    let thickness = SPLIT.height - 2.0 * SEGMENT_INSET;
    let length = SPLIT.width - SEGMENT_INSET - COMPACT.width;

    if hang.sideways() {
        Rect {
            x: SEGMENT_INSET,
            y: COMPACT.width,
            width: thickness,
            height: length,
        }
    } else {
        Rect {
            x: COMPACT.width,
            y: SEGMENT_INSET,
            width: length,
            height: thickness,
        }
    }
}

/*
 * the Satellite in `slot`, beside the body toward the middle of the output and centered on its
 * small form, between places while it slides: right of it, left of an island at the right, below
 * one standing along a side edge. Past a Peek, or the longest upright small form, a body that grows
 * on only covers them, so they stay inside the canvas while they fade out. At `presence` 0 it is
 * tucked under the body's end, as it comes out and goes back
 */
pub fn satellite(body: Rect, hang: Hang, canvas: Canvas, slot: f32, presence: f32) -> Rect {
    let step = SATELLITE_GAP + slot * (SATELLITE + SATELLITE_GAP);

    if hang.sideways() {
        let longest = self::body(UPRIGHT_TRAY, hang, canvas);
        let width = body.width.min(PEEK.height);
        let end = body.bottom().min(longest.bottom());
        let (placed, tucked) = (end + step, end - SATELLITE);

        // against the edge, centered on the small form's thickness
        let x = match hang.edge {
            Edge::Right => body.right() - width + (width - SATELLITE) / 2.0,
            _ => body.x + (width - SATELLITE) / 2.0,
        };

        return Rect {
            x,
            y: tucked + (placed - tucked) * presence,
            width: SATELLITE,
            height: SATELLITE,
        };
    }

    let peek = self::body(PEEK, hang, canvas);
    let height = body.height.min(PEEK.height);

    let (placed, tucked) = if hang.across() == Side::End {
        let end = body.x.max(peek.x);

        (end - step - SATELLITE, end)
    } else {
        let end = body.right().min(peek.right());

        (end + step, end - SATELLITE)
    };

    let y = match hang.edge {
        Edge::Bottom => body.bottom() - height + (height - SATELLITE) / 2.0,
        _ => body.y + (height - SATELLITE) / 2.0,
    };

    Rect {
        x: tucked + (placed - tucked) * presence,
        y,
        width: SATELLITE,
        height: SATELLITE,
    }
}

/*
 * Satellites belong to the small forms: whole up to a Peek, gone by the smallest Surface, as far
 * as the body reaches off its edge
 */
pub fn satellite_opacity(shape: Shape, hang: Hang) -> f32 {
    let (reach, peek, media) = match hang.sideways() {
        true => (shape.width, PEEK.width, MEDIA.width),
        false => (shape.height, PEEK.height, MEDIA.height),
    };

    1.0 - ((reach - peek) / (media - peek)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // an output holds the largest body down to the room it leaves, and the least of several holds it for all
    #[test]
    fn the_largest_body_fits_each_output() {
        let asked = largest(900.0, 700.0);
        let small = fitted(asked, [(800.0, 600.0)].into_iter());
        let large = fitted(asked, [(1920.0, 1080.0)].into_iter());

        assert_eq!(large, asked);
        assert!(small.width < asked.width && small.height < asked.height);
        assert_eq!(
            fitted(asked, [(800.0, 600.0), (1920.0, 1080.0)].into_iter()),
            small
        );

        // an output with no size yet holds any
        assert_eq!(fitted(asked, [(0.0, 0.0)].into_iter()), asked);
    }

    // the widest strip fits a body as narrow as the output leaves it, down to what the rest of the Peek needs
    #[test]
    fn the_tray_strip_fits_a_narrow_body() {
        let narrow = fitted(DEFAULT_LARGEST, [(500.0, 800.0)].into_iter());
        let peek = TimePeek {
            agenda: agenda_most(narrow.width),
            ..TimePeek::ALL
        };

        assert!(narrow.width < CALENDAR.width);
        assert!(tray(TRAY_SLOTS, peek).width <= narrow.width);
        assert!(agenda_most(narrow.width) < AGENDA_MOST);
    }

    // fewer slots where even the least of the time's Peek leaves too little room for all
    #[test]
    fn the_tray_strip_takes_fewer_slots_on_a_very_narrow_body() {
        let narrow = fitted(DEFAULT_LARGEST, [(390.0, 800.0)].into_iter());
        let peek = TimePeek {
            agenda: agenda_most(narrow.width),
            ..TimePeek::ALL
        };

        assert!(tray(TRAY_SLOTS, peek).width > narrow.width);

        let strip = shape(
            Presentation::Tray(TRAY_SLOTS as u8),
            narrow,
            Hang::default(),
            peek,
        );
        assert!(strip.width <= narrow.width);
        assert!(tray_slots(TRAY_SLOTS as u8, narrow, Hang::default(), peek) > 0);
    }

    // a list Surface grows to the largest body of the output it opens on, not another's
    #[test]
    fn a_list_surface_grows_to_its_outputs_largest_body() {
        let asked = largest(900.0, 700.0);
        let small = fitted(asked, [(800.0, 600.0)].into_iter());
        let mut sizes = Sizes {
            largest: asked,
            fits: HashMap::from([(
                "small".to_owned(),
                Holds {
                    largest: small,
                    peek: TimePeek::ALL,
                },
            )]),
            ..Sizes::default()
        };
        let launcher = Presentation::Expanded(Surface::Launcher);

        assert_eq!(sizes.largest_on("small"), small);
        assert_eq!(sizes.largest_on("large"), asked);

        sizes.ask(Surface::Launcher, 5000.0, None);
        let on = |monitor| sizes.target(launcher, 0, sizes.holds(monitor));

        assert_eq!(on("small").height, small.height);
        assert_eq!(on("large").height, asked.height);
    }

    // the Tray strip is as wide as the Peek its own output holds
    #[test]
    fn a_tray_strip_follows_its_outputs_peek() {
        let bare = TimePeek::default();
        let sizes = Sizes {
            fits: HashMap::from([(
                "bare".to_owned(),
                Holds {
                    largest: DEFAULT_LARGEST,
                    peek: bare,
                },
            )]),
            ..Sizes::default()
        };
        let tray = Presentation::Tray(2);
        let width = |monitor| sizes.target(tray, 0, sizes.holds(monitor)).width;

        assert!(width("bare") < width("other"));
    }
}
