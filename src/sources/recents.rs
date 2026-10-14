//! How often and how lately the Launcher started each app. The most recent few list first for an
//! empty query (the Recents), and the count orders apps that fit a query alike, never above a
//! better fit. Counts are small and bounded: when one reaches `CAP` every count halves, so an old
//! habit fades, and only the `KEPT` most used apps are remembered. One app per line, `count`,
//! `last` and the id separated by tabs, in `$XDG_STATE_HOME/kanade/launcher-usage`, readable by
//! its owner only; `last` counts starts, not time. A file that cannot be read or written leaves the
//! counts as they are in memory. The list of the last six starts this replaced
//! (`launcher-recent`) is read once when there is no usage file yet.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;

use kanade_runtime::MutexExt;

use crate::atomic::{self, Mode};
use crate::config;

// as many as the empty query lists ahead of the rest
const RECENT: usize = 6;

// the apps remembered
const KEPT: usize = 300;

// a count this high halves them all
const CAP: u32 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Used {
    count: u32,

    // the place of its last start among all starts, the larger the later
    last: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Usage(BTreeMap<String, Used>);

impl Usage {
    // how often it started, 0 for an app never started
    pub fn count(&self, id: &str) -> u32 {
        self.0.get(id).map_or(0, |used| used.count)
    }

    // the apps started last, newest first
    pub fn recent(&self) -> Vec<&str> {
        let mut ids: Vec<(&String, u64)> =
            self.0.iter().map(|(id, used)| (id, used.last)).collect();

        ids.sort_by_key(|&(_, last)| std::cmp::Reverse(last));

        ids.into_iter()
            .take(RECENT)
            .map(|(id, _)| id.as_str())
            .collect()
    }

    // one more start of `id`, the newest
    pub(crate) fn start(&mut self, id: &str) {
        let last = self.0.values().map(|used| used.last).max().unwrap_or(0) + 1;
        let used = self
            .0
            .entry(id.to_owned())
            .or_insert(Used { count: 0, last });

        used.count += 1;
        used.last = last;

        // rounded up, so an app started once is not forgotten by a busy one
        if used.count >= CAP {
            for used in self.0.values_mut() {
                used.count = used.count.div_ceil(2);
            }
        }

        self.keep();
    }

    // the most used `KEPT`, the later start breaking a tie
    fn keep(&mut self) {
        while self.0.len() > KEPT {
            let least = self
                .0
                .iter()
                .min_by_key(|(_, used)| (used.count, used.last))
                .map(|(id, _)| id.clone());

            match least {
                Some(id) => self.0.remove(&id),
                None => break,
            };
        }
    }

    // a count or a place that does not read is a line skipped, and no count passes `CAP`
    fn parse(text: &str) -> Usage {
        let mut usage = Usage::default();

        for line in text.lines() {
            let mut fields = line.splitn(3, '\t');

            if let (Some(count), Some(last), Some(id)) =
                (fields.next(), fields.next(), fields.next())
                && let (Ok(count), Ok(last)) = (count.parse::<u32>(), last.parse::<u64>())
                && !id.trim().is_empty()
                && count > 0
            {
                usage.0.insert(
                    id.trim().to_owned(),
                    Used {
                        count: count.min(CAP),
                        last,
                    },
                );
            }
        }

        usage.keep();
        usage
    }

    fn text(&self) -> String {
        self.0
            .iter()
            .map(|(id, used)| format!("{}\t{}\t{id}\n", used.count, used.last))
            .collect()
    }

    // the old list of ids, newest first: each started once, in that order
    fn migrate(text: &str) -> Usage {
        let ids: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .take(RECENT)
            .collect();
        let mut usage = Usage::default();

        for id in ids.into_iter().rev() {
            usage.start(id);
        }

        usage
    }
}

static USAGE: Mutex<Option<Usage>> = Mutex::new(None);

// one save at a time, so the file is never written by two threads at once
static SAVING: Mutex<()> = Mutex::new(());

fn file(name: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok();

    config::base("XDG_STATE_HOME", ".local/state", home.as_deref())
        .map(|dir| dir.join("kanade").join(name))
}

// what `f` makes of the usage; the file is read at the first ask
pub fn with<R>(f: impl FnOnce(&Usage) -> R) -> R {
    f(USAGE.locked().get_or_insert_with(read))
}

// the app `id` was started: counted, saved off the view thread
pub fn used(id: &str) {
    USAGE.locked().get_or_insert_with(read).start(id);

    thread::spawn(save);
}

fn read() -> Usage {
    match file("launcher-usage").and_then(|file| fs::read_to_string(file).ok()) {
        Some(text) => Usage::parse(&text),
        None => file("launcher-recent")
            .and_then(|file| fs::read_to_string(file).ok())
            .map(|text| Usage::migrate(&text))
            .unwrap_or_default(),
    }
}

// the usage as it is when the save gets its turn, so a save that runs late never writes an older one
fn save() {
    let _saving = SAVING.locked();
    let text = USAGE.locked().clone().unwrap_or_default().text();

    if let Some(file) = file("launcher-usage") {
        let _ = atomic::write(&file, text.as_bytes(), Mode::Private);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // the newest start is first, the count grows and fades, and the memory is bounded
    #[test]
    fn counts_fade_and_are_bounded() {
        let mut usage = Usage::default();

        for id in ["a", "b", "c", "a", "d", "e", "f", "g"] {
            usage.start(id);
        }

        assert_eq!(usage.recent(), ["g", "f", "e", "d", "a", "c"]);
        assert_eq!(usage.count("a"), 2);
        assert_eq!(usage.count("z"), 0);

        // reaching the cap halves every count, rounded up, so `b` stays remembered
        for _ in 0..CAP - 2 {
            usage.start("a");
        }
        assert_eq!(usage.count("a"), CAP / 2);
        assert_eq!(usage.count("b"), 1);

        for n in 0..KEPT + 20 {
            usage.start(&format!("app-{n}"));
        }
        assert_eq!(usage.0.len(), KEPT);
        assert_eq!(usage.count("a"), CAP / 2);
    }

    // a file is read as written, bad lines and numbers out of bounds are tamed, and the old
    // list of ids becomes counts of one in its order
    #[test]
    fn a_file_is_read_whole_or_by_line_and_the_old_list_migrates() {
        let usage = Usage::parse("3\t10\ta\n\t\t\nnot\t1\tb\n999\t4\tc\n2\t7\t \n0\t1\td");

        assert_eq!(usage.count("a"), 3);
        assert_eq!(usage.count("c"), CAP);
        assert_eq!(usage.0.len(), 2);
        assert_eq!(Usage::parse(&usage.text()), usage);

        let old = Usage::migrate("x\n\ny\nz");
        assert_eq!(old.recent(), ["x", "y", "z"]);
        assert_eq!(old.count("y"), 1);
    }
}
