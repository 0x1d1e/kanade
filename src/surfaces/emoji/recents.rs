//! The emoji used lately, for the Emoji Surface's first row: `$XDG_STATE_HOME/kanade/emoji-recents`,
//! owner-only (the file 0600, its directory 0700 when Kanade creates it), newest first, one emoji per
//! line under a first line `kanade-emoji-recents 1`. Nothing else about an emoji is kept.
//!
//! A plain newest-first list, not the Launcher's frecency (`sources::recents`): the Surface shows
//! up to 24, in the order they were last used, so the last one used is the first one offered.
//!
//! The file is outside input and is read defensively: a missing, unreadable, truncated, oversized or
//! hostile file never stops the shell or loads anything but emoji Kanade knows, at most `RECENTS`.
//! Errors are logged by kind, never with content or a path. A file that could not be read at all, or
//! is a later Kanade's, is never written over for the run; a damaged one is first moved aside to
//! `emoji-recents.bak`, so the next use cannot destroy what was left of it.

use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;

use kanade_runtime::MutexExt;

use crate::atomic::{self, Mode};
use crate::config;

const FILE: &str = "kanade/emoji-recents";

// the first line without its version
const MAGIC: &str = "kanade-emoji-recents";
const VERSION: u32 = 1;

// how many the Surface offers in its Recent row
pub const RECENTS: usize = 24;

// the most a valid store holds: every recent one, with the longest of them and its newline
const FILE_BYTES: usize = RECENTS * 64 + 64;

// what the file on disk allows to be done to it, from how it was read at start
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    // read whole, or there was none: written as an emoji is used
    Ready,

    // read in part or not as a store: moved aside once, before the first write, then `Ready`
    Damaged,

    // not read for a reason that may pass (an I/O error, not a regular file), or a later Kanade's:
    // untouched, and nothing is written for the run
    Held,
}

// why a file was not read as a store
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refused {
    // a version this Kanade does not know
    Newer,

    // not a store: no first line of ours, or too large
    Unreadable,
}

// the emoji `bytes` holds, newest first, as far as they read: a line `emojis::get` rejects is
// skipped, and the list is capped at `RECENTS`
fn recents(bytes: &[u8]) -> Result<Vec<&'static str>, Refused> {
    if bytes.len() > FILE_BYTES {
        return Err(Refused::Unreadable);
    }

    let text = str::from_utf8(bytes).map_err(|_| Refused::Unreadable)?;
    let mut lines = text.lines();

    let version = lines
        .next()
        .and_then(|first| first.strip_prefix(MAGIC)?.strip_prefix(' '))
        .and_then(|version| version.parse::<u32>().ok())
        .ok_or(Refused::Unreadable)?;

    if version != VERSION {
        return Err(Refused::Newer);
    }

    Ok(lines
        .filter_map(|line| emojis::get(line.trim()).map(|emoji| emoji.as_str()))
        .fold(Vec::new(), |mut kept, emoji| {
            if kept.len() < RECENTS && !kept.contains(&emoji) {
                kept.push(emoji);
            }

            kept
        }))
}

// the store for `recents`, newest first
pub fn encode(recents: &[&str]) -> Vec<u8> {
    let mut text = format!("{MAGIC} {VERSION}\n");

    for emoji in recents.iter().take(RECENTS) {
        text.push_str(emoji);
        text.push('\n');
    }

    text.into_bytes()
}

fn write(path: &Path, recents: &[&str]) -> io::Result<()> {
    atomic::write(path, &encode(recents), Mode::Private)
}

// the file's bytes; none when there is no file, no more than a store can be
fn read_file(path: &Path) -> io::Result<Option<Vec<u8>>> {
    // opened first, never blocking and never through a link, and judged by the handle: a pipe put
    // in the file's place cannot hold the start thread, and nothing changes between check and read
    let flags = rustix::fs::OFlags::NONBLOCK | rustix::fs::OFlags::NOFOLLOW;
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags.bits() as i32)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };

    if !file.metadata()?.is_file() {
        return Err(io::ErrorKind::InvalidInput.into());
    }

    let mut bytes = Vec::new();
    file.take(FILE_BYTES as u64 + 1).read_to_end(&mut bytes)?;

    Ok(Some(bytes))
}

/*
 * the emoji in the store at `path`, newest first, and what may be done to the file after: none for
 * no file, a later Kanade's or one that could not be read (said by kind, and held); a damaged file
 * gives what read before the damage and is to be moved aside
 */
pub fn read(path: &Path) -> (Vec<&'static str>, Store) {
    let bytes = match read_file(path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return (Vec::new(), Store::Ready),
        Err(error) => {
            eprintln!(
                "kanade: cannot read the emoji recents ({}), left as they are",
                error.kind()
            );
            return (Vec::new(), Store::Held);
        }
    };

    match recents(&bytes) {
        Ok(kept) => {
            // a line the file holds but no emoji of is damage, kept aside rather than dropped
            let lines = str::from_utf8(&bytes).map_or(0, |text| text.lines().count());

            let whole = kept.len() == lines.saturating_sub(1);

            match whole {
                true => (kept, Store::Ready),
                false => {
                    eprintln!("kanade: the emoji recents file is damaged, kept as a .bak");
                    (kept, Store::Damaged)
                }
            }
        }
        Err(Refused::Newer) => {
            eprintln!("kanade: the emoji recents file is from a newer Kanade, left as it is");
            (Vec::new(), Store::Held)
        }
        Err(Refused::Unreadable) => {
            eprintln!("kanade: the emoji recents file is not readable, kept as a .bak");
            (Vec::new(), Store::Damaged)
        }
    }
}

// the damaged store to `<path>.bak`, owner-only, over an older backup: the remains of the latest
// damage are what the user last had, and one bounded file stays at rest, not one per damage
fn move_aside(path: &Path) -> io::Result<()> {
    let mut backup = path.as_os_str().to_owned();
    backup.push(".bak");

    match fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }

    match fs::rename(path, &backup) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

// the store for `recents`, as far as the file allows; a failure to move a damaged file aside holds
// the write, so the remains are not destroyed, and is tried again by the next
fn save(path: &Path, recents: &[&str], store: &mut Store) -> io::Result<()> {
    match *store {
        Store::Held => return Ok(()),
        Store::Damaged => {
            move_aside(path)?;
            *store = Store::Ready;
        }
        Store::Ready => {}
    }

    write(path, recents)
}

fn file() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok();

    config::base("XDG_STATE_HOME", ".local/state", home.as_deref()).map(|dir| dir.join(FILE))
}

// the recents in the store now, newest first; read at the first ask
static RECENT: Mutex<Option<Vec<&'static str>>> = Mutex::new(None);

// what the file allows to be done to it, from how it was read
static SAVING: Mutex<Store> = Mutex::new(Store::Ready);

// snapshot and write together, so an older snapshot cannot replace a newer one
static SAVE_NOW: Mutex<()> = Mutex::new(());

// the emoji used lately, newest first
pub fn with<R>(f: impl FnOnce(&[&'static str]) -> R) -> R {
    let mut recent = RECENT.locked();

    f(recent.get_or_insert_with(load))
}

// the store at start: what it holds, and what may be done to it from now on
fn load() -> Vec<&'static str> {
    let Some(path) = file() else {
        return Vec::new();
    };

    let (kept, store) = read(&path);
    *SAVING.locked() = store;

    kept
}

/*
 * `emoji` was copied: it goes to the front of the recents, and the file is written off the view
 * thread, as `sources::recents` saves
 */
pub fn used(emoji: &str) {
    let Some(emoji) = emojis::get(emoji).map(|emoji| emoji.as_str()) else {
        return;
    };

    {
        let mut held = RECENT.locked();
        let recent = held.get_or_insert_with(load);

        let mut next: Vec<&'static str> = Vec::with_capacity(RECENTS);

        next.push(emoji);
        next.extend(recent.iter().copied().filter(|each| *each != emoji));
        next.truncate(RECENTS);

        if next == *recent {
            return;
        }

        *recent = next;
    }

    thread::spawn(save_now);
}

// take and write the latest snapshot as one serialized operation
fn save_latest(
    serial: &Mutex<()>,
    recent: &Mutex<Option<Vec<&'static str>>>,
    save: impl FnOnce(&[&'static str]),
) {
    let _serial = serial.locked();
    let recents = recent.locked().clone().unwrap_or_default();

    if !recents.is_empty() {
        save(&recents);
    }
}

// the recents as they are now to the file, on this thread
fn save_now() {
    save_latest(&SAVE_NOW, &RECENT, |recents| {
        let mut store = SAVING.locked();

        if *store == Store::Held {
            return;
        }

        let Some(path) = file() else { return };

        if let Err(error) = save(&path, recents, &mut store) {
            eprintln!("kanade: cannot save the emoji recents ({})", error.kind());
        }
    });
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::os::unix::fs::PermissionsExt;
    use std::process;

    use super::*;

    fn temporary(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("kanade-emoji-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        dir
    }

    #[test]
    fn serialized_saves_write_the_latest_snapshot_last() {
        use std::sync::mpsc;

        let dir = temporary("serialized");
        let path = dir.join("emoji-recents");
        let serial = Mutex::new(());
        let recent = Mutex::new(Some(vec!["\u{1f680}"]));
        let (captured_tx, captured_rx) = mpsc::channel();
        let (continue_tx, continue_rx) = mpsc::channel();

        thread::scope(|scope| {
            let serial_ref = &serial;
            let recent_ref = &recent;
            let path_ref = &path;
            let first = scope.spawn(move || {
                save_latest(serial_ref, recent_ref, |snapshot| {
                    captured_tx.send(()).unwrap();
                    continue_rx.recv().unwrap();
                    write(path_ref, snapshot).unwrap();
                });
            });

            captured_rx.recv().unwrap();
            *recent.locked() = Some(vec!["\u{1f44d}"]);

            let second = scope.spawn(|| {
                save_latest(&serial, &recent, |snapshot| write(&path, snapshot).unwrap());
            });

            continue_tx.send(()).unwrap();
            first.join().unwrap();
            second.join().unwrap();
        });

        assert_eq!(read(&path).0, ["\u{1f44d}"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn recents_survive_the_file_newest_first_and_owner_only() {
        let dir = temporary("round");
        let path = dir.join("state/kanade/emoji-recents");
        let used = ["\u{1f680}", "\u{1f44d}", "\u{2764}\u{fe0f}"];

        write(&path, &used).unwrap();

        assert_eq!(read(&path), (used.to_vec(), Store::Ready));

        let bits = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(bits(&path), 0o600);
        assert_eq!(bits(path.parent().unwrap()), 0o700);

        let _ = fs::remove_dir_all(&dir);
    }

    // a line no emoji of is skipped, a truncated file keeps its earlier lines, and more than the row
    // shows is capped; a later Kanade's file is left alone
    #[test]
    fn a_damaged_or_hostile_file_reads_only_what_it_can() {
        let whole = encode(&["\u{1f680}", "\u{1f44d}", "\u{2764}\u{fe0f}"]);
        let text = String::from_utf8(whole.clone()).unwrap();

        // a cut anywhere keeps the lines complete before it, and no more than RECENTS
        for cut in 0..whole.len() {
            let kept = recents(&whole[..cut]).unwrap_or_default();

            assert!(kept.len() <= 3, "cut at {cut}");
        }

        assert_eq!(
            recents(b"kanade-emoji-recents 1\nnot an emoji\n\xf0\x9f\x9a\x80\n").unwrap(),
            ["\u{1f680}"]
        );

        // a repeated emoji is offered once
        assert_eq!(
            recents(b"kanade-emoji-recents 1\n\xf0\x9f\x9a\x80\n\xf0\x9f\x9a\x80\n").unwrap(),
            ["\u{1f680}"]
        );

        // more than the row shows is cut to it
        let mut many = String::from("kanade-emoji-recents 1\n");
        for emoji in emojis::iter().take(RECENTS + 4) {
            many.push_str(emoji.as_str());
            many.push('\n');
        }
        assert_eq!(recents(many.as_bytes()).unwrap().len(), RECENTS);

        let cases: [(&str, &[u8]); 6] = [
            ("empty", b""),
            ("garbage", b"\x00\xff\xfe not a store"),
            ("wrong magic", b"kanade-recents 1\n\xf0\x9f\x9a\x80\n"),
            ("no version", b"kanade-emoji-recents\n\xf0\x9f\x9a\x80\n"),
            ("oversized", &vec![b'x'; FILE_BYTES + 1]),
            ("not UTF-8", b"kanade-emoji-recents 1\n\xff\xfe\n"),
        ];

        for (name, bytes) in cases {
            assert_eq!(recents(bytes).err(), Some(Refused::Unreadable), "{name}");
        }

        assert_eq!(
            recents(b"kanade-emoji-recents 2\n\xf0\x9f\x9a\x80\n").err(),
            Some(Refused::Newer)
        );

        assert_eq!(recents(text.as_bytes()).unwrap().len(), 3);
    }

    // what could not be read, or is a later Kanade's, is never written over; a damaged file is kept
    // aside before the first write
    #[test]
    fn a_held_file_is_left_and_a_damaged_one_kept_as_a_backup() {
        let dir = temporary("held");
        let path = dir.join("emoji-recents");
        let used = ["\u{1f680}"];

        let newer = b"kanade-emoji-recents 2\n\xf0\x9f\x9a\x80\n";
        fs::write(&path, newer).unwrap();

        let (kept, mut store) = read(&path);
        assert_eq!((kept.len(), store), (0, Store::Held));

        save(&path, &used, &mut store).unwrap();
        assert_eq!(fs::read(&path).unwrap(), newer);

        // a directory is no store, and not ours to touch; a missing file is simply none
        assert_eq!(read(&dir), (Vec::new(), Store::Held));
        assert_eq!(read(&dir.join("missing")), (Vec::new(), Store::Ready));

        // a damaged file is moved aside, owner-only, and what was left of it is kept there
        let damaged = b"kanade-emoji-recents 1\n\xff\xfe\n";
        fs::write(&path, damaged).unwrap();

        let (_, mut store) = read(&path);
        assert_eq!(store, Store::Damaged);

        save(&path, &used, &mut store).unwrap();

        let backup = PathBuf::from(format!("{}.bak", path.display()));
        assert_eq!(fs::read(&backup).unwrap(), damaged);
        assert_eq!(
            fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(read(&path), (used.to_vec(), Store::Ready));

        let _ = fs::remove_dir_all(&dir);
    }
}
