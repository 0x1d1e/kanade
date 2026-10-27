/*
 * lists open windows by title or app name and focuses the selected one. Refresh titles from niri
 * off the view thread when Launcher opens; Service updates let pending searches show late answers.
 */

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use kanade_runtime::service::Service;

use crate::modules;
use crate::sources::json::Json;
use crate::sources::niri;
use crate::sources::windows::{App, Running, Windows};

use super::provider::{self, Action, Answer, Fit, LauncherProvider, Mark};

// the fewest letters a window is searched by: one would list nearly every window
const FROM: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: u64,

    // its title, empty for a window without one
    pub title: String,

    // its app's name, else its `app_id`, empty for neither
    pub app: String,

    pub icon: Option<PathBuf>,
}

// the titles niri gave last, none before the first answer
#[derive(Debug, Default)]
pub struct Held {
    entries: Vec<Entry>,
}

impl Service for Held {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {}
}

// the newest refresh asked of niri: an answer that is not its own is dropped, as it may come last
static ASKED: AtomicU64 = AtomicU64::new(0);

// asks niri for the windows again on a thread of its own, when the `windows` Module runs
pub fn refresh() {
    if !modules::on("windows") {
        return;
    }

    let ticket = ASKED.fetch_add(1, Ordering::SeqCst) + 1;

    thread::spawn(move || match niri::ask("\"Windows\"") {
        Ok(reply) => {
            // its own statement, so the read is released before the write
            let entries = entries(&reply, Windows::read().running());

            hold(ticket, entries);
        }
        Err(error) => eprintln!("launcher: windows: {error}"),
    });
}

// keeps what the refresh asked for, unless a newer one was asked since or nothing changed
fn hold(ticket: u64, entries: Vec<Entry>) {
    let mut held = Held::write();

    if ASKED.load(Ordering::SeqCst) == ticket && held.entries != entries {
        held.entries = entries;
    } else {
        held.quiet();
    }
}

// read in the view, so it is drawn again when the titles arrive
pub fn titles() -> Vec<Entry> {
    Held::read().entries.clone()
}

/*
 * the one `Held` is the process's, so a test that writes it is another test's input: the
 * Launcher's ranking reads these titles, and a write landing between two of its calls ranks them
 * again. Every test that writes them takes this turn, and so does a test that reads them twice
 */
#[cfg(test)]
pub(super) static TITLES_TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

// the windows of niri's `Windows` reply, by title, each with its app as the Dock knows it
fn entries(reply: &Json, running: &[Running]) -> Vec<Entry> {
    let Some(windows) = reply.get("Windows").and_then(Json::as_array) else {
        return Vec::new();
    };

    let mut entries: Vec<Entry> = windows
        .iter()
        .filter_map(|window| {
            let id = window.get("id")?.as_u64()?;
            let text = |key| {
                window
                    .get(key)
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };

            let app = running
                .iter()
                .find(|app| app.windows.iter().any(|window| window.id.0 == id));

            let (name, icon) = match app.map(|app| &app.app) {
                Some(App::Desktop(entry)) => (entry.name.clone(), entry.icon_file.clone()),
                _ => (text("app_id"), None),
            };

            Some(Entry {
                id,
                title: text("title"),
                app: name,
                icon,
            })
        })
        .collect();

    entries.sort_by_key(|entry| (entry.title.to_lowercase(), entry.id));

    entries
}

pub struct Titles<'a>(pub &'a [Entry]);

impl LauncherProvider for Titles<'_> {
    // by title and app name within a fit; an empty query lists every window
    fn find(&self, query: &str) -> Vec<Answer> {
        // an empty query lists every window; a query of one letter lists the apps alone
        if !query.is_empty() && query.chars().count() < FROM {
            return Vec::new();
        }

        let query = query.to_lowercase();

        self.0
            .iter()
            .filter_map(|entry| {
                let title = entry.title.to_lowercase();
                let anywhere = format!("{title} {}", entry.app.to_lowercase());

                let fit = provider::fit(&query, &title).or_else(|| {
                    query
                        .split_whitespace()
                        .all(|term| anywhere.contains(term))
                        .then_some(Fit::Anywhere)
                })?;

                // a window whose title is its app's name has nothing more to say of the app
                let shown = if entry.title.is_empty() {
                    &entry.app
                } else {
                    &entry.title
                };

                Some(Answer {
                    fit,
                    title: shown.clone(),
                    detail: if shown == &entry.app {
                        String::new()
                    } else {
                        entry.app.clone()
                    },
                    mark: match &entry.icon {
                        Some(path) => Mark::Picture(path.clone()),
                        None => {
                            Mark::Tile(shown.chars().next().map_or_else(String::new, |initial| {
                                initial.to_uppercase().collect()
                            }))
                        }
                    },
                    action: Action::Focus(entry.id),
                })
            })
            .collect()
    }
}

// what niri is asked to do to focus the window
pub fn focus(id: u64) -> String {
    format!(r#"{{"Action":{{"FocusWindow":{{"id":{id}}}}}}}"#)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::json;
    use kanade_runtime::MutexExt;

    // niri's reply is outside input: an entry without an id is skipped, a missing title or app id is empty
    #[test]
    fn a_windows_reply_reads_to_entries() {
        let reply = json::parse(
            r#"{"Windows":[
                {"id":7,"title":"b Notes","app_id":"notes"},
                {"title":"no id"},
                {"id":3,"title":null,"app_id":null}
            ]}"#,
        )
        .unwrap();

        let entries = entries(&reply, &[]);

        assert_eq!(
            entries
                .iter()
                .map(|e| (e.id, e.title.as_str(), e.app.as_str()))
                .collect::<Vec<_>>(),
            [(3, "", ""), (7, "b Notes", "notes")]
        );
        assert!(super::entries(&json::parse("{}").unwrap(), &[]).is_empty());
    }

    fn entry(id: u64) -> Entry {
        Entry {
            id,
            title: format!("window {id}"),
            app: String::new(),
            icon: None,
        }
    }

    // a slow answer to an earlier refresh came last and replaced the newer titles
    #[test]
    fn an_answer_to_an_older_refresh_does_not_replace_a_newer_one() {
        let _turn = TITLES_TURN.locked();

        let older = ASKED.fetch_add(1, Ordering::SeqCst) + 1;
        let newer = ASKED.fetch_add(1, Ordering::SeqCst) + 1;

        hold(newer, vec![entry(2)]);
        hold(older, vec![entry(1)]);

        assert_eq!(titles(), [entry(2)]);
    }

    // with nothing typed the Windows section lists every open window, by title
    #[test]
    fn an_empty_query_lists_every_window() {
        let entries = [entry(1), entry(2)];
        let found = Titles(&entries).find("");

        assert_eq!(found.len(), 2);
        assert!(
            found.iter().all(|answer| answer.fit == Fit::Prefix),
            "every window lists as a Prefix"
        );
        assert!(
            found
                .iter()
                .all(|answer| matches!(answer.action, Action::Focus(_)))
        );

        // a query of one letter still lists nothing
        assert!(Titles(&entries).find("w").is_empty());
    }
}
