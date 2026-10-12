//! `kanade dmenu`: the Launcher as a chooser for a script. The client reads lines from stdin and
//! sends them; the Launcher opens on the focused island showing only those, searched as it searches
//! apps, and the call waits for the user. Pressing one answers its place in the list, so the client
//! prints its own line exactly; Escape, a collapse or another Surface dismisses it, and the client
//! exits 1. One at a time: a newer dmenu replaces the one waiting, which is dismissed, so a client
//! that went away never keeps the next one from opening.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use kanade_runtime::service::Service;
use kanade_runtime::{IpcReply, MutexExt};

use crate::cli::Reply;
use crate::island::command::Command;
use crate::island::presentation::Surface;
use crate::island::service::IslandService;

use super::super::store;
use super::Search;
use super::provider::{self, Action, Answer, LauncherProvider, Mark};
use crate::surfaces::visit::Visited;

// the most lines a dmenu takes, and the longest, in characters
pub const MOST: usize = 5000;
pub const LONGEST: usize = 1000;

// the longest prompt, in characters
const PROMPT: usize = 80;

// what a dismissed dmenu answers, which the client exits 1 on without a word
pub const DISMISSED: &str = "dismissed";

// what `kanade dmenu` asks: the prompt, then the lines to choose from, which only the client reads
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    pub prompt: Option<String>,
    pub choices: Vec<String>,
}

impl Asked {
    /*
     * `[--prompt <text>]` as typed, with `-- <choice>...` after it as the client sends it; none
     * past the bounds
     */
    pub fn parse(arguments: &[&str]) -> Option<Asked> {
        let (prompt, rest) = match arguments {
            ["--prompt", prompt, rest @ ..] => (Some(shown(prompt, PROMPT)), rest),
            rest => (None, rest),
        };

        let choices = match rest {
            [] => &[][..],
            ["--", choices @ ..] => choices,
            _ => return None,
        };

        let fits = choices.len() <= MOST
            && choices
                .iter()
                .all(|choice| choice.chars().count() <= LONGEST);

        fits.then(|| Asked {
            prompt: prompt.filter(|prompt| !prompt.is_empty()),
            choices: choices.iter().map(|choice| (*choice).to_owned()).collect(),
        })
    }

    // the words the client sends for it
    pub fn words(&self) -> Vec<String> {
        let mut words = vec![String::from("dmenu")];

        if let Some(prompt) = &self.prompt {
            words.extend([String::from("--prompt"), prompt.clone()]);
        }

        words.push(String::from("--"));
        words.extend(self.choices.iter().cloned());

        words
    }
}

pub fn usage() -> String {
    format!(
        "dmenu [--prompt <text>]
  <text>: what the Launcher's field says until something is typed
  stdin: the lines to choose from, up to {MOST} of up to {LONGEST} characters; empty ones are \
skipped. The chosen line is printed; exits 1 when dismissed"
    )
}

/*
 * `text` as one line of at most `most` characters, as the row shows it: control characters and
 * those that reorder text become spaces. The client prints the line it read, never this
 */
fn shown(text: &str, most: usize) -> String {
    text.chars()
        .map(|letter| {
            if letter.is_control()
                || matches!(letter, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                ' '
            } else {
                letter
            }
        })
        .take(most)
        .collect::<String>()
        .trim()
        .to_owned()
}

// one line to choose, as shown and lowercase for the search, kept apart so a key lowercases none
#[derive(Debug, PartialEq)]
pub(super) struct Choice {
    shown: String,
    lower: String,
}

/*
 * the dmenu the Launcher shows in one visit, read by its view; one kept from an earlier visit is
 * over. Written on a dmenu and when the visit after it begins, never by the view
 */
#[derive(Debug, Default, PartialEq)]
pub(super) struct Choosing {
    visit: u64,
    prompt: Option<String>,
    choices: Arc<[Choice]>,
}

impl Service for Choosing {
    fn new() -> Self {
        Choosing::default()
    }

    fn listen() {}
}

// the dmenu waiting on the user, with the visit it shows in
struct Waiting {
    visit: u64,
    reply: IpcReply,
}

static WAITING: Mutex<Option<Waiting>> = Mutex::new(None);

/*
 * what the Launcher shows in `visit`: none for a visit with no dmenu, else its prompt and the
 * lines the query finds, best first
 */
pub(super) fn of(visit: u64) -> Option<(Option<String>, Arc<[Choice]>)> {
    let choosing = Choosing::read();

    (choosing.visit == visit && !choosing.choices.is_empty())
        .then(|| (choosing.prompt.clone(), choosing.choices.clone()))
}

// the dmenu's lines, as a Launcher provider; pressing one answers it
pub(super) struct Choices<'a>(pub &'a [Choice]);

impl LauncherProvider for Choices<'_> {
    // in the order given within a fit; an empty query finds every line
    fn find(&self, query: &str) -> Vec<Answer> {
        let query = query.to_lowercase();

        self.0
            .iter()
            .enumerate()
            .filter_map(|(index, choice)| {
                let fit = provider::fit(&query, &choice.lower)?;

                Some(Answer {
                    fit,
                    title: choice.shown.clone(),
                    detail: String::new(),
                    mark: Mark::Tile(
                        choice
                            .shown
                            .chars()
                            .next()
                            .map(|first| first.to_uppercase().collect())
                            .unwrap_or_default(),
                    ),
                    action: Action::Choose(index),
                })
            })
            .collect()
    }
}

/*
 * opens the Launcher on the focused island for `asked`, answering `reply` once the user chooses
 * or dismisses it; at once with why when it cannot open
 */
pub fn choose(asked: Asked, reply: IpcReply) {
    let refuse = |reply: IpcReply, why: &str| reply.send(&Reply::Refused(why.to_owned()).encode());

    if asked.choices.is_empty() {
        return refuse(reply, "no lines to choose from on stdin");
    }

    // a newer one replaces it
    dismiss();

    match IslandService::read().resolve(Command::Open(Surface::Launcher)) {
        Ok(Some(effect)) => IslandService::write().apply(effect, Instant::now()),
        Ok(None) => {}
        Err(error) => return refuse(reply, &error.to_string()),
    }

    let (shows, visit, overview) = {
        let island = IslandService::read();

        (
            island.surface() == Some(Surface::Launcher),
            island.visit(),
            island.overview(),
        )
    };

    if !shows {
        return refuse(
            reply,
            if overview {
                "the Launcher does not open while niri's overview is"
            } else {
                "the Launcher did not open"
            },
        );
    }

    // the visit may be one open already, which this one takes over from the top
    store(Search::begin(visit));
    store(Choosing {
        visit,
        prompt: asked.prompt,
        choices: asked
            .choices
            .iter()
            .map(|choice| {
                let shown = shown(choice, LONGEST);

                Choice {
                    lower: shown.to_lowercase(),
                    shown,
                }
            })
            .collect(),
    });

    *WAITING.locked() = Some(Waiting { visit, reply });
}

// the line at `index` was chosen in `visit`; nothing once that dmenu is over
pub(super) fn chosen(visit: u64, index: usize) {
    let waiting = WAITING.locked().take_if(|waiting| waiting.visit == visit);

    if let Some(waiting) = waiting {
        waiting.reply.send(&Reply::Done(index.to_string()).encode());
    }
}

// the one waiting, if any, is dismissed
fn dismiss() {
    let waiting = WAITING.locked().take();

    if let Some(waiting) = waiting {
        waiting
            .reply
            .send(&Reply::Refused(String::from(DISMISSED)).encode());
    }
}

/*
 * after each change of the island: the dmenu waiting is dismissed once its visit no longer shows
 * the Launcher, as on Escape, a collapse or another Surface, and its lines go once a later visit
 * begins. Its lines stay while the Launcher fades out, so it does not show the apps meanwhile
 */
pub(super) fn follow() {
    let (visit, shows) = {
        let island = IslandService::read();

        (island.visit(), island.surface() == Some(Surface::Launcher))
    };

    let over = WAITING
        .locked()
        .as_ref()
        .is_some_and(|waiting| waiting.visit != visit || !shows);

    if over {
        dismiss();
    }

    if Choosing::read().visit != visit {
        store(Choosing::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_form_parses_back_within_its_bounds() {
        let asked = Asked {
            prompt: Some(String::from("Pick one")),
            choices: vec![String::from("a b"), String::from("--"), String::from("")],
        };
        let words = asked.words();
        let words: Vec<&str> = words[1..].iter().map(String::as_str).collect();

        assert_eq!(Asked::parse(&words), Some(asked));

        assert_eq!(
            Asked::parse(&[]),
            Some(Asked {
                prompt: None,
                choices: Vec::new(),
            })
        );
        assert_eq!(Asked::parse(&["a"]), None);
        assert_eq!(Asked::parse(&["--prompt"]), None);

        let long = "x".repeat(LONGEST + 1);

        assert_eq!(Asked::parse(&["--", &long]), None);

        let many = vec!["x"; MOST + 1];
        let mut words = vec!["--"];
        words.extend(many);

        assert_eq!(Asked::parse(&words), None);
    }

    #[test]
    fn a_line_shows_as_plain_text() {
        assert_eq!(shown("a\tb\r\u{1b}[1m", LONGEST), "a b  [1m");
        assert_eq!(shown("\u{202e}evil", LONGEST), "evil");
    }
}
