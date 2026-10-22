//! The pinned entries across restarts (ADR 0115): one file, `$XDG_STATE_HOME/kanade/clipboard-pins`,
//! owner-only (the file 0600, its directory 0700 when Kanade creates it), holding the pins as they were copied, plaintext.
//! Only entries that passed the sensitive gate (ADR 0019) are ever pinned, so only those are written.
//!
//! The file is outside input and is read defensively: a missing, unreadable, truncated, oversized or
//! hostile file never stops the shell or loads anything but entries that are valid on their own, and
//! only what the pins' caps allow. Errors are logged by kind, never with content or a path.
//!
//! What is read decides what may be written (`Store`): a file that could not be read at all, or is a
//! later Kanade's, is never written over for the run; a damaged one is first moved aside to
//! `clipboard-pins.bak`, so the next pin change cannot destroy what was left of it.
//!
//! Layout: a first line `kanade-clipboard-pins 1`, then per pin, first to last, a line
//! `text <length>` or `image <type> <length>`, that many bytes and a newline.

use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use kanade_runtime::{MutexExt, service::Service};

use super::{Clipboard, Content, ENTRY_BYTES, MIME_BYTES, PIN_BYTES, PINS, framed};
use crate::atomic::{self, Mode};
use crate::config;

const FILE: &str = "kanade/clipboard-pins";

// the first line without its version
const MAGIC: &str = "kanade-clipboard-pins";
const VERSION: u32 = 1;

// a pin's header line is at most `image `, a type and a length
const HEADER_BYTES: usize = MIME_BYTES + 64;

// the largest file a valid store comes to: the pins' bytes and, for each, its header and newline
const FILE_BYTES: usize = PIN_BYTES + PINS * (HEADER_BYTES + 1) + HEADER_BYTES;

// how long `flush` waits for the worker to say the file holds the pins
const FLUSHED: Duration = Duration::from_secs(5);

// what the file on disk allows to be done to it, from how it was read at start
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    // read whole, or there was none: written as the pins change
    Ready,
    // read in part or not as a store: moved aside once, before the first write, then `Ready`
    Damaged,
    // not read for a reason that may pass (an I/O error, not a regular file), or a later Kanade's:
    // untouched, and nothing is written for the run
    Held,
}

// why a file was not read as a store
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    // a version this Kanade does not know
    Newer,
    // not a store: no first line of ours, or too large
    Unreadable,
}

// the pins in `bytes`, in order, as far as they read: an entry that is not valid on its own is
// skipped, and one whose framing is lost (a bad length, a cut) ends the list with what came before
pub fn frames(bytes: &[u8]) -> Result<Frames<'_>, Refused> {
    if bytes.len() > FILE_BYTES {
        return Err(Refused::Unreadable);
    }

    let (first, rest) = line(bytes).ok_or(Refused::Unreadable)?;
    let version = str::from_utf8(first)
        .ok()
        .and_then(|first| first.strip_prefix(MAGIC)?.strip_prefix(' '))
        .and_then(|version| version.parse::<u32>().ok())
        .ok_or(Refused::Unreadable)?;

    match version {
        VERSION => Ok(Frames {
            rest,
            damaged: false,
        }),
        _ => Err(Refused::Newer),
    }
}

pub struct Frames<'a> {
    rest: &'a [u8],
    // an entry was skipped or the framing was lost: what the iterator gave is not all the file held
    damaged: bool,
}

impl Frames<'_> {
    pub fn damaged(&self) -> bool {
        self.damaged
    }

    fn lose(&mut self) {
        self.rest = &[];
        self.damaged = true;
    }
}

impl Iterator for Frames<'_> {
    type Item = Content;

    fn next(&mut self) -> Option<Content> {
        loop {
            if self.rest.is_empty() {
                return None;
            }

            let Some((header, rest)) = line(self.rest) else {
                self.lose();
                return None;
            };

            let Some((kind, mime, len)) = header_of(header) else {
                self.lose();
                return None;
            };

            // a length past the entry limit or the bytes left cannot be stepped over
            if len > ENTRY_BYTES || rest.len() <= len || rest[len] != b'\n' {
                self.lose();
                return None;
            }

            let (body, after) = rest.split_at(len);
            self.rest = &after[1..];

            if let Some(content) = content_of(kind, mime, body) {
                return Some(content);
            }

            self.damaged = true;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Image,
}

// the line up to the first newline, within a header's length, and what follows it
fn line(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let end = bytes
        .iter()
        .take(HEADER_BYTES + 1)
        .position(|b| *b == b'\n')?;

    Some((&bytes[..end], &bytes[end + 1..]))
}

// `text <length>` or `image <type> <length>`
fn header_of(header: &[u8]) -> Option<(Kind, &str, usize)> {
    let header = str::from_utf8(header).ok()?;
    let mut parts = header.split(' ');

    let (kind, mime) = match parts.next()? {
        "text" => (Kind::Text, ""),
        "image" => (Kind::Image, parts.next()?),
        _ => return None,
    };

    let len = parts.next()?.parse::<usize>().ok()?;

    parts.next().is_none().then_some((kind, mime, len))
}

// what an entry holds, if it is one Kanade keeps: text that is UTF-8, an `image/` type of the shape
// a copy's is, never nothing
fn content_of(kind: Kind, mime: &str, body: &[u8]) -> Option<Content> {
    if body.is_empty() {
        return None;
    }

    match kind {
        Kind::Text => Some(Content::Text(str::from_utf8(body).ok()?.into())),
        Kind::Image => (mime.starts_with("image/") && framed(mime)).then(|| Content::Image {
            mime: mime.into(),
            bytes: body.into(),
        }),
    }
}

// the store for `pins`, first to last; a pin that is not one Kanade could read back is left out
pub fn encode(pins: &[Content]) -> Vec<u8> {
    let mut bytes = format!("{MAGIC} {VERSION}\n").into_bytes();

    for pin in pins {
        let header = match pin {
            Content::Text(_) => String::from("text"),
            Content::Image { mime, .. } if mime.starts_with("image/") && framed(mime) => {
                format!("image {mime}")
            }
            Content::Image { .. } => continue,
        };

        bytes.extend_from_slice(format!("{header} {}\n", pin.len()).as_bytes());
        bytes.extend_from_slice(pin.bytes());
        bytes.push(b'\n');
    }

    bytes
}

// writes the store, or deletes it when nothing is pinned, so no unpinned entry stays on disk
pub fn write(path: &Path, pins: &[Content]) -> io::Result<()> {
    if pins.is_empty() {
        return match fs::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }

    atomic::write(path, &encode(pins), Mode::Private)
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

// the pins in the store at `path`, and what may be done to the file after: none for no file, a
// later Kanade's or one that could not be read (said by kind, and held); a damaged file gives what
// read before the damage and is to be moved aside
pub fn read(path: &Path) -> (Vec<Content>, Store) {
    let bytes = match read_file(path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return (Vec::new(), Store::Ready),
        Err(error) => {
            eprintln!(
                "kanade: cannot read the clipboard pins ({}), left as they are",
                error.kind()
            );
            return (Vec::new(), Store::Held);
        }
    };

    match frames(&bytes) {
        Ok(mut frames) => {
            let pins = within_caps(&mut frames);

            if frames.damaged() {
                eprintln!("kanade: the clipboard pins file is damaged, kept as a .bak");
                (pins, Store::Damaged)
            } else {
                (pins, Store::Ready)
            }
        }
        Err(Refused::Newer) => {
            eprintln!("kanade: the clipboard pins file is from a newer Kanade, left as it is");
            (Vec::new(), Store::Held)
        }
        Err(Refused::Unreadable) => {
            eprintln!("kanade: the clipboard pins file is not readable, kept as a .bak");
            (Vec::new(), Store::Damaged)
        }
    }
}

/*
 * the pins `frames` holds, as far as the pins' caps allow: at most `PINS` and `PIN_BYTES`, which is
 * what `Clipboard::adopt` keeps. The caps are enforced here, while reading, since a store of legal
 * size can hold a million one-byte entries, and reading them all first would hold them all
 */
fn within_caps(frames: &mut Frames<'_>) -> Vec<Content> {
    let mut pins: Vec<Content> = Vec::new();
    let mut bytes = 0;

    for content in frames.by_ref() {
        let size = content.len();

        if pins.len() >= PINS || bytes + size > PIN_BYTES {
            // a file with more than the caps allow is not one Kanade wrote: what it holds past
            // them is left unread, and the file is kept aside rather than dropped on the next save
            frames.lose();
            break;
        }

        bytes += size;
        pins.push(content);
    }

    pins
}

// the damaged store to `<path>.bak`, owner-only, over an older backup: the remains of the latest damage
// are what the user last had, and one bounded file stays at rest, not one per damage
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

// the store for `pins`, as far as the file allows; a failure to move a damaged file aside holds
// the write, so the remains are not destroyed, and is tried again by the next
fn save(path: &Path, pins: &[Content], store: &mut Store) -> io::Result<()> {
    match *store {
        Store::Held => return Ok(()),
        Store::Damaged => {
            move_aside(path)?;
            *store = Store::Ready;
        }
        Store::Ready => {}
    }

    write(path, pins)
}

fn file() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok();

    config::base("XDG_STATE_HOME", ".local/state", home.as_deref()).map(|dir| dir.join(FILE))
}

// at start, before the watcher records anything: the stored pins into the history
pub fn load() {
    let Some(path) = file() else { return };

    let (pins, store) = read(&path);
    *saving() = store;

    if !pins.is_empty() {
        Clipboard::write().adopt(pins);
    }
}

// one save at a time, so the file is never written by two threads at once; what it allows is in it
static SAVING: Mutex<Store> = Mutex::new(Store::Ready);

fn saving() -> MutexGuard<'static, Store> {
    SAVING.locked()
}

// who may be waiting for a save to reach the file
struct Asked {
    // hears once the file holds the pins, none for a save nobody waits on
    done: Option<Sender<()>>,
}

// the one worker that writes the file, started by the first ask
static ASKERS: Mutex<Option<Sender<Asked>>> = Mutex::new(None);

// whether anything asked for a save this run, so the end of the shell writes only then
static ASKED: AtomicBool = AtomicBool::new(false);

fn askers() -> MutexGuard<'static, Option<Sender<Asked>>> {
    ASKERS.locked()
}

/*
 * writes the pins as they are when the save gets its turn, off the caller's thread, so a save that
 * runs late never writes an older set. One worker takes the asks, so a burst of changes past what
 * one write carries is one further write, not one write each. The history is read only to take the
 * pins, and no lock is held across the disk
 */
pub fn save_soon() {
    ask(None);
}

/*
 * saves now and waits for the file to hold the pins: for the end of the shell, so a pin made a
 * moment before it is not lost. A worker that cannot be started writes on this thread instead.
 * Nothing is written where no pin changed this run, so a shell whose pins were left alone leaves
 * the file as it found it
 */
pub fn flush() {
    if !ASKED.load(Ordering::Relaxed) {
        return;
    }

    let (done, waited) = mpsc::channel();

    ask(Some(done));

    // the worker keeps the channel open for the whole process, so a worker that panicked would
    // otherwise wait here for good
    if waited.recv_timeout(FLUSHED).is_err() {
        eprintln!("kanade: the clipboard pins may not have been saved");
    }
}

fn ask(done: Option<Sender<()>>) {
    ASKED.store(true, Ordering::Relaxed);

    if let Some(asked) = sender() {
        if let Err(mpsc::SendError(unsent)) = asked.send(Asked { done }) {
            // the worker is gone, so the pins are written here rather than lost
            save_now();
            if let Some(done) = unsent.done {
                let _ = done.send(());
            }
        }

        return;
    }

    // no worker could start: written here, so a pin is never lost for want of a thread
    save_now();
}

// the worker's ask channel, started the first time one is needed
fn sender() -> Option<Sender<Asked>> {
    let mut askers = askers();

    if askers.is_none() {
        let (send, asked) = mpsc::channel();
        let started = thread::Builder::new()
            .name("clipboard-pins".into())
            .spawn(move || follow(asked));

        match started {
            Ok(_) => *askers = Some(send),
            Err(error) => {
                eprintln!("kanade: cannot save the clipboard pins ({})", error.kind());
                return None;
            }
        }
    }

    askers.clone()
}

// every ask until the shell ends, each burst of them one write
fn follow(asked: Receiver<Asked>) {
    while let Ok(first) = asked.recv() {
        let done = batch(first, &asked);

        save_now();

        for done in done {
            let _ = done.send(());
        }
    }
}

/*
 * the waiters one write answers: the ask taken and every ask that queued meanwhile, so a burst of
 * changes costs one write and each waiter hears of the file that holds what it asked for
 */
fn batch(first: Asked, asked: &Receiver<Asked>) -> Vec<Sender<()>> {
    let mut done: Vec<Sender<()>> = first.done.into_iter().collect();

    while let Ok(next) = asked.try_recv() {
        done.extend(next.done);
    }

    done
}

// the pins as they are now to the file, on this thread
fn save_now() {
    let mut store = saving();

    if *store == Store::Held {
        return;
    }

    let Some(path) = file() else { return };

    let pins: Vec<Content> = Clipboard::read()
        .entries()
        .iter()
        .filter(|entry| entry.pinned)
        .map(|entry| entry.content.clone())
        .collect();

    if let Err(error) = save(&path, &pins, &mut store) {
        eprintln!("kanade: cannot save the clipboard pins ({})", error.kind());
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::os::unix::fs::PermissionsExt;
    use std::process;

    use super::*;

    fn temporary(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("kanade-pins-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        dir
    }

    fn text(text: &str) -> Content {
        Content::Text(text.into())
    }

    fn image(mime: &str, bytes: &[u8]) -> Content {
        Content::Image {
            mime: mime.into(),
            bytes: bytes.into(),
        }
    }

    fn decoded(bytes: &[u8]) -> Vec<Content> {
        frames(bytes).map(Iterator::collect).unwrap_or_default()
    }

    #[test]
    fn pins_survive_the_file_in_order() {
        let dir = temporary("round");
        let path = dir.join("state/kanade/clipboard-pins");

        // every byte value and newlines, in text and in a picture
        let binary: Vec<u8> = (0..=255).chain(*b"\n\n").collect();
        let pins = vec![
            text("two\nlines\n"),
            image("image/png", &binary),
            text("last"),
            image("image/x-weird.type+1", b"\x00\n"),
        ];

        write(&path, &pins).unwrap();

        assert_eq!(read(&path), (pins, Store::Ready));

        let bits = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(bits(&path), 0o600);
        assert_eq!(bits(path.parent().unwrap()), 0o700);

        let _ = fs::remove_dir_all(&dir);
    }

    // a burst of changes is one write, not one ~16 MiB write each, and every waiter hears of it
    #[test]
    fn a_burst_of_changes_is_one_batch() {
        let (send, asked) = mpsc::channel();
        let (one, waited_one) = mpsc::channel();
        let (two, waited_two) = mpsc::channel();

        send.send(Asked { done: Some(one) }).unwrap();
        send.send(Asked { done: None }).unwrap();
        send.send(Asked { done: Some(two) }).unwrap();

        let done = batch(asked.recv().unwrap(), &asked);
        assert_eq!(done.len(), 2);

        for waiter in done {
            waiter.send(()).unwrap();
        }
        assert!(waited_one.try_recv().is_ok());
        assert!(waited_two.try_recv().is_ok());

        // the queue is empty now, so the next ask is a batch of its own
        send.send(Asked { done: None }).unwrap();
        assert!(batch(asked.recv().unwrap(), &asked).is_empty());
    }

    #[test]
    fn nothing_pinned_leaves_no_file_and_an_unpinned_entry_leaves_no_bytes() {
        let dir = temporary("gone");
        let path = dir.join("clipboard-pins");

        write(&path, &[text("secret-one"), text("secret-two")]).unwrap();
        write(&path, &[text("secret-two")]).unwrap();

        let on_disk = fs::read(&path).unwrap();
        assert!(!String::from_utf8_lossy(&on_disk).contains("secret-one"));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "no part left over");

        write(&path, &[]).unwrap();
        assert!(!path.exists());

        // nothing to delete is no error
        write(&path, &[]).unwrap();
        assert_eq!(read(&path), (Vec::new(), Store::Ready));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_a_store_loads_nothing() {
        let dir = temporary("hostile");

        let long_line = vec![b'a'; 1 << 20];
        let oversized = [encode(&[text("a")]), vec![b'x'; FILE_BYTES]].concat();

        let cases: [(&str, &[u8]); 9] = [
            ("empty", b""),
            ("garbage", b"\x00\xff\xfe not a store"),
            ("no newline", b"kanade-clipboard-pins 1"),
            ("long first line", &long_line),
            ("wrong magic", b"kanade-clipboard 1\ntext 1\na\n"),
            ("no version", b"kanade-clipboard-pins\ntext 1\na\n"),
            ("bad version", b"kanade-clipboard-pins x\ntext 1\na\n"),
            ("oversized", &oversized),
            ("dangling", b"kanade-clipboard-pins 1\n/"),
        ];

        for (name, bytes) in cases {
            assert!(decoded(bytes).is_empty(), "{name}");

            let path = dir.join(name.replace(' ', "-"));
            fs::write(&path, bytes).unwrap();
            assert_eq!(
                read(&path),
                (Vec::new(), Store::Damaged),
                "{name} from disk"
            );
        }

        // a directory is no store, and not ours to touch; a missing file is simply none
        assert_eq!(read(&dir), (Vec::new(), Store::Held));
        assert_eq!(read(&dir.join("missing")), (Vec::new(), Store::Ready));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_later_version_is_told_apart_and_left() {
        assert_eq!(
            frames(b"kanade-clipboard-pins 2\ntext 1\na\n").err(),
            Some(Refused::Newer)
        );
        assert!(frames(b"kanade-clipboard-pins 1\n").is_ok());
    }

    #[test]
    fn a_truncated_or_lying_entry_keeps_what_came_before_it() {
        let whole = encode(&[text("one"), text("two"), text("three")]);

        // cut anywhere: the entries complete before the cut are the ones kept
        for cut in 0..whole.len() {
            let kept = decoded(&whole[..cut]);

            assert!(kept.len() <= 3);
            assert!(
                ["one", "two", "three"]
                    .iter()
                    .zip(&kept)
                    .all(|(want, got)| *got == text(want)),
                "cut at {cut}"
            );
        }

        assert_eq!(decoded(&whole).len(), 3);

        for (bytes, kept) in [
            // a length longer than what is left, and one past the entry limit
            (&b"kanade-clipboard-pins 1\ntext 1\na\ntext 99\nb\n"[..], 1),
            (
                b"kanade-clipboard-pins 1\ntext 1\na\ntext 99999999999\nb\n",
                1,
            ),
            (b"kanade-clipboard-pins 1\ntext 1\na\ntext 16777217\nb\n", 1),
            // a length shorter than the body: the byte after it is not the newline
            (b"kanade-clipboard-pins 1\ntext 1\na\ntext 1\nbc\n", 1),
            (b"kanade-clipboard-pins 1\ntext -1\na\n", 0),
            (b"kanade-clipboard-pins 1\ntext\na\n", 0),
            (b"kanade-clipboard-pins 1\ntext 1 2\na\n", 0),
            (b"kanade-clipboard-pins 1\nvideo 1\na\n", 0),
        ] {
            assert_eq!(
                decoded(bytes).len(),
                kept,
                "{:?}",
                String::from_utf8_lossy(bytes)
            );
        }
    }

    #[test]
    fn an_entry_that_is_not_valid_alone_is_skipped_and_the_rest_read() {
        let bytes = [
            b"kanade-clipboard-pins 1\n".as_slice(),
            b"text 2\n\xff\xfe\n",         // not UTF-8
            b"text 0\n\n",                 // nothing
            b"image text/plain 1\nx\n",    // not an image type
            b"image image/png\x07 1\nx\n", // a control character in the type
            b"image  1\nx\n",              // no type
            b"text 4\nkept\n",
        ]
        .concat();

        assert_eq!(decoded(&bytes), [text("kept")]);
    }

    // a store as large as one may be, of entries of one byte each: reading it makes at most the
    // pins the caps allow, never one String per entry
    #[test]
    fn a_hostile_count_of_tiny_entries_is_read_within_the_caps() {
        let mut bytes = b"kanade-clipboard-pins 1\n".to_vec();
        while bytes.len() + 9 < FILE_BYTES {
            bytes.extend_from_slice(b"text 1\nx\n");
        }

        let mut store = frames(&bytes).unwrap();
        let taken = within_caps(&mut store);

        assert_eq!(taken.len(), PINS);
        assert!(store.damaged());

        // and such a file is kept aside, not dropped on the next save
        let dir = temporary("hostile");
        let path = dir.join("clipboard-pins");

        fs::write(&path, &bytes).unwrap();

        let (pins, store) = read(&path);
        assert_eq!(pins.len(), PINS);
        assert_eq!(store, Store::Damaged);
    }

    // the largest store the caps allow is not refused as oversized
    #[test]
    fn the_largest_store_the_caps_allow_is_read_whole() {
        let mime = format!("image/{}", "x".repeat(MIME_BYTES - "image/".len()));
        let mut all = vec![image("image/png", &vec![7; PIN_BYTES - 3 * (PINS - 1)])];

        all.extend((1..PINS).map(|n| image(&mime, &[n as u8; 3])));

        assert!(encode(&all).len() <= FILE_BYTES);
        assert_eq!(decoded(&encode(&all)), all);
    }

    fn bak(path: &Path) -> PathBuf {
        PathBuf::from(format!("{}.bak", path.display()))
    }

    // what read gives back for `bytes` on disk, then one save of `pins` as the run would do it
    fn read_then_save(path: &Path, bytes: &[u8], pins: &[Content]) -> Store {
        fs::write(path, bytes).unwrap();

        let (_, mut store) = read(path);
        let before = store;
        save(path, pins, &mut store).unwrap();

        before
    }

    #[test]
    fn a_store_that_could_not_be_read_or_is_newer_is_never_written_over() {
        let dir = temporary("held");
        let path = dir.join("clipboard-pins");
        let pins = [text("new")];

        let newer = b"kanade-clipboard-pins 2\ntext 1\na\n";
        assert_eq!(read_then_save(&path, newer, &pins), Store::Held);
        assert_eq!(fs::read(&path).unwrap(), newer);

        // a transient failure to read (here no permission) must not read as an empty store; skipped
        // for a user that can read anything
        let good = encode(&[text("kept")]);
        fs::write(&path, &good).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();

        if fs::File::open(&path).is_err() {
            let (loaded, mut store) = read(&path);
            assert_eq!((loaded.len(), store), (0, Store::Held));

            save(&path, &pins, &mut store).unwrap();
            // and an empty set deletes nothing either
            save(&path, &[], &mut store).unwrap();

            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            assert_eq!(fs::read(&path).unwrap(), good);
        }

        assert!(!bak(&path).exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_store_is_kept_as_a_backup_before_the_first_write() {
        let dir = temporary("damaged");
        let path = dir.join("clipboard-pins");
        let pins = [text("new")];

        let whole = encode(&[text("one"), text("two")]);
        let cut = &whole[..whole.len() - 3];

        for (name, bytes) in [
            ("garbage", &b"\x00\xff not a store"[..]),
            ("truncated", cut),
            (
                "skipped entry",
                b"kanade-clipboard-pins 1\ntext 2\n\xff\xfe\ntext 1\na\n",
            ),
        ] {
            let _ = fs::remove_file(bak(&path));

            assert_eq!(
                read_then_save(&path, bytes, &pins),
                Store::Damaged,
                "{name}"
            );
            assert_eq!(fs::read(bak(&path)).unwrap(), bytes, "{name}");
            assert_eq!(
                fs::metadata(bak(&path)).unwrap().permissions().mode() & 0o777,
                0o600,
                "{name}"
            );
            assert_eq!(read(&path), (pins.to_vec(), Store::Ready), "{name}");
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_store_is_created_by_the_first_pin_without_a_backup() {
        let dir = temporary("absent");
        let path = dir.join("state/kanade/clipboard-pins");

        let (loaded, mut store) = read(&path);
        assert_eq!((loaded.len(), store), (0, Store::Ready));

        save(&path, &[text("first")], &mut store).unwrap();
        assert_eq!(read(&path), (vec![text("first")], Store::Ready));
        assert!(!bak(&path).exists());

        let _ = fs::remove_dir_all(&dir);
    }
}
