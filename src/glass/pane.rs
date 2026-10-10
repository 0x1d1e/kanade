//! A pane's glass as one layer: the tint by material, tone and the backdrop's light, and the
//! shader's values.

use kanade_runtime::{Color, Rectangle};

use crate::config;
use crate::look::{Material, Tone};
use crate::theme;

use super::Body;
use super::Spot;
use super::shader;

/*
 * how much of what is behind a pane shows, as Apple's glass variants: `Clear` lets it through, for
 * the Island and the Dock, whose few glyphs read over anything; `Regular` tints it enough for lines
 * of text, as on the Banners
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

    // what is behind it, for liquid glass; none draws the transparent look
    pub backdrop: Option<Seen>,

    /*
     * the pane as two rects united, as the Island with the Dock (ADR 0031), by their places in a
     * pane `width` and `height` big; the shader then cuts the tint and the highlights to the outline,
     * as no rounded rect clips them. None for one rounded rect
     */
    pub united: Option<Body>,
}

// what liquid glass shows of what is behind a body
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub(super) spot: Spot,

    // how bright the backdrop is, 0 to 1
    pub(super) light: f32,

    // the copy's place, as it was asked for, where it fits the body; none where it is too far off
    pub(super) rim: Option<[[f32; 4]; RIM_ROWS]>,
}

// the rows of values a copy's place takes: its body's three and where it lies in the copy
pub(super) const RIM_ROWS: usize = 4;

// the shader's values, by row
const BODY_ROW: usize = 2;
const TINT_ROW: usize = 5;
const RIM_ROW: usize = 6;
const OPACITY_ROW: usize = RIM_ROW + RIM_ROWS;

// the tint over what is behind, by material and tone, and how bright that is for liquid glass
fn tint(material: Material, tone: Tone, variant: Variant, light: Option<f32>) -> Color {
    let opacity = match (material, light) {
        (Material::Monochrome, _) => 1.0,

        // the brighter the backdrop, the more tint light text needs over it, and dark text less
        (Material::LiquidGlass, Some(light)) => match tone {
            Tone::Dark => 0.10 + 0.25 * light,
            Tone::Light => 0.62 - 0.30 * light,
        },

        (Material::LiquidGlass, None) => 0.30,
    };

    // text over a busy backdrop needs most of it hidden, whatever the material
    let opacity = match (variant, tone) {
        (Variant::Clear, _) => opacity,
        (Variant::Regular, Tone::Dark) => opacity.max(0.58 + 0.08 * light.unwrap_or(0.5)),
        (Variant::Regular, Tone::Light) => opacity.max(0.72 - 0.04 * light.unwrap_or(0.5)),
    };

    let (base, opacity) = match (tone, light) {
        (Tone::Dark, _) => (theme::GLASS_DARK, opacity),

        // liquid glass sets its own light tint by the backdrop, under the floor over a bright one
        (Tone::Light, Some(_)) => (theme::GLASS_LIGHT, opacity),

        // dark text needs more of a light tint under it to read
        (Tone::Light, _) => (theme::GLASS_LIGHT, f32::max(opacity, 0.5)),
    };

    Color::rgba(
        base.red(),
        base.green(),
        base.blue(),
        (opacity * 255.0).round() as u8,
    )
}

// a material's tint in a tone over nothing behind it, for Settings' material picker
pub fn swatch(material: Material, tone: Tone) -> Color {
    tint(material, tone, Variant::Clear, None)
}

/*
 * the tint of the lock screen's field and disc as `appearance.material` makes them, under light
 * text if `dark`; none for liquid glass, which `kanade-lock` tints by its backdrop (ADR 0025)
 */
pub fn lock_tint(dark: bool) -> Option<Color> {
    let material = config::get().appearance.material;
    let tone = if dark { Tone::Dark } else { Tone::Light };

    (material != Material::LiquidGlass).then(|| tint(material, tone, Variant::Clear, None))
}

// how much a body's rim catches the light, by `appearance.material`
pub fn highlight() -> f32 {
    match config::get().appearance.material {
        Material::Monochrome => 0.45,
        Material::LiquidGlass => 1.0,
    }
}

/*
 * the pane's glass, as big as the body and drawn from its top left corner under the content; the
 * shader cuts all of it to the body's outline. As many layers whatever is behind, so the targets
 * of the content over it keep their place mid-press or mid-drag
 */
pub fn layer(pane: Pane) -> Rectangle {
    let config = config::get();
    let material = config.appearance.material;
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
        material,
        tone,
        pane.variant,
        pane.backdrop.as_ref().map(|seen| seen.light),
    );

    let mut values = vec![[0.0; 4]; OPACITY_ROW + 1];

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

    // only where `place` said liquid glass is there, whatever the config says by now
    if let Some(seen) = pane.backdrop {
        if let Some(rim) = seen.rim {
            values[RIM_ROW..OPACITY_ROW].copy_from_slice(&rim);
            values[OPACITY_ROW][0] = 1.0;
        }

        layer = layer.backdrop(seen.spot);
    }

    layer.shader_values(values)
}
