//! A pane: the solid black the Island's body, the Dock and the Banners are drawn on (ADR 0049).
//! Its shader fills the body's outline, one rounded rect or two united, and nothing else.

mod body;
mod shader;

pub use self::body::{Body, Join};
pub use self::shader::shader;

use kanade_runtime::{Color, Rectangle};

use crate::theme;

// a body's size and shape
#[derive(Debug, Clone, PartialEq)]
pub struct Pane {
    pub width: f32,
    pub height: f32,
    pub radius: f32,

    /*
     * the pane as two rects united, as the Island with the Dock (ADR 0031), by their places in a
     * pane `width` and `height` big; the shader then cuts the fill to the outline, as no rounded
     * rect clips it. None for one rounded rect
     */
    pub united: Option<Body>,
}

// the shader's values, by row
const BODY_ROW: usize = 1;
const FILL_ROW: usize = 4;

/*
 * the pane as big as the body and drawn from its top left corner under the content; the shader
 * cuts it to the body's outline. As many layers whatever else is drawn, so the targets of the
 * content over it keep their place mid-press or mid-drag
 */
pub fn layer(pane: Pane) -> Rectangle {
    let mut values = vec![[0.0; 4]; FILL_ROW + 1];

    values[0] = [pane.radius, 0.0, 0.0, 0.0];

    if let Some(body) = pane.united
        && let Some(join) = body.join
    {
        let clamp =
            |width: f32, height: f32, radius: f32| radius.min(width / 2.0).min(height / 2.0);

        values[0] = [clamp(body.width, body.height, body.radius), 0.0, 0.0, 1.0];
        values[BODY_ROW] = [body.x, body.y, body.width, body.height];
        values[BODY_ROW + 1] = [join.x, join.y, join.width, join.height];
        values[BODY_ROW + 2] = [
            clamp(join.width, join.height, join.radius),
            join.blend,
            0.0,
            0.0,
        ];
    }

    values[FILL_ROW] = rgba(theme::SEMANTIC.island_surface);

    Rectangle::new()
        .width(pane.width)
        .height(pane.height)
        .shader(shader().clone())
        .shader_values(values)
}

fn rgba(color: Color) -> [f32; 4] {
    [
        f32::from(color.red()) / 255.0,
        f32::from(color.green()) / 255.0,
        f32::from(color.blue()) / 255.0,
        f32::from(color.alpha()) / 255.0,
    ]
}
