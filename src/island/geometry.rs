//! Where the island body sits inside the fixed canvas, and which part of the canvas takes the
//! pointer. The window never resizes (plan 6.1), only the body moves inside it.

// the layer window, sized once for the largest body plus room for its shadow
pub const CANVAS_WIDTH: f32 = 560.0;
pub const CANVAS_HEIGHT: f32 = 380.0;

// gap between the top of the screen and the body
pub const TOP: f32 = 8.0;

// the pointer counts as on the body this far outside it, so grazing an edge does not flicker
pub const HOVER_PADDING: f32 = 8.0;

pub const REST: Shape = Shape {
    width: 150.0,
    height: 32.0,
    radius: 16.0,
};

pub const EXPANDED: Shape = Shape {
    width: 440.0,
    height: 160.0,
    radius: 32.0,
};

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

#[cfg(test)]
mod tests {
    use super::*;

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

    // every shape on the way between rest and expanded, including fractional mid-animation ones
    fn shapes() -> impl Iterator<Item = Shape> {
        (0..=100).map(|step| {
            let amount = step as f32 / 100.0;
            let blend = |from: f32, to: f32| from + (to - from) * amount;

            Shape {
                width: blend(REST.width, EXPANDED.width),
                height: blend(REST.height, EXPANDED.height),
                radius: blend(REST.radius, EXPANDED.radius),
            }
        })
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
            let rect = body(shape);
            let padded = Rect::from_edges(
                rect.x - HOVER_PADDING,
                rect.y - HOVER_PADDING,
                rect.right() + HOVER_PADDING,
                rect.bottom() + HOVER_PADDING,
            );

            assert!(contains(canvas(), padded), "{shape:?}");
        }
    }

    #[test]
    fn input_area_is_whole_pixels_inside_the_canvas_around_the_body() {
        for shape in shapes() {
            let rect = body(shape);
            let area = input_area(rect);

            assert!(contains(canvas(), area), "{shape:?}");
            assert!(contains(area, rect), "{shape:?}");

            for value in [area.x, area.y, area.width, area.height] {
                assert!(whole(value), "{shape:?}: {area:?}");
            }
        }
    }

    #[test]
    fn input_area_is_the_body_plus_padding() {
        let area = input_area(body(EXPANDED));

        // 440 wide in 560 leaves 60 on each side, the top padding stops at the canvas edge
        assert_eq!(
            area,
            Rect::from_edges(52.0, 0.0, 508.0, TOP + EXPANDED.height + HOVER_PADDING)
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
}
