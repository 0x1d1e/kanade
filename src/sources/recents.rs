//! What the Launcher started, and the queries that led to it: one anchor per item whose score
//! halves every ten days, so an old habit fades and a new one takes over (ADR 0127). The highest
//! scores list first for an empty query (the Recents), and the score orders the answers that fit a
//! query alike, never above a better fit; a query that launched an item before lifts it again for
//! seventeen days. Apps, Kanade's own commands and the destinations it opens are all kept here, one
//! per line: the key, its anchor and its queries, tabs between them, in
//! `$XDG_STATE_HOME/kanade/launcher-usage`, readable by its owner only. A file in the older
//! `count`, `last` and id form, or the older `launcher-recent` list of ids, is read once and saved
//! as this one. A file that cannot be read or written leaves the usage as it is in memory.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;

use kanade_runtime::MutexExt;

use crate::atomic::{self, Mode};
use crate::{clock, config};

// as many as the empty query lists ahead of the rest
const RECENT: usize = 6;

// the items remembered
const KEPT: usize = 300;

// the score's half-life, in days
const HALF_LIFE: f64 = 10.0;

// per day, so a score halves every `HALF_LIFE` days
const LAMBDA: f64 = std::f64::consts::LN_2 / HALF_LIFE;

const DAY: f64 = 86_400.0;

// what one launch adds to the score
const BONUS: f64 = 100.0;

// where `exp` overflows: past it a score stays there, so an anchor stays finite
const GUARD: f64 = 709.78;

// how long a query keeps lifting what it launched, and how many of them an item keeps
const TERM_DAYS: i64 = 17;
const TERM_WINDOW: i64 = TERM_DAYS * 86_400;
const TERMS: usize = 20;

// the count the older file capped at; it never held more
const OLD_CAP: u32 = 64;

// the file's first line, which tells it from the older forms
const HEAD: &str = "kanade-launcher-usage\t2";

// the key an app's launches are kept under
pub fn app(id: &str) -> String {
    format!("app:{id}")
}

// the key of the destination `name`
pub fn destination(name: &str) -> String {
    format!("destination:{name}")
}

// the key of the Kanade verb `verb`
pub fn command(verb: &[&str]) -> String {
    format!("command:{}", verb.join(" "))
}

#[derive(Debug, Clone, PartialEq)]
struct Term {
    // unix seconds of the launch that recorded it
    at: i64,

    // the query as typed, lowercased with its whitespace collapsed
    text: String,
}

/*
 * `anchor` is unix seconds, not a count: the score at `t` is
 * `max(1, exp(LAMBDA * (anchor - t) / DAY))`, so it halves every ten days and is 1 once the
 * anchor is ten days past. A launch at `t` moves the anchor to `t + DAY / LAMBDA * ln(score +
 * BONUS)`, `score` being the one at `t` before, so the score rises by exactly `BONUS` and never
 * falls: the anchor first moves forward to at least `t` (a clock that went back takes no score
 * down), the score after a launch is at least 101, and it is capped where `exp` would overflow.
 */
#[derive(Debug, Clone, PartialEq)]
struct Item {
    anchor: f64,

    // the queries that launched it, oldest first
    terms: Vec<Term>,
}

impl Item {
    fn score(&self, now: i64) -> f64 {
        decayed(self.anchor, now)
    }

    // one launch at `now`: the score rises by `BONUS`
    fn launched(&mut self, now: i64) {
        let score = decayed(self.anchor.max(now as f64), now);

        self.anchor = quantized(now as f64 + DAY * (score + BONUS).ln() / LAMBDA);
    }

    // the query that launched it, remembered until `TERM_WINDOW` passes
    fn learn(&mut self, query: &str, now: i64) {
        let text = query
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();

        if text.is_empty() {
            return;
        }

        // the same query again is the newest, not a second one
        self.terms.retain(|term| term.text != text);
        self.terms.push(Term { at: now, text });
        self.expire(now);

        let extra = self.terms.len().saturating_sub(TERMS);
        self.terms.drain(..extra);
    }

    // the queries still worth remembering at `now`
    fn expire(&mut self, now: i64) {
        self.terms
            .retain(|term| now.saturating_sub(term.at) <= TERM_WINDOW);
    }

    // how much a query it was launched by still lifts it: `SAME` the query, `PREFIX` the query
    // starts it, `NEAR` the query adds at most three letters to it, `NONE` otherwise
    fn learned(&self, query: &str, now: i64) -> u8 {
        // a score back at 1 has faded: a query it was started by no longer says anything
        if query.is_empty() || self.score(now) <= 1.0 {
            return NONE;
        }

        self.terms
            .iter()
            .filter(|term| now.saturating_sub(term.at) <= TERM_WINDOW)
            .map(|term| class(&term.text, query))
            .max()
            .unwrap_or(NONE)
    }
}

const NONE: u8 = 0;
const NEAR: u8 = 1;
const PREFIX: u8 = 2;
const SAME: u8 = 3;

// how well the query `term` answers `query`
fn class(term: &str, query: &str) -> u8 {
    if term == query {
        SAME
    } else if term.starts_with(query) {
        PREFIX
    } else if query.starts_with(term) && query.len() - term.len() <= 3 {
        NEAR
    } else {
        NONE
    }
}

// the score at `now` of an item anchored at `anchor`, 1 once it has faded
fn decayed(anchor: f64, now: i64) -> f64 {
    (LAMBDA * (anchor - now as f64) / DAY)
        .min(GUARD)
        .exp()
        .max(1.0)
}

// the anchor to the millisecond, which is all the file holds and a billionth of a score
fn quantized(anchor: f64) -> f64 {
    (anchor * 1000.0).round() / 1000.0
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Usage(BTreeMap<String, Item>);

impl Usage {
    // its score at `now`, 1 for one never started
    pub fn score(&self, key: &str, now: i64) -> f64 {
        self.0.get(key).map_or(1.0, |item| item.score(now))
    }

    // how much the query `query`, lowercased and trimmed, lifts `key` now; `NONE` for none
    pub fn learned(&self, key: &str, query: &str, now: i64) -> u8 {
        self.0
            .get(key)
            .map_or(NONE, |item| item.learned(query, now))
    }

    // the keys started lately, the highest score first, the newer anchor breaking a tie
    pub fn recent(&self, now: i64) -> Vec<&str> {
        let mut keys: Vec<(&String, f64, f64)> = self
            .0
            .iter()
            .map(|(key, item)| (key, item.score(now), item.anchor))
            .collect();

        keys.sort_by(|left, right| right.1.total_cmp(&left.1).then(right.2.total_cmp(&left.2)));

        keys.into_iter()
            .take(RECENT)
            .map(|(key, ..)| key.as_str())
            .collect()
    }

    // one more start of `key`, by the query `query` (none when nothing was typed), at `now`
    pub(crate) fn start(&mut self, key: &str, query: &str, now: i64) {
        let item = self.0.entry(key.to_owned()).or_insert_with(|| Item {
            anchor: now as f64,
            terms: Vec::new(),
        });

        item.launched(now);
        item.learn(query, now);
        self.keep(key, now);
    }

    // the `KEPT` highest scores, the one just started staying whatever its score
    fn keep(&mut self, started: &str, now: i64) {
        while self.0.len() > KEPT {
            let least = self
                .0
                .iter()
                .filter(|(key, _)| key.as_str() != started)
                .min_by(|left, right| left.1.score(now).total_cmp(&right.1.score(now)))
                .map(|(key, _)| key.clone());

            match least {
                Some(key) => {
                    self.0.remove(&key);
                }
                None => break,
            }
        }
    }

    // the queries no longer worth remembering are dropped, and so is nothing else
    fn expire(&mut self, now: i64) {
        for item in self.0.values_mut() {
            item.expire(now);
        }
    }

    // a line that does not read is a line skipped; the head line comes first
    fn parse(text: &str, now: i64) -> Usage {
        let mut usage = Usage::default();

        for line in text.lines().skip(1) {
            let mut fields = line.split('\t');
            let (Some(key), Some(anchor)) = (fields.next(), fields.next()) else {
                continue;
            };
            let key = key.trim();
            let Ok(anchor) = anchor.trim().parse::<f64>() else {
                continue;
            };

            if key.is_empty() || !anchor.is_finite() || anchor <= 0.0 {
                continue;
            }

            let mut terms = Vec::new();

            while let (Some(at), Some(text)) = (fields.next(), fields.next()) {
                let (Ok(at), text) = (at.trim().parse::<i64>(), text.trim()) else {
                    break;
                };

                if text.is_empty() {
                    break;
                }

                terms.push(Term {
                    at,
                    text: text.to_owned(),
                });
            }

            let mut item = Item { anchor, terms };
            item.expire(now);

            let extra = item.terms.len().saturating_sub(TERMS);
            item.terms.drain(..extra);

            usage.0.insert(key.to_owned(), item);
        }

        usage.keep("", now);
        usage
    }

    fn text(&self) -> String {
        let mut text = String::from(HEAD);
        text.push('\n');

        for (key, item) in &self.0 {
            text.push_str(key);
            text.push('\t');
            text.push_str(&format!("{:.3}", item.anchor));

            for term in &item.terms {
                text.push('\t');
                text.push_str(&term.at.to_string());
                text.push('\t');
                text.push_str(&term.text);
            }

            text.push('\n');
        }

        text
    }

    /*
     * the older `count`, `last` and id lines: each keeps the score its count gave it, and the later
     * start is a hair ahead of the older one, so the old order stands among equal counts
     */
    fn migrated_usage(text: &str, now: i64) -> Usage {
        let mut rows: Vec<(u32, u64, String)> = Vec::new();

        for line in text.lines() {
            let mut fields = line.splitn(3, '\t');

            if let (Some(count), Some(last), Some(id)) =
                (fields.next(), fields.next(), fields.next())
                && let (Ok(count), Ok(last)) =
                    (count.trim().parse::<u32>(), last.trim().parse::<u64>())
                && !id.trim().is_empty()
                && count > 0
            {
                rows.push((count.min(OLD_CAP), last, id.trim().to_owned()));
            }
        }

        let newest = rows.iter().map(|&(_, last, _)| last).max().unwrap_or(0);
        let mut usage = Usage::default();

        for (count, last, id) in rows {
            let anchor =
                now as f64 + DAY * f64::from(count).ln() / LAMBDA - (newest - last) as f64 * 0.001;

            usage.0.insert(
                app(&id),
                Item {
                    anchor: quantized(anchor),
                    terms: Vec::new(),
                },
            );
        }

        usage.keep("", now);
        usage
    }

    // the older list of ids, newest first: each one start
    fn migrated_recent(text: &str, now: i64) -> Usage {
        let ids: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .take(RECENT)
            .collect();
        let mut usage = Usage::default();

        for (place, id) in ids.into_iter().enumerate() {
            usage.start(&app(id), "", now - place as i64);
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
    let now = clock::unix_seconds();

    f(USAGE.locked().get_or_insert_with(|| read(now)))
}

// `key` was started by `query`: its score rises and the query is remembered, saved off the view thread
pub fn used(key: &str, query: &str) {
    let now = clock::unix_seconds();

    USAGE
        .locked()
        .get_or_insert_with(|| read(now))
        .start(key, query, now);

    thread::spawn(save);
}

fn read(now: i64) -> Usage {
    read_from(
        file("launcher-usage").as_deref(),
        file("launcher-recent").as_deref(),
        now,
    )
}

/*
 * the usage `usage` holds, or the older `recent` list of ids when there is none; an older usage
 * file is migrated and written back as this format at once, so it is never read as the old one
 * twice. Not `save`, which waits for the usage lock this runs under
 */
fn read_from(usage: Option<&Path>, recent: Option<&Path>, now: i64) -> Usage {
    match usage.and_then(|path| fs::read_to_string(path).ok()) {
        Some(text) if head(&text) => Usage::parse(&text, now),
        Some(text) => {
            let migrated = Usage::migrated_usage(&text, now);

            if let Some(path) = usage {
                let _ = atomic::write(path, migrated.text().as_bytes(), Mode::Private);
            }

            migrated
        }
        None => recent
            .and_then(|path| fs::read_to_string(path).ok())
            .map(|text| Usage::migrated_recent(&text, now))
            .unwrap_or_default(),
    }
}

// whether `text` is this format rather than one of the older ones
fn head(text: &str) -> bool {
    text.lines().next().is_some_and(|line| line.trim() == HEAD)
}

// the usage as it is when the save gets its turn, so a save that runs late never writes an older one;
// the queries past their seventeen days are left out, as a read leaves them out
fn save() {
    let _saving = SAVING.locked();
    let mut usage = USAGE.locked().clone().unwrap_or_default();
    usage.expire(clock::unix_seconds());
    let text = usage.text();

    if let Some(file) = file("launcher-usage") {
        let _ = atomic::write(&file, text.as_bytes(), Mode::Private);
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    const NOW: i64 = 1_700_000_000;

    // the anchor is time-like: a launch adds 100, the score halves in ten days, never falls
    #[test]
    fn a_launch_adds_a_hundred_and_ten_days_halve_it() {
        let mut usage = Usage::default();

        usage.start("app:a", "", NOW);
        assert!((usage.score("app:a", NOW) - 101.0).abs() < 1e-6);

        usage.start("app:a", "", NOW);
        assert!((usage.score("app:a", NOW) - 201.0).abs() < 1e-6);
        assert_eq!(usage.score("app:z", NOW), 1.0);

        let later = NOW + (HALF_LIFE as i64) * 86_400;
        assert!((usage.score("app:a", later) - 100.5).abs() < 1e-6);

        // faded to nothing, and never below 1
        assert_eq!(usage.score("app:a", NOW + 400 * 86_400), 1.0);

        // a clock that went back takes no score down
        let mut usage = Usage::default();
        usage.start("app:a", "", NOW);
        let before = usage.score("app:a", NOW);
        usage.start("app:a", "", NOW - 86_400);
        assert!(usage.score("app:a", NOW - 86_400) >= before + BONUS);

        // and the memory is bounded, the newest start staying
        let mut usage = Usage::default();
        for n in 0..KEPT + 20 {
            usage.start(&format!("app:{n}"), "", NOW);
        }
        assert_eq!(usage.0.len(), KEPT);
        assert!(usage.0.contains_key(&format!("app:{}", KEPT + 19)));
        assert!(!usage.0.contains_key("app:0"));
    }

    // a file is read as written, a line that does not read is skipped, and the old list of ids
    // becomes one start each in its order
    #[test]
    fn a_file_is_read_whole_or_by_line_and_the_old_list_migrates() {
        let mut usage = Usage::default();
        usage.start("app:a", "Fire  Fox", NOW);
        usage.start("command:caffeine toggle", "", NOW);

        let text = usage.text();
        assert_eq!(Usage::parse(&text, NOW), usage);
        assert_eq!(Usage::parse(&text, NOW).text(), text);
        assert_eq!(
            Usage::parse(&text, NOW).learned("app:a", "fire", NOW),
            PREFIX
        );

        let spoiled = format!("{text}app:b\tnot-a-number\t\n\t\t\napp:c\t0\n");
        let read = Usage::parse(&spoiled, NOW);
        assert_eq!(read.0.len(), 2);
        assert!((read.score("app:a", NOW) - 101.0).abs() < 1e-6);

        let old = Usage::migrated_recent("x\n\ny\nz\n", NOW);
        assert_eq!(old.recent(NOW), [app("x"), app("y"), app("z")]);
        assert!((old.score(&app("y"), NOW - 1) - 101.0).abs() < 1e-6);

        // the older count form keeps the score its count gave it, the later start first
        let older = Usage::migrated_usage(
            "2\t10\tfirefox.desktop\n2\t12\tkitty.desktop\n1\t11\tx.desktop\n",
            NOW,
        );
        assert_eq!(
            older.recent(NOW),
            [
                app("kitty.desktop"),
                app("firefox.desktop"),
                app("x.desktop")
            ]
        );
        assert!((older.score(&app("kitty.desktop"), NOW) - 2.0).abs() < 1e-2);
        assert!((older.score(&app("x.desktop"), NOW) - 1.0).abs() < 1e-2);
    }

    // a query lifts what it launched for seventeen days, and no more than twenty are kept
    #[test]
    fn a_query_lifts_what_it_launched_for_seventeen_days() {
        let mut usage = Usage::default();
        usage.start("app:a", "fire fox", NOW);

        assert_eq!(usage.learned("app:a", "fire fox", NOW), SAME);
        assert_eq!(usage.learned("app:a", "fire", NOW), PREFIX);
        assert_eq!(usage.learned("app:a", "fire fox 2", NOW), NEAR);
        assert_eq!(usage.learned("app:a", "firefox", NOW), NONE);
        assert_eq!(usage.learned("app:a", "", NOW), NONE);
        assert_eq!(usage.learned("app:z", "fire", NOW), NONE);

        // the same query again is the newest, not a second one
        usage.start("app:a", "FIRE  fox", NOW + 1);
        assert_eq!(usage.0["app:a"].terms.len(), 1);

        // past seventeen days it lifts nothing, and a read drops it
        let stale = NOW + TERM_WINDOW + 2;
        assert_eq!(usage.learned("app:a", "fire fox", stale), NONE);
        assert!(
            Usage::parse(&usage.text(), stale).0["app:a"]
                .terms
                .is_empty()
        );

        for n in 0..TERMS + 5 {
            usage.start("app:b", &format!("query {n}"), NOW);
        }
        assert_eq!(usage.0["app:b"].terms.len(), TERMS);
        assert_eq!(usage.learned("app:b", "query 24", NOW), SAME);
        assert_eq!(usage.learned("app:b", "query 4", NOW), NONE);
    }

    /*
     * the older `count`, `last` and id file keeps every item and the score its count gave it, and
     * is kept as this format on disk, the owner's alone; an old file is never read as the new one
     */
    #[test]
    fn an_older_file_on_disk_is_migrated_and_kept_private() {
        let dir = env::temp_dir().join(format!("kanade-recents-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let path = dir.join("launcher-usage");
        fs::write(
            &path,
            "3\t10\tfirefox.desktop\n1\t11\tkitty.desktop\n2\t9\torg.gnome.Files.desktop\n",
        )
        .unwrap();

        let usage = read_from(Some(&path), None, NOW);

        assert_eq!(usage.0.len(), 3);

        // the count is the score, and the later start only breaks a tie, so a habit keeps its weight
        assert_eq!(
            usage.recent(NOW),
            [
                app("firefox.desktop"),
                app("org.gnome.Files.desktop"),
                app("kitty.desktop")
            ]
        );
        assert_eq!(usage.score(&app("kitty.desktop"), NOW), 1.0);
        assert!((usage.score(&app("firefox.desktop"), NOW) - 3.0).abs() < 1e-2);

        // the file itself is this format now, readable by its owner only
        let saved = fs::read_to_string(&path).unwrap();
        assert!(head(&saved), "{saved}");
        assert_eq!(Usage::parse(&saved, NOW), usage);

        let bits = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(bits, 0o600);

        // and the next read takes it as this format, keeping what was written
        assert_eq!(read_from(Some(&path), None, NOW), usage);

        let _ = fs::remove_dir_all(&dir);
    }
}
