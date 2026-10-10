/*
 * wallpapers, as a Launcher provider (#145): a query starting with `@` searches the images in the
 * wallpaper directory by name, and pressing one sets it. Only on that prefix, so a search for apps
 * never fills with images
 */

use std::path::{Path, PathBuf};

use super::provider::{self, Action, Answer, Fit, LauncherProvider, Mark};

pub const PREFIX: char = '@';

pub struct Wallpapers<'a> {
    // by name, as `wallpaper::images` lists them
    pub images: &'a [PathBuf],

    // the one Kanade set last, which says so
    pub current: Option<&'a Path>,
}

impl LauncherProvider for Wallpapers<'_> {
    // by name within a fit; `@` alone lists every wallpaper
    fn find(&self, query: &str) -> Vec<Answer> {
        let Some(term) = query.strip_prefix(PREFIX) else {
            return Vec::new();
        };

        let term = term.trim().to_lowercase();

        self.images
            .iter()
            .filter_map(|path| {
                let title = path.file_stem()?.to_string_lossy().into_owned();

                let fit = if term.is_empty() {
                    Fit::Anywhere
                } else {
                    provider::fit(&term, &title.to_lowercase())?
                };

                let detail = if self.current == Some(path.as_path()) {
                    "Current wallpaper"
                } else {
                    "Wallpaper"
                };

                Some(Answer {
                    fit,
                    title,
                    detail: String::from(detail),
                    mark: Mark::Photo(path.clone()),
                    action: Action::Wallpaper(path.clone()),
                })
            })
            .collect()
    }
}
