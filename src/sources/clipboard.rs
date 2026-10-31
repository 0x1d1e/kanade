//! The clipboard (ADR 0011): a history of what was copied, text and images, newest first,
//! bounded and kept only in memory. Selection announcements arrive from the runtime's data-control
//! protocol (ADR 0028). Restoring an entry hands it to `wl-copy --foreground`, a holder that serves
//! it until another program takes the selection; restoring another kills it. The new selection is
//! announced like any other, so a restored entry moves to the top, and that announcement is what says
//! the restore is done: a paste then gets it. Removing an entry or clearing the history forgets only
//! the history, never what is on the clipboard now. What was copied is never logged, and an entry's
//! `Debug` leaves it out.
//! A selection its owner marks sensitive, like a password manager's (`x-kde-passwordManagerHint`),
//! is never read (ADR 0019): offers with that MIME type are rejected before reading.
//! An entry may be pinned (ADR 0114): it sorts first, the bounds never evict it and Clear all keeps
//! it. Pins, and only pins, are kept in a private file across restarts (ADR 0115, `pins`); the rest
//! of the history stays in memory. The pictures' previews are files on tmpfs only, `preview`.

use std::fmt;
use std::io::{self, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::{MutexExt, service::Service};

use crate::sources::wake;
use crate::supervise;

pub mod pins;
pub mod preview;

pub const COPY: &str = "wl-copy";

// the most entries kept, the most bytes one may have, and the most all of them together may
pub const ENTRIES: usize = 50;
pub const ENTRY_BYTES: usize = 16 << 20;
pub const TOTAL_BYTES: usize = 64 << 20;

// the most entries pinned, and the most bytes all of them together may have (ADR 0114)
pub const PINS: usize = 20;
pub const PIN_BYTES: usize = 16 << 20;

// whether `mime` is a type a frame can carry: within MIME_BYTES, no space or control character
pub(crate) fn framed(mime: &str) -> bool {
    (1..=MIME_BYTES).contains(&mime.len())
        && !mime.contains(|c: char| c.is_whitespace() || c.is_control())
}

// what text is restored as; wl-copy offers the other text types with it
const TEXT: &str = "text/plain;charset=utf-8";

// bytes an image has at an offset
type Signature = (usize, &'static [u8]);

// the longest type kept
pub(crate) const MIME_BYTES: usize = 255;

/*
 * the images known by how they start, as (type, [(offset, signature)]), for a wl-paste that does
 * not say the type it handed over (2.3.0 and older)
 */
const IMAGES: [(&str, &[Signature]); 5] = [
    ("image/png", &[(0, b"\x89PNG\r\n\x1a\n")]),
    ("image/jpeg", &[(0, b"\xff\xd8\xff")]),
    ("image/gif", &[(0, b"GIF87a")]),
    ("image/gif", &[(0, b"GIF89a")]),
    ("image/webp", &[(0, b"RIFF"), (8, b"WEBP")]),
];

// what one entry holds; shared, so a read of the history copies none of it
#[derive(Clone, PartialEq, Eq)]
pub enum Content {
    Text(Arc<str>),
    Image { mime: String, bytes: Arc<[u8]> },
}

impl Content {
    /*
     * from what a selection held, in `mime` when wl-paste says it: an image as any `image/` type, text
     * as a text type wl-paste reads in, if UTF-8. None for the rest. Without the type, UTF-8 text,
     * as wl-paste reads text first, else an image Kanade knows by its start
     */
    fn new(mime: Option<&str>, bytes: Vec<u8>) -> Option<Content> {
        let image = |mime: &str| Content::Image {
            mime: String::from(mime),
            bytes: bytes.clone().into(),
        };
        let text = || {
            String::from_utf8(bytes.clone())
                .ok()
                .map(|text| Content::Text(text.into()))
        };

        match mime {
            Some(mime) if mime.starts_with("image/") => Some(image(mime)),
            Some(mime) if textual(mime) => text(),
            Some(_) => None,
            None => text().or_else(|| {
                let (mime, _) = IMAGES.iter().find(|(_, signatures)| {
                    signatures.iter().all(|(at, signature)| {
                        bytes
                            .get(*at..*at + signature.len())
                            .is_some_and(|start| start == *signature)
                    })
                })?;

                Some(image(mime))
            }),
        }
    }

    fn len(&self) -> usize {
        match self {
            Content::Text(text) => text.len(),
            Content::Image { bytes, .. } => bytes.len(),
        }
    }

    // the type it is restored as
    fn mime(&self) -> &str {
        match self {
            Content::Text(_) => TEXT,
            Content::Image { mime, .. } => mime,
        }
    }

    fn bytes(&self) -> &[u8] {
        match self {
            Content::Text(text) => text.as_bytes(),
            Content::Image { bytes, .. } => bytes,
        }
    }
}

// only the kind and size: what was copied never prints
impl fmt::Debug for Content {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Content::Text(text) => write!(formatter, "Text({} bytes)", text.len()),
            Content::Image { mime, bytes } => {
                write!(formatter, "Image({mime}, {} bytes)", bytes.len())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    // never reused, so an old row cannot reach what replaced it; kept when copied again
    pub id: u64,

    pub content: Content,

    // kept first and out of the bounds' reach; it marks an entry that passed ADR 0019 already
    pub pinned: bool,
}

// pinning refused: the pins are at their cap
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinsFull;

// what pinning an entry would do
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pinning {
    Changed,
    Unchanged,
    Full,
}

// the history: the pinned entries first, then the rest, each newest first
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Clipboard {
    entries: Vec<Entry>,
    next: u64,
}

impl Service for Clipboard {
    fn new() -> Self {
        Clipboard::default()
    }

    fn listen() {}
}

impl Clipboard {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    // for `kanade status`, without what was copied
    pub fn status(&self) -> String {
        let images = self
            .entries
            .iter()
            .filter(|entry| matches!(entry.content, Content::Image { .. }))
            .count();

        let pinned = match self.pins() {
            0 => String::new(),
            pins => format!(", {pins} pinned"),
        };

        format!(
            "clipboard: {} entries, {} text, {images} images{pinned}",
            self.entries.len(),
            self.entries.len() - images
        )
    }

    // how many entries are pinned, which are the first of `entries`
    pub fn pins(&self) -> usize {
        self.entries.iter().filter(|entry| entry.pinned).count()
    }

    // whether the entry `id` is kept
    pub fn has(&self, id: u64) -> bool {
        self.entries.iter().any(|entry| entry.id == id)
    }

    // where a copy of an entry goes: the top of the pinned entries, or of the rest
    fn top(&self, pinned: bool) -> usize {
        if pinned { 0 } else { self.pins() }
    }

    // whether `record` would change the history: an empty or oversized copy, or one of the entry
    // already first of its kind, would not
    fn takes(&self, content: &Content) -> bool {
        let len = content.len();

        (1..=ENTRY_BYTES).contains(&len)
            && self
                .entries
                .iter()
                .position(|entry| entry.content == *content)
                .is_none_or(|at| at != self.top(self.entries[at].pinned))
    }

    // puts what was copied first of its kind, moving the entry it equals there with its id and pin,
    // then drops the oldest unpinned past the bounds; says whether it changed anything
    fn record(&mut self, content: Content) -> bool {
        if !self.takes(&content) {
            return false;
        }

        let entry = match self
            .entries
            .iter()
            .position(|entry| entry.content == content)
        {
            Some(at) => self.entries.remove(at),
            None => {
                self.next += 1;
                Entry {
                    id: self.next,
                    content,
                    pinned: false,
                }
            }
        };

        self.entries.insert(self.top(entry.pinned), entry);

        // the newest alone is within both, with the pins: `takes` let in nothing larger than
        // ENTRY_BYTES, and the pins hold at most PIN_BYTES
        while self.entries.len() > ENTRIES || self.bytes() > TOTAL_BYTES {
            match self.entries.iter().rposition(|entry| !entry.pinned) {
                Some(oldest) => self.entries.remove(oldest),
                None => break,
            };
        }

        true
    }

    fn bytes(&self) -> usize {
        self.entries.iter().map(|entry| entry.content.len()).sum()
    }

    // what pinning the entry `id` would do: nothing to one gone or pinned, and refused past the pins' caps
    fn pinning(&self, id: u64) -> Pinning {
        let Some(entry) = self.entries.iter().find(|entry| entry.id == id) else {
            return Pinning::Unchanged;
        };

        let pinned = self.entries.iter().filter(|entry| entry.pinned);
        let bytes: usize = pinned.clone().map(|entry| entry.content.len()).sum();

        if entry.pinned {
            Pinning::Unchanged
        } else if pinned.count() >= PINS || bytes + entry.content.len() > PIN_BYTES {
            Pinning::Full
        } else {
            Pinning::Changed
        }
    }

    // pins the entry `id` first of the pins
    fn pin(&mut self, id: u64) -> Pinning {
        let pinning = self.pinning(id);

        if pinning == Pinning::Changed
            && let Some(at) = self.entries.iter().position(|entry| entry.id == id)
        {
            let mut entry = self.entries.remove(at);
            entry.pinned = true;
            self.entries.insert(0, entry);
        }

        pinning
    }

    // unpins the entry `id` to the top of the rest; says whether it was pinned
    fn unpin(&mut self, id: u64) -> bool {
        let Some(at) = self
            .entries
            .iter()
            .position(|entry| entry.id == id && entry.pinned)
        else {
            return false;
        };

        let mut entry = self.entries.remove(at);
        entry.pinned = false;
        self.entries.insert(self.pins(), entry);

        true
    }

    // the ids of the pinned entries in order, which is what a store of the pins is a function of
    fn pin_ids(&self) -> Vec<u64> {
        self.entries
            .iter()
            .filter(|entry| entry.pinned)
            .map(|entry| entry.id)
            .collect()
    }

    // adds the stored pins after the ones kept, in their order, each under a fresh id; one whose
    // content a kept entry has, or past the pins' caps, is left out
    fn adopt(&mut self, contents: Vec<Content>) {
        for content in contents {
            let bytes: usize = self
                .entries
                .iter()
                .filter(|entry| entry.pinned)
                .map(|entry| entry.content.len())
                .sum();

            if !(1..=ENTRY_BYTES).contains(&content.len())
                || self.pins() >= PINS
                || bytes + content.len() > PIN_BYTES
                || self.entries.iter().any(|entry| entry.content == content)
            {
                continue;
            }

            self.next += 1;
            let at = self.pins();

            self.entries.insert(
                at,
                Entry {
                    id: self.next,
                    content,
                    pinned: true,
                },
            );
        }
    }

    // forgets the entry `id`, pinned or not, if it is still kept; says whether it was
    fn remove(&mut self, id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.id != id);

        self.entries.len() != before
    }

    // forgets every entry that is not pinned; `next` stays, so no id is ever reused
    fn clear(&mut self) -> bool {
        let before = self.entries.len();
        self.entries.retain(|entry| entry.pinned);

        self.entries.len() != before
    }
}

/*
 * forgets an entry, pinned or not, and `clear` all of them but the pinned; only the history, so
 * what is on the clipboard now stays there, and its wl-copy holder with it. Each writes only a
 * change, and drops the previews of what is gone
 */
pub fn remove(id: u64) {
    if Clipboard::read().has(id) {
        editing(|clipboard| clipboard.remove(id));
        preview::reconcile();
    }
}

pub fn clear() {
    if Clipboard::read().entries.iter().any(|entry| !entry.pinned) {
        Clipboard::write().clear();
        preview::reconcile();
    }
}

// how many entries are pinned now
pub fn pinned() -> usize {
    Clipboard::read().pins()
}

// pins the entry `id`, which already passed ADR 0019 to be kept; refused past the pins' caps. Writes
// only a change
pub fn pin(id: u64) -> Result<(), PinsFull> {
    let pinning = Clipboard::read().pinning(id);

    match pinning {
        Pinning::Unchanged => Ok(()),
        Pinning::Full => Err(PinsFull),
        Pinning::Changed => match editing(|clipboard| clipboard.pin(id)) {
            Pinning::Full => Err(PinsFull),
            _ => Ok(()),
        },
    }
}

// unpins the entry `id`, which stays in the history; writes only a change
pub fn unpin(id: u64) {
    let pinned = Clipboard::read()
        .entries
        .iter()
        .any(|entry| entry.id == id && entry.pinned);

    if pinned {
        editing(|clipboard| clipboard.unpin(id));
    }
}

/*
 * changes the history, and saves the pins (ADR 0115) when that changed which are pinned or their
 * order: a pin, an unpin, a pinned entry removed, or one copied again, which moves it first. The
 * write is let go of before the save is asked for, and the save runs off this thread
 */
fn editing<R>(change: impl FnOnce(&mut Clipboard) -> R) -> R {
    let mut clipboard = Clipboard::write();
    let before = clipboard.pin_ids();
    let changed = change(&mut clipboard);
    let moved = clipboard.pin_ids() != before;

    drop(clipboard);

    if moved {
        pins::save_soon();
    }

    changed
}

// asks the holder thread to restore
static RESTORES: OnceLock<Sender<Restore>> = OnceLock::new();

// what to restore, and who hears whether it reached the clipboard
struct Restore {
    content: Content,
    done: Box<dyn FnOnce(bool) + Send>,
}

// how long a restore may take, from feeding wl-copy to hearing its selection announced, looking
// this often whether its holder ended first
const ANNOUNCED: Duration = Duration::from_secs(3);
const LOOK: Duration = Duration::from_millis(50);

// how long the Launcher's wl-copy may take to get the selection, as a restore may, looking this
// often whether it did; it ends once it has, in a few milliseconds
const PUT: Duration = ANNOUNCED;
const PUT_LOOK: Duration = Duration::from_millis(5);

// the content a restore waits to hear announced, and where to say it was
static AWAITED: Mutex<Option<(Content, Sender<()>)>> = Mutex::new(None);

fn awaited() -> MutexGuard<'static, Option<(Content, Sender<()>)>> {
    AWAITED.locked()
}

// the wl-copy serving the last restored entry, until another took the selection and it was reaped
static HELD: Mutex<Option<Child>> = Mutex::new(None);

fn held() -> MutexGuard<'static, Option<Child>> {
    HELD.locked()
}

// starts its threads, each for good; restoring entries uses wl-copy
pub fn follow() {
    let (restores, heard) = mpsc::channel();

    if RESTORES.set(restores).is_err() {
        return;
    }

    // the previews of a Kanade that ended without cleaning up
    preview::sweep();

    // the stored pins, before the watcher records anything
    pins::load();

    // wl-copy dies with the thread that started it, so this one lives as long as Kanade
    supervise::spawn("clipboard-copy", move || copy(&heard));
    kanade_runtime::on_clipboard(record_selection);
}

/*
 * puts an entry back on the clipboard, off the caller's thread. `done` hears, on that thread,
 * whether it is there, once the new selection was announced or it failed; a failure is logged
 */
pub fn restore(entry: &Entry, done: impl FnOnce(bool) + Send + 'static) {
    let restore = Restore {
        content: entry.content.clone(),
        done: Box::new(done),
    };

    let unsent = match RESTORES.get() {
        Some(restores) => restores.send(restore).err().map(|unsent| unsent.0),
        None => Some(restore),
    };

    if let Some(restore) = unsent {
        (restore.done)(false);
    }
}

/*
 * puts `text` on the clipboard apart from the history, for the Launcher: wl-copy serves it from a
 * process of its own, which outlives Kanade, until another program takes the selection. Blocks
 * until wl-copy has it, at most `PUT`, so call it off the view's thread; needs no `clipboard`
 * Module
 */
pub fn put(text: &str) -> io::Result<()> {
    // that process keeps what it inherits open, so it gets nothing to hold
    let child = Command::new(COPY)
        .args(["--type", TEXT, "--", text])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => io::Error::new(error.kind(), "wl-copy not found"),
            _ => error,
        })?;

    let status = ended(child, Instant::now() + PUT)?;

    match status.success() {
        true => Ok(()),
        false => Err(io::Error::other(format!("wl-copy failed ({status})"))),
    }
}

/*
 * as `put`, but `text` goes to wl-copy on its stdin rather than its arguments, which any process
 * of the user's can read while it runs: for text that is private, like a dictation. `sensitive`
 * marks it so a clipboard history leaves it out
 */
pub fn put_private(text: &str, sensitive: bool) -> io::Result<()> {
    // an older wl-copy does not know `--sensitive` and fails; the text is still wanted, so it goes
    // without the mark, as the copy of a dictation that could not be inserted
    if sensitive && put_with(text, &["--type", TEXT, "--sensitive"]).is_ok() {
        return Ok(());
    }

    put_with(text, &["--type", TEXT])
}

fn put_with(text: &str, args: &[&str]) -> io::Result<()> {
    let mut child = Command::new(COPY)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => io::Error::new(error.kind(), "wl-copy not found"),
            _ => error,
        })?;

    // closed once written, which is how wl-copy knows the text is all there
    let written = child
        .stdin
        .take()
        .map_or(Ok(()), |mut stdin| stdin.write_all(text.as_bytes()));

    if let Err(error) = written {
        drop(child.kill());
        drop(child.wait());

        return Err(error);
    }

    let status = ended(child, Instant::now() + PUT)?;

    match status.success() {
        true => Ok(()),
        false => Err(io::Error::other(format!("wl-copy failed ({status})"))),
    }
}

/*
 * how `child` ended, looking every `PUT_LOOK`; one still running at `deadline`, like a wl-copy
 * that never got the selection, is killed and reaped
 */
fn ended(mut child: Child, deadline: Instant) -> io::Result<ExitStatus> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }

        if Instant::now() >= deadline {
            drop(child.kill());
            drop(child.wait());

            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "wl-copy did not end",
            ));
        }

        thread::sleep(PUT_LOOK);
    }
}

fn copy(heard: &Receiver<Restore>) {
    // never ends: `RESTORES` keeps the sender
    for Restore { content, done } in heard {
        let restored = restored(&content);

        if let Err(error) = &restored {
            eprintln!("kanade: cannot restore a clipboard entry ({error})");
        }

        done(restored.is_ok());
    }
}

/*
 * holds `content`, then waits for runtime data-control to announce it as the selection; a holder that
 * ended before, refused or replaced at once, never will
 */
fn restored(content: &Content) -> io::Result<()> {
    let (announce, announced) = mpsc::channel();
    *awaited() = Some((content.clone(), announce));

    let deadline = Instant::now() + ANNOUNCED;

    let restored = hold(content, deadline).and_then(|()| {
        loop {
            if announced.recv_timeout(LOOK).is_ok() {
                return Ok(());
            }

            if !holding() {
                return Err(io::Error::other("wl-copy ended first"));
            }

            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the new selection was not announced",
                ));
            }
        }
    });

    *awaited() = None;
    restored
}

// whether the last holder still runs
fn holding() -> bool {
    held()
        .as_mut()
        .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
}

// a selection was announced; a restore waiting for it is done
fn announce(content: &Content) {
    let mut awaited = awaited();

    if awaited.as_ref().is_some_and(|(want, _)| want == content)
        && let Some((_, announce)) = awaited.take()
    {
        // the restore gave up waiting
        let _ = announce.send(());
    }
}

// starts a wl-copy serving `content`, fed by `deadline`, then ends the one serving the last
fn hold(content: &Content, deadline: Instant) -> io::Result<()> {
    let mut child = wake::hold(COPY, &["--foreground", "--type", content.mime()])?;

    // wl-copy reads all of it before it takes the selection
    let written = match child.stdin.take() {
        Some(stdin) => feed(stdin, content.clone(), deadline),
        None => Err(io::Error::other("no input")),
    };

    if let Err(error) = written {
        drop(child.kill());

        return Err(match child.wait()?.code() {
            Some(wake::NOT_FOUND) => io::Error::new(io::ErrorKind::NotFound, "wl-copy not found"),
            _ => error,
        });
    }

    if let Some(mut old) = held().replace(child) {
        drop(old.kill());
        drop(old.wait());
    }

    Ok(())
}

/*
 * writes `content` to a holder off this thread, so one that stops reading cannot outlast
 * `deadline`; killing it then makes the write fail, which ends the writer
 */
fn feed(mut stdin: ChildStdin, content: Content, deadline: Instant) -> io::Result<()> {
    let (fed, written) = mpsc::channel();

    thread::Builder::new()
        .name("clipboard-feed".into())
        .spawn(move || {
            // dropping `stdin` after ends the input; the restore gave up waiting
            let _ = fed.send(stdin.write_all(content.bytes()));
        })?;

    written
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .unwrap_or_else(|_| {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "wl-copy did not read the entry",
            ))
        })
}

// forgets the holder once it exited on its own, another program having taken the selection
fn reap() {
    let mut held = held();

    if let Some(child) = held.as_mut()
        && !matches!(child.try_wait(), Ok(None))
    {
        *held = None;
    }
}

// whether wl-paste takes `mime` for text, as its `mime_type_is_text` does
fn textual(mime: &str) -> bool {
    mime.starts_with("text/")
        || matches!(mime, "TEXT" | "STRING" | "UTF8_STRING")
        || mime.contains("json")
        || ["script", "xml", "yaml", "csv", "ini", "pgp-keys"]
            .iter()
            .any(|suffix| mime.ends_with(suffix))
        || mime.contains("application/vnd.ms-publisher")
}

// records a selection received from the runtime's data-control protocol
pub fn record_selection(mime: Option<String>, bytes: Vec<u8>) {
    reap();

    if let Some(content) = Content::new(mime.as_deref(), bytes) {
        record(content);
    }
}

// writes only a change, since a write wakes every window; a restore of the newest changes nothing
// but is announced all the same
fn record(content: Content) {
    announce(&content);

    if Clipboard::read().takes(&content) {
        editing(|clipboard| clipboard.record(content));

        // the oldest may have gone past the bounds
        preview::reconcile();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(text: &str) -> Content {
        Content::Text(text.into())
    }

    #[test]
    fn a_put_that_never_ends_is_killed_at_the_deadline() {
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let start = Instant::now();

        let error = ended(child, start + Duration::from_millis(100)).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "reaped"
        );
    }

    fn image(len: usize, fill: u8) -> Content {
        Content::Image {
            mime: String::from("image/png"),
            bytes: vec![fill; len].into(),
        }
    }

    #[test]
    fn the_history_keeps_within_its_bytes() {
        let mut clipboard = Clipboard::default();
        let large = ENTRY_BYTES;

        for fill in 0..6 {
            clipboard.record(image(large, fill));
        }

        assert!(clipboard.bytes() <= TOTAL_BYTES);
        assert_eq!(clipboard.entries.len(), TOTAL_BYTES / large);
        assert_eq!(clipboard.entries[0].content, image(large, 5));
    }

    #[test]
    fn an_empty_or_oversized_copy_is_not_kept() {
        let mut clipboard = Clipboard::default();

        assert!(!clipboard.record(text("")));
        assert!(!clipboard.record(image(ENTRY_BYTES + 1, 0)));
        assert!(clipboard.entries.is_empty());
    }

    #[test]
    fn what_was_copied_never_prints() {
        let mut clipboard = Clipboard::default();
        clipboard.record(text("hunter2"));
        clipboard.record(image(4, b'z'));

        let printed = format!("{clipboard:?} {}", clipboard.status());

        assert!(!printed.contains("hunter2"));
        assert!(!printed.contains("zzzz"));
        assert!(!printed.contains("122"));
        assert_eq!(clipboard.status(), "clipboard: 2 entries, 1 text, 1 images");
    }

    fn pinned(clipboard: &Clipboard) -> Vec<u64> {
        clipboard
            .entries
            .iter()
            .filter(|entry| entry.pinned)
            .map(|entry| entry.id)
            .collect()
    }

    // the bounds drop the oldest that is not pinned, never a pin, and pins stay first
    #[test]
    fn a_pinned_entry_survives_the_bounds() {
        let mut clipboard = Clipboard::default();

        clipboard.record(text("keep"));
        assert_eq!(clipboard.pin(1), Pinning::Changed);

        for number in 0..ENTRIES + 5 {
            clipboard.record(text(&format!("copy {number}")));
        }

        assert_eq!(clipboard.entries.len(), ENTRIES);
        assert_eq!(clipboard.entries[0].content, text("keep"));
        assert_eq!(pinned(&clipboard), [1]);

        // one pin of the most a pin may hold, then bytes past the total: only the rest goes
        let mut clipboard = Clipboard::default();
        clipboard.record(image(PIN_BYTES, 1));
        clipboard.pin(1);

        for fill in 2..10 {
            clipboard.record(image(ENTRY_BYTES, fill));
        }

        assert!(clipboard.bytes() <= TOTAL_BYTES);
        assert_eq!(clipboard.entries[0].content, image(PIN_BYTES, 1));
        assert_eq!(clipboard.entries[1].content, image(ENTRY_BYTES, 9));
        assert_eq!(pinned(&clipboard), [1]);
    }

    #[test]
    fn the_pins_have_their_own_caps() {
        let mut clipboard = Clipboard::default();

        for number in 0..=PINS {
            clipboard.record(text(&format!("pin {number}")));
        }

        let ids: Vec<u64> = clipboard.entries.iter().map(|entry| entry.id).collect();

        for id in &ids[..PINS] {
            assert_eq!(clipboard.pin(*id), Pinning::Changed);
        }

        let before = clipboard.clone();

        assert_eq!(clipboard.pin(ids[PINS]), Pinning::Full);
        assert_eq!(clipboard, before, "a refused pin changes nothing");
        assert_eq!(clipboard.pin(ids[0]), Pinning::Unchanged);
        assert_eq!(clipboard.pin(9999), Pinning::Unchanged);

        // an unpin makes room
        assert!(clipboard.unpin(ids[0]));
        assert!(!clipboard.unpin(ids[0]));
        assert_eq!(clipboard.pin(ids[PINS]), Pinning::Changed);

        // the bytes cap holds before the count does
        let mut clipboard = Clipboard::default();
        clipboard.record(image(PIN_BYTES, 1));
        clipboard.record(text("a"));

        assert_eq!(clipboard.pin(1), Pinning::Changed);
        assert_eq!(clipboard.pin(2), Pinning::Full);
    }

    #[test]
    fn a_copy_of_a_pinned_entry_keeps_its_pin_and_id_and_moves_it() {
        let mut clipboard = Clipboard::default();

        for word in ["a", "b", "c"] {
            clipboard.record(text(word));
        }

        clipboard.pin(1);
        clipboard.pin(2);

        // pins first, the last pinned first of them; "c" is the first of the rest
        let order = |clipboard: &Clipboard| -> Vec<(u64, bool)> {
            clipboard
                .entries
                .iter()
                .map(|entry| (entry.id, entry.pinned))
                .collect()
        };

        assert_eq!(order(&clipboard), [(2, true), (1, true), (3, false)]);

        // a copy of the first pinned changes nothing; of the other pin moves it first
        assert!(!clipboard.record(text("b")));
        assert!(clipboard.record(text("a")));
        assert_eq!(order(&clipboard), [(1, true), (2, true), (3, false)]);

        // the unpinned never takes the place of a pin
        assert!(clipboard.record(text("d")));
        assert_eq!(
            order(&clipboard),
            [(1, true), (2, true), (4, false), (3, false)]
        );
        assert!(clipboard.record(text("c")));
        assert_eq!(
            order(&clipboard),
            [(1, true), (2, true), (3, false), (4, false)]
        );
        assert!(!clipboard.record(text("c")));
    }

    #[test]
    fn clear_keeps_the_pins_and_remove_takes_one_and_no_id_is_reused() {
        let mut clipboard = Clipboard::default();

        for word in ["a", "b", "c"] {
            clipboard.record(text(word));
        }

        clipboard.pin(2);

        assert!(clipboard.clear());
        assert_eq!(pinned(&clipboard), [2]);
        assert_eq!(clipboard.entries.len(), 1);
        assert!(!clipboard.clear(), "nothing left to clear");

        assert!(clipboard.remove(2), "remove takes a pin too");
        assert!(!clipboard.remove(2));
        assert!(clipboard.entries.is_empty());

        clipboard.record(text("a"));
        assert_eq!(clipboard.entries[0].id, 4, "ids go on, never back");
        assert!(!clipboard.entries[0].pinned);
    }

    #[test]
    fn a_pin_never_prints_what_was_copied() {
        let mut clipboard = Clipboard::default();
        clipboard.record(text("hunter2"));
        clipboard.pin(1);

        let printed = format!("{clipboard:?} {}", clipboard.status());

        assert!(!printed.contains("hunter2"));
        assert_eq!(
            clipboard.status(),
            "clipboard: 1 entries, 1 text, 0 images, 1 pinned"
        );
    }

    #[test]
    fn stored_pins_load_in_order_under_fresh_ids_within_the_caps() {
        let mut clipboard = Clipboard::default();
        clipboard.adopt(vec![text("a"), image(3, 1), text("a"), text("b")]);

        // the duplicate is left out; order kept; ids from 1
        let loaded: Vec<(u64, Content, bool)> = clipboard
            .entries
            .iter()
            .map(|entry| (entry.id, entry.content.clone(), entry.pinned))
            .collect();
        assert_eq!(
            loaded,
            [
                (1, text("a"), true),
                (2, image(3, 1), true),
                (3, text("b"), true)
            ]
        );

        // a copy of a loaded pin is that entry, not another; a new copy gets an id after them
        assert!(clipboard.record(text("b")));
        assert_eq!(clipboard.entries.len(), 3);
        assert_eq!(clipboard.pin_ids(), [3, 1, 2]);
        assert!(clipboard.record(text("new")));
        assert_eq!(clipboard.entries[3].id, 4);
        assert!(!clipboard.entries[3].pinned);

        // the pins' own caps hold on load, and nothing empty or oversized comes in
        let mut clipboard = Clipboard::default();
        let many: Vec<Content> = (0..PINS + 5).map(|n| text(&format!("pin {n}"))).collect();
        clipboard.adopt(many);
        assert_eq!(clipboard.pins(), PINS);
        assert_eq!(clipboard.entries[0].content, text("pin 0"));

        let mut clipboard = Clipboard::default();
        clipboard.adopt(vec![
            text(""),
            image(ENTRY_BYTES + 1, 0),
            image(PIN_BYTES, 1),
            text("over the bytes"),
            image(PIN_BYTES, 2),
        ]);
        assert_eq!(clipboard.entries.len(), 1);
        assert_eq!(clipboard.entries[0].content, image(PIN_BYTES, 1));
    }

    // what saves the pins: which are pinned and their order, nothing else
    #[test]
    fn the_pins_change_only_by_a_pin_an_unpin_a_removal_or_a_recopy() {
        let mut clipboard = Clipboard::default();

        for word in ["a", "b", "c"] {
            clipboard.record(text(word));
        }

        clipboard.pin(1);
        clipboard.pin(2);
        assert_eq!(clipboard.pin_ids(), [2, 1]);

        let before = clipboard.pin_ids();
        clipboard.record(text("d"));
        clipboard.clear();
        assert_eq!(clipboard.pin_ids(), before, "a new copy or a clear");

        clipboard.record(text("a"));
        assert_eq!(
            clipboard.pin_ids(),
            [1, 2],
            "a pin copied again moves first"
        );

        clipboard.unpin(1);
        assert_eq!(clipboard.pin_ids(), [2]);
        clipboard.remove(2);
        assert!(clipboard.pin_ids().is_empty());
    }

    #[test]
    fn content_from_mime_and_bytes() {
        assert_eq!(
            Content::new(Some("text/plain"), b"hello".to_vec()),
            Some(text("hello"))
        );
        assert_eq!(
            Content::new(Some("text/plain;charset=utf-8"), b"world".to_vec()),
            Some(text("world"))
        );
        let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[0, 0xff, b'\n', 0x80]].concat();
        assert_eq!(
            Content::new(Some("image/png"), png.clone()),
            Some(Content::Image {
                mime: "image/png".into(),
                bytes: png.into(),
            })
        );
        // Magic bytes detection when mime is None
        let jpeg = b"\xff\xd8\xff5678".to_vec();
        assert_eq!(
            Content::new(None, jpeg.clone()),
            Some(Content::Image {
                mime: "image/jpeg".into(),
                bytes: jpeg.into(),
            })
        );
        // Unsupported mime
        assert_eq!(
            Content::new(Some("application/octet-stream"), b"raw".to_vec()),
            None
        );
    }

    #[test]
    fn record_selection_updates_clipboard() {
        Clipboard::write().clear();
        record_selection(Some("text/plain".into()), b"first".to_vec());
        assert_eq!(
            Clipboard::read().entries().first().map(|e| &e.content),
            Some(&text("first"))
        );

        record_selection(Some("text/plain".into()), b"second".to_vec());
        assert_eq!(
            Clipboard::read().entries().first().map(|e| &e.content),
            Some(&text("second"))
        );
    }
}
