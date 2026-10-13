/*
 * the clipboard history, as a Launcher provider: a query starting with `>` searches what was
 * copied, newest first, and pressing an entry puts it back on the clipboard. Only on that prefix, so a
 * search for apps never shows what was copied
 */

use crate::icon::Icon;
use crate::sources::clipboard::Entry;

use super::super::clipboard::{found_at, summary};
use super::provider::{Action, Answer, Fit, LauncherProvider, Mark};

pub const PREFIX: char = '>';

pub struct History<'a> {
    // newest first, as the history keeps them
    pub entries: &'a [Entry],
}

impl LauncherProvider for History<'_> {
    // in the history's own order; `>` alone lists every entry
    fn find(&self, query: &str) -> Vec<Answer> {
        let Some(term) = query.strip_prefix(PREFIX) else {
            return Vec::new();
        };

        found_at(self.entries, term.trim())
            .into_iter()
            .map(|at| {
                let entry = &self.entries[at];
                let summary = summary(entry);

                Answer {
                    fit: Fit::Anywhere,
                    title: summary.title,
                    detail: summary.detail,
                    mark: Mark::Icon(Icon::Clipboard),
                    action: Action::Paste(entry.id),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::clipboard::Content;
    use kanade_runtime::MutexExt;

    fn entry(id: u64, text: &str) -> Entry {
        Entry {
            id,
            content: Content::Text(text.into()),
        }
    }

    // what was copied shows only to a query that asks for it, and pressing puts that entry back
    #[test]
    fn copied_text_shows_only_after_the_prefix() {
        let _turn = super::super::super::clipboard::MEMO_TURN.locked();
        let entries = [entry(2, "invoice 42"), entry(1, "hunter2")];
        let history = History { entries: &entries };

        assert!(history.find("hunter").is_empty());
        assert!(history.find("").is_empty());
        assert_eq!(history.find(">").len(), 2);

        let found = history.find(">hunt");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].action, Action::Paste(1));
    }
}
