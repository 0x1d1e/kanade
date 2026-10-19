//! The Clipboard Surface: the clipboard history, newest first, searched as it is typed.
//! It opens only from IPC or a keybind, so it always holds the keyboard, like the Launcher: typing
//! searches, the arrow keys move a ring over each entry, its pin, its delete and Clear all, and Enter
//! presses what the ring is on. Pressing an entry copies it and closes the island once it is on the
//! clipboard, or says it was not copied; pressing its pin pins it, or unpins it (ADR 0114). It says
//! when the history is empty and when nothing matches. Text shows as its first words; an image as
//! a thumbnail, once its preview is written to tmpfs (`sources::clipboard::preview`), else its kind
//! and size. Only the pins are kept across restarts (ADR 0115); the rest of the history never leaves
//! memory.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Column, Cursor, Image, ImageState, Key, Padding, Parent, Rectangle, Row, Scroll, Size,
    SpaceBetween, Stack, Start, Text, Widget, children,
};
use kanade_runtime::{
    MutexExt,
    service::{self, Service},
};

use crate::icon::Icon;
use crate::island::geometry;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::sources::clipboard::{self, Clipboard, Content, Entry, PIN_BYTES, PINS, preview};
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, DISABLED, radius};
use crate::view;

use super::visit::Visited;
use super::{Outline, RING, confirm, store};

// the content's width, which every row fills, as wide as the largest body (`island.width`)
fn width() -> f32 {
    view::largest().width - 2.0 * INSET
}

const FIELD: f32 = 44.0;
const FIELD_INSET: f32 = 16.0;
const FOOTER: f32 = 28.0;
const GAP: f32 = 12.0;

// what the rows scroll in, as tall as the largest body (`island.height`) leaves
fn room() -> f32 {
    view::largest().height - 2.0 * INSET - FIELD - FOOTER - 2.0 * GAP
}

// what a state, as an empty history, takes of the list
const STATE: f32 = 120.0;

// `ROWS` at a time in the least largest body, more in a taller one
const LEAST_LIST: f32 = geometry::CALENDAR.height - 2.0 * INSET - FIELD - FOOTER - 2.0 * GAP;

const ROWS: usize = 4;
const ROW_GAP: f32 = 6.0;
const ROW: f32 = (LEAST_LIST - (ROWS - 1) as f32 * ROW_GAP) / ROWS as f32;
const ROW_INSET: f32 = 8.0;

// a row's mark, a kind icon or a picture's thumbnail, and the thumbnail's corners
const ICON: f32 = 32.0;
const ICON_GAP: f32 = 12.0;
const THUMB_RADIUS: f32 = 6.0;

// the pixels a thumbnail is kept at per point, as the Wallpapers' tiles
const SCALE: f32 = 2.0;

// the typed text's size, and the caret after it
const QUERY: f32 = theme::text::TITLE;
const CARET: f32 = 1.5;

// the most of a text a row shows; it is elided to its width anyway
const PREVIEW: usize = 200;

// pixels per wheel line
const WHEEL: f32 = 40.0;

// what Clear all is held as, and how wide its pill is
const CLEAR: &str = "clipboard clear";
const PILL: f32 = 96.0;

/*
 * the query, the ring and the scroll for one visit of the Surface (`IslandService::visit`), so every
 * opening starts empty at the top. Written by input only, never by the view
 */
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Search {
    visit: u64,
    query: String,

    // in the entries the query finds
    selected: usize,

    // where the ring is on the selected entry: the entry, its pin or its delete
    stop: Stop,

    // the ring is on Clear all, below the rows
    clear: bool,

    // how far the rows scrolled, in pixels
    offset: f32,

    copying: Copying,

    // a pin was refused, the pins being at their cap; said until the next key
    full: bool,
}

// the ring's stops on an entry, in the order Right and Tab go
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Stop {
    #[default]
    Entry,
    Pin,
    Delete,
}

impl Stop {
    fn next(self) -> Stop {
        match self {
            Stop::Entry => Stop::Pin,
            Stop::Pin | Stop::Delete => Stop::Delete,
        }
    }

    fn previous(self) -> Stop {
        match self {
            Stop::Entry | Stop::Pin => Stop::Entry,
            Stop::Delete => Stop::Pin,
        }
    }
}

// an entry's copy, from its press until it is on the clipboard or failed
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Copying {
    #[default]
    Idle,
    Waiting,
    Failed,
}

impl Service for Search {
    fn new() -> Self {
        Search::default()
    }

    fn listen() {}
}

// what the ring presses; a copy holds the entry as it was pressed, which the history may drop meanwhile
#[derive(Debug, Clone, PartialEq, Eq)]
enum Press {
    Copy(Entry),

    // pins the entry, or unpins it if it is
    Pin(u64),
    Remove(u64),
    Clear,
}

impl Visited for Search {
    fn visit(&self) -> u64 {
        self.visit
    }

    fn begin(visit: u64) -> Self {
        Search {
            visit,
            ..Search::default()
        }
    }
}

impl Search {
    /*
     * the search after a key and whether it presses what the ring is on, none when the key is not
     * for the Surface. `count` is how many entries the query finds and `any` whether the history
     * has any to clear (all but the pinned), before the key. Space types, as in the Launcher, so
     * only Enter presses
     */
    fn step(mut self, key: Key, count: usize, any: bool) -> Option<(Search, bool)> {
        let last = count.saturating_sub(1);

        match key {
            Key::Character(letter) => return Some((self.typed(Some(letter)), false)),
            Key::Space => return Some((self.typed(Some(' ')), false)),
            Key::Backspace => return Some((self.typed(None), false)),
            Key::Enter => {
                let presses = if self.clear { any } else { count > 0 };

                return Some((self, presses));
            }
            Key::Up if self.clear && count > 0 => {
                self.clear = false;
                self.selected = last;
            }
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down if self.clear => {}
            Key::Down if self.selected < last => self.selected += 1,
            Key::Down => self.clear = any,
            Key::Left if !self.clear => self.stop = self.stop.previous(),
            Key::Right if !self.clear && count > 0 => self.stop = self.stop.next(),
            Key::Left | Key::Right => {}
            Key::Tab if self.clear => {
                self.clear = false;
                self.selected = 0;
                self.stop = Stop::Entry;
            }
            Key::Tab if self.stop != Stop::Delete && count > 0 => self.stop = self.stop.next(),
            Key::Tab if self.selected < last => {
                self.selected += 1;
                self.stop = Stop::Entry;
            }
            Key::Tab => {
                self.clear = any;
                self.stop = Stop::Entry;
            }
            Key::Home => {
                self.clear = false;
                self.selected = 0;
            }
            Key::End => {
                self.clear = false;
                self.selected = last;
            }
            _ => return None,
        }

        if !self.clear {
            self.offset = reveal(self.offset.clamp(0.0, most(count)), self.selected);
        }

        Some((self, false))
    }

    // a letter typed, or the last one erased; a new query finds new entries, so it starts at the top
    fn typed(mut self, letter: Option<char>) -> Search {
        match letter {
            Some(letter) => self.query.push(letter),
            None => {
                self.query.pop();
            }
        }

        self.selected = 0;
        self.stop = Stop::Entry;
        self.clear = false;
        self.offset = 0.0;

        self
    }

    // scrolled by the wheel, the selection kept on a row that shows whole
    fn wheel(mut self, lines: f32, count: usize) -> Search {
        let most = most(count);

        self.offset = (self.offset.clamp(0.0, most) + lines * WHEEL).clamp(0.0, most);

        let (first, last) = whole(self.offset, count);
        self.selected = self.selected.clamp(first, last);

        self
    }

    /*
     * what the view draws; fewer entries found leave the selection on the last, and a history of
     * only pins the ring on nothing, as there is nothing to clear
     */
    fn bounded(mut self, count: usize, any: bool) -> Search {
        self.selected = self.selected.min(count.saturating_sub(1));

        if count == 0 {
            self.stop = Stop::Entry;
        }

        self.clear = self.clear && any;
        self.offset = self.offset.clamp(0.0, most(count));

        self
    }

    /*
     * the ring on the entry `id` among the entries found, at its pin: a pin or an unpin moves the
     * entry (to the top of the pins, or of the rest), and the ring goes with it. Where it is no
     * longer found, the ring stays on the entry at its place, or the last
     */
    fn following(mut self, found: &[Entry], id: u64) -> Search {
        match found.iter().position(|entry| entry.id == id) {
            Some(at) => {
                self.selected = at;
                self.stop = Stop::Pin;
                self.clear = false;
                self.offset = reveal(self.offset.clamp(0.0, most(found.len())), at);
            }
            None => {
                self.selected = self.selected.min(found.len().saturating_sub(1));
                self.stop = Stop::Entry;
                self.offset = self.offset.clamp(0.0, most(found.len()));
            }
        }

        self
    }

    // what Enter presses, from the entries the query finds; Clear all is held to act, and `any` is
    // whether there is anything for it to clear
    fn press(&self, found: &[Entry], any: bool) -> Option<Press> {
        if self.clear {
            return any.then_some(Press::Clear);
        }

        let entry = found.get(self.selected)?;

        Some(match self.stop {
            Stop::Entry => Press::Copy(entry.clone()),
            Stop::Pin => Press::Pin(entry.id),
            Stop::Delete => Press::Remove(entry.id),
        })
    }
}

// how far `count` rows can scroll: none while they fit
fn most(count: usize) -> f32 {
    (content(count) - room()).max(0.0)
}

fn content(count: usize) -> f32 {
    count as f32 * ROW + count.saturating_sub(1) as f32 * ROW_GAP
}

// how tall `count` entries found ask the body to be, or a state for none; the island caps it (ADR 0030)
fn asks(count: usize) -> f32 {
    let list = if count == 0 { STATE } else { content(count) };

    2.0 * INSET + FIELD + list + FOOTER + 2.0 * GAP
}

// the list as tall as its rows, scrolling once they reach past the room
fn listed(count: usize) -> f32 {
    content(count).min(room())
}

/*
 * tells the island how tall the Clipboard asks to be: a new visit lists the whole history, the one
 * open what its query found. After each change of the history or the search, never from a view
 */
pub fn fit() {
    let _ordered = super::fitting();
    let visit = IslandService::read().visit();

    let (fresh, open) = {
        let entries = Clipboard::read().entries().to_vec();
        let query = Search::read().of(visit).query;

        (
            asks(found(&entries, "").len()),
            asks(found(&entries, &query).len()),
        )
    };

    super::ask(Surface::Clipboard, fresh, Some((visit, open)));
}

// at start: the body follows the history and the search from now on
pub fn start() {
    service::watch::<Clipboard>(fit);
    service::watch::<Search>(fit);
    fit();
}

fn top(row: usize) -> f32 {
    row as f32 * (ROW + ROW_GAP)
}

// the least scroll from `offset` that shows `row` whole
fn reveal(offset: f32, row: usize) -> f32 {
    let top = top(row);

    offset.min(top).max(top + ROW - room())
}

// the rows any part of shows at `offset`, as a range; only these are built
fn shown(offset: f32, count: usize) -> (usize, usize) {
    let step = ROW + ROW_GAP;
    let first = (offset / step).floor() as usize;
    let end = ((offset + room()) / step).ceil() as usize;

    (first.min(count), end.min(count))
}

// the first and last rows that show whole at `offset`
fn whole(offset: f32, count: usize) -> (usize, usize) {
    let step = ROW + ROW_GAP;
    let first = (offset / step).ceil() as usize;
    let last = ((offset + room() + ROW_GAP) / step).floor() as usize;

    (
        first.min(count.saturating_sub(1)),
        last.saturating_sub(1).min(count.saturating_sub(1)),
    )
}

/*
 * whether an entry answers the query: every word of it, in any case, somewhere in the text, or in
 * an image's kind ("image png"). A query of only spaces finds every entry
 */
pub(super) fn matches(content: &Content, query: &str) -> bool {
    match content {
        Content::Text(text) => query.split_whitespace().all(|term| contains(text, term)),
        Content::Image { mime, .. } => {
            let kind = format!("image {}", mime.trim_start_matches("image/"));

            query.split_whitespace().all(|term| contains(&kind, term))
        }
    }
}

/*
 * whether `text` holds `term` in any case; a text may be megabytes, so an ASCII term, the usual
 * one, is found without lowering a copy of it
 */
fn contains(text: &str, term: &str) -> bool {
    if !term.is_ascii() {
        return text.to_lowercase().contains(&term.to_lowercase());
    }

    let term = term.as_bytes();

    text.as_bytes()
        .windows(term.len())
        .any(|window| window.eq_ignore_ascii_case(term))
}

/*
 * what a search last found, so the view, which runs every frame of a morph, searches again only
 * when the query or the history changed, and what each entry shows, by id, as an entry never changes
 */
#[derive(Default)]
struct Memo {
    query: String,
    ids: Vec<u64>,
    found: Vec<usize>,
    rows: HashMap<u64, Summary>,
}

static MEMO: LazyLock<Mutex<Memo>> = LazyLock::new(Mutex::default);

// tests that read the one memo take turns
#[cfg(test)]
pub(super) static MEMO_TURN: Mutex<()> = Mutex::new(());

// the entries the query finds, newest first
fn found(entries: &[Entry], query: &str) -> Vec<Entry> {
    found_at(entries, query)
        .into_iter()
        .map(|at| entries[at].clone())
        .collect()
}

/*
 * where in `entries` the query finds them, scanned again only when the query or the history
 * changed: the Launcher asks on every redraw, and a history holds up to 64 MiB
 */
pub(super) fn found_at(entries: &[Entry], query: &str) -> Vec<usize> {
    let mut memo = MEMO.locked();

    let same = memo.query == query
        && memo.ids.len() == entries.len()
        && memo
            .ids
            .iter()
            .zip(entries)
            .all(|(id, entry)| *id == entry.id);

    if !same {
        memo.query = query.to_owned();
        memo.ids = entries.iter().map(|entry| entry.id).collect();
        memo.found = (0..entries.len())
            .filter(|&at| matches(&entries[at].content, query))
            .collect();

        // what the history forgot is never shown again
        let ids: HashSet<u64> = memo.ids.iter().copied().collect();
        memo.rows.retain(|id, _| ids.contains(id));
    }

    memo.found.clone()
}

// what a row says about its entry
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Summary {
    pub(super) title: String,
    pub(super) detail: String,

    // the title says what the entry is rather than showing it, like "Image"
    named: bool,
}

pub(super) fn summary(entry: &Entry) -> Summary {
    let mut memo = MEMO.locked();

    memo.rows
        .entry(entry.id)
        .or_insert_with(|| summarize(&entry.content))
        .clone()
}

/*
 * text as its first words, its whitespace one space, and what it is: a link or file, else how many lines or
 * characters it has; an
 * image as its kind and size
 */
fn summarize(content: &Content) -> Summary {
    match content {
        Content::Text(text) => {
            let mut title = String::new();

            for word in text.split_whitespace() {
                if title.chars().count() >= PREVIEW {
                    break;
                }

                if !title.is_empty() {
                    title.push(' ');
                }

                title.extend(word.chars().take(PREVIEW));
            }

            let lines = text.lines().count();
            let trimmed = text.trim();
            let one = !trimmed.contains(char::is_whitespace);

            let detail = if lines > 1 {
                format!("{lines} lines")
            } else if one && trimmed.starts_with("file://") {
                String::from("File")
            } else if one && (trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
                String::from("Link")
            } else {
                plural(text.chars().count(), "character")
            };

            if title.is_empty() {
                Summary {
                    title: String::from("Blank text"),
                    detail,
                    named: true,
                }
            } else {
                Summary {
                    title,
                    detail,
                    named: false,
                }
            }
        }
        Content::Image { mime, bytes } => {
            let kind = mime.trim_start_matches("image/");
            let kind = kind.split(['+', ';']).next().unwrap_or(kind);

            Summary {
                title: String::from("Image"),
                detail: format!("{}, {}", kind.to_uppercase(), size(bytes.len())),
                named: true,
            }
        }
    }
}

fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

// in decimal units, like the file managers
fn size(bytes: usize) -> String {
    match bytes {
        0..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{:.1} kB", bytes as f64 / 1e3),
        _ => format!("{:.1} MB", bytes as f64 / 1e6),
    }
}

// the view's own read of the visit, so this never reads IslandService again
pub fn surface(monitor: &str, visit: u64) -> Rectangle {
    let entries = Clipboard::read().entries().to_vec();
    let search = Search::read().of(visit);
    let found = found(&entries, &search.query);
    let pinned = entries.iter().filter(|entry| entry.pinned).count();
    let search = search.bounded(found.len(), entries.len() > pinned);

    let list: Box<dyn Widget> = if entries.is_empty() {
        Box::new(state("Clipboard is empty", "What you copy shows here"))
    } else if found.is_empty() {
        Box::new(state(
            "No matching entries",
            &format!("Nothing found for \u{201c}{}\u{201d}", search.query.trim()),
        ))
    } else {
        Box::new(list(monitor, &found, &search))
    };

    let shape = view::largest();

    let sections = vec![
        Box::new(view::rise(0, field(&search.query))) as Box<dyn Widget>,
        Box::new(view::rise(1, list)),
        Box::new(view::rise(
            2,
            footer(&search, found.len(), entries.len(), pinned),
        )),
    ];

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::Region, "clipboard"))
        .child(Column::new(sections).width(width()).gap(GAP))
}

/*
 * a search icon, what is typed with the caret after it, or what to type faded; a query wider than
 * the field shows its end, where the typing is
 */
fn field(query: &str) -> Rectangle {
    let room = width() - 2.0 * FIELD_INSET - TARGET - ICON_GAP - CARET;

    let caret = Rectangle::new()
        .width(CARET)
        .height(QUERY + 4.0)
        .fill(theme::island().on_surface);

    let typed: Box<dyn Widget> = if query.is_empty() {
        Box::new(
            Row::new(children![
                caret,
                Text::new("Search clipboard")
                    .size(QUERY)
                    .color(theme::island().on_surface_variant)
                    .weight(theme::text::MEDIUM),
            ])
            .align(Center),
        )
    } else {
        Box::new(Row::new(children![tail(query, room), caret]).align(Center))
    };

    Rectangle::new()
        .width(width())
        .height(FIELD)
        .radius(FIELD / 2.0)
        .fill(theme::island().surface_container)
        .padding(Padding {
            top: 0.0,
            right: FIELD_INSET,
            bottom: 0.0,
            left: FIELD_INSET,
        })
        .align_child(Start, Center)
        .semantics(|| {
            Semantics::new(Role::TextInput, "search")
                .text(query)
                .focused(true)
        })
        .child(
            Row::new(vec![
                Box::new(
                    Rectangle::new()
                        .width(TARGET)
                        .height(TARGET)
                        .align_child(Center, Center)
                        .child(Icon::Search.on(16.0, theme::island().on_surface_variant)),
                ) as Box<dyn Widget>,
                typed,
            ])
            .gap(ICON_GAP)
            .align(Center),
        )
}

// the end of `query` that fits in `width`, an ellipsis before it when the start is cut
fn tail(query: &str, width: f32) -> Text {
    let text = |shown: &str| {
        Text::new(shown)
            .size(QUERY)
            .color(theme::island().on_surface)
            .weight(theme::text::MEDIUM)
    };
    let fits = |text: &Text| matches!(text.width(), Size::Fixed(natural) if natural <= width);

    let whole = text(query);

    if fits(&whole) {
        return whole;
    }

    let letters: Vec<char> = query.chars().collect();

    (1..letters.len())
        .map(|cut| {
            text(&format!(
                "\u{2026}{}",
                letters[cut..].iter().collect::<String>()
            ))
        })
        .find(fits)
        .unwrap_or_else(|| text("\u{2026}"))
}

// what it means, in the middle of where the rows go
fn state(title: &str, detail: &str) -> Rectangle {
    let detail = Text::new(detail)
        .size(theme::text::LABEL_SMALL)
        .color(theme::island().on_surface_variant)
        .weight(theme::text::MEDIUM);

    // centred while it fits, elided only when a long query does not, since elided text fills its
    // width
    let detail = match detail.width() {
        Size::Fixed(natural) if natural <= width() => detail,
        _ => detail.elide(),
    };

    Rectangle::new()
        .width(width())
        .height(STATE.min(room()))
        .align_child(Center, Center)
        .child(
            Column::new(children![
                Icon::Clipboard.draw(28.0),
                Text::new(title)
                    .size(theme::text::BODY)
                    .color(theme::island().on_surface)
                    .weight(theme::text::SEMIBOLD),
                detail,
            ])
            .gap(6.0)
            .align(Center),
        )
}

/*
 * the rows scrolled `offset` down, clipped to the list; a thumb in the inset says where while they
 * do not all fit
 */
fn list(monitor: &str, found: &[Entry], search: &Search) -> Stack {
    let (first, end) = shown(search.offset, found.len());

    // only the rows any part of shows get a preview written
    for entry in &found[first..end] {
        preview::request(entry);
    }

    let column = Column::new(
        found[first..end]
            .iter()
            .zip(first..)
            .map(|(entry, index)| {
                let ring = (index == search.selected && !search.clear).then_some(search.stop);

                Box::new(row(monitor, search.visit, entry, ring)) as Box<dyn Widget>
            })
            .collect(),
    )
    .width(width())
    .gap(ROW_GAP);

    let viewport = Rectangle::new()
        .width(width())
        .height(listed(found.len()))
        .clip()
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::List, "entries"))
        .on_scroll(|Scroll { y, .. }| wheel(y))
        .child(
            Rectangle::new()
                .width(width())
                .height(content(end - first))
                .align_child(Start, Start)
                .translate(0.0, top(first) - search.offset)
                .child(column),
        );

    let mut layers = children![viewport];

    let most = most(found.len());

    if most > 0.0 {
        let length = (room() * room() / content(found.len())).max(TARGET);
        let at = (room() - length) * search.offset / most;

        layers.push(Box::new(
            Rectangle::new()
                .width(3.0)
                .height(length)
                .radius(radius::HAIRLINE)
                .fill(theme::island().surface_container_high)
                .translate(width() + 7.0, at),
        ));
    }

    Stack::new(layers)
        .width(width())
        .height(listed(found.len()))
}

/*
 * the entry's kind or thumbnail, what it holds and how much, and a pin and a delete at the end.
 * `ring` is where the ring is when it is on this row: on the entry, its pin or its delete. Pressing
 * the entry copies it
 */
fn row(monitor: &str, visit: u64, entry: &Entry, ring: Option<Stop>) -> Rectangle {
    let summary = summary(entry);

    let title = Text::new(&summary.title)
        .size(theme::text::BODY)
        .color(if summary.named {
            theme::island().on_surface_variant
        } else {
            theme::island().on_surface
        })
        .weight(theme::text::SEMIBOLD)
        .elide();

    let detail = Text::new(&summary.detail)
        .size(theme::text::LABEL_SMALL)
        .color(theme::island().on_surface_variant)
        .weight(theme::text::MEDIUM)
        .elide();

    let row = Rectangle::new()
        .width(width())
        .height(ROW)
        .radius(radius::ROW)
        .padding(Padding {
            top: 0.0,
            right: ROW_INSET,
            bottom: 0.0,
            left: ROW_INSET,
        })
        .align_child(Start, Center)
        .cursor(Cursor::Pointer)
        .child(
            Row::new(children![
                mark(entry),
                Column::new(children![title, detail]).width(Parent).gap(1.0),
                pin(monitor, visit, entry, ring == Some(Stop::Pin)),
                delete(entry.id, ring == Some(Stop::Delete)),
            ])
            .width(Parent)
            .gap(ICON_GAP)
            .align(Center),
        );

    let row = if ring.is_some() {
        row.fill(theme::island().surface_container)
    } else {
        row
    };
    let row = row.border_if(ring == Some(Stop::Entry));

    let monitor = monitor.to_owned();
    let entry = entry.clone();

    row.identity(entry.id)
        .semantics(|| Semantics::new(Role::Item, "entry").content(summary.title.as_str()))
        .on_click(super::on_left(move || {
            run(&monitor, visit, Press::Copy(entry.clone()));
        }))
}

/*
 * what stands for the entry at the start of its row: the kind's icon, or for a picture its
 * thumbnail once the preview is written and decoded, a quiet tile while it decodes
 */
fn mark(entry: &Entry) -> Rectangle {
    let icon = |kind: Icon| {
        Rectangle::new()
            .width(ICON)
            .height(ICON)
            .align_child(Center, Center)
            .child(kind.on(20.0, theme::island().on_surface_variant))
    };

    let Content::Image { .. } = entry.content else {
        return icon(Icon::Text);
    };

    let Some(path) = preview::path(entry.id) else {
        return icon(Icon::Picture);
    };

    let picture = Image::cover(path).thumbnail((ICON * SCALE) as u32, (ICON * SCALE) as u32);

    let tile = Rectangle::new()
        .width(ICON)
        .height(ICON)
        .radius(THUMB_RADIUS)
        .fill(theme::island().surface_container_high);

    match picture.state() {
        ImageState::Ready => tile.fill(picture),
        ImageState::Loading => tile,
        ImageState::Failed => icon(Icon::Picture),
    }
}

// a round target with a pin, filled while the entry is pinned; pressed to pin it or unpin it
fn pin(monitor: &str, visit: u64, entry: &Entry, ring: bool) -> Rectangle {
    let (id, pinned) = (entry.id, entry.pinned);
    let monitor = monitor.to_owned();

    let (icon, ink) = if pinned {
        (Icon::Pinned, theme::island().on_surface)
    } else {
        (Icon::Pin, theme::island().on_surface_variant)
    };

    Rectangle::new()
        .width(TARGET)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .semantics(|| Semantics::new(Role::Button, "pin").checked(pinned))
        .on_click(super::on_left(move || {
            run(&monitor, visit, Press::Pin(id));
        }))
        .child(icon.on(20.0, ink))
        .border_if(ring)
}

// a round target with a cross, pressed to forget the entry
fn delete(id: u64, ring: bool) -> Rectangle {
    Rectangle::new()
        .width(TARGET)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .semantics(|| Semantics::new(Role::Button, "delete"))
        .on_click(super::on_left(move || clipboard::remove(id)))
        .child(Icon::Dismiss.on(20.0, theme::island().on_surface_variant))
        .border_if(ring)
}

// what Clear all clears: every entry, or all but the pinned, which it says it keeps
fn clearing(all: usize, pinned: usize) -> String {
    if pinned == 0 {
        format!("all {all}")
    } else {
        format!("{}, keeping {pinned} pinned", all - pinned)
    }
}

/*
 * how many entries there are, of how many while searching, and Clear all, which clears the whole
 * history but the pinned whatever is found, faded while there is nothing to clear
 */
fn footer(search: &Search, found: usize, all: usize, pinned: usize) -> Row {
    let clearable = all - pinned;

    let count = match all {
        _ if search.copying == Copying::Failed => String::from("The entry was not copied"),
        _ if search.full => format!("Pins are full: at most {PINS}, {} MiB", PIN_BYTES >> 20),
        _ if confirm::fill(CLEAR) > 0.0 => {
            format!("Keep holding to clear {}", clearing(all, pinned))
        }
        _ if search.clear => format!("Hold Enter to clear {}, no undo", clearing(all, pinned)),
        0 => String::from("0 entries"),
        _ if !search.query.trim().is_empty() => format!("{found} of {all}"),
        1 if pinned > 0 => String::from("1 entry, pinned"),
        all if pinned > 0 => format!("{all} entries, {pinned} pinned"),
        1 => String::from("1 entry"),
        all => format!("{all} entries"),
    };

    let ink = theme::island().on_surface;

    let label = Rectangle::new()
        .width(PILL)
        .height(FOOTER)
        .align_child(Center, Center)
        .child(
            Text::new("Hold to clear")
                .size(theme::text::LABEL_SMALL)
                .color(ink)
                .weight(theme::text::SEMIBOLD),
        );

    let pill = Rectangle::new()
        .width(PILL)
        .height(FOOTER)
        .radius(FOOTER / 2.0)
        .clip()
        .child(confirm::behind(
            CLEAR,
            PILL,
            FOOTER,
            theme::signals().critical,
            label,
        ));

    let pill = if search.clear {
        pill.border(RING, ink)
    } else {
        pill.border(1.0, theme::island().surface_container_high)
    };

    // kept for the hold, so a press by name, which is a tap, clears nothing
    let pill = pill.semantics(|| Semantics::new(Role::Button, "clear").disabled(clearable == 0));

    let pill = if clearable > 0 {
        confirm::target(
            pill.cursor(Cursor::Pointer),
            CLEAR,
            Arc::new(clipboard::clear),
        )
    } else {
        pill.opacity(DISABLED)
    };

    Row::new(children![
        Text::new(count)
            .size(theme::text::LABEL_SMALL)
            .color(theme::island().on_surface_variant)
            .weight(theme::text::MEDIUM),
        pill,
    ])
    .width(width())
    .height(FOOTER)
    .justify(SpaceBetween)
    .align(Center)
}

/*
 * a key while this island shows the Clipboard; false for one it does not use, which the window's
 * own keys then get. A key it uses keeps the held island open for another hold
 */
pub fn key(monitor: &str, key: Key) -> bool {
    let visit = {
        let island = IslandService::read();

        if island.presentation(monitor) != Presentation::Expanded(Surface::Clipboard) {
            return false;
        }

        island.visit()
    };

    let entries = Clipboard::read().entries().to_vec();
    let any = entries.iter().any(|entry| !entry.pinned);
    let search = Search::read().of(visit);
    let found = found(&entries, &search.query);

    let Some((mut search, presses)) = search.bounded(found.len(), any).step(key, found.len(), any)
    else {
        return false;
    };

    let press = presses.then(|| search.press(&found, any)).flatten();

    // a failed copy and a refused pin are said until the next key
    if search.copying == Copying::Failed {
        search.copying = Copying::Idle;
    }

    search.full = false;

    store(search);

    IslandService::write().attend(monitor, Instant::now());

    if let Some(press) = press {
        run(monitor, visit, press);
    }

    true
}

/*
 * a copied entry goes back on the clipboard to be pasted elsewhere, so the island closes out of
 * the way once it is there; a pin, a delete or Clear all leaves it open on what is left. One copy at
 * a time
 */
fn run(monitor: &str, visit: u64, press: Press) {
    match press {
        Press::Copy(entry) => {
            let search = Search::read().of(visit);

            if search.copying == Copying::Waiting {
                return;
            }

            store(Search {
                copying: Copying::Waiting,
                ..search
            });

            let monitor = monitor.to_owned();
            clipboard::restore(&entry, move |done| copied(&monitor, visit, done));
        }
        Press::Pin(id) => toggle(visit, id),
        Press::Remove(id) => clipboard::remove(id),
        Press::Clear => confirm::press(CLEAR, Arc::new(clipboard::clear)),
    }
}

/*
 * pins the entry, or unpins it if it is; a pin past the caps is refused, which the footer says until
 * the next key
 */
fn toggle(visit: u64, id: u64) {
    let pinned = Clipboard::read()
        .entries()
        .iter()
        .any(|entry| entry.id == id && entry.pinned);

    let full = if pinned {
        clipboard::unpin(id);
        false
    } else {
        clipboard::pin(id).is_err()
    };

    // read apart, since `store` writes; the entry moved, so the ring follows it
    let search = Search::read().of(visit);
    let found = found(Clipboard::read().entries(), &search.query);

    store(Search {
        full,
        ..search.following(&found, id)
    });
}

/*
 * how the copy pressed in `visit` went, on the clipboard's thread: closes the island, or says it
 * was not copied. Nothing once that visit is over, so it never closes another
 */
fn copied(monitor: &str, visit: u64, done: bool) {
    let open = {
        let island = IslandService::read();

        island.visit() == visit
            && island.presentation(monitor) == Presentation::Expanded(Surface::Clipboard)
    };

    if !open {
        return;
    }

    if done {
        view::collapse(monitor);
        return;
    }

    // read apart, since `store` writes
    let search = Search::read().of(visit);

    store(Search {
        copying: Copying::Failed,
        ..search
    });
}

// down scrolls further down the list
fn wheel(lines: f32) {
    let visit = IslandService::read().visit();

    let entries = Clipboard::read().entries().to_vec();
    let search = Search::read().of(visit);
    let count = found(&entries, &search.query).len();
    let any = entries.iter().any(|entry| !entry.pinned);

    store(search.bounded(count, any).wheel(lines, count));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u64, text: &str) -> Entry {
        Entry {
            id,
            content: Content::Text(text.into()),
            pinned: false,
        }
    }

    // the Launcher asks on every redraw: an unchanged history and query are not scanned again, and
    // what the history forgot loses its summary
    #[test]
    fn a_search_scans_only_when_the_query_or_history_changed() {
        let _turn = MEMO_TURN.locked();
        let entries = [entry(7, "gone soon"), entry(8, "stays")];

        for entry in &entries {
            summary(entry);
        }

        assert_eq!(found_at(&entries, "stays"), [1]);

        // same ids and query: the answer comes from the memo, not from the text
        let changed = [entry(7, "stays"), entry(8, "other")];
        assert_eq!(found_at(&changed, "stays"), [1]);

        assert_eq!(found_at(&entries[1..], "stays"), [0]);

        let memo = MEMO.locked();
        assert!(!memo.rows.contains_key(&7));
        assert!(memo.rows.contains_key(&8));
    }
}
