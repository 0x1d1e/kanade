//! The finishes' roles (ADRs 0050, 0051): the Island, Dock and Banners' ramp, taken from one hue,
//! the wallpaper's: dark for the tinted solid, light for paper. Built in OKLCH so the steps are
//! even to the eye whatever the hue, and the contrast of text on them does not depend on it.

use kanade_runtime::Color;

use super::ThemeRoles;

// the hue and chroma of the wallpaper's source color, in OKLCH
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Source {
    // degrees
    pub hue: f32,

    pub chroma: f32,
}

// no wallpaper known: a cool slate, barely tinted
pub const NEUTRAL: Source = Source {
    hue: 265.0,
    chroma: 0.0,
};

// the source chroma at and above which the ramp takes its full tint
const FULL: f32 = 0.06;

// the share of the tint a grey wallpaper keeps, so the pane never reads as pure grey or black
const LEAST: f32 = 0.25;

// L and C of the surface, and the lightness of each step above it
const SURFACE: (f32, f32) = (0.16, 0.032);
const CONTAINER: f32 = 0.05;
const HIGH: f32 = 0.10;

impl Source {
    pub fn of(color: Color) -> Self {
        let [_, a, b] = oklab(color);

        Source {
            hue: b.atan2(a).to_degrees().rem_euclid(360.0),
            chroma: a.hypot(b),
        }
    }

    // how much of the tint the ramp takes, 0.25 to 1
    fn share(self) -> f32 {
        (self.chroma / FULL).clamp(LEAST, 1.0)
    }
}

/*
 * the roles on a surface tinted toward `source`: opaque steps, since a pane is solid, the text
 * nearly white, and one accent, `primary`, light enough to stand on the surface
 */
pub fn roles(source: Source) -> ThemeRoles {
    let share = source.share();
    let color = |lightness: f32, chroma: f32| oklch(lightness, chroma * share, source.hue);
    let (lightness, chroma) = SURFACE;

    ThemeRoles {
        primary: color(0.84, 0.1),
        on_primary: color(0.2, chroma * 1.2),
        surface: color(lightness, chroma),
        on_surface: color(0.96, 0.008),
        on_surface_variant: color(0.82, 0.02),
        surface_container: color(lightness + CONTAINER, chroma * 1.1),
        surface_container_high: color(lightness + HIGH, chroma * 1.15),
        outline: super::TINT_OUTLINE,
    }
}

/*
 * paper's roles toward `source`: the same steps turned over, so the surface is near white and each
 * container a step darker, the text near black and the accent a dark ink that stands on the
 * surface
 */
pub fn paper(source: Source) -> ThemeRoles {
    let share = source.share();
    let color = |lightness: f32, chroma: f32| oklch(lightness, chroma * share, source.hue);

    ThemeRoles {
        primary: color(0.30, 0.06),
        on_primary: color(0.97, 0.01),
        surface: color(0.955, 0.018),
        on_surface: color(0.22, 0.02),
        on_surface_variant: color(0.42, 0.02),
        surface_container: color(0.915, 0.022),
        surface_container_high: color(0.87, 0.028),
        outline: super::PAPER_OUTLINE,
    }
}

// the tinted solid's ramp for glass: its containers are light veils, not solid fills, so a tile on
// the glass is more glass and shows the page through it
pub fn clear(roles: ThemeRoles) -> ThemeRoles {
    ThemeRoles {
        surface_container: super::GLASS_CONTAINER,
        surface_container_high: super::GLASS_CONTAINER_HIGH,
        ..roles
    }
}

// paper's roles for glass: its containers are dark veils, not solid fills
pub fn veiled(roles: ThemeRoles) -> ThemeRoles {
    ThemeRoles {
        surface_container: super::GLASS_PAPER_CONTAINER,
        surface_container_high: super::GLASS_PAPER_CONTAINER_HIGH,
        ..roles
    }
}

// the roles `amount` of the way from `from` to `to`, 0 to 1, alpha too
pub fn fade(from: ThemeRoles, to: ThemeRoles, amount: f32) -> ThemeRoles {
    let blend = |from: Color, to: Color| {
        let channel = |from: u8, to: u8| {
            (f32::from(from) + (f32::from(to) - f32::from(from)) * amount).round() as u8
        };

        Color::rgba(
            channel(from.red(), to.red()),
            channel(from.green(), to.green()),
            channel(from.blue(), to.blue()),
            channel(from.alpha(), to.alpha()),
        )
    };

    ThemeRoles {
        primary: blend(from.primary, to.primary),
        on_primary: blend(from.on_primary, to.on_primary),
        surface: blend(from.surface, to.surface),
        on_surface: blend(from.on_surface, to.on_surface),
        on_surface_variant: blend(from.on_surface_variant, to.on_surface_variant),
        surface_container: blend(from.surface_container, to.surface_container),
        surface_container_high: blend(from.surface_container_high, to.surface_container_high),
        outline: blend(from.outline, to.outline),
    }
}

// a hair lighter than the surface, which the pane's fill is at its top edge
pub fn lit(surface: Color) -> Color {
    let [lightness, a, b] = oklab(surface);

    from_oklab(lightness + 0.02, a, b)
}

fn oklch(lightness: f32, chroma: f32, hue: f32) -> Color {
    let hue = hue.to_radians();

    from_oklab(lightness, chroma * hue.cos(), chroma * hue.sin())
}

fn oklab(color: Color) -> [f32; 3] {
    let [r, g, b] = super::channels(color).map(linear);
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();

    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn from_oklab(lightness: f32, a: f32, b: f32) -> Color {
    let l = (lightness + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m = (lightness - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s = (lightness - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);

    let channel = |linear: f32| (encode(linear.clamp(0.0, 1.0)) * 255.0).round() as u8;

    Color::rgb(
        channel(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s),
        channel(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s),
        channel(-0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s),
    )
}

fn linear(channel: f32) -> f32 {
    if channel <= 0.040_45 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

fn encode(linear: f32) -> f32 {
    if linear <= 0.003_130_8 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{contrast, over};

    const RAMPS: [fn(Source) -> ThemeRoles; 2] = [roles, paper];

    fn sources() -> impl Iterator<Item = Source> {
        (0..360).step_by(15).flat_map(|hue| {
            [0.0, 0.03, 0.06, 0.15].map(|chroma| Source {
                hue: hue as f32,
                chroma,
            })
        })
    }

    // the text and the accent hold their contrast on every step of each ramp, whatever the wallpaper
    #[test]
    fn text_reads_on_every_step_whatever_the_hue() {
        for (source, ramp) in sources().flat_map(|source| RAMPS.map(|ramp| (source, ramp))) {
            let roles = ramp(source);
            let steps = [
                roles.surface,
                roles.surface_container,
                roles.surface_container_high,
            ];

            for step in steps {
                assert!(contrast(roles.on_surface, step) >= 7.0, "{source:?}");
                assert!(
                    contrast(roles.on_surface_variant, step) >= 4.5,
                    "{source:?}"
                );
            }

            assert!(contrast(roles.primary, roles.surface) >= 4.5, "{source:?}");
            assert!(
                contrast(roles.on_primary, roles.primary) >= 4.5,
                "{source:?}"
            );
            assert!(
                contrast(over(roles.outline, roles.surface), roles.surface) >= 1.5,
                "{source:?}"
            );
        }
    }

    #[test]
    fn a_source_is_read_back_from_its_color() {
        let source = Source::of(Color::rgb(0x90, 0xa9, 0xcb));

        assert!((source.hue - 255.0).abs() < 10.0, "{source:?}");
        assert!(source.chroma > 0.04 && source.chroma < 0.09, "{source:?}");
    }
}
