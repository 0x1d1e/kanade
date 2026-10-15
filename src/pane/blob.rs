//! The Island's body and the Satellites beside it as one liquid shape (ADR 0101): each dot is
//! smoothly united with the body and the dots before it, so one coming out of the body pulls a neck
//! that thins and snaps as it parts. A layer of its own under the body, which draws its own fill.

use kanade_runtime::Rectangle;

use super::shader::blob_shader;
use super::{Body, blur, glass, rgba};
use crate::config;
use crate::island::motion::Mode;
use crate::theme;

// how many dots the shader takes
const MOST: usize = 6;

// how far a neck reaches when a dot is within the body: it is gone at the dots' resting gap
const REACH: f32 = 32.0;

// a round dot beside the body, in the body's canvas
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dot {
    // the corner of its square
    pub x: f32,
    pub y: f32,
    pub diameter: f32,
    pub opacity: f32,
}

/*
 * the shape of `dots`, nearest the body first, drawn from `body`'s canvas's corner, whose dots rest
 * `gap` apart and so have no neck at rest. None where the dots are drawn alone, each its own fill,
 * which is the look the neck degrades to: Liquid Glass, whose copy would read the neck (the dots
 * are no pane), reduced motion, where a dot is already in its place, and more dots than the shader takes
 */
pub fn blob(body: Body, gap: f32, dots: &[Dot]) -> Option<Rectangle> {
    if dots.is_empty()
        || dots.len() > MOST
        || glass()
        || config::get().island.motion == Mode::Reduced
    {
        return None;
    }

    // out to where a neck reaches
    let (mut left, mut top) = (f32::MAX, f32::MAX);
    let (mut right, mut bottom) = (f32::MIN, f32::MIN);

    for dot in dots {
        left = left.min(dot.x - REACH);
        top = top.min(dot.y - REACH);
        right = right.max(dot.x + dot.diameter + REACH);
        bottom = bottom.max(dot.y + dot.diameter + REACH);
    }

    let island = theme::island();
    let dot = rgba(island.surface_container_high);

    // the neck takes the body's color where it leaves it, but a translucent body shows no step
    let near = if blur::on() {
        dot
    } else {
        rgba(island.surface)
    };

    let mut values = vec![[0.0; 4]; 4 + MOST];

    values[0] = [body.x - left, body.y - top, body.width, body.height];
    values[1] = [
        body.radius.min(body.width / 2.0).min(body.height / 2.0),
        REACH,
        gap,
        dots.len() as f32,
    ];
    values[2] = near;
    values[3] = dot;

    for (row, dot) in values[4..].iter_mut().zip(dots) {
        let radius = dot.diameter / 2.0;

        *row = [
            dot.x - left + radius,
            dot.y - top + radius,
            radius,
            dot.opacity,
        ];
    }

    Some(
        Rectangle::new()
            .width(right - left)
            .height(bottom - top)
            .translate(left, top)
            .shader(blob_shader().clone())
            .shader_values(values),
    )
}
