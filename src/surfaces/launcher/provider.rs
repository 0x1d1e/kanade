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

    // the name is a typo or two away; never over a real answer (ADR 0152)
    Typo,
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

    // focuses the open window of this compositor id
    Focus(String),

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

    // runs this argv list directly, no shell (a quicklink's command target)
    Quicklink(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub fit: Fit,

    // offered for any text though it does not match it: a row waiting at the end, not an answer
    // to the query, so it does not keep the web's rows away (ADR 0139)
    pub filler: bool,

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

    ordered(
        providers
            .iter()
            .flat_map(|provider| provider.find(query))
            .collect(),
        query,
    )
}

// `answers` in Fit order, then each fit's best match of `query`; stable, so a provider's own order
// breaks score ties. A single provider's answers sorted by itself: `?` lists the command table
// alone this way (ADR 0135)
pub fn ordered(mut answers: Vec<Answer>, query: &str) -> Vec<Answer> {
    let query = query.trim();

    // Non-filler answers rank ahead of `passes_query` fillers (`false < true`); Fit remains the
    // outer ordering across real answers and scores sort only within the same tier. `sort_by` is
    // stable, so a provider's own order breaks score ties.
    answers.sort_by(|left, right| {
        left.filler
            .cmp(&right.filler)
            .then_with(|| left.fit.cmp(&right.fit))
            .then_with(|| {
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

// Medium sensitivity: score >= 1.5 * (query length - 2) + 4, the length counting the query's
// characters except a separator it skipped (ADR 0152).
fn match_score(query: &str, name: &str) -> Option<i32> {
    let query = normalized(query);
    let name = normalized(name);
    if query.is_empty() {
        return None;
    }
    if query
        .iter()
        .map(|(ch, _)| ch)
        .eq(name.iter().map(|(ch, _)| ch))
    {
        return Some(i32::MAX);
    }

    // a query separator the name does not match is spacing between words the name runs together
    // (`libre office` for `LibreOffice`): the row is skipped, carrying the previous row's scores
    // forward, and the threshold counts the shorter query
    let mut skipped = 0usize;
    let mut previous = vec![None::<i32>; name.len()];
    let mut row = 0usize;
    for &(query_char, _) in &query {
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

        // a character the name does not have ends the match; a separator is spacing to skip
        if !current.iter().any(Option::is_some) {
            if separator(query_char) {
                skipped += 1;
                continue;
            }
            return None;
        }
        previous = current;
        row += 1;
    }

    let best = previous.into_iter().flatten().max()?;
    let threshold = 1.5 * (query.len() as f32 - skipped as f32 - 2.0) + 4.0;
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

    // a name with no separator of its own runs its words together, so the query's separators are
    // spacing the score skips: `libre office` reads `libreoffice` for the tiers too (ADR 0152)
    let compact = query
        .chars()
        .filter(|ch| !separator(*ch))
        .collect::<String>();
    let classified = if compact.is_empty() || name_lowered.chars().any(separator) {
        query.clone()
    } else {
        compact
    };

    if name_lowered == classified {
        return Some(Fit::Exact);
    }

    // an empty query finds every name, and finds it as a prefix; it never goes through the score,
    // so no threshold can admit or reject it
    if query.is_empty() {
        return Some(Fit::Prefix);
    }

    match_score(&query, name)?;

    if name_lowered.starts_with(&classified) {
        return Some(Fit::Prefix);
    }
    if name_lowered
        .match_indices(&classified)
        .any(|(at, _)| !name_lowered[..at].ends_with(char::is_alphanumeric))
    {
        return Some(Fit::Word);
    }
    if folded(name).contains(&folded(&classified)) {
        return Some(Fit::Within);
    }
    Some(Fit::Terms)
}

// the most edits a typo may make: one from four to seven alphanumeric characters, two from eight
fn typo_budget(length: usize) -> Option<usize> {
    match length {
        0..=3 => None,
        4..=7 => Some(1),
        _ => Some(2),
    }
}

// the characters a typo compares: folded letters and digits, so separators and punctuation are
// ignored and `fire-fox` reads `firefox`
fn compared(text: &str) -> Vec<char> {
    folded(text)
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .collect()
}

/*
 * how well `name` answers a query that is a typo of it, after folding case and diacritics: the
 * first alphanumeric character must be the name's, the query needs four of them, and the optimal
 * string alignment must be within `typo_budget` and above zero, a zero being a real fit. Digits are
 * literal: only the same digit answers one (ADR 0152).
 */
pub fn typo(query: &str, name: &str) -> Option<Fit> {
    let query = compared(query);
    let name = compared(name);
    let budget = typo_budget(query.len())?;

    let (Some(first), Some(named)) = (query.first(), name.first()) else {
        return None;
    };
    if first != named {
        return None;
    }

    let distance = alignment(&query, &name, budget)?;
    (distance > 0).then_some(Fit::Typo)
}

// `fit`, then a typo of the name alone, for a provider whose only haystack is the name
pub fn fit_or_typo(query: &str, name: &str) -> Option<Fit> {
    fit(query, name).or_else(|| typo(query, name))
}

// the optimal string alignment distance of `query` and `name`, bounded by `budget`; a digit is
// matched only by the same digit, never edited
fn alignment(query: &[char], name: &[char], budget: usize) -> Option<usize> {
    let beyond = budget + 1;
    let step = |from: usize| if from >= beyond { beyond } else { from + 1 };

    let mut older = vec![beyond; name.len() + 1];
    let mut previous = vec![beyond; name.len() + 1];
    previous[0] = 0;
    for column in 1..=name.len() {
        previous[column] = if name[column - 1].is_alphabetic() {
            step(previous[column - 1])
        } else {
            beyond
        };
    }

    for row in 1..=query.len() {
        let query_char = query[row - 1];
        let mut current = vec![beyond; name.len() + 1];
        current[0] = if query_char.is_alphabetic() {
            step(previous[0])
        } else {
            beyond
        };

        for column in 1..=name.len() {
            let name_char = name[column - 1];
            let mut value = if query_char == name_char {
                previous[column - 1]
            } else if query_char.is_alphabetic() && name_char.is_alphabetic() {
                step(previous[column - 1])
            } else {
                beyond
            };

            // a dropped query character, or an extra character of the name
            if query_char.is_alphabetic() {
                value = value.min(step(previous[column]));
            }
            if name_char.is_alphabetic() {
                value = value.min(step(current[column - 1]));
            }

            // an adjacent transposition, never through a digit
            if row > 1
                && column > 1
                && query_char == name[column - 2]
                && query[row - 2] == name_char
                && query_char.is_alphabetic()
                && name_char.is_alphabetic()
            {
                value = value.min(step(older[column - 2]));
            }

            current[column] = value;
        }

        older = previous;
        previous = current;
    }

    let distance = previous[name.len()];
    (distance <= budget).then_some(distance)
}

#[cfg(test)]
mod tests {
    use super::{Fit, fit, fit_or_typo, match_score, typo};

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
        assert_eq!(fit(" ", ""), None);
        assert_eq!(fit("cafe", "Café"), Some(Fit::Within));
        assert_eq!(fit("火", "火狐"), Some(Fit::Prefix));
    }

    // a separator the name does not spell is skipped for the score and the tiers, and the
    // threshold counts the query without it (ADR 0152)
    #[test]
    fn a_separator_the_name_runs_together_is_skipped() {
        // the name has no separator, so its words are joined by the spelling alone
        assert_eq!(match_score("libre office", "LibreOffice"), Some(25));
        assert_eq!(fit("libre office", "LibreOffice"), Some(Fit::Exact));
        assert_eq!(fit("my app", "MyApp"), Some(Fit::Exact));

        // carrying the previous row forward: a trailing separator keeps the score of what was
        // matched before it, and two skipped separators leave the shorter query's threshold
        assert_eq!(match_score("fire ", "Firefox"), Some(10));
        assert_eq!(match_score("l i b", "lib"), Some(8));
        assert_eq!(fit("l i b", "lib"), Some(Fit::Exact));

        // a character the name does not have still ends the match
        assert_eq!(match_score("f b", "Firefox"), None);
        assert_eq!(fit("f b", "Firefox"), None);
    }

    // a typo is one edit from four to seven alphanumeric characters, two from eight, and never a
    // real fit, another first letter, a literal digit, or a short query (ADR 0152)
    #[test]
    fn a_typo_is_admitted_within_its_budget() {
        assert_eq!(typo("fierfox", "Firefox"), Some(Fit::Typo));
        assert_eq!(typo("firefox", "Firefox"), None, "distance zero is a fit");
        assert_eq!(typo("fir", "Fier"), None, "three characters never");
        assert_eq!(typo("f b", "Firefox"), None, "two characters never");
        assert_eq!(typo("wierfox", "Firefox"), None, "the first letter matches");
        assert_eq!(typo("fierfoxes", "Firefox"), None, "three edits");
        assert_eq!(
            typo("fierfoxx", "Firefox"),
            Some(Fit::Typo),
            "two from eight"
        );

        // two edits fail a seven character query and pass an eight character one
        assert_eq!(typo("fierfoy", "Firefox"), None);
        assert_eq!(typo("fierfoyx", "Firefox"), Some(Fit::Typo));
        assert_eq!(typo("fierfoxyx", "Firefox"), None, "three from eight");

        // separators are ignored, case and diacritics fold, and digits stay literal
        assert_eq!(typo("fier fox", "Firefox"), Some(Fit::Typo));
        assert_eq!(typo("fierfóxx", "Firefox"), Some(Fit::Typo));
        assert_eq!(typo("fierfóx", "Firefox"), Some(Fit::Typo), "folded first");
        assert_eq!(typo("fir3fox", "Firefox"), None);
        assert_eq!(typo("fir3fox", "Fir4Fox"), None);

        // the tiers run in order: a real fit first, then a typo of the name
        assert!(Fit::Anywhere < Fit::Typo);
        assert_eq!(fit_or_typo("firefox", "Firefox"), Some(Fit::Exact));
        assert_eq!(fit_or_typo("fierfox", "Firefox"), Some(Fit::Typo));
    }
}
