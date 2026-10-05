//! Where the island body sits inside the fixed canvas, and which part of the canvas takes the
//! pointer. The window never resizes (plan 6.1), only the body moves inside it.

use super::presentation::{Presentation, Surface};

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

pub const PEEK: Shape = Shape {
    width: 300.0,
    height: 52.0,
    radius: 26.0,
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

// a Satellite's diameter, and the gap before each one
pub const SATELLITE: f32 = 28.0;
pub const SATELLITE_GAP: f32 = 6.0;

// where the body of an island in this Presentation morphs to
pub fn shape(presentation: Presentation) -> Shape {
    match presentation {
        Presentation::Rest => REST,
        Presentation::Compact => COMPACT,
        Presentation::Peek => PEEK,
        Presentation::Expanded(Surface::Controls) => CONTROLS,
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
    fn right(self) -> f32 {
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
 * the `index`th Satellite, right of the body and centered on it. Past a Peek a body that grows on
 * only covers them, so they stay inside the canvas while they fade out
 */
pub fn satellite(body: Rect, index: usize) -> Rect {
    let left = body.right().min(self::body(PEEK).right());
    let height = body.height.min(PEEK.height);

    Rect {
        x: left + SATELLITE_GAP + index as f32 * (SATELLITE + SATELLITE_GAP),
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

    const PRESENTATIONS: [Presentation; 7] = [
        Presentation::Rest,
        Presentation::Compact,
        Presentation::Peek,
        Presentation::Expanded(Surface::Media),
        Presentation::Expanded(Surface::Notifications),
        Presentation::Expanded(Surface::Controls),
        Presentation::Expanded(Surface::Launcher),
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
        let small = [REST, COMPACT, PEEK];

        for pair in small.windows(2) {
            assert!(pair[0].width < pair[1].width && pair[0].height < pair[1].height);
        }

        for shape in small {
            assert_eq!(shape.radius, shape.height / 2.0, "{shape:?}");
        }
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
        let mut spring = Spring::new(
            REST.into(),
            Mode::Spring {
                response: Duration::from_millis(180),
            },
        );

        let mut now = start;

        for from in PRESENTATIONS {
            for to in PRESENTATIONS {
                spring.to(shape(from).into(), now);

                // change of mind 60 ms in, then follow every millisecond until it rests
                now += Duration::from_millis(60);
                spring.to(shape(to).into(), now);

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
        for small in [COMPACT, PEEK] {
            let body = body(small);
            let first = satellite(body, 0);
            let second = satellite(body, 1);

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
            let last = satellite(body(shape), crate::island::arbiter::SATELLITES);

            assert!(contains(canvas(), last), "{shape:?}");
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
