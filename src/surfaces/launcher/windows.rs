/*
 * the open windows, as a Launcher provider: a query that fits a window's title or its app's name
 * lists it, and pressing it focuses it. niri's titles are not kept by the `windows` Service, which
 * holds window ids only; they are asked of niri off the view thread each time the Launcher opens
 * (`refresh`) and held here, in memory, for the providers to read, so a title changed since shows
 * from the next opening
 */

use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;

use kanade_runtime::MutexExt;
use kanade_runtime::service::Service;

use crate::modules;
use crate::sources::json::Json;
use crate::sources::niri;
use crate::sources::windows::{App, Running, Windows};

use super::provider::{self, Action, Answer, Fit, LauncherProvider, Mark};
use super::{emoji, history};

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

static TITLES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

// asks niri for the windows again on a thread of its own, when the `windows` Module runs
pub fn refresh() {
    if !modules::on("windows") {
        return;
    }

    thread::spawn(|| match niri::ask("\"Windows\"") {
        Ok(reply) => {
            let entries = entries(&reply, Windows::read().running());
            *TITLES.locked() = entries;
        }
        Err(error) => eprintln!("launcher: windows: {error}"),
    });
}

pub fn titles() -> Vec<Entry> {
    TITLES.locked().clone()
}

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
    // never for an empty or short query, nor for what other providers answer to a prefix
    fn find(&self, query: &str) -> Vec<Answer> {
        if query.chars().count() < FROM
            || query.starts_with(emoji::PREFIX)
            || query.starts_with(history::PREFIX)
        {
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
}
