//! The Launcher's providers (#138): each answers a query with what it finds, how well each answer
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

    // an image cropped to fill the tile, like a wallpaper
    Photo(PathBuf),

    // one of Kanade's own icons, like a Settings page's
    Icon(Icon),

    // a letter or sign on the quiet tile
    Tile(String),

    // a glyph drawn as it is, like an emoji
    Glyph(&'static str),
}

// what pressing an answer does; each closes the island once done
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Launch(Launch),

    // puts the text on the clipboard
    Copy(String),

    // sets the image as the wallpaper
    Wallpaper(PathBuf),

    // opens Controls, the Clipboard or Settings, as the island does
    Open(Destination),
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
