//! Clock-free distance shadows follow the current shell outline without raster-bank handoffs.

use kanade_runtime::{Color, Rectangle, Widget};

use crate::island::geometry::Rect;
use crate::{pane, theme};

const PIECES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShadowStyle {
    // the gaussian's spread, in logical pixels
    pub blur: u32,

    // how far below the body it falls, at most the smallest radius so the body hides its middle
    pub drop: u32,

    // its alpha is the opacity under the body's middle
    pub color: Color,
}

impl ShadowStyle {
    /*
     * soft and wide, like CSS's `0 4px 24px`: a 24 pixel blur is a spread of 12. The drop stays
     * under 4.8, past which a side edge's piece, a drop lower than the see-through body, would
     * reach into the curve of a corner of radius 12
     */
    pub(crate) const AMBIENT: Self = Self {
        blur: 12,
        drop: 4,
        color: theme::SHADOW_AMBIENT,
    };

    // close to the edge, like CSS's `0 2px 6px`
    pub(crate) const CONTACT: Self = Self {
        blur: 3,
        drop: 2,
        color: theme::SHADOW_CONTACT,
    };

    // both, the ambient one under; the ambient one reaches furthest
    pub(crate) const ALL: [Self; 2] = [Self::AMBIENT, Self::CONTACT];

    // how far past the body's edge the blur reaches, in logical pixels
    pub(crate) fn reach(self) -> u32 {
        3 * self.blur + 1
    }
}

// Keep the same target slots whether the shadow is present, absent or changing shape.
pub(crate) fn none(layers: &mut Vec<Box<dyn Widget>>) {
    for _ in 0..ShadowStyle::ALL.len() * PIECES {
        layers.push(Box::new(nothing()));
    }
}

fn nothing() -> Rectangle {
    Rectangle::new().width(0.0).height(0.0)
}

pub(crate) fn draw(layers: &mut Vec<Box<dyn Widget>>, body: Rect, radius: f32) {
    for style in ShadowStyle::ALL {
        let reach = style.reach() as f32;
        let color = style.color;
        let channel = |value: u8| f32::from(value) / 255.0;
        layers.push(Box::new(
            Rectangle::new()
                .width(body.width + reach * 2.0)
                .height(body.height + reach * 2.0)
                .translate(body.x - reach, body.y - reach)
                .shader(pane::shadow_shader().clone())
                .shader_values(vec![
                    [reach, reach, body.width, body.height],
                    [radius, style.blur as f32, style.drop as f32, 0.0],
                    [
                        channel(color.red()),
                        channel(color.green()),
                        channel(color.blue()),
                        channel(color.alpha()),
                    ],
                ]),
        ));
        for _ in 1..PIECES {
            layers.push(Box::new(nothing()));
        }
    }
}
