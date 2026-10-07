//! The clipboard (#136, ADR 0011): a history of what was copied, text and images, newest first,
//! bounded and kept only in memory. `wl-paste --watch` runs Kanade itself at each new selection
//! (`hand_over`), which hands back that selection's content with its state and type, so what is
//! kept is what that state was about. Restoring an entry hands
//! it to `wl-copy --foreground`, a holder that serves it until another program takes the selection;
//! restoring another kills it. The new selection is announced like any other, so a restored entry
//! moves to the top. What was copied is never logged, and an entry's `Debug` leaves it out.

use std::env;
use std::ffi::OsStr;
use std::fmt;
use std::io::{self, BufRead, Read, Write};
use std::process::Child;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use amane::Service;

use crate::sources::wake;
use crate::supervise;

pub const PASTE: &str = "wl-paste";
pub const COPY: &str = "wl-copy";

// the argument wl-paste runs Kanade with at each new selection; not a verb
pub const HAND_OVER: &str = "--clipboard-selection";

/*
 * what wl-paste runs at each new selection: this Kanade, through setpriv so it dies with wl-paste,
 * which forks it and so does not pass on its own parent-death signal. One process, no shell
 */
fn watch_command(exe: &str) -> Vec<&str> {
    let mut command = vec!["--watch"];

    if wake::guards() {
        command.extend([wake::SETPRIV, "--pdeathsig", "KILL"]);
    }

    command.extend([exe, HAND_OVER]);
    command
}

// the most entries kept, the most bytes one may have, and the most all of them together may
pub const ENTRIES: usize = 50;
pub const ENTRY_BYTES: usize = 16 << 20;
pub const TOTAL_BYTES: usize = 64 << 20;

// what text is restored as; wl-copy offers the other text types with it
const TEXT: &str = "text/plain;charset=utf-8";

// bytes an image has at an offset
type Signature = (usize, &'static [u8]);

// the longest type kept; one wl-paste hands over longer, or with a space, is not
const MIME_BYTES: usize = 255;

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

fn paste() {
    let stop = wake::Stop::default();

    // this Kanade even once a rebuild replaced its file, while it runs
    let exe = format!("/proc/{}/exe", std::process::id());

    let error = wake::run(PASTE, &watch_command(&exe), &stop, |output| {
        watch(output, &mut record)
    });

    // nothing stops it
    let Some(error) = error else { return };

    let why = format!("cannot run wl-paste ({error}), no clipboard history");

    eprintln!("kanade: {why}");
    supervise::stopped("clipboard", why);
}

/*
 * `kanade --clipboard-selection`, as wl-paste runs it at each new selection with the selection's
 * content on stdin and its state and type in the environment
 */
pub fn hand_over_selection() {
    let handed = hand_over(
        env::var_os("CLIPBOARD_STATE").as_deref(),
        env::var_os("CLIPBOARD_TYPE").as_deref(),
        io::stdin().lock(),
        io::stdout().lock(),
    );

    // wl-paste ignores how it exits, and Kanade reads a frame cut short as the watch ending
    drop(handed);
}

/*
 * writes one frame for Kanade: `data <length>[ <type>]` and the content, when the selection's state
 * is `data` (or unset, by a wl-paste older than 2.2), it holds at most ENTRY_BYTES and its type, if
 * said, is one Kanade can frame; else `-` alone, the content unread. So a sensitive selection's,
 * or one in a state Kanade does not know, never leaves wl-paste
 */
fn hand_over(
    state: Option<&OsStr>,
    mime: Option<&OsStr>,
    input: impl Read,
    mut output: impl Write,
) -> io::Result<()> {
    let data = state.is_none_or(|state| state == "data");
    let mime = match mime.map(OsStr::to_str) {
        None => Ok(None),
        Some(Some(mime))
            if (1..=MIME_BYTES).contains(&mime.len())
                && !mime.contains(|c: char| c.is_whitespace() || c.is_control()) =>
        {
            Ok(Some(mime))
        }
        Some(_) => Err(()),
    };

    let Ok(mime) = mime.and_then(|mime| if data { Ok(mime) } else { Err(()) }) else {
        return output.write_all(b"-\n");
    };

    let mut bytes = Vec::new();
    input
        .take(u64::try_from(ENTRY_BYTES).unwrap_or(u64::MAX) + 1)
        .read_to_end(&mut bytes)?;

    if bytes.len() > ENTRY_BYTES {
        return output.write_all(b"-\n");
    }

    match mime {
        Some(mime) => writeln!(output, "data {} {mime}", bytes.len())?,
        None => writeln!(output, "data {}", bytes.len())?,
    }

    output.write_all(&bytes)?;
    output.flush()
}

/*
 * follows the frames `hand_over` wrote until the output ends, keeping each selection with data.
 * Returns why it ended, also a frame it cannot read
 */
fn watch(mut frames: impl BufRead, record: &mut impl FnMut(Content)) -> io::Error {
    let mut header = Vec::new();

    loop {
        header.clear();

        // a header holds a length and a type within MIME_BYTES
        let read = (&mut frames)
            .take(MIME_BYTES as u64 + 64)
            .read_until(b'\n', &mut header);

        match read {
            Ok(0) => return io::ErrorKind::UnexpectedEof.into(),
            Ok(_) => {}
            Err(error) => return error,
        }

        reap();

        let Some(header) = header.strip_suffix(b"\n") else {
            return io::Error::new(
                io::ErrorKind::InvalidData,
                "a clipboard frame without its end",
            );
        };

        if header == b"-" {
            continue;
        }

        let Some((len, mime)) = parse(header) else {
            return io::Error::new(
                io::ErrorKind::InvalidData,
                "a clipboard frame Kanade cannot read",
            );
        };

        let mut bytes = vec![0; len];
        if let Err(error) = frames.read_exact(&mut bytes) {
            return error;
        }

        if let Some(content) = Content::new(mime, bytes) {
            record(content);
        }
    }
}

// a header's length, within ENTRY_BYTES, and type, if any
fn parse(header: &[u8]) -> Option<(usize, Option<&str>)> {
    let header = str::from_utf8(header).ok()?.strip_prefix("data ")?;

    let (len, mime) = match header.split_once(' ') {
        Some((len, mime)) => (len, Some(mime)),
        None => (header, None),
    };

    let len = len.parse().ok().filter(|len| *len <= ENTRY_BYTES)?;

    Some((len, mime))
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

    fn image_of(content: Option<Content>) -> Option<String> {
        match content? {
            Content::Image { mime, .. } => Some(mime),
            Content::Text(_) => None,
        }
    }

    #[test]
    fn a_said_type_decides_what_is_kept() {
        let svg = b"<svg/>".to_vec();
        let bmp = b"BM\0\0".to_vec();

        assert_eq!(
            image_of(Content::new(Some("image/svg+xml"), svg)).as_deref(),
            Some("image/svg+xml")
        );
        assert_eq!(
            image_of(Content::new(Some("image/bmp"), bmp)).as_deref(),
            Some("image/bmp")
        );
        assert_eq!(
            Content::new(Some("text/plain"), b"GIF89a".to_vec()),
            Some(text("GIF89a"))
        );
        assert_eq!(
            Content::new(Some("UTF8_STRING"), b"hi".to_vec()),
            Some(text("hi"))
        );
        assert_eq!(
            Content::new(Some("application/json"), b"{}".to_vec()),
            Some(text("{}"))
        );
        assert_eq!(Content::new(Some("text/plain"), vec![0xff]), None);
        assert_eq!(
            Content::new(Some("application/octet-stream"), b"hi".to_vec()),
            None
        );
    }

    #[test]
    fn without_a_type_text_comes_before_an_image_known_by_its_start() {
        let png = b"\x89PNG\r\n\x1a\n\0\0".to_vec();
        let webp = b"RIFF\x10\0\0\xffWEBPVP8 ".to_vec();

        assert_eq!(
            image_of(Content::new(None, png)).as_deref(),
            Some("image/png")
        );
        assert_eq!(
            image_of(Content::new(None, webp)).as_deref(),
            Some("image/webp")
        );
        assert_eq!(
            Content::new(None, b"GIF89a, a picture".to_vec()),
            Some(text("GIF89a, a picture"))
        );
        assert_eq!(Content::new(None, b"\xffxxxxxxxWEBP".to_vec()), None);
        assert_eq!(Content::new(None, vec![0xff, 0xfe]), None);
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

    // the frames `hand_over` writes for each (state, type, content), as wl-paste runs it
    fn handed(selections: &[(Option<&str>, Option<&str>, &[u8])]) -> Vec<u8> {
        let mut frames = Vec::new();

        for (state, mime, content) in selections {
            hand_over(
                state.map(OsStr::new),
                mime.map(OsStr::new),
                *content,
                &mut frames,
            )
            .unwrap();
        }

        frames
    }

    fn recorded(selections: &[(Option<&str>, Option<&str>, &[u8])]) -> Vec<Content> {
        let mut recorded = Vec::new();

        let lost = watch(handed(selections).as_slice(), &mut |content| {
            recorded.push(content);
        });

        assert_eq!(lost.kind(), io::ErrorKind::UnexpectedEof);
        recorded
    }

    const PLAIN: Option<&str> = Some("text/plain");

    #[test]
    fn only_a_selection_with_data_is_kept() {
        let selections: [(Option<&str>, Option<&str>, &[u8]); 7] = [
            (Some("data"), PLAIN, b"a"),
            (Some("nil"), None, b""),
            (Some("sensitive"), PLAIN, b"hunter2"),
            (Some("clear"), None, b""),
            (Some("something new"), PLAIN, b"b"),
            (Some("data "), PLAIN, b"c"),
            (None, None, b"d"),
        ];

        assert_eq!(recorded(&selections), [text("a"), text("d")]);
    }

    #[test]
    fn a_sensitive_selection_right_after_another_is_never_kept() {
        let selections: [(Option<&str>, Option<&str>, &[u8]); 2] = [
            (Some("data"), PLAIN, b"a"),
            (Some("sensitive"), PLAIN, b"hunter2"),
        ];

        assert_eq!(recorded(&selections), [text("a")]);
    }

    #[test]
    fn a_sensitive_selection_is_never_read() {
        let mut frames = Vec::new();
        let mut unread: &[u8] = b"hunter2";

        hand_over(
            Some(OsStr::new("sensitive")),
            PLAIN.map(OsStr::new),
            &mut unread,
            &mut frames,
        )
        .unwrap();

        assert_eq!(frames, b"-\n");
        assert_eq!(unread, b"hunter2");
    }

    #[test]
    fn a_selection_is_kept_whole_up_to_its_bounds() {
        let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[0, 0xff, b'\n', 0x80]].concat();
        let most = vec![b'x'; ENTRY_BYTES];
        let over = vec![b'x'; ENTRY_BYTES + 1];

        let selections: [(Option<&str>, Option<&str>, &[u8]); 4] = [
            (Some("data"), Some("image/png"), &png),
            (Some("data"), PLAIN, &over),
            (Some("data"), PLAIN, &most),
            (Some("data"), PLAIN, b"after"),
        ];

        let recorded = recorded(&selections);

        assert_eq!(recorded.len(), 3);
        assert_eq!(recorded[0].bytes(), png);
        assert_eq!(recorded[1].len(), ENTRY_BYTES);
        assert_eq!(recorded[2], text("after"));
    }

    #[test]
    fn a_type_kanade_cannot_frame_is_not_kept() {
        let long = format!("text/{}", "x".repeat(MIME_BYTES));
        let selections: [(Option<&str>, Option<&str>, &[u8]); 4] = [
            (Some("data"), Some("text/plain extra"), b"a"),
            (Some("data"), Some(&long), b"b"),
            (Some("data"), Some(""), b"c"),
            (Some("data"), PLAIN, b"d"),
        ];

        assert_eq!(recorded(&selections), [text("d")]);
    }

    #[test]
    fn a_frame_kanade_cannot_read_ends_the_watch() {
        for frames in [
            "data x\n".as_bytes(),
            b"data 99999999999\n",
            b"something\n",
            b"data 5\nab",
            b"data 2",
        ] {
            let mut recorded = Vec::new();
            watch(frames, &mut |content| recorded.push(content));

            assert!(recorded.is_empty(), "{frames:?}");
        }
    }
}
