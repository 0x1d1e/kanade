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
use crate::sources::windows::{App, Running, Windows};
use crate::sources::{niri, umbriel};

use super::provider::{self, Action, Answer, Fit, LauncherProvider, Mark};

// the fewest letters a window is searched by: one would list nearly every window
const FROM: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,

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

// the newest refresh asked of its compositor: an answer that is not its own is dropped, as it may come last
static ASKED: AtomicU64 = AtomicU64::new(0);

// asks the active compositor for the windows again on a thread of its own, when the `windows` Module runs
pub fn refresh() {
    if !modules::on("windows") {
        return;
    }

    let ticket = ASKED.fetch_add(1, Ordering::SeqCst) + 1;

    thread::spawn(move || {
        let entries = if umbriel::is_active() {
            umbriel::windows().map(|reply| entries(&reply, &[], true))
        } else {
            niri::ask("\"Windows\"")
                .map(|reply| entries(&reply, Windows::read().running(), false))
                .map_err(|error| error.to_string())
        };

        match entries {
            Ok(entries) => hold(ticket, entries),
            Err(error) => eprintln!("launcher: windows: {error}"),
        }
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

// the windows of either compositor's reply, by title, each with its app as the Dock knows it
fn entries(reply: &Json, running: &[Running], umbriel: bool) -> Vec<Entry> {
    let Some(windows) = (if umbriel {
        reply.get("ok").and_then(Json::as_array)
    } else {
        reply.get("Windows").and_then(Json::as_array)
    }) else {
        return Vec::new();
    };

    let mut entries: Vec<Entry> = windows
        .iter()
        .filter_map(|window| {
            let raw_id = window.get("id")?;
            let (id, window_id) = if umbriel {
                let id = raw_id.as_str()?;
                (
                    id.to_owned(),
                    u64::from_str_radix(id.trim_start_matches("0x"), 16).ok(),
                )
            } else {
                let id = raw_id.as_u64()?;
                (id.to_string(), Some(id))
            };
            let text = |key| {
                window
                    .get(key)
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };

            let app = window_id.and_then(|id| {
                running
                    .iter()
                    .find(|app| app.windows.iter().any(|window| window.id.0 == id))
            });

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

    entries.sort_by_key(|entry| (entry.title.to_lowercase(), entry.id.clone()));

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
                    filler: false,
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
                    action: Action::Focus(entry.id.clone()),
                })
            })
            .collect()
    }
}

// what niri is asked to do to focus the window
pub fn focus(id: &str) -> String {
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

        let entries = entries(&reply, &[], false);

        assert_eq!(
            entries
                .iter()
                .map(|e| (e.id.as_str(), e.title.as_str(), e.app.as_str()))
                .collect::<Vec<_>>(),
            [("3", "", ""), ("7", "b Notes", "notes")]
        );
        assert!(super::entries(&json::parse("{}").unwrap(), &[], false).is_empty());
    }

    fn entry(id: u64) -> Entry {
        Entry {
            id: id.to_string(),
            title: format!("window {id}"),
            app: String::new(),
            icon: None,
        }
    }

    #[test]
    fn umbriel_windows_reply_reads_hexadecimal_ids() {
        let reply = json::parse(
            r#"{"ok":[
                {"id":"58f26771340d94c9fd41b7841be9e3e0","title":"Test Window","app_id":"test-app"},
                {"id":17,"title":"invalid id"}
            ]}"#,
        )
        .unwrap();

        let entries = entries(&reply, &[], true);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "58f26771340d94c9fd41b7841be9e3e0");
        assert_eq!(entries[0].title, "Test Window");
        assert_eq!(entries[0].app, "test-app");
        assert_eq!(
            umbriel::focus_window_action(&entries[0].id),
            r#"{"cmd":"action","action":"window-focus","window":"58f26771340d94c9fd41b7841be9e3e0"}"#
        );
        assert_eq!(Titles(&entries).find("").len(), 1);
        assert_eq!(Titles(&entries).find("test").len(), 1);
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
