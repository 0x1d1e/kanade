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

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(answers: &[Answer]) -> Vec<(&str, Fit)> {
        answers
            .iter()
            .map(|answer| (answer.title.as_str(), answer.fit))
            .collect()
    }

    #[test]
    fn only_a_query_starting_with_the_prefix_finds_wallpapers_by_name() {
        let images = [
            PathBuf::from("/w/Dark Sea.png"),
            PathBuf::from("/w/seaside.jpg"),
            PathBuf::from("/w/forest.webp"),
        ];
        let wallpapers = Wallpapers {
            images: &images,
            current: Some(Path::new("/w/forest.webp")),
        };

        assert!(wallpapers.find("sea").is_empty());
        assert_eq!(
            titles(&wallpapers.find("@ SEA")),
            [("Dark Sea", Fit::Word), ("seaside", Fit::Prefix)]
        );
        assert_eq!(wallpapers.find("@").len(), 3);
        assert!(wallpapers.find("@zzz").is_empty());

        let forest = &wallpapers.find("@forest")[0];
        assert_eq!(forest.detail, "Current wallpaper");
        assert_eq!(forest.action, Action::Wallpaper(images[2].clone()));
        assert_eq!(wallpapers.find("@dark")[0].detail, "Wallpaper");
    }
}
