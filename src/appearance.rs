//! Appearance adapters and wallpaper-derived theme roles.

mod adapters;
mod child;
mod matugen;
pub mod pywalfox;

use crate::config::Config;
use crate::look::Tone;
use crate::theme::ThemeRoles;

pub const MATUGEN: &str = "matugen";

pub fn sync(config: &Config) {
    adapters::sync(config);
}

pub fn tone_changed(tone: Tone) {
    matugen::tone_changed(tone);
}

pub fn wallpaper(path: std::path::PathBuf) {
    matugen::wallpaper(path);
}

pub fn seed_wallpaper(path: std::path::PathBuf) {
    matugen::seed_wallpaper(path);
}

// a wallpaper shown from outside Kanade, such as `awww img`
pub fn shown_wallpaper(path: std::path::PathBuf) {
    matugen::shown_wallpaper(path);
}

pub fn roles(tone: Tone) -> ThemeRoles {
    matugen::roles(tone)
}

// the Island, Dock and Banners' tinted solid ramp (ADR 0050)
pub fn island_roles() -> ThemeRoles {
    matugen::island_roles()
}
