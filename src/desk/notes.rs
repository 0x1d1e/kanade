//! The Desk's notes card (ADR 0140): the one Desk face that takes a caret. `desk.notes` draws the
//! notes under the Desk's other lines, the newest last; a click on the card types into its last
//! note in place, Enter starts one, and Escape leaves and keeps it.
//!
//! The notes live in `$XDG_STATE_HOME/kanade/desk-notes`, one note per line, readable by its owner
//! only. The worker below reads and writes that file and no view touches it: the draw thread only
//! posts a snapshot, at most about once a second while typing and once more when the card is left.
//! What the file holds is the `Notes` Service; what a click started is the draw thread's own, since
//! a view writes no Service.
//!
//! A note is one line: a newline, a tab and a return in one are a space, and a note past its cap is
//! cut. Note text is never logged, and a file that does not read, or is larger than its cap, leaves
//! the card empty rather than stopping anything.

use std::fs;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use kanade_runtime::Key;
use kanade_runtime::MutexExt;
use kanade_runtime::service::Service;

use crate::atomic::{self, Mode};
use crate::config;

// the notes the card shows at once, the newest last, so the window stays on the output
pub const SHOWN: usize = 5;

// what the file holds: past these the oldest note goes. A larger file is not trusted at all, so a
// file another program grew cannot fill the memory or the card
const LINES: usize = 200;
const CHARS: usize = 500;
const BYTES: usize = 64 * 1024;

// while typing the file takes at most this often; leaving the card saves at once
const EVERY: Duration = Duration::from_secs(1);

// the file, under the state directory
const FILE: &str = "kanade/desk-notes";

// The notes as the shell last wrote or read them; `loaded` is false until the first read, so the
// card asks for it once. Empty until then, which is what an unreadable file leaves too
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notes {
    pub loaded: bool,
    pub lines: Vec<String>,
}

impl Service for Notes {
    fn new() -> Self {
        Notes::default()
    }

    fn listen() {}
}

// what the worker has been asked for: the file read once, and the newest snapshot to write
#[derive(Debug, Default)]
struct Jobs {
    read: bool,
    write: Option<Vec<String>>,
}

static JOBS: Mutex<Jobs> = Mutex::new(Jobs {
    read: false,
    write: None,
});

static WAKE: Condvar = Condvar::new();

// when the file was last asked to be written, for the throttle while typing
static SAVED: Mutex<Option<Instant>> = Mutex::new(None);

// The worker the desk Module starts on its own thread, so nothing of this runs while it is off:
// the file is read when a card first draws, and written from what the draw thread posted
pub fn run() {
    loop {
        let (read, write) = {
            let mut jobs = JOBS.locked();

            while !jobs.read && jobs.write.is_none() {
                jobs = WAKE.wait(jobs).unwrap_or_else(PoisonError::into_inner);
            }

            (std::mem::take(&mut jobs.read), jobs.write.take())
        };

        // the lock is let go first, since writing the Service draws the windows that read it
        if read {
            load();
        }

        if let Some(lines) = write {
            store(&lines);
        }
    }
}

// ask for the file the first time a card draws
pub fn ask() {
    JOBS.locked().read = true;

    WAKE.notify_one();
}

fn load() {
    let lines = path()
        .and_then(|path| fs::read(path).ok())
        .map(|bytes| parse(&bytes))
        .unwrap_or_default();

    let mut notes = Notes::write();

    notes.loaded = true;
    notes.lines = lines;
}

fn store(lines: &[String]) {
    let Some(path) = path() else {
        return;
    };

    let lines = keep(lines);

    if let Err(error) = atomic::write(&path, &text(&lines), Mode::Private) {
        eprintln!(
            "kanade: cannot write the desk notes {}: {error}",
            path.display()
        );

        return;
    }

    let mut notes = Notes::write();

    notes.loaded = true;
    notes.lines = lines;
}

// one snapshot of the notes for the worker, written now when `now`, else at most once EVERY
fn save(lines: Vec<String>, now: bool) {
    let mut saved = SAVED.locked();
    let due = now || saved.is_none_or(|last| last.elapsed() >= EVERY);

    if !due {
        // the next key, or leaving the card, saves what this one typed
        return;
    }

    *saved = Some(Instant::now());

    drop(saved);

    JOBS.locked().write = Some(lines);

    WAKE.notify_one();
}

fn path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok();

    config::base("XDG_STATE_HOME", ".local/state", home.as_deref()).map(|dir| dir.join(FILE))
}

// the notes a file holds: one per line, each normalized and capped; a file past its cap holds none
pub fn parse(bytes: &[u8]) -> Vec<String> {
    if bytes.len() > BYTES {
        return Vec::new();
    }

    let text = String::from_utf8_lossy(bytes);

    keep(&text.split('\n').map(note).collect::<Vec<String>>())
}

// one note as the card and the file hold it: one line, and no longer than its cap
pub fn note(text: &str) -> String {
    let mut line = String::with_capacity(text.len());
    let mut space = false;

    for letter in text.chars() {
        if matches!(letter, '\n' | '\r' | '\t') {
            space = true;

            continue;
        }

        if std::mem::take(&mut space) && !line.is_empty() && letter != ' ' {
            line.push(' ');
        }

        line.push(letter);
    }

    let line = line.trim();

    if line.chars().count() > CHARS {
        line.chars().take(CHARS).collect()
    } else {
        line.to_owned()
    }
}

// the notes worth keeping: no empty one, and no more than the file may hold, the oldest going first
fn keep(lines: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = lines
        .iter()
        .filter(|line| !line.is_empty())
        .map(|line| note(line))
        .collect();

    kept.drain(..kept.len().saturating_sub(LINES));

    while kept.len() > 1 && text(&kept).len() > BYTES {
        kept.remove(0);
    }

    kept
}

// what the file holds: the notes, one per line, the last with a newline of its own
pub fn text(lines: &[String]) -> Vec<u8> {
    let mut bytes = Vec::new();

    for (at, line) in lines.iter().enumerate() {
        if at > 0 {
            bytes.push(b'\n');
        }

        bytes.extend_from_slice(line.as_bytes());
    }

    if !bytes.is_empty() {
        bytes.push(b'\n');
    }

    bytes
}

// what a key asked of the card being typed into
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    // the card has no use for the key
    Ignored,

    // the card took it, and its notes may be saved as it shows them now
    Save(Vec<String>),

    // Escape: these are the notes as the card shows them, and the card stops taking the keyboard
    Left(Vec<String>),
}

// A notes card a click started, on one output: the notes as the card shows them, with the caret at
// the end of the last one, since notes are written in the order they are typed
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    monitor: String,
    lines: Vec<String>,
}

impl Edit {
    // a click starts a note of its own, under the ones the file holds: the caret is on that line
    fn new(monitor: &str, stored: &[String]) -> Self {
        let mut lines = keep(stored);

        lines.push(String::new());

        Edit {
            monitor: monitor.to_owned(),
            lines,
        }
    }

    // a typed key, into the last note
    fn press(&mut self, key: Key) -> Step {
        match key {
            Key::Character(letter) => self.push(letter.encode_utf8(&mut [0; 4])),
            Key::Space => self.push(" "),
            Key::Enter => self.break_note(),
            Key::Backspace => self.erase(),
            Key::Escape => return Step::Left(keep(&self.lines)),
            _ => return Step::Ignored,
        }

        Step::Save(self.lines.clone())
    }

    fn push(&mut self, text: &str) {
        let Some(line) = self.lines.last_mut() else {
            return;
        };

        if line.chars().count() + text.chars().count() > CHARS {
            return;
        }

        line.push_str(text);
    }

    // Enter starts a note of its own; the oldest goes once the card holds as many as the file may
    fn break_note(&mut self) {
        self.lines.push(String::new());

        self.lines.drain(..self.lines.len().saturating_sub(LINES));
    }

    // the last letter goes; a note left empty gives up its own line break with it
    fn erase(&mut self) {
        let Some(line) = self.lines.last_mut() else {
            return;
        };

        if !line.is_empty() {
            line.pop();
        } else if self.lines.len() > 1 {
            self.lines.pop();
        }
    }
}

// What the Desk's cards are doing this session: the one the pointer is over, which asks for the
// keyboard before the press that focuses it, and the one a click started
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Cards {
    over: Option<String>,
    edit: Option<Edit>,
}

impl Cards {
    pub const fn new() -> Self {
        Cards {
            over: None,
            edit: None,
        }
    }

    // the pointer came onto a monitor's card, or left it
    pub fn hover(&mut self, monitor: &str, inside: bool) {
        self.over = inside.then(|| monitor.to_owned());
    }

    // A click on a card: it takes the typing, from the notes as the shell holds them. A second
    // press on the card being typed into keeps what was typed; a press on another output's card
    // gives up the one being typed into, whose notes come back to be saved
    pub fn start(&mut self, monitor: &str, stored: &[String]) -> Option<Vec<String>> {
        if self.editing(monitor) {
            return None;
        }

        let left = self.close();

        self.edit = Some(Edit::new(monitor, stored));

        left
    }

    // whether this output's card is the one being typed into
    pub fn editing(&self, monitor: &str) -> bool {
        self.edit
            .as_ref()
            .is_some_and(|edit| edit.monitor == monitor)
    }

    // the card takes the keyboard while the pointer is on it, so the press that focuses it comes
    // after it is asked for, and all the time it is being typed into
    pub fn keyboard(&self, monitor: &str) -> bool {
        self.editing(monitor) || self.over.as_deref() == Some(monitor)
    }

    // the notes this output's card shows: what is being typed into, else the file's newest
    pub fn shown(&self, monitor: &str, stored: &[String]) -> Vec<String> {
        match &self.edit {
            Some(edit) if edit.monitor == monitor => edit.lines.clone(),
            _ => stored[stored.len().saturating_sub(SHOWN)..].to_vec(),
        }
    }

    // a key for the card being typed into
    pub fn press(&mut self, key: Key) -> Step {
        let Some(edit) = &mut self.edit else {
            return Step::Ignored;
        };

        let step = edit.press(key);

        // Escape leaves the card, and the pointer no longer asks for the keyboard for it
        if matches!(step, Step::Left(_)) {
            self.edit = None;
            self.over = None;
        }

        step
    }

    // the card on this output stops editing, and its notes are to be saved as they are
    pub fn blur(&mut self, monitor: &str) -> Option<Vec<String>> {
        if !self.editing(monitor) {
            return None;
        }

        self.edit.take().map(|edit| keep(&edit.lines))
    }

    // every card stops editing, as the face turns off or niri's overview opens
    pub fn close(&mut self) -> Option<Vec<String>> {
        self.over = None;

        self.edit.take().map(|edit| keep(&edit.lines))
    }
}

static CARDS: Mutex<Cards> = Mutex::new(Cards::new());

// the pointer came onto this output's card, or left it
pub fn hover(monitor: &str, inside: bool) {
    CARDS.locked().hover(monitor, inside);
}

// a click on this output's card, from the notes as the shell holds them
pub fn begin(monitor: &str, stored: &[String]) {
    let left = CARDS.locked().start(monitor, stored);

    if let Some(lines) = left {
        save(lines, true);
    }
}

// whether this output's window has to take the keyboard, for a press to focus it
pub fn keyboard(monitor: &str) -> bool {
    CARDS.locked().keyboard(monitor)
}

// whether this output's card is being typed into, so its window takes keys
pub fn editing(monitor: &str) -> bool {
    CARDS.locked().editing(monitor)
}

// the notes this output's card draws
pub fn shown(monitor: &str, stored: &[String]) -> Vec<String> {
    CARDS.locked().shown(monitor, stored)
}

// a key for the card being typed into; false when that card has no use for it
pub fn press(monitor: &str, key: Key) -> bool {
    let step = {
        let mut cards = CARDS.locked();

        if !cards.editing(monitor) {
            return false;
        }

        cards.press(key)
    };

    match step {
        Step::Ignored => false,
        Step::Save(lines) => {
            save(lines, false);

            true
        }
        Step::Left(lines) => {
            save(lines, true);

            true
        }
    }
}

// the keyboard went elsewhere: what was typed is kept
pub fn blur(monitor: &str) {
    let lines = CARDS.locked().blur(monitor);

    if let Some(lines) = lines {
        save(lines, true);
    }
}

// the face turned off or the overview opened: whatever was being typed into is kept and let go
pub fn close() {
    let lines = CARDS.locked().close();

    if let Some(lines) = lines {
        save(lines, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: &str = "eDP-1";
    const OTHER: &str = "HDMI-A-1";

    fn lines(notes: &[&str]) -> Vec<String> {
        notes.iter().map(|note| (*note).to_owned()).collect()
    }

    fn saved(step: Step) -> Vec<String> {
        match step {
            Step::Save(lines) | Step::Left(lines) => lines,
            Step::Ignored => panic!("the card did not take the key"),
        }
    }

    #[test]
    fn a_note_is_one_line_and_its_cap() {
        assert_eq!(note("buy milk"), "buy milk");
        assert_eq!(note("buy\nmilk"), "buy milk");
        assert_eq!(note("buy\r\nmilk"), "buy milk");
        assert_eq!(note("buy\tmilk"), "buy milk");
        assert_eq!(note("  buy milk  "), "buy milk");
        assert_eq!(note("a\n\nb"), "a b");
        assert_eq!(note("feat. e\u{301}"), "feat. e\u{301}");
        assert_eq!(note(""), "");

        let long = "\u{e9}".repeat(CHARS + 40);

        assert_eq!(note(&long).chars().count(), CHARS);
    }

    #[test]
    fn a_file_is_one_note_per_line_with_its_caps() {
        assert_eq!(parse(b""), Vec::<String>::new());
        assert_eq!(parse(b"one\ntwo\n"), lines(&["one", "two"]));
        assert_eq!(parse(b"one\r\ntwo"), lines(&["one", "two"]));
        assert_eq!(parse(b"one\n\ntwo\n   \n"), lines(&["one", "two"]));
        assert_eq!(parse(b"one\ntwo\nthree"), lines(&["one", "two", "three"]));

        let many = (0..LINES + 5)
            .map(|at| format!("note {at}\n"))
            .collect::<String>();
        let kept = parse(many.as_bytes());

        assert_eq!(kept.len(), LINES);
        assert_eq!(kept.first().map(String::as_str), Some("note 5"));
        assert_eq!(kept.last().map(String::as_str), Some("note 204"));

        // a file past its cap is not trusted, and the card is empty rather than slow
        assert_eq!(parse(&vec![b'a'; BYTES + 1]), Vec::<String>::new());

        // notes too long for the file's bytes keep the newest, and lose the rest
        let heavy = vec!["x".repeat(CHARS); BYTES / (CHARS + 1) + 2];

        let kept = keep(&heavy);

        assert_eq!(kept.len(), BYTES / (CHARS + 1));
        assert!(text(&kept).len() <= BYTES);
    }

    #[test]
    fn the_file_holds_the_notes_one_per_line() {
        assert_eq!(text(&lines(&[])), Vec::<u8>::new());
        assert_eq!(text(&lines(&["one"])), b"one\n");
        assert_eq!(text(&lines(&["one", "two"])), b"one\ntwo\n");
        assert_eq!(parse(&text(&lines(&["a b", "c"]))), lines(&["a b", "c"]));
    }

    #[test]
    fn a_click_starts_a_card_and_the_pointer_asks_for_the_keyboard() {
        let mut cards = Cards::new();
        let stored = lines(&["one", "two"]);

        assert!(!cards.keyboard(MONITOR));
        assert_eq!(cards.shown(MONITOR, &stored), stored);

        cards.hover(MONITOR, true);

        assert!(cards.keyboard(MONITOR));
        assert!(!cards.editing(MONITOR));

        // hovering takes no keyboard focus of its own
        assert_eq!(cards.press(Key::Character('x')), Step::Ignored);

        cards.start(MONITOR, &stored);

        assert!(cards.editing(MONITOR));
        assert_eq!(
            saved(cards.press(Key::Character('x'))),
            lines(&["one", "two", "x"])
        );
        assert_eq!(cards.shown(MONITOR, &stored), lines(&["one", "two", "x"]));

        // the keyboard stays while the card is typed into, the pointer having gone
        cards.hover(MONITOR, false);

        assert!(cards.keyboard(MONITOR));

        // a click on another output's card takes the typing over, and gives these notes back
        assert_eq!(
            cards.start(OTHER, &lines(&["x"])),
            Some(lines(&["one", "two", "x"]))
        );
        assert!(!cards.editing(MONITOR));
        assert!(cards.editing(OTHER));

        // the new card has a note of its own, empty until something is typed
        assert_eq!(cards.shown(OTHER, &[]), lines(&["x", ""]));
    }

    #[test]
    fn typing_goes_into_the_last_note_and_enter_starts_one() {
        let mut cards = Cards::new();

        cards.start(MONITOR, &lines(&["one"]));

        assert_eq!(
            saved(cards.press(Key::Character('a'))),
            lines(&["one", "a"])
        );

        for key in [Key::Character('b'), Key::Space, Key::Character('c')] {
            cards.press(key);
        }

        assert_eq!(cards.shown(MONITOR, &[]), lines(&["one", "ab c"]));

        cards.press(Key::Enter);

        assert_eq!(cards.shown(MONITOR, &[]), lines(&["one", "ab c", ""]));
        assert_eq!(saved(cards.press(Key::Character('d'))), {
            lines(&["one", "ab c", "d"])
        });

        // backspace takes the last letter, then the empty note's line break
        cards.press(Key::Backspace);
        cards.press(Key::Backspace);

        assert_eq!(cards.shown(MONITOR, &[]), lines(&["one", "ab c"]));

        cards.press(Key::Backspace);

        assert_eq!(cards.shown(MONITOR, &[]), lines(&["one", "ab "]));

        // a note is saved trimmed, and one left empty is not written at all
        assert_eq!(saved(cards.press(Key::Escape)), lines(&["one", "ab"]));
    }

    #[test]
    fn a_key_the_card_does_not_use_is_left_alone() {
        let mut cards = Cards::new();

        cards.start(MONITOR, &lines(&["one"]));

        for key in [Key::Left, Key::Tab, Key::Up, Key::Shortcut('k')] {
            assert_eq!(cards.press(key), Step::Ignored);
        }

        // another output's card keeps the notes it holds
        assert_eq!(cards.shown(OTHER, &lines(&["x"])), lines(&["x"]));
        assert!(!cards.editing(OTHER));
    }

    #[test]
    fn escape_keeps_the_notes_and_leaves_the_card() {
        let mut cards = Cards::new();

        cards.hover(MONITOR, true);
        cards.start(MONITOR, &lines(&["one"]));
        cards.press(Key::Character('x'));

        assert_eq!(cards.press(Key::Escape), Step::Left(lines(&["one", "x"])));
        assert!(!cards.editing(MONITOR));
        assert!(!cards.keyboard(MONITOR));
        assert_eq!(cards.press(Key::Character('y')), Step::Ignored);
        assert_eq!(cards.shown(MONITOR, &lines(&["other"])), lines(&["other"]));
    }

    #[test]
    fn leaving_the_window_or_the_face_keeps_what_was_typed() {
        let mut cards = Cards::new();

        cards.hover(MONITOR, true);
        cards.start(MONITOR, &lines(&["one"]));
        cards.press(Key::Character('x'));

        assert_eq!(cards.blur(MONITOR), Some(lines(&["one", "x"])));
        assert!(!cards.editing(MONITOR));
        assert_eq!(cards.blur(MONITOR), None);

        cards.start(MONITOR, &lines(&["one"]));
        cards.press(Key::Enter);

        // another output's window losing the keyboard leaves this card alone
        assert_eq!(cards.blur(OTHER), None);
        assert!(cards.editing(MONITOR));

        assert_eq!(cards.close(), Some(lines(&["one"])));
        assert!(!cards.editing(MONITOR));
        assert!(!cards.keyboard(MONITOR));
        assert_eq!(cards.close(), None);
    }

    #[test]
    fn a_card_holds_no_more_notes_than_the_file_may() {
        let mut cards = Cards::new();

        cards.start(MONITOR, &[]);

        for _ in 0..LINES + 3 {
            cards.press(Key::Character('n'));
            cards.press(Key::Enter);
        }

        // the note after the last break fills the card, and the oldest is long gone
        cards.press(Key::Character('n'));

        let kept = cards.close().unwrap_or_default();

        assert_eq!(kept.len(), LINES);
        assert!(kept.iter().all(|note| note == "n"));
    }
}
