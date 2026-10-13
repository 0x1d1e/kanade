//! A pane of glass as one layer: its tone-aware tint, the backdrop's light and the shader's values.

use kanade_runtime::{Color, Rectangle};

use crate::config;
use crate::look::Tone;
use crate::theme;

use super::Body;
use super::Spot;
use super::shader;

/*
 * how strongly a pane is tinted: `Clear` for the Island and Dock, whose few glyphs read over
 * anything; `Regular` for the Banners, with more tint under lines of text
 */
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Clear,
    Regular,
}

// a body's size and shape, and where the pointer lights it
#[derive(Debug, Clone, PartialEq)]
pub struct Pane {
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    pub variant: Variant,

    // dark or light glass where the text over it is set so, as on the lock screen; none as `appearance.tone`
    pub tone: Option<Tone>,

    // the pointer over it, from its top left corner, which the highlight follows
    pub light: Option<(f32, f32)>,

    // the captured area around it, used to frost the body
    pub backdrop: Option<Seen>,

    /*
     * the pane as two rects united, as the Island with the Dock (ADR 0031), by their places in a
     * pane `width` and `height` big; the shader then cuts the tint and the highlights to the outline,
     * as no rounded rect clips them. None for one rounded rect
     */
    pub united: Option<Body>,
}

// the captured area around a body's outline
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub(super) spot: Spot,

    // how bright the backdrop is, 0 to 1
    pub(super) light: f32,

    // the copy's place, as it was asked for, where a copy is there at all
    pub(super) place: Option<[[f32; 4]; COPY_ROWS]>,
}

// the rows of values a copy's place takes: its body's three and where it lies in the copy
pub(super) const COPY_ROWS: usize = 4;

// the shader's values, by row
const BODY_ROW: usize = 2;
const TINT_ROW: usize = 5;
const COPY_ROW: usize = 6;
const FROST_ROW: usize = COPY_ROW + COPY_ROWS;

// the tint over the frosted backdrop, with stronger coverage where text needs it
fn tint(tone: Tone, variant: Variant, light: Option<f32>) -> Color {
    let opacity = match (tone, light) {
        (Tone::Dark, Some(light)) => 0.32 + 0.16 * light,
        (Tone::Light, Some(light)) => 0.72 - 0.16 * light,
        (Tone::Dark, None) => 0.50,
        (Tone::Light, None) => 0.72,
    };

    let opacity = match (variant, tone) {
        (Variant::Clear, _) => opacity,
        (Variant::Regular, Tone::Dark) => opacity.max(0.58 + 0.08 * light.unwrap_or(0.5)),
        (Variant::Regular, Tone::Light) => opacity.max(0.72 - 0.04 * light.unwrap_or(0.5)),
    };

    let base = match tone {
        Tone::Dark => theme::GLASS_DARK,
        Tone::Light => theme::GLASS_LIGHT,
    };

    Color::rgba(
        base.red(),
        base.green(),
        base.blue(),
        (opacity * 255.0).round() as u8,
    )
}

// the tint over nothing behind it, for Settings' material preview
pub fn swatch(tone: Tone) -> Color {
    tint(tone, Variant::Clear, None)
}

// the lock screen's tint under light text if `dark`; the lock crate frosts its own backdrop
pub fn lock_tint(dark: bool) -> Color {
    let tone = if dark { Tone::Dark } else { Tone::Light };
    tint(tone, Variant::Clear, None)
}

// Settings asks the compositor to blur its window when the protocol is available
pub fn blurred() -> bool {
    kanade_runtime::blur_available()
}

// a window's fill when it is glass (`blurred`): mostly opaque, for the text on it to read
pub fn see_through(color: Color) -> Color {
    const OPACITY: f32 = 0.74;

    if !blurred() {
        return color;
    }

    Color::rgba(
        color.red(),
        color.green(),
        color.blue(),
        (f32::from(color.alpha()) * OPACITY).round() as u8,
    )
}

// a quiet rim highlight for frosted glass
pub fn highlight() -> f32 {
    0.45
}

/*
 * the pane's glass, as big as the body and drawn from its top left corner under the content; the
 * shader cuts all of it to the body's outline. As many layers whatever is behind, so the targets
 * of the content over it keep their place mid-press or mid-drag
 */
pub fn layer(pane: Pane) -> Rectangle {
    let config = config::get();
    let tone = pane.tone.unwrap_or(config.appearance.tone);

    let (light_x, light_y, light_share) = match pane.light {
        Some((x, y)) => (x, y, 1.0),
        None => (pane.width * 0.3, 0.0, 0.0),
    };

    let dark = match tone {
        Tone::Dark => 1.0,
        Tone::Light => 0.0,
    };

    let tint = tint(
        tone,
        pane.variant,
        pane.backdrop.as_ref().map(|seen| seen.light),
    );

    let mut values = vec![[0.0; 4]; FROST_ROW + 1];

    values[0] = [
        pane.radius,
        config.appearance.highlight.share(),
        dark,
        highlight(),
    ];
    values[1] = [
        light_x,
        light_y,
        pane.width.max(pane.height) * 0.45,
        light_share,
    ];

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

    values[TINT_ROW] = [
        f32::from(tint.red()) / 255.0,
        f32::from(tint.green()) / 255.0,
        f32::from(tint.blue()) / 255.0,
        f32::from(tint.alpha()) / 255.0,
    ];

    let mut layer = Rectangle::new()
        .width(pane.width)
        .height(pane.height)
        .shader(shader().clone());

    // frost only where a captured copy has its placement values
    if let Some(seen) = pane.backdrop {
        if let Some(place) = seen.place {
            values[COPY_ROW..FROST_ROW].copy_from_slice(&place);
            values[FROST_ROW][0] = 1.0;
        }

        layer = layer.backdrop(seen.spot);
    }

    layer.shader_values(values)
}
