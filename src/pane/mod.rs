//! A pane: what the Island's body, the Dock and the Banners are drawn on, as the finish says (ADRs
//! 0050 to 0052). The solid finishes fill the body's outline, one rounded rect or two united, with
//! the fill a hair lighter at the top, a top light edge and a rim, and nothing else; Liquid Glass
//! draws clear glass over the screen, tinted by the light around it (`glass`).

mod body;
mod capture;
mod glass;
mod shader;

use std::time::Duration;

pub use self::body::{Body, Join, ahead};
pub use self::capture::{forget, locked, origin, place, shown};
pub use self::glass::Seen;
pub use self::shader::shader;
pub use kanade_runtime::backdrop::Spot;

use kanade_runtime::{Color, Rectangle};

use crate::config;
use crate::look::Finish;
use crate::theme;

// how far ahead of its body a pane captures, so the copy fits once it lands as the body morphs:
// about how long one takes, a couple of frames at 60 Hz
pub const LEAD: Duration = Duration::from_millis(33);

// whether panes are Liquid Glass now, which is the only finish that captures the screen
pub fn glass() -> bool {
    config::get().appearance.finish == Finish::LiquidGlass
}

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

    // Liquid Glass only: the copy of the screen around the body, where the runtime has landed one
    pub backdrop: Option<Seen>,
}

// the shader's values, by row
const BODY_ROW: usize = 1;
const FILL_ROW: usize = 4;
const LIT_ROW: usize = 5;
const EDGE_ROW: usize = 6;
const RIM_ROW: usize = 7;

/*
 * the pane as big as the body and drawn from its top left corner under the content; the shader
 * cuts it to the body's outline. As many layers whatever else is drawn, so the targets of the
 * content over it keep their place mid-press or mid-drag
 */
pub fn layer(pane: Pane) -> Rectangle {
    if glass() {
        return glass::layer(pane);
    }

    let mut values = vec![[0.0; 4]; RIM_ROW + 1];

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

    let surface = theme::island().surface;

    values[FILL_ROW] = rgba(surface);
    values[LIT_ROW] = rgba(theme::tint::lit(surface));
    values[EDGE_ROW] = rgba(theme::pane_edge());
    values[RIM_ROW] = rgba(theme::pane_rim());

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
