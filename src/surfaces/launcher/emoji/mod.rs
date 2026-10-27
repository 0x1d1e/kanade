/*
 * a `:` query searches emoji by shortcode or name, and selecting one copies it; other Launcher
 * searches stay clear. Share the Emoji Surface's table and load pictures only for matches.
 */

use super::super::emoji::data;
use super::provider::{self, Action, Answer, Fit, LauncherProvider, Mark};

pub const PREFIX: char = ':';

pub struct Emoji;

impl LauncherProvider for Emoji {
    // `:` alone lists every emoji
    fn find(&self, query: &str) -> Vec<Answer> {
        let Some(term) = query.strip_prefix(PREFIX) else {
            return Vec::new();
        };

        let term = term.strip_suffix(PREFIX).unwrap_or(term);
        let terms = data::terms(term);
        let normalized = terms.join(" ");
        let terms: Vec<&str> = terms.iter().map(String::as_str).collect();

        data::table()
            .entries
            .iter()
            .filter(|entry| entry.matches(&terms))
            .map(|entry| {
                let fit = if terms.is_empty() {
                    Fit::Anywhere
                } else {
                    fit(&normalized, entry)
                };

                answer(entry.emoji, fit)
            })
            .collect()
    }
}

// the best of how its shortcodes and its name fit the term
fn fit(term: &str, entry: &data::Entry) -> Fit {
    entry
        .shortcodes
        .iter()
        .chain([&entry.name])
        .filter_map(|said| provider::fit(term, said))
        .min()
        .unwrap_or(Fit::Anywhere)
}

// its name, as a title, over its shortcodes; the emoji as its picture, else as a letter
fn answer(emoji: &'static emojis::Emoji, fit: Fit) -> Answer {
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
        mark: Mark::Emoji(emoji.as_str()),
        action: Action::Copy(emoji.as_str().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn underscore_shortcode_keeps_exact_ranking() {
        let (emoji, shortcode) = emojis::iter()
            .find_map(|emoji| {
                emoji
                    .shortcodes()
                    .find(|shortcode| shortcode.contains('_'))
                    .map(|shortcode| (emoji, shortcode.to_owned()))
            })
            .expect("an underscore shortcode");
        let entry = data::table()
            .entries
            .iter()
            .find(|entry| entry.emoji == emoji)
            .expect("shortcode entry");
        let query = format!(":{shortcode}");

        let answer = Emoji
            .find(&query)
            .into_iter()
            .find(|answer| answer.action == Action::Copy(emoji.as_str().to_owned()))
            .expect("shortcode match");

        assert_eq!(fit(&data::terms(&shortcode).join(" "), entry), Fit::Exact);
        assert_eq!(answer.fit, Fit::Exact);
    }
}
