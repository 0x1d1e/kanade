//! The apps the Launcher started last, newest first, which an empty query lists before the rest.
//! One id per line in `$XDG_STATE_HOME/kanade/launcher-recent`; a file that cannot be read or
//! written leaves the list as it is in memory.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;

use kanade_runtime::MutexExt;

use crate::atomic::{self, Mode};
use crate::config;

// as many as the empty query lists ahead of the alphabet
const KEPT: usize = 6;

static RECENT: Mutex<Option<Vec<String>>> = Mutex::new(None);

// one save at a time, so the file is never written by two threads at once
static SAVING: Mutex<()> = Mutex::new(());

fn file() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok();

    config::base("XDG_STATE_HOME", ".local/state", home.as_deref())
        .map(|dir| dir.join("kanade/launcher-recent"))
}

// the app ids, newest first; the file is read at the first ask
pub fn ids() -> Vec<String> {
    RECENT.locked().get_or_insert_with(read).clone()
}

// the app `id` was started: first from now on, saved off the view thread
pub fn used(id: &str) {
    {
        let mut recent = RECENT.locked();
        let before = recent.take().unwrap_or_else(read);

        *recent = Some(bump(before, id));
    }

    thread::spawn(save);
}

fn read() -> Vec<String> {
    file()
        .and_then(|file| fs::read_to_string(file).ok())
        .map(|text| parse(&text))
        .unwrap_or_default()
}

// the list as it is when the save gets its turn, so a save that runs late never writes an older one
fn save() {
    let _saving = SAVING.locked();
    let list = RECENT.locked().clone().unwrap_or_default();

    if let Some(file) = file() {
        let _ = atomic::write(&file, list.join("\n").as_bytes(), Mode::Shared);
    }
}

fn parse(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .take(KEPT)
        .map(str::to_owned)
        .collect()
}

// `id` first, once, and no more than `KEPT`
fn bump(mut list: Vec<String>, id: &str) -> Vec<String> {
    list.retain(|kept| kept != id);
    list.insert(0, id.to_owned());
    list.truncate(KEPT);

    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_start_is_first_once_and_few_are_kept() {
        let mut list = Vec::new();

        for id in ["a", "b", "c", "a", "d", "e", "f", "g"] {
            list = bump(list, id);
        }

        assert_eq!(list, ["g", "f", "e", "d", "a", "c"]);
        assert_eq!(parse("g\n\n f \nx"), ["g", "f", "x"]);
    }
}
