//! The Launcher's providers: each answers a query with what it finds, how well each answer
//! fits, what its row shows and what pressing it does. The seam is Kanade's own; there is no
//! plugin API (design.md, Launcher).

use std::path::PathBuf;

use crate::icon::Icon;
use crate::sources::launch::Launch;

use super::destinations::Destination;

/*
 * how well an answer fits the query, best first. Every provider ranks on this one scale, so their
 * answers interleave; within a fit, the provider listed first, then its own order
 */
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fit {
    // what the query asks for, like the value of a sum
    Answer,

    // an app started lately, before the rest while nothing is typed
    Recent,

    // the name is the query
    Exact,

    // the name starts with it
    Prefix,

    // a word in the name starts with it
    Word,

    // the name has it
    Within,

    // the name has every word of it
    Terms,

    // every word of it is somewhere in what the answer says about itself
    Anywhere,
}

// what a row shows before its title
#[derive(Debug, Clone, PartialEq)]
pub enum Mark {
    // an app's icon
    Picture(PathBuf),

    // one of Kanade's own icons, like a Settings page's
    Icon(Icon),

    // a letter or sign on the quiet tile
    Tile(String),
}

// what pressing an answer does; each closes the island once done
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Action {
    // an app's way to start, and its id for the recents
    Launch(Launch, String),

    // one of its Desktop Actions: the app's way to start, its id for the recents, then the action's id
    Perform(Launch, String, String),

    // focuses the open window of this niri id
    Focus(u64),

    // puts the text on the clipboard
    Copy(String),

    // opens another Surface or Settings, as the island does
    Open(Destination),

    // runs this Kanade verb through the shell's own socket, as the CLI does (ADR 0112)
    Run(&'static [&'static str]),

    // answers the dmenu waiting with the place of the line chosen
    Choose(usize),

    /*
     * opens this `https` address with `xdg-open`, by argv with no shell (ADR 0130). The one answer
     * the Launcher sends what was typed off the machine, so its row says where it goes
     */
    Browse(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub fit: Fit,
    pub title: String,

    // a second line, empty for none
    pub detail: String,

    pub mark: Mark,
    pub action: Action,
}

pub trait LauncherProvider {
    // what it finds for `query`, trimmed but in the case typed; nothing when it has no answer
    fn find(&self, query: &str) -> Vec<Answer>;
}

// what every provider finds for `query`, best first
pub fn ranked(providers: &[&dyn LauncherProvider], query: &str) -> Vec<Answer> {
    let query = query.trim();

    let mut answers: Vec<Answer> = providers
        .iter()
        .flat_map(|provider| provider.find(query))
        .collect();

    // Fit remains the outer ordering; scores sort only answers in the same tier. `sort_by`
    // is stable, so providers' order and each provider's own order break score ties.
    answers.sort_by(|left, right| {
        left.fit.cmp(&right.fit).then_with(|| {
            let left_score = match_score(query, &left.title).unwrap_or(i32::MIN);
            let right_score = match_score(query, &right.title).unwrap_or(i32::MIN);
            right_score.cmp(&left_score)
        })
    });

    answers
}

/*
 * Match after folding case and common Latin diacritics. Boundary positions are
 * recorded before lowercasing so camelCase remains searchable as separate words.
 */
fn normalized(text: &str) -> Vec<(char, bool)> {
    let chars: Vec<char> = text.chars().collect();
    let mut result = Vec::with_capacity(chars.len());
    for (index, &ch) in chars.iter().enumerate() {
        let camel = index > 0 && chars[index - 1].is_ascii_lowercase() && ch.is_ascii_uppercase();
        if let Some(folded) = transliterate(ch) {
            for folded in folded.chars() {
                result.push((folded.to_ascii_lowercase(), camel));
            }
        } else {
            for folded in ch.to_lowercase() {
                result.push((folded, camel));
            }
        }
    }
    result
}

// the text as the matcher reads it: lowercase, common Latin diacritics transliterated
fn folded(text: &str) -> String {
    normalized(text).into_iter().map(|(ch, _)| ch).collect()
}

fn transliterate(ch: char) -> Option<&'static str> {
    match ch {
        'À'..='Å' | 'à'..='å' => Some("a"),
        'Ç' | 'ç' => Some("c"),
        'È'..='Ë' | 'è'..='ë' => Some("e"),
        'Ì'..='Ï' | 'ì'..='ï' => Some("i"),
        'Ñ' | 'ñ' => Some("n"),
        'Ò'..='Ö' | 'ò'..='ö' | 'Ø' | 'ø' => Some("o"),
        'Ù'..='Ü' | 'ù'..='ü' => Some("u"),
        'Ý' | 'ý' | 'ÿ' => Some("y"),
        'Æ' | 'æ' => Some("ae"),
        'ß' => Some("ss"),
        'Ð' | 'ð' | 'Đ' | 'đ' => Some("d"),
        'Ł' | 'ł' => Some("l"),
        'Œ' | 'œ' => Some("oe"),
        'Þ' | 'þ' => Some("th"),
        'ı' => Some("i"),
        _ => None,
    }
}

fn separator(ch: char) -> bool {
    matches!(
        ch,
        '\t' | '\n' | ' ' | '-' | '.' | '/' | '(' | ')' | '[' | ']'
    )
}

// Medium sensitivity: score >= 1.5 * (query length - 2) + 4.
fn match_score(query: &str, name: &str) -> Option<i32> {
    let query = normalized(query);
    let name = normalized(name);
    if query.is_empty() || query.len() > name.len() {
        return None;
    }
    if query
        .iter()
        .map(|(ch, _)| ch)
        .eq(name.iter().map(|(ch, _)| ch))
    {
        return Some(i32::MAX);
    }

    let mut previous = vec![None::<i32>; name.len()];
    for (row, &(query_char, _)) in query.iter().enumerate() {
        let mut current = vec![None::<i32>; name.len()];

        // the best score at a column already passed, so a jump over two or more characters
        // starts from the best of them, not only from two columns back. A jump costs one point
        // however far it skips, not one per character skipped: `fx` in `Firefox` scores 5
        let mut passed: Option<i32> = None;
        for (column, &(name_char, camel)) in name.iter().enumerate() {
            let adjacent = column.checked_sub(1).and_then(|before| previous[before]);
            let jump = passed.map(|score| score - 1);
            if let Some(score) = adjacent {
                passed = Some(passed.map_or(score, |best| best.max(score)));
            }

            let sep_match = separator(query_char) && separator(name_char);
            if query_char != name_char && !sep_match {
                continue;
            }
            let points = if sep_match {
                1
            } else if row == 0 && column == 0 {
                4
            } else if camel || column > 0 && separator(name[column - 1].0) {
                3
            } else {
                2
            };
            current[column] = if row == 0 {
                Some(points)
            } else {
                adjacent
                    .into_iter()
                    .chain(jump)
                    .max()
                    .map(|score| score + points)
            };
        }
        previous = current;
    }
    let best = previous.into_iter().flatten().max()?;
    let threshold = 1.5 * (query.len() as f32 - 2.0) + 4.0;
    (best as f32 >= threshold).then_some(best)
}

/*
 * how well `name` answers `query`: equal, starts with it, has a word starting so, has it, then
 * matches its characters in order. Case folds away, but diacritics do not: `Café` is never a prefix
 * of what the name shows. Fit orders categories; the score admits and ranks matches within a fit.
 */
pub fn fit(query: &str, name: &str) -> Option<Fit> {
    let query = query.to_lowercase();
    let name_lowered = name.to_lowercase();
    if name_lowered == query {
        return Some(Fit::Exact);
    }

    // an empty query finds every name, and finds it as a prefix; it never goes through the score,
    // so no threshold can admit or reject it
    if query.is_empty() {
        return Some(Fit::Prefix);
    }

    match_score(&query, name)?;

    if name_lowered.starts_with(&query) {
        return Some(Fit::Prefix);
    }
    if name_lowered
        .match_indices(&query)
        .any(|(at, _)| !name_lowered[..at].ends_with(char::is_alphanumeric))
    {
        return Some(Fit::Word);
    }
    if folded(name).contains(&folded(&query)) {
        return Some(Fit::Within);
    }
    Some(Fit::Terms)
}

#[cfg(test)]
mod tests {
    use super::{Fit, fit, match_score};

    #[test]
    fn fuzzy_match_golden_scores() {
        for (name, query, expected) in [
            ("Firefox", "fir", 8),
            ("Firefox", "fx", 5),
            ("LibreOffice Writer", "lw", 6),
            ("LibreOffice Writer", "lo", 6),
            ("Visual Studio Code", "vsc", 8),
            ("foo-bar", "f-b", 7),
        ] {
            assert_eq!(match_score(query, name), Some(expected), "{name} / {query}");
        }
    }

    #[test]
    fn fuzzy_launcher_regressions() {
        assert_eq!(fit("vsc", "Visual Studio Code"), Some(Fit::Terms));
        assert_eq!(fit("pavu", "pavucontrol"), Some(Fit::Prefix));
        assert_eq!(fit("xyz", "pavucontrol"), None);
    }

    #[test]
    fn fuzzy_match_empty_and_non_ascii_inputs() {
        // an empty query finds every name, as it did before the score: the name is the query when
        // both are empty, else it starts with it
        assert_eq!(fit("", "Firefox"), Some(Fit::Prefix));
        assert_eq!(fit("", ""), Some(Fit::Exact));
        assert_eq!(fit("fir", ""), None);
        assert_eq!(fit("cafe", "Café"), Some(Fit::Within));
        assert_eq!(fit("火", "火狐"), Some(Fit::Prefix));
    }
}
