//! The Launcher's providers (#138): each answers a query with what it finds, how well each answer
//! fits, what its row shows and what pressing it does. The seam is Kanade's own; there is no
//! plugin API (design.md, Launcher).

use std::path::PathBuf;

use amane::DesktopApp;

/*
 * how well an answer fits the query, best first. Every provider ranks on this one scale, so their
 * answers interleave; within a fit, the provider listed first, then its own order
 */
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fit {
    // what the query asks for, like the value of a sum
    Answer,

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

    // a letter or sign on the quiet tile
    Tile(String),

    // a glyph drawn as it is, like an emoji
    Glyph(&'static str),
}

// what pressing an answer does; either closes the island once done
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Launch(DesktopApp),

    // puts the text on the clipboard
    Copy(String),
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

    // stable, so a fit keeps the providers' order and each one's own
    answers.sort_by_key(|answer| answer.fit);

    answers
}

/*
 * how well `name` answers `query`, both lowercase, none when it does not: the name is the query,
 * starts with it, has a word starting so, has it, then has every word of it
 */
pub fn fit(query: &str, name: &str) -> Option<Fit> {
    if name == query {
        return Some(Fit::Exact);
    }

    if name.starts_with(query) {
        return Some(Fit::Prefix);
    }

    let word = name
        .match_indices(query)
        .any(|(at, _)| !name[..at].ends_with(char::is_alphanumeric));

    if word {
        return Some(Fit::Word);
    }

    if name.contains(query) {
        return Some(Fit::Within);
    }

    query
        .split_whitespace()
        .all(|term| name.contains(term))
        .then_some(Fit::Terms)
}

#[cfg(test)]
mod tests {
    use super::*;

    // answers any query with each (title, fit)
    struct Fixed(Vec<(&'static str, Fit)>);

    impl LauncherProvider for Fixed {
        fn find(&self, _: &str) -> Vec<Answer> {
            self.0
                .iter()
                .map(|&(title, fit)| Answer {
                    fit,
                    title: title.to_owned(),
                    detail: String::new(),
                    mark: Mark::Tile(String::new()),
                    action: Action::Copy(title.to_owned()),
                })
                .collect()
        }
    }

    // answers with the query it got
    struct Echo;

    impl LauncherProvider for Echo {
        fn find(&self, query: &str) -> Vec<Answer> {
            Fixed(vec![("", Fit::Answer)])
                .find(query)
                .into_iter()
                .map(|answer| Answer {
                    title: query.to_owned(),
                    ..answer
                })
                .collect()
        }
    }

    fn titles(answers: &[Answer]) -> Vec<&str> {
        answers.iter().map(|answer| answer.title.as_str()).collect()
    }

    #[test]
    fn answers_interleave_by_fit_then_provider_then_their_own_order() {
        let apps = Fixed(vec![
            ("app within", Fit::Within),
            ("app prefix", Fit::Prefix),
            ("app prefix 2", Fit::Prefix),
        ]);
        let emoji = Fixed(vec![
            ("emoji prefix", Fit::Prefix),
            ("emoji exact", Fit::Exact),
        ]);
        let sum = Fixed(vec![("8", Fit::Answer)]);

        assert_eq!(
            titles(&ranked(&[&apps, &emoji, &sum], "q")),
            [
                "8",
                "emoji exact",
                "app prefix",
                "app prefix 2",
                "emoji prefix",
                "app within"
            ]
        );
    }

    #[test]
    fn providers_get_the_query_trimmed() {
        assert_eq!(titles(&ranked(&[&Echo], "  2 + 2 ")), ["2 + 2"]);
        assert!(ranked(&[], "q").is_empty());
    }

    #[test]
    fn a_name_fits_best_whole_then_by_where_the_query_is() {
        assert_eq!(fit("files", "files"), Some(Fit::Exact));
        assert_eq!(fit("fi", "files"), Some(Fit::Prefix));
        assert_eq!(fit("code", "visual studio code"), Some(Fit::Word));
        assert_eq!(fit("tor", "monitor"), Some(Fit::Within));
        assert_eq!(fit("visual code", "visual studio code"), Some(Fit::Terms));
        assert_eq!(fit("zzz", "files"), None);
    }
}
