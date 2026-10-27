//! The shell's design tokens: colors, type, radii and spacing; motion is the Island's own
//! `island::service::Timings` and `island::motion::REDUCED_FADE`, as `island/` cannot see this
//! file. No component writes a color literal (`src/boundary.rs`).
//!
//! Settings and the app adapters take light or dark roles Matugen derives from the wallpaper awww
//! shows at startup or one Kanade sets; otherwise the shell uses its built-in roles. The Island,
//! Dock and Banners take a tinted solid ramp from that wallpaper's hue (`tint`, ADR 0050).

use std::ffi::CString;

use fontconfig::{FC_FAMILY, FC_STYLE, Fontconfig, Pattern};
use kanade_runtime::{App, Color};

use crate::look::Finish;

pub mod tint;

// colors that mean something, the same in every theme
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SemanticColors {
    // a microphone or camera in use
    pub privacy: Color,

    // the screen being cast or recorded
    pub capture: Color,

    // a warning that waits, like a low battery
    pub warning: Color,

    // critical, like a battery about to die or a critical notification
    pub critical: Color,

    // the black of the lock screen's pill and disc (ADR 0049); the Island's body is tinted (ADR 0050)
    pub island_surface: Color,

    // text and glyphs on the Island
    pub on_island_surface: Color,
}

pub const SEMANTIC: SemanticColors = SemanticColors {
    privacy: Color::rgb(48, 209, 88),
    capture: Color::rgb(255, 176, 32),
    warning: Color::rgb(255, 176, 32),
    critical: Color::rgb(255, 69, 58),
    island_surface: Color::rgb(0, 0, 0),
    on_island_surface: Color::rgb(242, 242, 247),
};

// the signal colors on a light window, where the bright ones fall under 3:1 (dots, glyphs) or 4.5:1 (text)
const SIGNALS_LIGHT: SemanticColors = SemanticColors {
    privacy: Color::rgb(14, 122, 46),
    capture: Color::rgb(143, 82, 0),
    warning: Color::rgb(143, 82, 0),
    critical: Color::rgb(179, 38, 30),
    ..SEMANTIC
};

// the colors that mean something on the Island: the bright ones on a dark finish, the darker hues on paper
pub fn signals() -> SemanticColors {
    match finish() {
        Finish::TintedSolid => SEMANTIC,
        Finish::Blur if !blur_light() => SEMANTIC,
        Finish::Blur | Finish::Paper => SIGNALS_LIGHT,
    }
}

// whether Blur is paper's ramp: `appearance.tone` is light (ADR 0057)
fn blur_light() -> bool {
    crate::config::get().appearance.tone == crate::look::Tone::Light
}

fn finish() -> Finish {
    crate::config::get().appearance.finish
}

// the same in a shell-owned window's tone: darker hues of the same meaning on a light one
pub fn window_signals() -> SemanticColors {
    match crate::config::get().appearance.tone {
        crate::look::Tone::Dark => SEMANTIC,
        crate::look::Tone::Light => SIGNALS_LIGHT,
    }
}

// M3-like roles a theme changes
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThemeRoles {
    // a filled control, like a switch that is on
    pub primary: Color,

    // a glyph on primary
    pub on_primary: Color,

    // the body
    pub surface: Color,

    // high-contrast text on the surface
    pub on_surface: Color,

    // secondary text, like an artist under a title
    pub on_surface_variant: Color,

    // a notification on the surface, a quieter step than a Satellite so text on it keeps its contrast
    pub surface_container: Color,

    // a Satellite or missing cover art, a step above the surface so it reads beside it
    pub surface_container_high: Color,

    // the ring of a pinned Island, quiet enough not to read as an alert
    pub outline: Color,
}

// the roles on a dark surface: white text, and containers a step above it
pub const ISLAND_DARK: ThemeRoles = ThemeRoles {
    primary: SEMANTIC.on_island_surface,
    on_primary: SEMANTIC.island_surface,
    surface: SEMANTIC.island_surface,
    on_surface: SEMANTIC.on_island_surface,
    on_surface_variant: Color::rgba(235, 235, 245, 184),
    surface_container: Color::rgba(255, 255, 255, 20),
    surface_container_high: Color::rgba(255, 255, 255, 36),
    outline: Color::rgba(255, 255, 255, 64),
};

// the roles on a light surface: black text
pub const ISLAND_LIGHT: ThemeRoles = ThemeRoles {
    primary: Color::rgb(28, 28, 30),
    on_primary: Color::rgb(250, 250, 252),
    surface: Color::rgb(246, 246, 250),
    on_surface: Color::rgb(28, 28, 30),
    on_surface_variant: Color::rgba(60, 60, 67, 210),
    surface_container: Color::rgba(0, 0, 0, 14),
    surface_container_high: Color::rgba(0, 0, 0, 26),
    outline: Color::rgba(0, 0, 0, 46),
};

// the Island's roles, the same whatever `appearance.tone` is: tinted by the wallpaper, dark or
// light as `appearance.finish` says
pub fn island() -> ThemeRoles {
    crate::appearance::island_roles(finish())
}

// Settings' mark, from top left to bottom right
pub const SCENE: [Color; 3] = [
    Color::rgb(255, 154, 98),
    Color::rgb(214, 86, 146),
    Color::rgb(86, 72, 178),
];

/*
 * the ANSI hues apps' terminal colors are toned from, as Matugen custom colors harmonized toward
 * the wallpaper; a little warm or cool, so each keeps its hue once Matugen tones it
 */
pub const TERMINAL_HUES: [(&str, &str); 6] = [
    ("red", "#f04040"),
    ("green", "#40c040"),
    ("yellow", "#f0c020"),
    ("blue", "#3080f0"),
    ("magenta", "#d040d0"),
    ("cyan", "#20c0c0"),
];

// a glyph or a rim over the scene
pub const ON_SCENE: Color = Color::rgb(255, 255, 255);

/*
 * the pane's shadows: an ambient one lifting it off what is under it, and a contact one along its
 * edge. Faint, so they reinforce its elevation over a bright desktop and all but vanish on a dark one
 */
pub const SHADOW_AMBIENT: Color = Color::rgba(0, 0, 0, 38);
pub const SHADOW_CONTACT: Color = Color::rgba(0, 0, 0, 26);

// the ring of a pinned Island on the tinted ramp: white, so it reads on every hue (ADR 0050)
pub const TINT_OUTLINE: Color = Color::rgba(255, 255, 255, 71);

/*
 * a pane's top light edge and its rim, white over the fill: the edge a 1 px line along the top,
 * following the corners, the rim a 1 px line all round, inside the outline
 */
const PANE_EDGE: Color = Color::rgba(255, 255, 255, 26);
const PANE_RIM: Color = Color::rgba(255, 255, 255, 15);

// paper's: a white edge catching the light, a faint ink rim, the outline of a pinned Island in ink
const PAPER_EDGE: Color = Color::rgba(255, 255, 255, 230);
const PAPER_RIM: Color = Color::rgba(30, 30, 50, 18);
pub const PAPER_OUTLINE: Color = Color::rgba(30, 30, 50, 89);

// the pane's top light edge and rim in the finish
pub fn pane_edge() -> Color {
    match finish() {
        Finish::TintedSolid => PANE_EDGE,
        Finish::Blur if !blur_light() => PANE_EDGE,
        Finish::Blur | Finish::Paper => PAPER_EDGE,
    }
}

pub fn pane_rim() -> Color {
    match finish() {
        Finish::TintedSolid => PANE_RIM,
        Finish::Blur if !blur_light() => PANE_RIM,
        Finish::Blur | Finish::Paper => PAPER_RIM,
    }
}

// what the idle dim grace fades the outputs to before the lock (ADR 0047)
pub const IDLE_DIM: Color = Color::rgb(0, 0, 0);

// the gesture overlay (ADR 0109): a light scrim over the output, and a trail of a soft wide line
// under a bright narrow one, which read on a light and on a dark desktop alike
pub const GESTURE_SCRIM: Color = Color::rgba(0, 0, 0, 56);
pub const GESTURE_GLOW: Color = Color::rgba(110, 170, 255, 70);
pub const GESTURE_CORE: Color = Color::rgb(235, 245, 255);
pub const GESTURE_TEXT: Color = Color::WHITE;

// the region selection overlay: dimmed desktop mask, selection outline and dimension badge
pub const REGION_SCRIM: Color = Color::rgba(0, 0, 0, 75);
pub const REGION_BORDER: Color = Color::WHITE;
pub const REGION_BADGE_BG: Color = Color::rgba(20, 20, 22, 230);
pub const REGION_BADGE_BORDER: Color = Color::rgba(255, 255, 255, 50);
pub const REGION_BADGE_TEXT: Color = Color::WHITE;

// the OSD's capsule over an open Surface: darker, as it floats over the Island, not the desktop
pub const SHADOW_HUD: Color = Color::rgba(0, 0, 0, 89);

/*
 * over the wallpaper behind the lock screen, of mean luma `light` if known: barely over a dark one,
 * as much as light text needs over a light one, more for `lock.backdrop` dimmed. One not decoded
 * for it is taken as fairly light, 0.6
 */
pub fn lock_shade(light: Option<f32>, dimmed: bool) -> Color {
    let least: f32 = if dimmed { 0.45 } else { 0.12 };

    // what shows through is at most as light as 0.4, under which light text reads
    let needed = 1.0 - 0.4 / light.unwrap_or(0.6).max(0.4);

    Color::rgba(0, 0, 0, (least.max(needed) * 255.0).round() as u8)
}

// the family drawn when `appearance.font` is unset: vendored, so it does not need to be installed
pub const FONT: &str = "Open Sauce One";

/*
 * the family the shell's text is set in: `appearance.font`, else Open Sauce One. A set family still
 * resolves through fontconfig. The runtime sets weights only from static files, so a family counts
 * when fontconfig finds it in a SemiBold of its own, not a variable font's instance
 */
pub fn font(app: App) -> App {
    let chosen = crate::config::get().appearance.font.clone();
    // by its family, not its full name, which may add the style
    let static_face = |family: &str| {
        let semibold = || -> Option<(String, String)> {
            let fontconfig = Fontconfig::new()?;
            let mut pattern = Pattern::new(&fontconfig).ok()?;

            pattern
                .add_string(FC_FAMILY, &CString::new(family).ok()?)
                .ok()?;
            pattern.add_string(FC_STYLE, c"SemiBold").ok()?;

            let found = pattern.font_match().ok()?;

            Some((
                found.get_string(FC_FAMILY).ok()?.to_owned(),
                found.filename().ok()?.to_owned(),
            ))
        };

        semibold().is_some_and(|(found, path)| {
            found.eq_ignore_ascii_case(family) && !path.contains("Variable")
        })
    };

    match chosen {
        Some(family) => {
            if !static_face(&family) {
                eprintln!(
                    "kanade: appearance.font {family} has no static SemiBold, so its weights may \
                     all draw regular"
                );
            }

            app.font(&family)
        }
        None => {
            crate::face::install();

            app.font(FONT)
        }
    }
}

// how faded a control is while it has nothing to do
pub const DISABLED: f32 = 0.35;

pub mod text {
    // secondary lines, captions, a status under a name
    pub const LABEL_SMALL: f32 = 12.0;

    // a title in a row or a Compact Presentation
    pub const LABEL: f32 = 13.0;

    pub const BODY: f32 = 14.0;

    // a Peek Presentation's label
    pub const BODY_LARGE: f32 = 15.0;

    // a query being typed
    pub const TITLE: f32 = 16.0;

    // a Peek's numbers, an Expanded Presentation's label
    pub const TITLE_LARGE: f32 = 17.0;

    // the one number a Surface is about, like the temperature now
    pub const DISPLAY: f32 = 44.0;

    pub const MEDIUM: u16 = 500;
    pub const SEMIBOLD: u16 = 600;
}

pub mod radius {
    // a progress bar, a caret, a scroll thumb
    pub const HAIRLINE: f32 = 1.5;

    // cover art or a notification tile in a Compact Presentation
    pub const ART_COMPACT: f32 = 6.0;

    // an app icon without a picture
    pub const ICON: f32 = 7.0;

    // cover art or a notification tile in a Peek Presentation
    pub const ART_PEEK: f32 = 9.0;

    // a notification tile on a card
    pub const TILE: f32 = 10.0;

    // a launcher row, cover art on the Media Surface
    pub const ROW: f32 = 12.0;

    // a notification card
    pub const CARD: f32 = 16.0;
}

pub mod space {
    // a Surface's content from the body's edge, concentric with its corner
    pub const INSET: f32 = 20.0;

    // a pressable target is never smaller than 24 px
    pub const TARGET: f32 = 24.0;
}

// roles used by the Settings window and other shell-owned windows, in the tone `appearance.tone` sets
pub fn roles() -> ThemeRoles {
    crate::appearance::roles(crate::config::get().appearance.tone)
}

impl ThemeRoles {
    // a track or Satellite as it shows, `surface_container_high` over the surface
    pub fn track(&self) -> Color {
        over(self.surface_container_high, self.surface)
    }

    // the grounds a control sits on in a window: the page, a card, the sidebar, a trough
    fn grounds(&self) -> [Color; 4] {
        [
            self.surface,
            self.card(),
            over(self.surface_container, self.surface),
            self.track(),
        ]
    }

    /*
     * the edge of a control (a field, a trough, an off switch), the faintest step from the surface
     * toward the secondary text that is 3:1 against every ground (WCAG 1.4.11). The roles'
     * `outline` is a quiet hairline for lines that carry nothing
     */
    pub fn edge(&self) -> Color {
        let grounds = self.grounds();

        let variant = over(self.on_surface_variant, self.surface);

        (1..=20)
            .map(|step| mix(self.surface, variant, step as f32 / 20.0))
            .find(|edge| grounds.iter().all(|&ground| contrast(*edge, ground) >= 3.1))
            .unwrap_or(self.on_surface)
    }

    // a card on the page: a step the eye finds without a shadow, where `surface_container` is faint
    pub fn card(&self) -> Color {
        mix(self.surface, self.on_surface, 0.05)
    }

    // the keyboard's ring: the accent while it is 3:1 against every ground, else the text color
    pub fn ring(&self) -> Color {
        let grounds = self.grounds();

        if grounds
            .iter()
            .all(|&ground| contrast(self.primary, ground) >= 3.1)
        {
            self.primary
        } else {
            self.on_surface
        }
    }
}

// `color` at `opacity`, to fade a single text without a group of its own
pub fn faded(color: Color, opacity: f32) -> Color {
    let alpha = (f32::from(color.alpha()) * opacity.clamp(0.0, 1.0)).round() as u8;

    Color::rgba(color.red(), color.green(), color.blue(), alpha)
}

// chroma, how far from grey; unlike HSL saturation a near-black red does not count as vivid
pub fn saturation(color: Color) -> f32 {
    let [r, g, b] = channels(color);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));

    max - min
}

pub fn mix(from: Color, to: Color, amount: f32) -> Color {
    let [from, to] = [channels(from), channels(to)];
    let channel = |index: usize| {
        let value = from[index] + (to[index] - from[index]) * amount;

        (value * 255.0).round() as u8
    };

    Color::rgb(channel(0), channel(1), channel(2))
}

// WCAG contrast ratio, 1 to 21
pub fn contrast(a: Color, b: Color) -> f32 {
    let (a, b) = (luminance(a), luminance(b));

    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn luminance(color: Color) -> f32 {
    let linear = |channel: f32| {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = channels(color).map(linear);

    0.2126 * r + 0.7152 * g + 0.0722 * b
}

// `top` drawn over the opaque `under`
pub fn over(top: Color, under: Color) -> Color {
    let share = f32::from(top.alpha()) / 255.0;
    let mix = |top: u8, under: u8| {
        (f32::from(top) * share + f32::from(under) * (1.0 - share)).round() as u8
    };

    Color::rgba(
        mix(top.red(), under.red()),
        mix(top.green(), under.green()),
        mix(top.blue(), under.blue()),
        under.alpha(),
    )
}

pub fn channels(color: Color) -> [f32; 3] {
    [color.red(), color.green(), color.blue()].map(|channel| f32::from(channel) / 255.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles(surface: Color, text: Color, primary: Color, container: u8, high: u8) -> ThemeRoles {
        let alpha =
            |color: Color, alpha| Color::rgba(color.red(), color.green(), color.blue(), alpha);

        ThemeRoles {
            primary,
            on_primary: surface,
            surface,
            on_surface: text,
            on_surface_variant: text,
            surface_container: alpha(text, container),
            surface_container_high: alpha(text, high),
            outline: alpha(text, 64),
        }
    }

    #[test]
    fn a_control_edge_interpolates_to_the_composited_variant() {
        let roles = ThemeRoles {
            surface: Color::rgb(246, 246, 250),
            on_surface_variant: Color::rgba(60, 60, 67, 210),
            ..ISLAND_LIGHT
        };
        let endpoint = over(roles.on_surface_variant, roles.surface);

        assert_eq!(endpoint, Color::rgb(93, 93, 99));
        assert_eq!(mix(roles.surface, endpoint, 1.0), endpoint);
    }

    // a control's edge and the ring read at 3:1 on every ground, in dark and light tones and on a
    // wallpaper's accent that is too dim or too bright for its tone
    #[test]
    fn a_control_edge_and_the_ring_reach_three_to_one_in_every_tone() {
        let dark = Color::rgb(18, 19, 22);
        let light = Color::rgb(250, 248, 252);
        let near_black = Color::rgb(230, 225, 232);
        let near_white = Color::rgb(30, 28, 34);

        for roles in [
            ISLAND_DARK,
            ISLAND_LIGHT,
            roles(dark, near_black, Color::rgb(208, 188, 255), 20, 36),
            roles(light, near_white, Color::rgb(103, 80, 164), 14, 26),
            roles(dark, near_black, Color::rgb(60, 50, 90), 20, 36),
            roles(light, near_white, Color::rgb(230, 220, 255), 14, 26),
        ] {
            for ground in roles.grounds() {
                assert!(contrast(roles.edge(), ground) >= 3.0, "edge on {ground:?}");
                assert!(contrast(roles.ring(), ground) >= 3.0, "ring on {ground:?}");
            }
        }
    }
}
