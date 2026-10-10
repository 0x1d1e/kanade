/*
 * emoji, as a Launcher provider: a query starting with `:` searches them by shortcode (`:smile`,
 * `:thumbs_up:`) and name, and pressing one copies it. Only on that prefix, so a search for apps
 * never fills with emoji
 */

use std::path::PathBuf;
use std::sync::LazyLock;

use super::provider::{self, Action, Answer, Fit, LauncherProvider, Mark};

mod picture;

pub const PREFIX: char = ':';

pub struct Emoji;

// an emoji with what it is searched by, lowercase, a shortcode's `_` as spaces like a name's
struct Searched {
    emoji: &'static emojis::Emoji,
    name: String,
    shortcodes: Vec<String>,
    picture: Option<PathBuf>,
}

// every emoji in Unicode's order, skin tones left to the default
static EMOJI: LazyLock<Vec<Searched>> = LazyLock::new(|| {
    let emoji: Vec<&'static emojis::Emoji> = emojis::iter().collect();

    let texts: Vec<&str> = emoji.iter().map(|emoji| emoji.as_str()).collect();

    emoji
        .into_iter()
        .zip(picture::pictures(&texts))
        .map(|(emoji, picture)| Searched {
            emoji,
            name: emoji.name().to_lowercase(),
            shortcodes: emoji
                .shortcodes()
                .map(|shortcode| shortcode.replace('_', " "))
                .collect(),
            picture,
        })
        .collect()
});

impl LauncherProvider for Emoji {
    // `:` alone lists every emoji
    fn find(&self, query: &str) -> Vec<Answer> {
        let Some(term) = query.strip_prefix(PREFIX) else {
            return Vec::new();
        };

        let term = term.strip_suffix(PREFIX).unwrap_or(term);
        let term = term.trim().replace('_', " ").to_lowercase();

        EMOJI
            .iter()
            .filter_map(|searched| {
                Some(answer(
                    searched.emoji,
                    fit(&term, searched)?,
                    searched.picture.as_ref(),
                ))
            })
            .collect()
    }
}

// the best of how its shortcodes and its name fit the term
fn fit(term: &str, searched: &Searched) -> Option<Fit> {
    if term.is_empty() {
        return Some(Fit::Anywhere);
    }

    searched
        .shortcodes
        .iter()
        .chain([&searched.name])
        .filter_map(|said| provider::fit(term, said))
        .min()
}

// its name, as a title, over its shortcodes; the emoji as its picture, else as a letter
fn answer(emoji: &'static emojis::Emoji, fit: Fit, picture: Option<&PathBuf>) -> Answer {
    let mut name = emoji.name().chars();
    let title = name
        .next()
        .map(|first| first.to_uppercase().chain(name).collect())
        .unwrap_or_default();

    let detail = emoji
        .shortcodes()
        .map(|shortcode| format!("{PREFIX}{shortcode}{PREFIX}"))
        .collect::<Vec<_>>()
        .join(" ");

    Answer {
        fit,
        title,
        detail,
        mark: picture.map_or(Mark::Glyph(emoji.as_str()), |path| {
            Mark::Picture(path.clone())
        }),
        action: Action::Copy(emoji.as_str().to_owned()),
    }
}
