//! The clipboard (#136, ADR 0011): a history of what was copied, text and images, newest first,
//! bounded and kept only in memory. `wl-paste --watch` hands over each new selection with its
//! content, so what is kept is what that selection's state was about. Restoring an entry hands
//! it to `wl-copy --foreground`, a holder that serves it until another program takes the selection;
//! restoring another kills it. The new selection is announced like any other, so a restored entry
//! moves to the top. What was copied is never logged, and an entry's `Debug` leaves it out.

use std::fmt;
use std::io::{self, BufRead, Write};
use std::iter;
use std::process::Child;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use amane::Service;

use crate::sources::wake;
use crate::supervise;

pub const PASTE: &str = "wl-paste";
pub const COPY: &str = "wl-copy";

/*
 * what wl-paste runs at each new selection, with that selection's content on its input, in text if
 * it offers any: a line with `data` and the first ENTRY_BYTES + 1 bytes in base64 when its state
 * is `data` (or unset, by a wl-paste older than 2.2), else a `-`, without reading it. wl-paste
 * waits for it before the next selection, so the content and the state are the same selection's
 */
fn watch_command() -> [String; 4] {
    let script = format!(
        "if [ \"${{CLIPBOARD_STATE:-data}}\" = data ]; then printf 'data '; head -c {} | base64 -w0; echo; else echo -; fi",
        ENTRY_BYTES + 1
    );

    [
        String::from("--watch"),
        String::from("sh"),
        String::from("-c"),
        script,
    ]
}

// the most entries kept, the most bytes one may have, and the most all of them together may
pub const ENTRIES: usize = 50;
pub const ENTRY_BYTES: usize = 16 << 20;
pub const TOTAL_BYTES: usize = 64 << 20;

// what text is restored as; wl-copy offers the other text types with it
const TEXT: &str = "text/plain;charset=utf-8";

// the images kept, by how their content starts: (type, offset, signature)
const IMAGES: [(&str, usize, &[u8]); 5] = [
    ("image/png", 0, b"\x89PNG\r\n\x1a\n"),
    ("image/jpeg", 0, b"\xff\xd8\xff"),
    ("image/gif", 0, b"GIF87a"),
    ("image/gif", 0, b"GIF89a"),
    ("image/webp", 8, b"WEBP"),
];

// what one entry holds; shared, so a read of the history copies none of it
#[derive(Clone, PartialEq, Eq)]
pub enum Content {
    Text(Arc<str>),
    Image { mime: String, bytes: Arc<[u8]> },
}

impl Content {
    /*
     * from what a selection held: an image Kanade knows by its start, else UTF-8 text; wl-paste
     * hands over text when a selection offers it, so an image comes only on its own. None for the
     * rest
     */
    fn new(bytes: Vec<u8>) -> Option<Content> {
        let image = IMAGES.iter().find(|(_, at, signature)| {
            bytes
                .get(*at..*at + signature.len())
                .is_some_and(|start| start == *signature)
        });

        if let Some((mime, ..)) = image {
            return Some(Content::Image {
                mime: String::from(*mime),
                bytes: bytes.into(),
            });
        }

        String::from_utf8(bytes)
            .ok()
            .map(|text| Content::Text(text.into()))
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

    let command = watch_command();
    let args: Vec<&str> = command.iter().map(String::as_str).collect();

    let error = wake::run(PASTE, &args, &stop, |output| watch(output, &mut record));

    // nothing stops it
    let Some(error) = error else { return };

    let why = format!("cannot run wl-paste ({error}), no clipboard history");

    eprintln!("kanade: {why}");
    supervise::stopped("clipboard", why);
}

/*
 * follows the selections until the output ends, keeping each one wl-paste handed over: only a
 * selection with data, never one empty, marked sensitive like a password manager's, or in a state
 * Kanade does not know. Returns why it ended
 */
fn watch(mut selections: impl BufRead, record: &mut impl FnMut(Content)) -> io::Error {
    // within ENTRY_BYTES + 1 in base64, as `watch_command` reads no more
    let mut line = Vec::new();

    loop {
        line.clear();
        match selections.read_until(b'\n', &mut line) {
            Ok(0) => return io::ErrorKind::UnexpectedEof.into(),
            Ok(_) => {}
            Err(error) => return error,
        }

        reap();

        let Some(encoded) = line.strip_prefix(b"data ") else {
            continue;
        };

        let Some(bytes) = decode(encoded.trim_ascii_end()) else {
            eprintln!("kanade: cannot read the clipboard (wl-paste handed over no base64)");
            continue;
        };

        if bytes.len() > ENTRY_BYTES {
            continue;
        }

        if let Some(content) = Content::new(bytes) {
            record(content);
        }
    }
}

// standard base64 with its padding, as coreutils' `base64` writes it
fn decode(encoded: &[u8]) -> Option<Vec<u8>> {
    let value = |byte: u8| match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };

    if !encoded.len().is_multiple_of(4) {
        return None;
    }

    let mut bytes = Vec::with_capacity(encoded.len() / 4 * 3);

    for (at, quad) in encoded.chunks(4).enumerate() {
        let last = at + 1 == encoded.len() / 4;
        let padding = quad.iter().rev().take_while(|byte| **byte == b'=').count();

        if padding > 2 || (padding > 0 && !last) {
            return None;
        }

        let mut word = 0u32;
        for byte in &quad[..4 - padding] {
            word = word << 6 | u32::from(value(*byte)?);
        }
        word <<= 6 * padding;

        let [_, first, second, third] = word.to_be_bytes();
        bytes.extend(iter::once(first).chain([second, third]).take(3 - padding));
    }

    Some(bytes)
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
            mime: String::from("image/png"),
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
    fn an_image_is_known_by_its_start() {
        let png = b"\x89PNG\r\n\x1a\n\0\0".to_vec();
        let webp = b"RIFF\x10\0\0\0WEBPVP8 ".to_vec();

        assert!(
            matches!(Content::new(png), Some(Content::Image { mime, .. }) if mime == "image/png")
        );
        assert!(
            matches!(Content::new(webp), Some(Content::Image { mime, .. }) if mime == "image/webp")
        );
        assert_eq!(Content::new(b"GIF8".to_vec()), Some(text("GIF8")));
    }

    #[test]
    fn content_that_is_neither_text_nor_a_known_image_is_not_kept() {
        assert_eq!(Content::new(vec![0xff, 0xfe]), None);
        assert_eq!(Content::new(b"hi".to_vec()), Some(text("hi")));
    }

    #[test]
    fn base64_decodes_as_coreutils_writes_it() {
        assert_eq!(decode(b""), Some(Vec::new()));
        assert_eq!(decode(b"aGk="), Some(b"hi".to_vec()));
        assert_eq!(decode(b"aA=="), Some(b"h".to_vec()));
        assert_eq!(decode(b"/+8A"), Some(vec![0xff, 0xef, 0]));
        assert_eq!(decode(b"aGk"), None);
        assert_eq!(decode(b"aA==aGk="), None);
        assert_eq!(decode(b"a==="), None);
        assert_eq!(decode(b"a-k="), None);
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

    // what wl-paste hands over, run through the watch command's script, so the test covers both
    fn handed(selections: &[(Option<&str>, &[u8])]) -> Vec<u8> {
        let script = watch_command()[3].clone();

        selections
            .iter()
            .flat_map(|(state, content)| {
                let mut command = std::process::Command::new("sh");
                command.args(["-c", &script]);
                if let Some(state) = state {
                    command.env("CLIPBOARD_STATE", state);
                }

                let mut child = command
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .spawn()
                    .unwrap();

                // written beside the read, as it is larger than a pipe holds; the script may stop
                // reading early, as it does for a sensitive selection
                let mut stdin = child.stdin.take().unwrap();
                std::thread::scope(|scope| {
                    scope.spawn(move || drop(stdin.write_all(content)));
                    child.wait_with_output().unwrap().stdout
                })
            })
            .collect()
    }

    fn recorded(selections: &[(Option<&str>, &[u8])]) -> Vec<Content> {
        let mut recorded = Vec::new();

        let lost = watch(handed(selections).as_slice(), &mut |content| {
            recorded.push(content);
        });

        assert_eq!(lost.kind(), io::ErrorKind::UnexpectedEof);
        recorded
    }

    #[test]
    fn only_a_selection_with_data_is_kept() {
        let selections: [(Option<&str>, &[u8]); 7] = [
            (Some("data"), b"a"),
            (Some("nil"), b""),
            (Some("sensitive"), b"hunter2"),
            (Some("clear"), b""),
            (Some("something new"), b"b"),
            (Some("data "), b"c"),
            (None, b"d"),
        ];

        assert_eq!(recorded(&selections), [text("a"), text("d")]);
    }

    #[test]
    fn a_sensitive_selection_right_after_another_is_never_kept() {
        let selections: [(Option<&str>, &[u8]); 2] =
            [(Some("data"), b"a"), (Some("sensitive"), b"hunter2")];

        assert_eq!(recorded(&selections), [text("a")]);
    }

    #[test]
    fn a_selection_is_kept_whole_up_to_its_bounds() {
        let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[0, 0xff, b'\n', 0x80]].concat();
        let most = vec![b'x'; ENTRY_BYTES];
        let over = vec![b'x'; ENTRY_BYTES + 1];

        let selections: [(Option<&str>, &[u8]); 4] = [
            (Some("data"), &png),
            (Some("data"), &over),
            (Some("data"), &most),
            (Some("data"), b"after"),
        ];

        let recorded = recorded(&selections);

        assert_eq!(recorded.len(), 3);
        assert_eq!(recorded[0].bytes(), png);
        assert_eq!(recorded[1].len(), ENTRY_BYTES);
        assert_eq!(recorded[2], text("after"));
    }

    #[test]
    fn content_wl_paste_garbled_is_not_kept() {
        let mut recorded = Vec::new();

        watch("data a\ndata aGk=\n".as_bytes(), &mut |content| {
            recorded.push(content);
        });

        assert_eq!(recorded, [text("hi")]);
    }
}
