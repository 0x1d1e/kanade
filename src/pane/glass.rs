//! The Liquid Glass finish as one layer: the copy of the screen around the body and the shader's values (ADR 0052).

use kanade_runtime::Rectangle;

use super::shader::glass_shader;
use super::{Pane, Spot};

// the captured area around a body
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub(super) spot: Spot,

    // the copy's place, as it was asked for, where a copy is there at all
    pub(super) place: Option<[[f32; 4]; COPY_ROWS]>,

    // how dark the content's ink is over this copy, 0 to 1 (`ink`)
    pub(super) paper: f32,

    // how black the glass is under the light ink, 0 to 1
    pub(super) scrim: f32,
}

impl Seen {
    // how dark the content's ink is, 0 the tinted ramp's light ink to 1 paper's dark one
    pub fn paper(&self) -> f32 {
        self.paper
    }
}

// the rows of values a copy's place takes: its body's three, where it lies in the copy, the rects
// of `NEAR` other panes on it, and how far out the edge reads, which is 0 while there is no copy
pub(super) const COPY_ROWS: usize = 4 + NEAR + 1;

// how many other panes on a copy are told to the shader, which reads none of them
pub(super) const NEAR: usize = 2;

// the shader's values, by row
const BODY_ROW: usize = 2;
const COPY_ROW: usize = 6;
const SCRIM_ROW: usize = COPY_ROW + COPY_ROWS;

// the pane as big as the body and drawn from its top left corner under the content, as `layer`
pub(super) fn layer(pane: Pane) -> Rectangle {
    let mut values = vec![[0.0; 4]; SCRIM_ROW + 1];

    values[0] = [pane.radius, 0.0, 0.0, 0.0];

    if let Some(body) = pane.united
        && let Some(join) = body.join
    {
        let clamp =
            |width: f32, height: f32, radius: f32| radius.min(width / 2.0).min(height / 2.0);

        values[0][0] = clamp(body.width, body.height, body.radius);
        values[BODY_ROW] = [body.x, body.y, body.width, body.height];
        values[BODY_ROW + 1] = [join.x, join.y, join.width, join.height];
        values[BODY_ROW + 2] = [
            clamp(join.width, join.height, join.radius),
            join.blend,
            1.0,
            0.0,
        ];
    }

    let mut layer = Rectangle::new()
        .width(pane.width)
        .height(pane.height)
        .shader(glass_shader().clone());

    // the copy fills only where it has its placement values
    if let Some(seen) = pane.backdrop {
        if let Some(place) = seen.place {
            values[COPY_ROW..SCRIM_ROW].copy_from_slice(&place);
        }

        values[SCRIM_ROW][0] = seen.scrim;

        layer = layer.backdrop(seen.spot);
    }

    layer.shader_values(values)
}
