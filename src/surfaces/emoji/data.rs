/*
 * every emoji Kanade offers, in Unicode's order, one table shared by the Launcher's `:` provider and
 * the Emoji Surface: what it is called, the shortcodes it answers, and the group it belongs to. The
 * name and the shortcodes are lowered once here, so a search compares them instead of building them
 * for every redraw
 */

use std::ops::Range;
use std::sync::LazyLock;

pub struct Entry {
    pub emoji: &'static emojis::Emoji,

    // lowercase
    pub name: String,

    // each with its `_` as a space, so a typed space matches
    pub shortcodes: Vec<String>,
}

impl Entry {
    // whether every one of the terms, already lowered and split, is somewhere in its name or one of
    // its shortcodes; no term finds every emoji
    pub fn matches(&self, terms: &[&str]) -> bool {
        terms.iter().all(|term| {
            self.name.contains(term)
                || self
                    .shortcodes
                    .iter()
                    .any(|shortcode| shortcode.contains(term))
        })
    }
}

pub struct Table {
    // every emoji, in Unicode's order, skin tones left to the default
    pub entries: Vec<Entry>,

    // each group, in that order, and where its emoji are in `entries`
    pub groups: Vec<(emojis::Group, Range<usize>)>,
}

impl Table {
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

// the one table, built the first time a search asks for it
static TABLE: LazyLock<Table> = LazyLock::new(|| {
    let mut entries: Vec<Entry> = Vec::new();
    let mut groups: Vec<(emojis::Group, Range<usize>)> = Vec::new();

    for group in emojis::Group::iter() {
        let start = entries.len();

        entries.extend(group.emojis().map(|emoji| {
            Entry {
                emoji,
                name: emoji.name().to_lowercase(),
                shortcodes: emoji
                    .shortcodes()
                    .map(|shortcode| shortcode.replace('_', " "))
                    .collect(),
            }
        }));

        groups.push((group, start..entries.len()));
    }

    Table { entries, groups }
});

pub fn table() -> &'static Table {
    &TABLE
}

// what a group is called, as its header says it
pub fn title(group: emojis::Group) -> &'static str {
    match group {
        emojis::Group::SmileysAndEmotion => "Smileys & Emotion",
        emojis::Group::PeopleAndBody => "People & Body",
        emojis::Group::AnimalsAndNature => "Animals & Nature",
        emojis::Group::FoodAndDrink => "Food & Drink",
        emojis::Group::TravelAndPlaces => "Travel & Places",
        emojis::Group::Activities => "Activities",
        emojis::Group::Objects => "Objects",
        emojis::Group::Symbols => "Symbols",
        emojis::Group::Flags => "Flags",
    }
}

// the terms a typed query searches by: lowered, a shortcode's `_` as a space, split on whitespace
pub fn terms(query: &str) -> Vec<String> {
    query
        .replace('_', " ")
        .to_lowercase()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

// whether `emoji`, one the recents keep by its string, answers the terms
pub fn answers(emoji: &emojis::Emoji, terms: &[&str]) -> bool {
    let name = emoji.name().to_lowercase();

    terms.iter().all(|term| {
        name.contains(term)
            || emoji
                .shortcodes()
                .any(|shortcode| shortcode.replace('_', " ").contains(term))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // the table keeps its names lowercase and its shortcodes' `_` as spaces, in the groups' order
    #[test]
    fn the_table_is_named_and_shortcoded_as_it_is_searched() {
        let table = table();

        assert_eq!(table.groups.len(), 9);
        assert_eq!(table.len(), table.entries.len());
        assert!(table.len() > 1800);

        let thumbs = table
            .entries
            .iter()
            .find(|entry| entry.emoji.as_str() == "\u{1f44d}")
            .expect("thumbs up");

        assert_eq!(thumbs.name, "thumbs up");
        assert!(thumbs.shortcodes.iter().any(|shortcode| shortcode == "+1"));
        assert!(
            thumbs
                .shortcodes
                .iter()
                .all(|shortcode| !shortcode.contains('_'))
        );

        // every group's range holds only its own emoji, in order
        for (group, range) in &table.groups {
            assert!(!range.is_empty(), "{group:?}");
            assert!(
                table.entries[range.clone()]
                    .iter()
                    .all(|entry| entry.emoji.group() == *group)
            );
        }

        assert_eq!(title(emojis::Group::Flags), "Flags");
        assert_eq!(terms("  Thumbs_UP "), ["thumbs", "up"]);
    }
}
