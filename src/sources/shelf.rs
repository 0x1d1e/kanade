//! The shelf of recent files (ADR 0141): what the desktop's own `recently-used.xbel` names, kept
//! newest first, read only when the Files Surface opens or asks again, never every frame. It is a
//! shelf, not a manager: nothing here writes, moves or removes a file, and a file's contents are
//! never read, only its path and its modification time. A missing, oversized or malformed file is
//! an empty shelf. No path or file name is ever logged.
//!
//! The desktop writes the list itself, as GTK apps close a document; Kanade only reads it.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, atomic};
use std::thread;
use std::time::UNIX_EPOCH;

use kanade_runtime::MutexExt;
use kanade_runtime::service::Service;

use crate::config;

// the desktop's list, in the data directory
const XBEL: &str = "recently-used.xbel";

// a list larger than this is not read at all: the shelf is empty rather than a stall
const MOST: u64 = 2 * 1024 * 1024;

// the most files the shelf keeps
pub const CAP: usize = 64;

// the most candidates its list is scanned for: a list naming more is not stat'ed further
const SCAN: usize = 1024;

// one recent file, as a row shows it: its name, and the folder it is in
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recent {
    pub path: PathBuf,
    pub name: String,
    pub folder: String,

    // seconds since the epoch it was last written; the shelf is ordered by it
    pub modified: u64,
}

impl Recent {
    // whether every word of `query` is somewhere in its name or its folder, in any case; a query of
    // only spaces finds every file
    pub fn matches(&self, query: &str) -> bool {
        let name = self.name.to_lowercase();
        let folder = self.folder.to_lowercase();

        query.split_whitespace().all(|term| {
            name.contains(&term.to_lowercase()) || folder.contains(&term.to_lowercase())
        })
    }
}

// the shelf, as the view reads it
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shelf {
    pub files: Vec<Recent>,
}

impl Service for Shelf {
    fn new() -> Self {
        Shelf::default()
    }

    fn listen() {}
}

/*
 * the visit of the Surface the shelf was last asked for, so opening it reads once and a frame of a
 * morph never reads again; `refresh` asks again within the same visit
 */
static ASKED: Mutex<Option<u64>> = Mutex::new(None);

// a read is running, so a second ask starts no second thread
static READING: atomic::AtomicBool = atomic::AtomicBool::new(false);

/*
 * the Files Surface opened in `visit`: the shelf is read on a thread of its own unless this visit
 * already read it. A read the Surface never sees is no harm, it is written to the Service
 */
pub fn opened(visit: u64) {
    {
        let mut asked = ASKED.locked();

        if *asked == Some(visit) {
            return;
        }

        *asked = Some(visit);
    }

    refresh();
}

// reads the shelf again, off the draw thread; a read already running is left to finish
pub fn refresh() {
    if READING.swap(true, atomic::Ordering::SeqCst) {
        return;
    }

    let read = thread::Builder::new()
        .name(String::from("kanade-shelf"))
        .spawn(|| {
            let shelf = Shelf { files: list() };

            // a write wakes the window even when nothing changed, so only write a real change
            if *Shelf::read() != shelf {
                *Shelf::write() = shelf;
            }

            READING.store(false, atomic::Ordering::SeqCst);
        });

    if let Err(error) = read {
        READING.store(false, atomic::Ordering::SeqCst);
        eprintln!("kanade: cannot read the recent files ({error})");
    }
}

// the user's recent files, newest first, at most `CAP`; nothing when there is no list to read
pub fn list() -> Vec<Recent> {
    xbel().map_or_else(Vec::new, |xbel| read(&xbel, &contained()))
}

// what the list at `xbel` names, leaving out what is under `skip`
fn read(xbel: &Path, skip: &[PathBuf]) -> Vec<Recent> {
    let Ok(meta) = fs::metadata(xbel) else {
        return Vec::new();
    };

    if !meta.is_file() || meta.len() > MOST {
        return Vec::new();
    }

    /*
     * read at most one byte past the guard, so a file that grows after the check is still not read
     * whole: the shelf is empty rather than a growing file read to its end
     */
    let Ok(file) = fs::File::open(xbel) else {
        return Vec::new();
    };
    let mut bytes = Vec::new();

    if file.take(MOST + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > MOST {
        return Vec::new();
    }

    shelf(&String::from_utf8_lossy(&bytes), skip)
}

// where the desktop keeps its list: the data directory, else `~/.local/share`
fn xbel() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok();

    config::base("XDG_DATA_HOME", ".local/share", home.as_deref()).map(|dir| dir.join(XBEL))
}

/*
 * Kanade's own directories: what it writes for itself is never a file the user opened, and its
 * notes and settings have no place on the shelf
 */
fn contained() -> Vec<PathBuf> {
    let home = std::env::var("HOME").ok();
    let home = home.as_deref();

    [
        ("XDG_CONFIG_HOME", ".config"),
        ("XDG_DATA_HOME", ".local/share"),
        ("XDG_STATE_HOME", ".local/state"),
    ]
    .into_iter()
    .filter_map(|(variable, default)| config::base(variable, default, home))
    .map(|dir| dir.join("kanade"))
    .collect()
}

/*
 * the shelf a list's text names: every `file://` address it holds, in the order they appear, with
 * the ones that are still files on this machine and not Kanade's own; `skip` is what is left out.
 * Each path is read with its `.` and `..` taken literally before it is kept, so a list that names
 * one file two ways is one row, and the list is scanned for at most `SCAN` candidates
 */
fn shelf(text: &str, skip: &[PathBuf]) -> Vec<Recent> {
    let mut files: Vec<Recent> = Vec::new();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let mut scanned = 0;

    for path in hrefs(text).filter_map(|href| local(&decoded(href))) {
        let path = normalized(&path);

        if skip.iter().any(|dir| path.starts_with(dir)) || !seen.insert(path.clone()) {
            continue;
        }

        if scanned == SCAN {
            break;
        }

        scanned += 1;

        if let Some(file) = recent(&path) {
            files.push(file);
        }
    }

    // newest first; files with no time read keep the list's own order after the rest
    files.sort_by_key(|file| std::cmp::Reverse(file.modified));
    files.truncate(CAP);

    files
}

// `path` with its `.` and `..` read as parts of it, so two spellings of one place are one path
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();

    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            part => out.push(part.as_os_str()),
        }
    }

    out
}

// every `file://` address the text holds, as it is written there
fn hrefs(text: &str) -> impl Iterator<Item = &str> + '_ {
    text.split("href=\"")
        .skip(1)
        .map(|rest| rest.split('"').next().unwrap_or_default())
        .filter(|href| href.starts_with("file://"))
}

// the path a `file://` address names, none for one of another host or that names no absolute path
fn local(address: &str) -> Option<PathBuf> {
    let rest = address.strip_prefix("file://")?;

    // `host/path` names another machine's file; only this one's are read
    rest.starts_with('/').then(|| PathBuf::from(rest))
}

// `value` with its `%XX` escapes read as bytes; an escape that is not one is kept as it is
fn decoded(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;

    while at < bytes.len() {
        let hex = bytes
            .get(at + 1..at + 3)
            .and_then(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok());

        match (bytes[at], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                at += 3;
            }
            (byte, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

// the file `path` names, once it is a regular file this user can read; none otherwise
fn recent(path: &Path) -> Option<Recent> {
    let meta = fs::metadata(path).ok().filter(|meta| meta.is_file())?;

    // readable: opening it reads no byte of it, and it is let go at once
    fs::File::open(path).ok()?;

    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_secs());

    Some(Recent {
        path: path.to_path_buf(),
        name: name(path),
        folder: folder(path),
        modified,
    })
}

fn name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// the folder it is in, the home directory written as `~`
fn folder(path: &Path) -> String {
    let parent = path.parent().map(Path::to_string_lossy).unwrap_or_default();

    let home = std::env::var("HOME").ok().filter(|home| !home.is_empty());

    match home {
        Some(home) if parent == home => String::from("~"),
        Some(home) if parent.starts_with(&format!("{home}/")) => {
            format!("~{}", &parent[home.len()..])
        }
        _ => parent.into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanade-shelf-{}-{name}", std::process::id()));

        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temporary directory");

        dir
    }

    // a list's hrefs are its `file://` addresses, each read out of its attribute as it is written
    #[test]
    fn a_list_names_its_files_by_href() {
        let text = r#"<?xml version="1.0"?>
<xbel version="1.0">
  <bookmark href="file:///home/someone/notes%20on%20it.txt" added="2024-01-01T00:00:00Z"/>
  <bookmark href="https://example.com/" added="2024-01-01T00:00:00Z"/>
  <bookmark href="file:///tmp/seconde.md"/>
</xbel>"#;

        let found: Vec<&str> = hrefs(text).collect();

        assert_eq!(
            found,
            [
                "file:///home/someone/notes%20on%20it.txt",
                "file:///tmp/seconde.md"
            ]
        );
    }

    // the escapes a path is written with are read back: a space, and a letter outside ascii
    #[test]
    fn escapes_are_read_as_bytes() {
        assert_eq!(decoded("/a%20b/c.txt"), "/a b/c.txt");
        assert_eq!(decoded("/a/caf%C3%A9.txt"), "/a/café.txt");
        assert_eq!(decoded("/a/100%.txt"), "/a/100%.txt");
        assert_eq!(decoded("/a/%zz.txt"), "/a/%zz.txt");
        assert_eq!(decoded(""), "");
    }

    // a list that is missing, or too large to read, is an empty shelf rather than a stall
    #[test]
    fn a_missing_or_huge_list_is_empty() {
        let dir = temp("guard");
        let gone = dir.join("gone.xbel");

        assert!(read(&gone, &[]).is_empty());

        let here = dir.join("here.txt");
        fs::write(&here, b"x").expect("a file");

        let entry = format!(r#"<bookmark href="file://{}"/>"#, here.display());

        let huge = dir.join("huge.xbel");
        let mut text = String::from("<xbel>");
        while (text.len() as u64) < MOST {
            text.push_str(&entry);
        }
        fs::write(&huge, text.as_bytes()).expect("a large list");

        assert!(fs::metadata(&huge).expect("its size").len() > MOST);
        assert!(read(&huge, &[]).is_empty());

        // the same file, under the guard, is read
        let small = dir.join("small.xbel");
        fs::write(&small, format!("<xbel>{entry}</xbel>")).expect("a list");
        assert_eq!(read(&small, &[]).len(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    // a file gone from the machine is dropped, and one of another host is never read
    #[test]
    fn a_file_that_is_gone_is_dropped() {
        let dir = temp("gone");
        let here = dir.join("here.txt");
        fs::write(&here, b"kept").expect("a file");

        let text = format!(
            r#"<xbel><bookmark href="file://{}/here.txt"/><bookmark href="file://{}/gone.txt"/><bookmark href="file://otherhost/etc/passwd"/></xbel>"#,
            dir.display(),
            dir.display()
        );

        let files = shelf(&text, &[]);

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, here);
        assert_eq!(files[0].name, "here.txt");

        let _ = fs::remove_dir_all(&dir);
    }

    // Kanade's own files are no file the user opened
    #[test]
    fn kanades_own_directories_are_left_out() {
        let dir = temp("own");
        let state = dir.join("kanade");
        fs::create_dir_all(&state).expect("its directory");
        let notes = state.join("desk-notes");
        fs::write(&notes, b"a note").expect("a note");

        let text = format!(
            r#"<xbel><bookmark href="file://{}"/></xbel>"#,
            notes.display()
        );

        assert!(shelf(&text, &[state]).is_empty());
        assert_eq!(
            shelf(&text, &[]).len(),
            1,
            "the same file is no file of Kanade's"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // the shelf is capped, newest first, whatever order the list holds
    #[test]
    fn the_shelf_is_newest_first_and_capped() {
        let dir = temp("cap");
        let mut text = String::from("<xbel>");

        // one more than the cap, a second apart, oldest first in the list
        for at in 0..=CAP {
            let path = dir.join(format!("file-{at}.txt"));
            let file = fs::File::create(&path).expect("a file");
            let written = UNIX_EPOCH + std::time::Duration::from_secs(1_000_000 + at as u64);

            file.set_times(fs::FileTimes::new().set_modified(written))
                .expect("its time");

            text.push_str(&format!(r#"<bookmark href="file://{}"/>"#, path.display()));
        }

        text.push_str("</xbel>");

        let files = shelf(&text, &[]);

        assert_eq!(files.len(), CAP);
        assert_eq!(files[0].name, format!("file-{CAP}.txt"));
        assert_eq!(files[CAP - 1].name, "file-1.txt");
        assert!(files.iter().all(|file| file.name != "file-0.txt"));

        // the same file named twice is one row, and so is one named two ways
        let twice = format!(
            r#"<xbel><bookmark href="file://{0}/file-1.txt"/><bookmark href="file://{0}/file-1.txt"/></xbel>"#,
            dir.display()
        );

        assert_eq!(shelf(&twice, &[]).len(), 1);

        let indirect = format!(
            r#"<xbel><bookmark href="file://{0}/sub/../file-1.txt"/><bookmark href="file://{0}/file-1.txt"/></xbel>"#,
            dir.display()
        );
        let found = shelf(&indirect, &[]);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, dir.join("file-1.txt"));

        let _ = fs::remove_dir_all(&dir);
    }

    // a query finds a file by its name, and by the folder it is in, in any case
    #[test]
    fn a_query_matches_the_name_and_the_folder() {
        let file = Recent {
            path: PathBuf::from("/home/someone/Projects/kanade/notes on it.txt"),
            name: String::from("notes on it.txt"),
            folder: String::from("~/Projects/kanade"),
            modified: 0,
        };

        assert!(file.matches(""));
        assert!(file.matches("NOTES"));
        assert!(file.matches("notes kanade"));
        assert!(file.matches("projects txt"));
        assert!(!file.matches("notes other"));
    }
}
