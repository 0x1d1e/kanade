//! What each output's lock screen shows, as `kanade-lock` draws it: fonts, face and wallpaper.

use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use fontconfig::{CharSet, FC_FAMILY, FC_WEIGHT, Fontconfig, Pattern};
use kanade_lock::{Backdrop, FontFile, Fonts, Image, Palette, Picture, Scene};
use kanade_runtime::service::Service;
use kanade_runtime::{Color, LayerWindow, Monitor, Rectangle};

use crate::look::{LockBackdrop, Tone};
use crate::sources::wallpaper;
use crate::{clock, config, glass, theme};

use super::who::Who;
use super::{Said, Status, locker};

/*
 * as macOS's: the date over a large time near the top, who is signed in and a pill of glass to
 * type the password in near the bottom, over the output's wallpaper as `lock.backdrop` says. A
 * hidden window that only hands `kanade-lock` the scene of its monitor, unlocked too, so a lock
 * shows it from its first frame; it runs again as anything it reads changes, as the minute turns
 */
pub fn scene(monitor: &Monitor) -> LayerWindow {
    if let Ok(locker) = locker() {
        locker.scene(&monitor.name, scened(monitor));
    }

    LayerWindow::new()
        .width(1.0)
        .height(1.0)
        .visible(false)
        .namespace("kanade-lock")
        .child(Rectangle::new().width(0.0).height(0.0))
}

fn scened(monitor: &Monitor) -> Scene {
    let backdrop = config::get().lock_backdrop;

    // reading subscribes the scene to the wallpaper awww shows
    let wallpaper = (backdrop != LockBackdrop::Solid)
        .then(|| wallpaper::Shown::read().on(&monitor.name).cloned())
        .flatten();

    // light text on dark glass over a wallpaper, as macOS's; the island's own over a solid color
    let (roles, dark) = match &wallpaper {
        Some(_) => (theme::ISLAND_DARK, true),
        None => (theme::island(), config::get().appearance.tone == Tone::Dark),
    };

    let backdrop = match wallpaper {
        Some(wallpaper) => Backdrop::Wallpaper {
            image: Image::new(wallpaper.path),
            blurred: backdrop == LockBackdrop::Blurred,
            shade: color(theme::lock_shade(
                wallpaper.light,
                backdrop == LockBackdrop::Dimmed,
            )),
            otherwise: color(roles.surface),
        },
        None => Backdrop::Solid(color(roles.surface)),
    };

    // reading subscribes the scene to what it says and who is signed in
    let said = *Said::read();
    let who = Who::read().clone();

    let status = match said.status {
        Status::None => "",
        Status::Checking => "Checking…",
        Status::Sleeping => "Locked for sleep",
        Status::Again => "Enter your password again",
        Status::Wrong => "Wrong password",
    };

    // the date and status are English, in the face
    let fonts = fonts_for(&who.name);

    Scene {
        backdrop,
        date: clock::today().format("%A, %-d %B").to_string(),
        time: clock::now(config::on(&monitor.name).clock),
        name: who.name,
        face: who.face.map(Image::new),
        status: String::from(status),
        critical: said.status == Status::Wrong,
        refused: said.wrong,
        palette: Palette {
            text: color(roles.on_surface),
            muted: color(roles.on_surface_variant),
            critical: color(theme::SEMANTIC.critical),
            dark,
            highlight: glass::highlight(),
            tint: glass::lock_tint(dark).map(color),
        },
        fonts,
    }
}

fn color(color: Color) -> kanade_lock::Color {
    kanade_lock::Color::rgba(color.red(), color.green(), color.blue(), color.alpha())
}

/*
 * the shell's family (`theme::font`) in the weights the lock screen sets, found once; the time in
 * Inter Display, as SF Pro sets large text in its display cut, when the family is Inter
 */
fn fonts() -> &'static Fonts {
    static FONTS: OnceLock<Fonts> = OnceLock::new();

    FONTS.get_or_init(|| {
        let family = family();
        let fontconfig = Fontconfig::new();
        let font = |family: &str, weight| {
            fontconfig
                .as_ref()
                .and_then(|fontconfig| font(fontconfig, family, weight))
        };

        // fontconfig's weights, and the same as CSS's, which a variable font is set to
        let regular = font(family, (80, 400.0)).unwrap_or_default();
        let semibold = font(family, (180, 600.0)).unwrap_or_default();

        // the time in Inter Display where Inter is the face and it is installed, else in the face
        let display = family
            .eq_ignore_ascii_case("Inter")
            .then(|| font("Inter Display", (180, 600.0)))
            .flatten()
            .filter(|display| display.path != semibold.path || display.index != semibold.index)
            .unwrap_or_else(|| semibold.clone());

        Fonts {
            medium: font(family, (100, 500.0)).unwrap_or_default(),
            regular,
            semibold,
            display,
            fallback: None,
        }
    })
}

// the shell's family, as at the first lock screen: `appearance.font` takes a restart
fn family() -> &'static str {
    static FAMILY: OnceLock<String> = OnceLock::new();

    FAMILY.get_or_init(|| {
        config::get()
            .appearance
            .font
            .clone()
            .unwrap_or_else(|| String::from("Inter"))
    })
}

/*
 * the fonts, with a fallback for `text` beyond ASCII, which the face may lack, as a name in
 * another script: the face fontconfig likes best with all its characters, set to the weight of
 * each face it stands in for. The last one is kept
 */
fn fonts_for(text: &str) -> Fonts {
    static LAST: Mutex<Option<(String, Option<FontFile>)>> = Mutex::new(None);

    let mut fonts = fonts().clone();
    if text.is_ascii() {
        return fonts;
    }

    let mut last = LAST.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((kept, fallback)) = last.as_ref()
        && kept == text
    {
        fonts.fallback = fallback.clone();
        return fonts;
    }

    let fallback = Fontconfig::new().and_then(|fontconfig| {
        let mut characters = CharSet::new(&fontconfig).ok()?;
        for character in text.chars().filter(|character| !character.is_ascii()) {
            characters.add_char(character).ok()?;
        }

        let mut pattern = Pattern::new(&fontconfig).ok()?;
        pattern
            .add_string(FC_FAMILY, &CString::new(family()).ok()?)
            .ok()?;
        pattern.add_integer(FC_WEIGHT, 180).ok()?;
        pattern.add_charset(characters).ok()?;

        let found = pattern.font_match().ok()?;
        Some(FontFile {
            path: PathBuf::from(found.filename().ok()?),
            index: u32::try_from(found.face_index().ok()?).ok()?,
            weight: 0.0,
        })
    });

    *last = Some((String::from(text), fallback.clone()));
    fonts.fallback = fallback;
    fonts
}

/*
 * the file fontconfig matches `family` in `weight` to: its fallback for a family not installed,
 * except for Inter Display, which only Inter's own replaces. No file draws no text
 */
fn font(fontconfig: &Fontconfig, family: &str, (weight, css): (i32, f32)) -> Option<FontFile> {
    let mut pattern = Pattern::new(fontconfig).ok()?;
    pattern
        .add_string(FC_FAMILY, &CString::new(family).ok()?)
        .ok()?;
    pattern.add_integer(FC_WEIGHT, weight).ok()?;

    let found = pattern.font_match().ok()?;
    if family == "Inter Display"
        && !found
            .get_string(FC_FAMILY)
            .ok()?
            .eq_ignore_ascii_case(family)
    {
        return None;
    }

    Some(FontFile {
        path: PathBuf::from(found.filename().ok()?),
        index: u32::try_from(found.face_index().ok()?).ok()?,
        weight: css,
    })
}

// a wallpaper or face, as `kanade-lock` draws it
pub(super) fn decode(path: &Path) -> Option<Picture> {
    let (width, height, rgba) = wallpaper::decoded(path)?;

    Some(Picture {
        width,
        height,
        rgba,
    })
}
