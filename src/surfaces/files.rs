//! The Files Surface: the desktop's recent files as a shelf (ADR 0141), searched by name and folder
//! as it is typed. It opens from IPC, a keybind or the Launcher, so it always holds the keyboard,
//! like the Clipboard: typing searches, the arrow keys move a ring, and Enter opens the file with
//! `xdg-open`, through the Launcher's own opener, so an address that begins with a dash is refused
//! there rather than run. Right opens the folder it is in, and Ctrl+R reads the shelf again.
//!
//! It is a shelf, never a manager: nothing is deleted, renamed, moved or copied, and no file is read
//! beyond its name, its folder and when it was last written. The shelf is read when the Surface
//! opens, off the draw thread (`sources::shelf`), never on a frame.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::Instant;

use kanade_runtime::service::{self, Service};
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Column, Cursor, Key, Padding, Parent, Rectangle, Row, Scroll, Stack, Start, Text,
    Widget, children,
};

use crate::icon::Icon;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::sources::shelf::{self, Recent, Shelf};
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, radius};
use crate::view;

use super::launcher::web;
use super::search::{self, FIELD};
use super::visit::Visited;
use super::{Outline, store};

// the content's width, which every row fills, as wide as the largest body (`island.width`)
fn width() -> f32 {
    view::largest().width - 2.0 * INSET
}

const FOOTER: f32 = 28.0;
const GAP: f32 = 12.0;

// what the rows scroll in, as tall as the largest body (`island.height`) leaves
fn room() -> f32 {
    view::largest().height - 2.0 * INSET - FIELD - FOOTER - 2.0 * GAP
}

// what the empty state takes of the list
const STATE: f32 = 120.0;

const ROW: f32 = 40.0;
const ROW_GAP: f32 = 2.0;
const ROW_INSET: f32 = 10.0;

// the folder target at the end of a row, and the mark at its start
const FOLDER: f32 = TARGET;
const MARK: f32 = 20.0;
const MARK_GAP: f32 = 12.0;

// pixels per wheel line
const WHEEL: f32 = 40.0;

// how wide the footer's Refresh pill is
const PILL: f32 = 84.0;

/*
 * the query, the ring and the scroll for one visit of the Surface (`IslandService::visit`), so
 * every opening starts empty at the top. Written by input only, never by the view
 */
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lookup {
    visit: u64,
    query: String,

    // in the files the query finds
    selected: usize,

    // how far the rows scrolled, in pixels
    offset: f32,

    // why the last file was not opened, said until the next key; it never names the file
    failed: Option<String>,
}

impl Service for Lookup {
    fn new() -> Self {
        Lookup::default()
    }

    fn listen() {}
}

impl Visited for Lookup {
    fn visit(&self) -> u64 {
        self.visit
    }

    fn begin(visit: u64) -> Self {
        Lookup {
            visit,
            ..Lookup::default()
        }
    }
}

// what a key asks of the file the ring is on
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Press {
    File,
    Folder,
}

impl Lookup {
    // a letter typed, or the last one erased; a new query finds other files, so it starts at the top
    fn typed(mut self, letter: Option<char>) -> Lookup {
        match letter {
            Some(letter) => self.query.push(letter),
            None => {
                self.query.pop();
            }
        }

        self.selected = 0;
        self.offset = 0.0;
        self.failed = None;

        self
    }

    // the lookup after a key, and what it presses, none for a key the Surface does not use
    fn step(mut self, key: Key, count: usize, content: f32) -> Option<(Lookup, Option<Press>)> {
        let last = count.saturating_sub(1);

        self.failed = None;

        match key {
            Key::Character(letter) => return Some((self.typed(Some(letter)), None)),
            Key::Space => return Some((self.typed(Some(' ')), None)),
            Key::Backspace => return Some((self.typed(None), None)),
            Key::Enter if count > 0 => return Some((self, Some(Press::File))),
            Key::Right if count > 0 => return Some((self, Some(Press::Folder))),
            Key::Enter | Key::Right | Key::Left | Key::PageUp | Key::PageDown => {}
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down if self.selected < last => self.selected += 1,
            Key::Tab if self.selected < last => self.selected += 1,
            Key::Tab | Key::Home => self.selected = 0,
            Key::End => self.selected = last,
            _ => return None,
        }

        self.offset = reveal(self.offset, self.selected, content);

        Some((self, None))
    }

    // scrolled by the wheel, the selection kept on a row that shows whole
    fn wheel(mut self, lines: f32, count: usize, content: f32) -> Lookup {
        self.offset = (self.offset + lines * WHEEL).clamp(0.0, most(content));

        let (first, last) = whole(self.offset, count);
        self.selected = self.selected.clamp(first, last);

        self
    }

    // what the view draws: fewer files found leave the selection on the last
    fn bounded(mut self, count: usize, content: f32) -> Lookup {
        self.selected = self.selected.min(count.saturating_sub(1));
        self.offset = self.offset.clamp(0.0, most(content));

        self
    }
}

fn content(count: usize) -> f32 {
    if count == 0 {
        0.0
    } else {
        count as f32 * ROW + (count - 1) as f32 * ROW_GAP
    }
}

// how far `count` rows can scroll: none while they fit
fn most(content: f32) -> f32 {
    (content - room()).max(0.0)
}

fn top(row: usize) -> f32 {
    row as f32 * (ROW + ROW_GAP)
}

// the least scroll from `offset` that shows `row` whole
fn reveal(offset: f32, row: usize, content: f32) -> f32 {
    let top = top(row);

    offset
        .min(top)
        .max(top + ROW - room())
        .clamp(0.0, most(content))
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

// the files the query finds, in the shelf's own order
fn found(files: &[Recent], query: &str) -> Vec<Recent> {
    files
        .iter()
        .filter(|file| file.matches(query))
        .cloned()
        .collect()
}

// the view's own read of the visit, so this never reads IslandService again
pub fn surface(_monitor: &str, visit: u64) -> Rectangle {
    let shelf = Shelf::read().clone();
    let lookup = Lookup::read().of(visit);
    let files = found(&shelf.files, &lookup.query);
    let content = content(files.len());
    let lookup = lookup.bounded(files.len(), content);

    let list: Box<dyn Widget> = if shelf.files.is_empty() {
        Box::new(search::state(
            Icon::Folder,
            "No recent files",
            "The files your desktop lists as recent show here",
            width(),
            STATE.min(room()),
        ))
    } else if files.is_empty() {
        Box::new(search::state(
            Icon::Search,
            "No matching files",
            &format!("Nothing found for \u{201c}{}\u{201d}", lookup.query.trim()),
            width(),
            STATE.min(room()),
        ))
    } else {
        Box::new(list(&files, &lookup, content))
    };

    let shape = view::largest();

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::Region, "files"))
        .child(
            Column::new(vec![
                Box::new(view::rise(
                    0,
                    search::field(Icon::Search, &lookup.query, "Search recent files", width()),
                )) as Box<dyn Widget>,
                Box::new(view::rise(1, list)),
                Box::new(view::rise(
                    2,
                    footer(&lookup, files.len(), shelf.files.len()),
                )),
            ])
            .width(width())
            .gap(GAP),
        )
}

/*
 * the rows scrolled `offset` down, clipped to the list; a thumb in the inset says where while they
 * do not all fit
 */
fn list(files: &[Recent], lookup: &Lookup, content: f32) -> Stack {
    let (first, end) = shown(lookup.offset, files.len());
    let listed = content.min(room());

    let rows: Vec<Box<dyn Widget>> = files[first..end]
        .iter()
        .zip(first..)
        .map(|(file, at)| {
            let fade = super::edge_fade(top(at), ROW, lookup.offset, room());

            Box::new(
                row(file, at == lookup.selected, lookup.visit)
                    .opacity(fade)
                    .translate(0.0, top(at) - lookup.offset),
            ) as Box<dyn Widget>
        })
        .collect();

    let viewport = Rectangle::new()
        .width(width())
        .height(listed)
        .clip()
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::List, "files"))
        .on_scroll(|Scroll { y, .. }| wheel(y))
        .child(Stack::new(rows).width(width()).height(listed));

    let mut layers = children![viewport];
    let most = most(content);

    if most > 0.0 {
        let length = (room() * room() / content).max(TARGET);
        let at = (room() - length) * lookup.offset / most;

        layers.push(Box::new(
            Rectangle::new()
                .width(3.0)
                .height(length)
                .radius(radius::HAIRLINE)
                .fill(theme::island().surface_container_high)
                .translate(width() + 7.0, at),
        ));
    }

    Stack::new(layers).width(width()).height(listed)
}

/*
 * the file's name, the folder it is in, and a folder target that opens that folder; `ring` is true on
 * the row the keyboard is on. Pressing the row opens the file
 */
fn row(file: &Recent, ring: bool, visit: u64) -> Rectangle {
    let path = file.path.clone();
    let folder = parent(&file.path);

    Rectangle::new()
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
        .border_if(ring)
        .identity(&file.path)
        .semantics(|| Semantics::new(Role::Item, "file").content(file.name.as_str()))
        .on_click(super::on_left(move || open(&path, visit)))
        .child(
            Row::new(children![
                Icon::Text.on(MARK, theme::island().on_surface_variant),
                Column::new(children![
                    Text::new(&file.name)
                        .size(theme::text::BODY)
                        .color(theme::island().on_surface)
                        .elide(),
                    Text::new(&file.folder)
                        .size(theme::text::LABEL_SMALL)
                        .color(theme::island().on_surface_variant)
                        .weight(theme::text::MEDIUM)
                        .elide(),
                ])
                .width(Parent)
                .gap(1.0),
                folder_target(folder, visit),
            ])
            .width(Parent)
            .gap(MARK_GAP)
            .align(Center),
        )
}

// the round target that opens the folder the row is in, so the pointer has what Right has
fn folder_target(folder: PathBuf, visit: u64) -> Rectangle {
    Rectangle::new()
        .width(FOLDER)
        .height(FOLDER)
        .radius(FOLDER / 2.0)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .semantics(|| Semantics::new(Role::Button, "folder"))
        .on_click(super::on_left(move || open(&folder, visit)))
        .child(Icon::Folder.on(MARK, theme::island().on_surface_variant))
}

fn parent(path: &Path) -> PathBuf {
    path.parent()
        .map_or_else(|| path.to_path_buf(), Path::to_path_buf)
}

// how many files there are, of how many while searching, and Refresh
fn footer(lookup: &Lookup, found: usize, all: usize) -> Row {
    let count = match &lookup.failed {
        Some(why) => format!("Not opened: {why}"),
        None if all == 0 => String::from("No recent files"),
        None if lookup.query.trim().is_empty() => match all {
            1 => String::from("1 recent file"),
            all => format!("{all} recent files"),
        },
        None => format!("{found} of {all}"),
    };

    let label = Rectangle::new()
        .width(PILL)
        .height(FOOTER)
        .align_child(Center, Center)
        .child(
            Text::new("Refresh")
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface)
                .weight(theme::text::SEMIBOLD),
        );

    let pill = super::pill_face(
        Rectangle::new()
            .width(PILL)
            .height(FOOTER)
            .radius(FOOTER / 2.0),
    )
    .cursor(Cursor::Pointer)
    .semantics(|| Semantics::new(Role::Button, "refresh"))
    .on_click(super::on_left(shelf::refresh))
    .child(label);

    Row::new(children![
        Text::new(count)
            .size(theme::text::LABEL_SMALL)
            .color(theme::island().on_surface_variant)
            .weight(theme::text::MEDIUM),
        pill,
    ])
    .width(width())
    .height(FOOTER)
    .justify(kanade_runtime::SpaceBetween)
    .align(Center)
}

// down scrolls further down the list
fn wheel(lines: f32) {
    let visit = IslandService::read().visit();
    let lookup = Lookup::read().of(visit);
    let count = found(&Shelf::read().files, &lookup.query).len();
    let content = content(count);

    store(lookup.bounded(count, content).wheel(lines, count, content));
}

/*
 * a key while this island shows the Files: `r` with ctrl held reads the shelf again, and false for a
 * key the Surface does not use, which the window's own keys then get
 */
pub fn key(monitor: &str, key: Key) -> bool {
    let visit = {
        let island = IslandService::read();

        if island.presentation(monitor) != Presentation::Expanded(Surface::Files) {
            return false;
        }

        island.visit()
    };

    // the shelf is read to answer the key, and let go of before the Lookup and the island are
    // written
    let (lookup, press) = {
        let shelf = Shelf::read();
        let lookup = Lookup::read().of(visit);
        let files = found(&shelf.files, &lookup.query);
        let content = content(files.len());
        let count = files.len();

        if key == Key::Shortcut('r') {
            drop(shelf);
            shelf::refresh();
            store(Lookup {
                failed: None,
                ..lookup.of(visit)
            });
            IslandService::write().attend(monitor, Instant::now());

            return true;
        }

        let Some((lookup, press)) = lookup.bounded(count, content).step(key, count, content) else {
            return false;
        };

        let press = press.map(|press| match press {
            Press::File => files[lookup.selected].path.clone(),
            Press::Folder => parent(&files[lookup.selected].path),
        });

        (lookup, press)
    };

    store(lookup);

    IslandService::write().attend(monitor, Instant::now());

    if let Some(path) = press {
        open(&path, visit);
    }

    true
}

/*
 * opens a file, or the folder it is in, with `xdg-open` through the Launcher's own opener, so the
 * dash guard and the argv with no shell are the Launcher's (ADR 0130, 0141). Off the draw thread, as
 * an opener may live as long as the app it started; the Surface says it was not opened if it was not
 */
fn open(path: &Path, visit: u64) {
    let address = String::from(path.to_string_lossy());

    // a path that is not absolute is no address of this machine's, and one that begins with a dash
    // is an option of the opener's own, which `browse` refuses
    if !address.starts_with('/') {
        failed(visit, String::from("not a path this machine holds"));

        return;
    }

    let opened = thread::Builder::new()
        .name(String::from("files-open"))
        .spawn(move || {
            if let Err(why) = web::browse(&address) {
                failed(visit, why);
            }
        });

    if let Err(error) = opened {
        failed(visit, error.to_string());
    }
}

// why the file was not opened, for the footer, until the next key; nothing once that visit is over
fn failed(visit: u64, why: String) {
    let lookup = Lookup::read().of(visit);

    if lookup.visit == visit {
        store(Lookup {
            failed: Some(why),
            ..lookup
        });
    }
}

/*
 * at start: the shelf is read once whenever the Surface opens, on a thread of its own. The watcher
 * runs before a draw, so it must not read the list itself
 */
pub fn start() {
    service::watch::<IslandService>(follow);
}

fn follow() {
    let open = {
        let island = IslandService::read();

        (island.surface(), island.visit())
    };

    if let (Some(Surface::Files), visit) = open {
        shelf::opened(visit);
    }
}
