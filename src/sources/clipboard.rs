//! The clipboard (#136, ADR 0011): a history of what was copied, text and images, newest first,
//! bounded and kept only in memory. `wl-paste --watch` announces each new selection, and Kanade
//! reads its types and then its content with `wl-paste`, an action each. Restoring an entry hands
//! it to `wl-copy --foreground`, a holder that serves it until another program takes the selection;
//! restoring another kills it. The new selection is announced like any other, so a restored entry
//! moves to the top. What was copied is never logged, and an entry's `Debug` leaves it out.

use std::fmt;
use std::io::{self, BufRead, Write};
use std::process::Child;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use amane::Service;

use crate::sources::wake;
use crate::supervise;

pub const PASTE: &str = "wl-paste";
pub const COPY: &str = "wl-copy";

// wl-paste runs the command after `--watch` at each new selection: it prints the selection's state,
// `data` when a wl-paste older than 2.2 says none
const WATCH: &[&str] = &["--watch", "sh", "-c", "echo \"${CLIPBOARD_STATE:-data}\""];

// the most entries kept, the most bytes one may have, and the most all of them together may
pub const ENTRIES: usize = 50;
pub const ENTRY_BYTES: usize = 16 << 20;
pub const TOTAL_BYTES: usize = 64 << 20;

// what text is read in, in order, and restored as; wl-copy offers the other text types with it
const TEXT: &str = "text/plain;charset=utf-8";
const TEXTS: [&str; 3] = [TEXT, "UTF8_STRING", "text/plain"];

const PNG: &str = "image/png";

// what one entry holds; shared, so a read of the history copies none of it
#[derive(Clone, PartialEq, Eq)]
pub enum Content {
    Text(Arc<str>),
    Image { mime: String, bytes: Arc<[u8]> },
}

impl Content {
    // from what wl-paste printed in `kind`; text that is not UTF-8 is none
    fn new(kind: Kind, bytes: Vec<u8>) -> Option<Content> {
        match kind {
            Kind::Text => String::from_utf8(bytes)
                .ok()
                .map(|text| Content::Text(text.into())),
            Kind::Image(mime) => Some(Content::Image {
                mime,
                bytes: bytes.into(),
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
}

// the history, newest first
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
    #[expect(dead_code, reason = "the clipboard Surface (#137) lists entries")]
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

        format!(
            "clipboard: {} entries, {} text, {images} images",
            self.entries.len(),
            self.entries.len() - images
        )
    }

    // whether `record` would change the history: an empty or oversized copy, or the newest again,
    // would not
    fn takes(&self, content: &Content) -> bool {
        let len = content.len();

        (1..=ENTRY_BYTES).contains(&len)
            && self
                .entries
                .first()
                .is_none_or(|newest| newest.content != *content)
    }

    // puts what was copied first, moving the entry it equals there, then drops the oldest past
    // the bounds; says whether it changed anything
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
                }
            }
        };

        self.entries.insert(0, entry);

        // the newest alone is within both, as `takes` let in nothing larger than ENTRY_BYTES
        while self.entries.len() > ENTRIES || self.bytes() > TOTAL_BYTES {
            self.entries.pop();
        }

        true
    }

    fn bytes(&self) -> usize {
        self.entries.iter().map(|entry| entry.content.len()).sum()
    }
}

// which of a selection's types is read
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Text,
    Image(String),
}

/*
 * the type a selection is read in, from those it offers: text first, since apps offer a picture
 * of copied text too, like a spreadsheet's cells; then png, then any image. None for the rest
 */
fn pick<'a>(types: impl Iterator<Item = &'a str> + Clone) -> Option<(&'a str, Kind)> {
    let text = TEXTS
        .iter()
        .find_map(|text| types.clone().find(|offered| offered == text));

    if let Some(text) = text {
        return Some((text, Kind::Text));
    }

    let image = types
        .clone()
        .find(|offered| *offered == PNG)
        .or_else(|| types.clone().find(|offered| offered.starts_with("image/")))?;

    Some((image, Kind::Image(image.to_owned())))
}

// asks the holder thread to restore
static RESTORES: OnceLock<Sender<Content>> = OnceLock::new();

// the wl-copy serving the last restored entry, until another took the selection and it was reaped
static HELD: Mutex<Option<Child>> = Mutex::new(None);

fn held() -> MutexGuard<'static, Option<Child>> {
    HELD.lock().unwrap_or_else(PoisonError::into_inner)
}

// starts its threads, each for good; without wl-paste there is no history
pub fn follow() {
    let (restores, heard) = mpsc::channel();

    if RESTORES.set(restores).is_err() {
        return;
    }

    // wl-copy dies with the thread that started it, so this one lives as long as Kanade
    supervise::spawn("clipboard-copy", move || copy(&heard));
    supervise::spawn("clipboard", paste);
}

// puts an entry back on the clipboard, off the caller's thread; a failure is logged
#[expect(dead_code, reason = "the clipboard Surface (#137) restores entries")]
pub fn restore(entry: &Entry) {
    if let Some(restores) = RESTORES.get() {
        drop(restores.send(entry.content.clone()));
    }
}

fn copy(heard: &Receiver<Content>) {
    // never ends: `RESTORES` keeps the sender
    for content in heard {
        if let Err(error) = hold(&content) {
            eprintln!("kanade: cannot restore a clipboard entry ({error})");
        }
    }
}

// starts a wl-copy serving `content`, then ends the one serving the last
fn hold(content: &Content) -> io::Result<()> {
    let mut child = wake::hold(COPY, &["--foreground", "--type", content.mime()])?;

    // wl-copy reads all of it before it takes the selection
    let written = match child.stdin.take() {
        Some(mut stdin) => stdin.write_all(content.bytes()),
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

// forgets the holder once it exited on its own, another program having taken the selection
fn reap() {
    let mut held = held();

    if let Some(child) = held.as_mut()
        && !matches!(child.try_wait(), Ok(None))
    {
        *held = None;
    }
}

fn paste() {
    let stop = wake::Stop::default();

    let error = wake::run(PASTE, WATCH, &stop, |output| {
        watch(output, &mut read, &mut record)
    });

    // nothing stops it
    let Some(error) = error else { return };

    let why = format!("cannot run wl-paste ({error}), no clipboard history");

    eprintln!("kanade: {why}");
    supervise::stopped("clipboard", why);
}

/*
 * follows the selection's states until the output ends, reading each new selection with data;
 * one that is empty, or marked sensitive like a password manager's, is not read. Returns why it
 * ended
 */
fn watch(
    states: impl BufRead,
    read: &mut impl FnMut() -> Result<Option<Content>, String>,
    record: &mut impl FnMut(Content),
) -> io::Error {
    for state in states.lines() {
        let state = match state {
            Ok(state) => state,
            Err(error) => return error,
        };

        reap();

        if matches!(state.trim(), "nil" | "clear" | "sensitive") {
            continue;
        }

        match read() {
            Ok(Some(content)) => record(content),
            Ok(None) => {}
            Err(why) => eprintln!("kanade: cannot read the clipboard ({why})"),
        }
    }

    io::ErrorKind::UnexpectedEof.into()
}

// the selection, if it offers a type Kanade keeps, within ENTRY_BYTES
fn read() -> Result<Option<Content>, String> {
    let types = wake::query(PASTE, &["--list-types"])?;

    let Some((mime, kind)) = pick(types.lines()) else {
        return Ok(None);
    };

    let bytes = wake::read(PASTE, &["--no-newline", "--type", mime], ENTRY_BYTES)?;

    Ok(bytes.and_then(|bytes| Content::new(kind, bytes)))
}

// writes only a change, since a write wakes every window
fn record(content: Content) {
    if Clipboard::read().takes(&content) {
        Clipboard::write().record(content);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(text: &str) -> Content {
        Content::Text(text.into())
    }

    fn image(len: usize, fill: u8) -> Content {
        Content::Image {
            mime: String::from(PNG),
            bytes: vec![fill; len].into(),
        }
    }

    fn texts(clipboard: &Clipboard) -> Vec<String> {
        clipboard
            .entries
            .iter()
            .map(|entry| match &entry.content {
                Content::Text(text) => text.to_string(),
                Content::Image { .. } => String::from("image"),
            })
            .collect()
    }

    #[test]
    fn the_history_keeps_the_newest_entries() {
        let mut clipboard = Clipboard::default();

        for at in 0..ENTRIES + 5 {
            assert!(clipboard.record(text(&at.to_string())));
        }

        assert_eq!(clipboard.entries.len(), ENTRIES);
        assert_eq!(
            clipboard.entries[0].content,
            text(&(ENTRIES + 4).to_string())
        );
        assert_eq!(clipboard.entries[ENTRIES - 1].content, text("5"));
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
    fn copying_the_newest_again_changes_nothing() {
        let mut clipboard = Clipboard::default();

        clipboard.record(text("a"));
        let before = clipboard.clone();

        assert!(!clipboard.takes(&text("a")));
        assert!(!clipboard.record(text("a")));
        assert_eq!(clipboard, before);
    }

    #[test]
    fn copying_an_older_entry_again_moves_it_first_under_its_id() {
        let mut clipboard = Clipboard::default();

        clipboard.record(text("a"));
        clipboard.record(image(3, 1));
        clipboard.record(text("b"));
        let id = clipboard.entries[2].id;

        assert!(clipboard.record(text("a")));
        assert_eq!(texts(&clipboard), ["a", "b", "image"]);
        assert_eq!(clipboard.entries[0].id, id);

        // an image is the same entry only with the same bytes
        assert!(clipboard.record(image(3, 2)));
        assert_eq!(texts(&clipboard), ["image", "a", "b", "image"]);
    }

    #[test]
    fn an_id_is_never_reused() {
        let mut clipboard = Clipboard::default();

        for at in 0..ENTRIES + 1 {
            clipboard.record(text(&at.to_string()));
        }

        let mut ids: Vec<u64> = clipboard.entries.iter().map(|entry| entry.id).collect();
        ids.sort_unstable();
        ids.dedup();

        assert_eq!(ids.len(), ENTRIES);
        assert!(!ids.contains(&1));
    }

    #[test]
    fn text_is_read_before_an_image() {
        let offered = ["image/png", "text/html", "UTF8_STRING", "text/plain"];

        assert_eq!(pick(offered.into_iter()), Some(("UTF8_STRING", Kind::Text)));
    }

    #[test]
    fn png_is_read_before_another_image() {
        let offered = ["image/bmp", "text/html", "image/png"];
        assert_eq!(
            pick(offered.into_iter()),
            Some(("image/png", Kind::Image(String::from("image/png"))))
        );

        let offered = ["text/html", "image/jpeg"];
        assert_eq!(
            pick(offered.into_iter()),
            Some(("image/jpeg", Kind::Image(String::from("image/jpeg"))))
        );
    }

    #[test]
    fn a_selection_without_text_or_an_image_is_not_read() {
        let offered = ["text/html", "text/uri-list", "x-special/gnome-copied-files"];

        assert_eq!(pick(offered.into_iter()), None);
    }

    #[test]
    fn text_that_is_not_utf8_is_not_kept() {
        assert_eq!(Content::new(Kind::Text, vec![0xff, 0xfe]), None);
        assert_eq!(Content::new(Kind::Text, b"hi".to_vec()), Some(text("hi")));
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

    #[test]
    fn only_a_selection_with_data_is_read() {
        let mut reads = 0;
        let mut recorded = Vec::new();

        let lost = watch(
            "data\nnil\nsensitive\nclear\nsomething new\n".as_bytes(),
            &mut || {
                reads += 1;
                Ok(Some(text(&reads.to_string())))
            },
            &mut |content| recorded.push(content),
        );

        assert_eq!(lost.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(recorded, [text("1"), text("2")]);
    }

    #[test]
    fn a_failed_read_records_nothing() {
        let mut recorded = Vec::new();

        watch(
            "data\n".as_bytes(),
            &mut || Err(String::from("gone")),
            &mut |content| recorded.push(content),
        );

        assert!(recorded.is_empty());
    }
}
