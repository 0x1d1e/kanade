//! The island's colors (plan 7). Near-black by default; with `theme.palette` in the config (#39)
//! the body and foreground follow an image, usually the wallpaper, through Amane's `Palette`, and
//! change with it. Either way the body stays near-black and both stay near grey, so amber, red and
//! green keep their meaning, which no theme changes.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use amane::{Color, Palette, Service};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    // the body, quiet at rest; opaque, as its shadow has no piece under its middle (#45)
    pub body: Color,

    // high-contrast foreground for text on the body
    pub fg: Color,

    // a Satellite or a badge, a step above the body so it reads beside it
    pub dot: Color,

    // secondary text, like an artist under a title
    pub muted: Color,

    // a notification on the body, a quieter step than a Satellite so text on it keeps its contrast
    pub card: Color,

    // the ring of a pinned island, quiet enough not to read as an alert
    pub pin: Color,

    // where cover art goes while there is none to draw
    pub art: Color,
}

pub const DARK: Theme = Theme {
    body: Color::rgb(12, 12, 14),
    fg: Color::rgb(242, 242, 247),
    dot: Color::rgb(44, 44, 50),
    muted: Color::rgb(152, 152, 160),
    card: Color::rgb(28, 28, 32),
    pin: Color::rgb(96, 96, 106),
    art: Color::rgb(44, 44, 50),
};

// a low battery, a warning that waits (plan 7)
pub const AMBER: Color = Color::rgb(255, 176, 32);

// reserved for a microphone or camera in use, which green reads as (plan 7)
pub const GREEN: Color = Color::rgb(48, 209, 88);

// reserved for critical, like a battery about to die (plan 7)
pub const RED: Color = Color::rgb(255, 69, 58);

// a palette body is at most this light, about rgb(28, 28, 30): still near-black (plan 7)
const BODY_LUMINANCE: f32 = 0.012;

// how far from grey a palette body and foreground may be, so neither reads as a state color
const BODY_CHROMA: f32 = 0.06;
const FG_CHROMA: f32 = 0.1;

// a palette foreground on its body, near the default's 17
const FG_CONTRAST: f32 = 15.0;

// secondary text on a card, WCAG AA for small text
const MUTED_CONTRAST: f32 = 4.5;

// the default's steps from body to foreground, so a palette theme keeps its hierarchy
const DOT: f32 = 0.14;
const MUTED: f32 = 0.61;
const CARD: f32 = 0.07;
const PIN: f32 = 0.365;

// colors picked from the image, as many as a whole shell's theme needs (Amane's `Palette`)
const PICKED: usize = 16;

// set while `follow` has an image open, so the near-black theme never reads the Palette
static FOLLOWING: AtomicBool = AtomicBool::new(false);

// the last palette colors and the theme they gave, since every color a view draws asks
static LAST: Mutex<Option<(Color, Color, Theme)>> = Mutex::new(None);

/*
 * at start and on each reload that changes it (#102): the theme follows the image at `path` from
 * now on, and Amane's Palette picks its colors again whenever the file changes. None is the default
 * theme; Amane cannot close an image, so a Palette already open keeps watching it, unread
 */
pub fn follow(path: Option<&str>) {
    let Some(path) = path else {
        FOLLOWING.store(false, Ordering::Relaxed);
        return;
    };

    if !Path::new(path).is_file() {
        eprintln!(
            "kanade: theme.palette {path} is not a file yet, the default theme shows until it is"
        );
    }

    Palette::write().open(path, PICKED);

    FOLLOWING.store(true, Ordering::Relaxed);
}

// the near-black theme until the image gives colors, the image's after
pub fn current() -> Theme {
    if !FOLLOWING.load(Ordering::Relaxed) {
        return DARK;
    }

    let (background, foreground) = {
        let palette = Palette::read();

        if palette.colors().is_empty() {
            return DARK;
        }

        (palette.background(), palette.foreground())
    };

    let mut last = LAST.lock().unwrap_or_else(PoisonError::into_inner);

    match *last {
        Some((b, f, theme)) if (b, f) == (background, foreground) => theme,
        _ => {
            let theme = Theme::from_palette(background, foreground);
            *last = Some((background, foreground, theme));

            theme
        }
    }
}

pub fn body() -> Color {
    current().body
}

pub fn fg() -> Color {
    current().fg
}

pub fn dot() -> Color {
    current().dot
}

pub fn muted() -> Color {
    current().muted
}

pub fn card() -> Color {
    current().card
}

pub fn pin() -> Color {
    current().pin
}

pub fn art() -> Color {
    current().art
}

impl Theme {
    /*
     * the image's darkest color, darkened to near-black and kept near grey, under its readable
     * color, lightened until it reads as the default does; every other color steps between them
     */
    pub fn from_palette(background: Color, foreground: Color) -> Theme {
        let black = Color::rgb(0, 0, 0);
        let white = Color::rgb(255, 255, 255);

        let body = toward(grey(background, BODY_CHROMA), black, |color| {
            luminance(color) <= BODY_LUMINANCE
        });
        let fg = toward(grey(foreground, FG_CHROMA), white, |color| {
            contrast(color, body) >= FG_CONTRAST
        });
        let step = |amount| mix(body, fg, amount);
        let card = step(CARD);

        Theme {
            body,
            fg,
            dot: step(DOT),
            muted: toward(step(MUTED), fg, |color| {
                contrast(color, card) >= MUTED_CONTRAST
            }),
            card,
            pin: step(PIN),
            art: step(DOT),
        }
    }
}

// `color` moved toward grey until its chroma is at most `chroma`
fn grey(color: Color, chroma: f32) -> Color {
    let [r, g, b] = [color.red(), color.green(), color.blue()].map(u16::from);
    let mean = ((r + g + b) / 3) as u8;

    toward(color, Color::rgb(mean, mean, mean), |color| {
        saturation(color) <= chroma
    })
}

// the first of `from` mixed toward `to` in small steps that is `good`, else `to`
fn toward(from: Color, to: Color, good: impl Fn(Color) -> bool) -> Color {
    const STEPS: u8 = 50;

    (0..=STEPS)
        .map(|step| mix(from, to, f32::from(step) / f32::from(STEPS)))
        .find(|&color| good(color))
        .unwrap_or(to)
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

pub fn channels(color: Color) -> [f32; 3] {
    [color.red(), color.green(), color.blue()].map(|channel| f32::from(channel) / 255.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // the near-black theme is the plan 7 one, and tests never follow an image
    #[test]
    fn without_a_palette_the_theme_is_near_black() {
        assert_eq!(current(), DARK);
        assert!(contrast(DARK.fg, DARK.body) > 17.0);
    }

    /*
     * #39: whatever the image, the body is near-black and near grey, text reads on the body and on
     * a card, and a Satellite's state colors stand out from it
     */
    #[test]
    fn a_palette_theme_keeps_the_contrast_and_the_color_meanings() {
        let samples = [0, 64, 128, 192, 255];
        let mut colors = Vec::new();

        for r in samples {
            for g in samples {
                for b in samples {
                    colors.push(Color::rgb(r, g, b));
                }
            }
        }

        let some: Vec<Color> = colors.iter().copied().step_by(4).collect();

        for &background in &colors {
            for &foreground in &some {
                let theme = Theme::from_palette(background, foreground);
                let at = format!("{background:?} {foreground:?} gave {theme:?}");

                assert!(luminance(theme.body) <= BODY_LUMINANCE, "{at}");
                assert!(saturation(theme.body) <= BODY_CHROMA + 0.01, "{at}");
                assert!(saturation(theme.fg) <= FG_CHROMA + 0.01, "{at}");
                assert!(contrast(theme.fg, theme.body) >= FG_CONTRAST, "{at}");
                assert!(contrast(theme.fg, theme.card) >= 7.0, "{at}");
                assert!(contrast(theme.muted, theme.card) >= MUTED_CONTRAST, "{at}");
                assert!(contrast(theme.muted, theme.body) >= 4.5, "{at}");

                for state in [AMBER, GREEN, RED] {
                    assert!(contrast(state, theme.dot) >= 3.0, "{state:?} on {at}");
                }
            }
        }
    }

    // a tinted wallpaper tints the body, quietly
    #[test]
    fn a_palette_body_takes_the_image_hue() {
        let theme = Theme::from_palette(Color::rgb(20, 24, 60), Color::rgb(200, 210, 255));

        assert!(theme.body.blue() > theme.body.red(), "{:?}", theme.body);
        assert!(theme.fg.blue() > theme.fg.red(), "{:?}", theme.fg);
    }
}
