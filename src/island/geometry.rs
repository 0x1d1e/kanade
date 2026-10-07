//! Where the island body sits inside the fixed canvas, and which part of the canvas takes the
//! pointer. The window never resizes (plan 6.1), only the body moves inside it.

use super::presentation::{Presentation, Segment, Surface};

// the layer window, sized once for the largest body plus room for its shadow
pub const CANVAS_WIDTH: f32 = 560.0;
pub const CANVAS_HEIGHT: f32 = 380.0;

// gap between the top of the screen and the body
pub const TOP: f32 = 8.0;

// the pointer counts as on the body this far outside it, so grazing an edge does not flicker
pub const HOVER_PADDING: f32 = 8.0;

// plan 6.1 starting values; the small forms are pills, radius half the height
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
 * the Tray strip: the time as at Rest, then a slot for each tray item, as tall as Compact so the
 * icons have room. Past `TRAY_SLOTS` items the last slot opens the Tray Surface with them all
 */
pub const TRAY_SLOTS: usize = 8;
pub const TRAY_SLOT: f32 = 32.0;
pub const TRAY_END: f32 = 6.0;

pub const TRAY: Shape = Shape {
    width: REST.width + TRAY_SLOTS as f32 * TRAY_SLOT + TRAY_END,
    height: COMPACT.height,
    radius: COMPACT.radius,
};

pub const CONTROLS: Shape = Shape {
    width: 440.0,
    height: 290.0,
    radius: 32.0,
};

// as wide as Controls, so the two Surfaces line up when one replaces the other
pub const MEDIA: Shape = Shape {
    width: 440.0,
    height: 216.0,
    radius: 32.0,
};

// the largest body, for the Surfaces with a list: Notifications and the Launcher
pub const EXPANDED_MAX: Shape = Shape {
    width: 520.0,
    height: 330.0,
    radius: 32.0,
};

// every shape a body morphs between; the springs never overshoot, so it stays within them
pub const SHAPES: [Shape; 8] = [
    REST,
    COMPACT,
    SPLIT,
    PEEK,
    TRAY,
    CONTROLS,
    MEDIA,
    EXPANDED_MAX,
];

// a Satellite's diameter, and the gap before each one
pub const SATELLITE: f32 = 28.0;
pub const SATELLITE_GAP: f32 = 6.0;

// where the body of an island in this Presentation morphs to
pub fn shape(presentation: Presentation) -> Shape {
    match presentation {
        Presentation::Rest => REST,
        Presentation::Compact => COMPACT,
        Presentation::Split => SPLIT,
        Presentation::Peek => PEEK,
        Presentation::Tray(slots) => Shape {
            width: REST.width + f32::from(slots) * TRAY_SLOT + TRAY_END,
            ..TRAY
        },
        Presentation::Expanded(Surface::Controls | Surface::Tray) => CONTROLS,
        Presentation::Expanded(Surface::Media) => MEDIA,
        Presentation::Expanded(Surface::Notifications | Surface::Launcher) => EXPANDED_MAX,
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

    fn bottom(self) -> f32 {
        self.y + self.height
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

// centered horizontally, hanging `TOP` below the top edge
pub fn body(shape: Shape) -> Rect {
    Rect {
        x: (CANVAS_WIDTH - shape.width) / 2.0,
        y: TOP,
        width: shape.width,
        height: shape.height,
    }
}

/*
 * the body plus hover padding, grown to whole pixels and kept inside the canvas;
 * whole pixels so the hover target and the Wayland input region are exactly the same area
 */
pub fn input_area(body: Rect) -> Rect {
    let left = (body.x - HOVER_PADDING).floor().max(0.0);
    let top = (body.y - HOVER_PADDING).floor().max(0.0);
    let right = (body.right() + HOVER_PADDING).ceil().min(CANVAS_WIDTH);
    let bottom = (body.bottom() + HOVER_PADDING).ceil().min(CANVAS_HEIGHT);

    Rect::from_edges(left, top, right, bottom)
}

/*
 * the segment of a Split body under the pointer at `x`, in canvas coordinates: the primary takes
 * Compact's width, the top Satellite the rest. Measured on the Split body whatever shows, so a
 * still pointer is on the right segment when the body becomes Split under it; any other body is
 * the primary's alone, wherever the pointer is
 */
pub fn segment(x: f32) -> Segment {
    if x < body(SPLIT).x + COMPACT.width {
        Segment::Primary
    } else {
        Segment::Satellite
    }
}

// where the trailing segment of a Split body draws, from the body's own top left corner
pub fn trailing() -> Rect {
    let height = SPLIT.height - 2.0 * SEGMENT_INSET;

    Rect {
        x: COMPACT.width,
        y: SEGMENT_INSET,
        width: SPLIT.width - SEGMENT_INSET - COMPACT.width,
        height,
    }
}

/*
 * the Satellite in `slot`, right of the body and centered on it, between places while it slides.
 * Past a Peek a body that grows on only covers them, so they stay inside the canvas while they
 * fade out. At `presence` 0 it is tucked under the body's end, as it comes out and goes back (#37)
 */
pub fn satellite(body: Rect, slot: f32, presence: f32) -> Rect {
    let end = body.right().min(self::body(PEEK).right());
    let height = body.height.min(PEEK.height);

    let placed = end + SATELLITE_GAP + slot * (SATELLITE + SATELLITE_GAP);
    let tucked = end - SATELLITE;

    Rect {
        x: tucked + (placed - tucked) * presence,
        y: body.y + (height - SATELLITE) / 2.0,
        width: SATELLITE,
        height: SATELLITE,
    }
}

// Satellites belong to the small forms: whole up to a Peek, gone by the smallest Surface
pub fn satellite_opacity(shape: Shape) -> f32 {
    1.0 - ((shape.height - PEEK.height) / (MEDIA.height - PEEK.height)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::island::motion::{Mode, Spring};

    fn canvas() -> Rect {
        Rect::from_edges(0.0, 0.0, CANVAS_WIDTH, CANVAS_HEIGHT)
    }

    fn contains(outer: Rect, inner: Rect) -> bool {
        outer.x <= inner.x
            && outer.y <= inner.y
            && outer.right() >= inner.right()
            && outer.bottom() >= inner.bottom()
    }

    fn whole(value: f32) -> bool {
        value.fract() == 0.0
    }

    const PRESENTATIONS: [Presentation; 11] = [
        Presentation::Rest,
        Presentation::Compact,
        Presentation::Split,
        Presentation::Peek,
        Presentation::Tray(1),
        Presentation::Tray(TRAY_SLOTS as u8),
        Presentation::Expanded(Surface::Media),
        Presentation::Expanded(Surface::Notifications),
        Presentation::Expanded(Surface::Controls),
        Presentation::Expanded(Surface::Launcher),
        Presentation::Expanded(Surface::Tray),
    ];

    /*
     * every Presentation's shape and every straight blend between any two of them, fractional
     * ones included. A retargeted spring can leave these blends, see the real-spring test below
     */
    fn shapes() -> impl Iterator<Item = Shape> {
        PRESENTATIONS.into_iter().flat_map(|from| {
            PRESENTATIONS.into_iter().flat_map(move |to| {
                let (from, to) = (shape(from), shape(to));

                (0..=100).map(move |step| {
                    let amount = step as f32 / 100.0;
                    let blend = |from: f32, to: f32| from + (to - from) * amount;

                    Shape {
                        width: blend(from.width, to.width),
                        height: blend(from.height, to.height),
                        radius: blend(from.radius, to.radius),
                    }
                })
            })
        })
    }

    fn padded(body: Rect) -> Rect {
        Rect::from_edges(
            body.x - HOVER_PADDING,
            body.y - HOVER_PADDING,
            body.right() + HOVER_PADDING,
            body.bottom() + HOVER_PADDING,
        )
    }

    #[test]
    fn small_forms_grow_and_are_pills() {
        let small = [REST, COMPACT, SPLIT, PEEK];

        for pair in small.windows(2) {
            assert!(pair[0].width <= pair[1].width && pair[0].height <= pair[1].height);
            assert_ne!(pair[0], pair[1]);
        }

        for shape in small {
            assert_eq!(shape.radius, shape.height / 2.0, "{shape:?}");
        }
    }

    // a pill the time leads, a slot wider for each item, the widest one of the shapes
    #[test]
    fn the_tray_strip_grows_by_a_slot_per_item() {
        let one = shape(Presentation::Tray(1));
        let all = shape(Presentation::Tray(TRAY_SLOTS as u8));

        assert!(one.width > REST.width && one.height >= REST.height);
        assert_eq!(one.radius, one.height / 2.0);
        assert_eq!(all, TRAY);
        assert_eq!(shape(Presentation::Tray(2)).width - one.width, TRAY_SLOT);
    }

    #[test]
    fn no_body_is_larger_than_the_largest() {
        for presentation in PRESENTATIONS {
            let shape = shape(presentation);

            assert!(shape.width <= EXPANDED_MAX.width, "{presentation:?}");
            assert!(shape.height <= EXPANDED_MAX.height, "{presentation:?}");
        }
    }

    #[test]
    fn body_is_centered_below_the_top() {
        let rect = body(REST);

        assert_eq!(rect.x, (CANVAS_WIDTH - REST.width) / 2.0);
        assert_eq!(rect.y, TOP);
        assert_eq!(rect.right() + rect.x, CANVAS_WIDTH);
    }

    #[test]
    fn every_body_fits_the_canvas_with_its_padding() {
        for shape in shapes() {
            assert!(contains(canvas(), padded(body(shape))), "{shape:?}");
        }
    }

    // nothing is clamped, since every padded body fits; only the rounding out to whole pixels
    #[test]
    fn input_area_is_the_body_plus_padding_in_whole_pixels_inside_the_canvas() {
        for shape in shapes() {
            let padded = padded(body(shape));
            let area = input_area(body(shape));

            assert!(contains(canvas(), area), "{shape:?}");
            assert!(contains(area, padded), "{shape:?}");

            for (edge, exact) in [
                (area.x, padded.x),
                (area.y, padded.y),
                (area.right(), padded.right()),
                (area.bottom(), padded.bottom()),
            ] {
                assert!(
                    whole(edge) && (edge - exact).abs() < 1.0,
                    "{shape:?}: {area:?}"
                );
            }
        }
    }

    // the real spring, retargeted mid-flight between every pair of Presentations
    #[test]
    fn input_area_holds_through_a_retargeted_spring() {
        let start = Instant::now();
        let response = Duration::from_millis(180);
        let mut spring = Spring::new(REST.into(), Mode::Spring);

        let mut now = start;

        for from in PRESENTATIONS {
            for to in PRESENTATIONS {
                spring.to(shape(from).into(), response, now);

                // change of mind 60 ms in, then follow every millisecond until it rests
                now += Duration::from_millis(60);
                spring.to(shape(to).into(), response, now);

                while !spring.settled(now) {
                    let shape = Shape::from(spring.at(now));
                    let area = input_area(body(shape));

                    assert!(contains(canvas(), area), "{shape:?}");
                    assert!(contains(area, padded(body(shape))), "{shape:?}");

                    now += Duration::from_millis(1);
                }
            }
        }
    }

    #[test]
    fn input_area_of_a_whole_pixel_body_is_exact() {
        let area = input_area(body(CONTROLS));

        // 440 wide in 560 leaves 60 on each side, the top padding ends at the canvas edge
        assert_eq!(
            area,
            Rect::from_edges(52.0, 0.0, 508.0, TOP + CONTROLS.height + HOVER_PADDING)
        );
    }

    #[test]
    fn input_area_covers_fractional_bodies_without_overshooting() {
        let rect = Rect {
            x: 100.25,
            y: 10.5,
            width: 50.5,
            height: 20.25,
        };

        assert_eq!(input_area(rect), Rect::from_edges(92.0, 2.0, 159.0, 39.0));
    }

    #[test]
    fn satellites_line_up_right_of_the_small_forms() {
        for small in [COMPACT, SPLIT, PEEK] {
            let body = body(small);
            let first = satellite(body, 0.0, 1.0);
            let second = satellite(body, 1.0, 1.0);

            assert_eq!(first.x, body.right() + SATELLITE_GAP);
            assert_eq!(second.x, first.right() + SATELLITE_GAP);
            assert_eq!(first.y - body.y, body.bottom() - first.bottom());
            assert_eq!(second.y, first.y);
        }
    }

    // the cap plus the overflow count, through any morph
    #[test]
    fn satellites_stay_inside_the_canvas() {
        for shape in shapes() {
            let last = satellite(body(shape), crate::island::arbiter::SATELLITES as f32, 1.0);

            assert!(contains(canvas(), last), "{shape:?}");
        }
    }

    // the primary keeps Compact's place, the Satellite's segment sits inside the body's end
    #[test]
    fn a_split_body_has_two_segments() {
        let body = body(SPLIT);
        let trailing = trailing();
        let inside = Rect::from_edges(0.0, 0.0, SPLIT.width, SPLIT.height);

        assert!(contains(inside, trailing));
        assert_eq!(trailing.y, inside.bottom() - trailing.bottom());
        assert_eq!(trailing.x, COMPACT.width);

        assert_eq!(segment(body.x), Segment::Primary);
        assert_eq!(segment(body.x + trailing.x - 0.5), Segment::Primary);
        assert_eq!(segment(body.x + trailing.x), Segment::Satellite);
        assert_eq!(segment(body.right()), Segment::Satellite);

        // the hover padding past either end counts as the segment there
        let area = input_area(body);
        assert_eq!(segment(area.x), Segment::Primary);
        assert_eq!(segment(area.right()), Segment::Satellite);
    }

    // a pointer still on the right end of Rest or Compact is on the trailing segment once Split
    #[test]
    fn the_segment_under_a_still_pointer_is_the_one_split_draws_there() {
        let line = body(SPLIT).x + trailing().x;

        for small in [REST, COMPACT] {
            let area = input_area(body(small));

            assert!(area.x < line && line < area.right(), "{small:?}");
            assert_eq!(segment(area.x), Segment::Primary, "{small:?}");
            assert_eq!(segment(area.right() - 1.0), Segment::Satellite, "{small:?}");
        }
    }

    // under the body, so it hides one coming out or going back, as tall as it is at Rest
    #[test]
    fn a_tucked_satellite_hides_under_the_body() {
        for small in [REST, COMPACT, SPLIT, PEEK] {
            let body = body(small);

            for slot in [0.0, 2.0] {
                let tucked = satellite(body, slot, 0.0);

                assert!(contains(body, tucked), "{small:?} {slot}");
            }
        }
    }

    // satellite_opacity fades them out by Media, so it must be the smallest Surface
    #[test]
    fn media_is_the_smallest_surface() {
        for surface in Surface::ALL {
            assert!(MEDIA.height <= shape(Presentation::Expanded(surface)).height);
        }
    }

    #[test]
    fn satellites_fade_out_toward_a_surface() {
        assert_eq!(satellite_opacity(REST), 1.0);
        assert_eq!(satellite_opacity(COMPACT), 1.0);
        assert_eq!(satellite_opacity(SPLIT), 1.0);
        assert_eq!(satellite_opacity(PEEK), 1.0);
        assert_eq!(satellite_opacity(MEDIA), 0.0);
        assert_eq!(satellite_opacity(CONTROLS), 0.0);
        assert_eq!(satellite_opacity(EXPANDED_MAX), 0.0);

        let halfway = Shape {
            height: (PEEK.height + MEDIA.height) / 2.0,
            ..PEEK
        };
        assert_eq!(satellite_opacity(halfway), 0.5);
    }
}
