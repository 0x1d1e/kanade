//! The Wallpapers Surface: the wallpaper folder (`wallpaper.directory`) as a grid of pictures,
//! searched by file name as it is typed. It opens only from IPC, a keybind or the Launcher, so it
//! always holds the keyboard, like the Clipboard: typing searches, the arrow keys move a ring over
//! the pictures and Shuffle below them, and Enter presses what the ring is on.
//!
//! Each picture is a small copy the runtime decodes off the draw thread and keeps in a bounded
//! cache, a quiet tile until it is ready; one it cannot decode shows a crossed picture. The
//! wallpaper showing is marked. Pressing a picture sets it through `wallpaper::set`, so Matugen
//! follows, a circle transition growing from the picture pressed; the Surface stays open to try
//! another, and says when one was not set.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Cursor, End, Image, ImageState, Key, Parent, Rectangle, Row, Scroll, Stack, Start,
    Text, Widget, children,
};
use kanade_runtime::{
    Column, MutexExt,
    service::{Service, Write},
};

use crate::icon::Icon;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::sources::wallpaper::{self, From, Output, Unset};
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, radius};
use crate::view;

use super::grid::{self, Place};
use super::search::{self, FIELD};
use super::visit::Visited;
use super::{Outline, RING, store};

// the content's width, which the grid fills, as wide as the largest body (`island.width`)
fn width() -> f32 {
    view::largest().width - 2.0 * INSET
}

const FOOTER: f32 = 28.0;
const GAP: f32 = 12.0;

// what the grid scrolls in, as tall as the largest body (`island.height`) leaves
fn room() -> f32 {
    view::largest().height - 2.0 * INSET - FIELD - FOOTER - 2.0 * GAP
}

// a picture is at least this wide, so a wider body fits more of them in a row, at least `LEAST`
const LEAST_TILE: f32 = 140.0;
const LEAST: usize = 2;
const TILE_GAP: f32 = 10.0;

// the pictures are as a screen is, 16 by 10
const ASPECT: f32 = 10.0 / 16.0;

// decoded at twice their size, crisp at scale 2
const SCALE: f32 = 2.0;

// the mark on the wallpaper showing, in its corner
const BADGE: f32 = 20.0;
const BADGE_INSET: f32 = 6.0;

// Shuffle, after the count in the footer
const SHUFFLE: f32 = 84.0;

// pixels per wheel line
const WHEEL: f32 = 40.0;

// how many pictures a row holds
fn columns() -> usize {
    (((width() + TILE_GAP) / (LEAST_TILE + TILE_GAP)).floor() as usize).max(LEAST)
}

fn tile_width() -> f32 {
    let columns = columns() as f32;

    (width() - (columns - 1.0) * TILE_GAP) / columns
}

fn tile_height() -> f32 {
    (tile_width() * ASPECT).round()
}

fn step() -> f32 {
    tile_height() + TILE_GAP
}

fn rows(count: usize) -> usize {
    count.div_ceil(columns())
}

fn content(count: usize) -> f32 {
    let rows = rows(count);

    rows as f32 * tile_height() + rows.saturating_sub(1) as f32 * TILE_GAP
}

// how far `count` pictures can scroll: none while they fit
fn most(count: usize) -> f32 {
    (content(count) - room()).max(0.0)
}

fn top(row: usize) -> f32 {
    row as f32 * step()
}

// the least scroll from `offset` that shows `row` whole
fn reveal(offset: f32, row: usize) -> f32 {
    let top = top(row);

    offset.min(top).max(top + tile_height() - room())
}

// the rows any part of shows at `offset`, as a range; only these are built
fn shown(offset: f32, count: usize) -> (usize, usize) {
    let rows = rows(count);
    let first = (offset / step()).floor() as usize;
    let end = ((offset + room()) / step()).ceil() as usize;

    (first.min(rows), end.min(rows))
}

// the first and last rows that show whole at `offset`
fn whole(offset: f32, count: usize) -> (usize, usize) {
    let last = rows(count).saturating_sub(1);
    let first = (offset / step()).ceil() as usize;
    let end = ((offset + room() + TILE_GAP) / step()).floor() as usize;

    (first.min(last), end.saturating_sub(1).min(last))
}

/*
 * the query, the ring and the scroll for one visit of the Surface (`IslandService::visit`), so every
 * opening starts on the wallpaper showing. Written by input, and by a set telling how it went
 */
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pick {
    visit: u64,
    query: String,

    // in the pictures the query finds; none is the one showing, else the first
    selected: Option<usize>,

    // the ring is on Shuffle, below the grid
    shuffle: bool,

    // how far the grid scrolled, in pixels
    offset: f32,

    setting: Setting,

    // the sets asked from this visit, and the last one that settled: awww may settle one before
    // its press writes Waiting, and an older one after a newer press
    asked: u64,
    settled: u64,
}

// the last set asked from this visit, from its press until awww showed it or failed
#[derive(Debug, Clone, Default, PartialEq)]
enum Setting {
    #[default]
    Idle,
    Waiting(PathBuf),

    // why, in a few words
    Failed(String),
}

impl Service for Pick {
    fn new() -> Self {
        Pick::default()
    }

    fn listen() {}
}

impl Visited for Pick {
    fn visit(&self) -> u64 {
        self.visit
    }

    fn begin(visit: u64) -> Self {
        Pick {
            visit,
            ..Pick::default()
        }
    }
}

// what the ring is on: a picture, by its place in what the query finds, or Shuffle
#[derive(Debug, Clone, Copy, PartialEq)]
enum At {
    Tile(usize),
    Shuffle,
}

// what Enter or a click presses
#[derive(Debug, Clone, PartialEq)]
enum Press {
    Set(PathBuf, usize),
    Shuffle,
}

impl Pick {
    /*
     * what the view draws for `found`, with `current` showing: the selection on a picture found,
     * the one showing until a key moves it, scrolled to show it whole
     */
    fn bounded(mut self, found: &[PathBuf], current: &[PathBuf]) -> Pick {
        let count = found.len();

        let selected = self
            .selected
            .or_else(|| found.iter().position(|path| current.contains(path)))
            .unwrap_or(0)
            .min(count.saturating_sub(1));

        self.selected = Some(selected);
        self.shuffle = self.shuffle || count == 0;
        self.offset = self.offset.clamp(0.0, most(count));

        if !self.shuffle {
            self.offset = reveal(self.offset, selected / columns()).clamp(0.0, most(count));
        }

        self
    }

    fn at(&self) -> At {
        if self.shuffle {
            At::Shuffle
        } else {
            At::Tile(self.selected.unwrap_or(0))
        }
    }

    fn put(mut self, at: At, count: usize) -> Pick {
        match at {
            At::Shuffle => self.shuffle = true,
            At::Tile(index) => {
                self.shuffle = false;
                self.selected = Some(index);
                self.offset = reveal(self.offset, index / columns()).clamp(0.0, most(count));
            }
        }

        self
    }

    /*
     * the pick after a key, already bounded, and whether it presses what the ring is on, none when
     * the key is not for the Surface. Space types, as in the Launcher, so only Enter presses
     */
    fn step(self, key: Key, count: usize) -> Option<(Pick, bool)> {
        match key {
            Key::Character(letter) => Some((self.typed(Some(letter)), false)),
            Key::Space => Some((self.typed(Some(' ')), false)),
            Key::Backspace => Some((self.typed(None), false)),
            Key::Enter => Some((self, true)),
            Key::Up | Key::Down | Key::Left | Key::Right | Key::Tab | Key::Home | Key::End => {
                let targets = targets(count);
                let at = self.at();

                // a key that moves nowhere is still the Surface's, so it never reaches the window
                let Some(place) = grid::find(&targets, &at) else {
                    return Some((self, false));
                };

                let to = grid::moved(place, key, &targets).unwrap_or(place);
                let Place { row, column } = to;

                Some((self.put(targets[row][column].0, count), false))
            }
            _ => None,
        }
    }

    // a letter typed, or the last one erased; a new query finds new pictures, so it starts at the top
    fn typed(mut self, letter: Option<char>) -> Pick {
        match letter {
            Some(letter) => self.query.push(letter),
            None => {
                self.query.pop();
            }
        }

        self.selected = Some(0);
        self.shuffle = false;
        self.offset = 0.0;

        self
    }

    // scrolled by the wheel, the selection kept on a row that shows whole
    fn wheel(mut self, lines: f32, count: usize) -> Pick {
        let most = most(count);

        self.offset = (self.offset.clamp(0.0, most) + lines * WHEEL).clamp(0.0, most);

        if let Some(selected) = self.selected
            && count > 0
        {
            let (first, last) = whole(self.offset, count);
            let columns = columns();
            let row = (selected / columns).clamp(first, last);

            self.selected = Some((row * columns + selected % columns).min(count - 1));
        }

        self
    }

    // what Enter presses, from the pictures the query finds
    fn press(&self, found: &[PathBuf]) -> Option<Press> {
        match self.at() {
            At::Shuffle => Some(Press::Shuffle),
            At::Tile(index) => found.get(index).map(|path| Press::Set(path.clone(), index)),
        }
    }
}

/*
 * the ring's targets as rows, each with where its middle is across from 0 to 1: the pictures a row
 * at a time, then Shuffle at the end of the footer
 */
fn targets(count: usize) -> Vec<Vec<(At, f32)>> {
    let columns = columns();

    let mut rows: Vec<Vec<(At, f32)>> = (0..count)
        .collect::<Vec<_>>()
        .chunks(columns)
        .map(|row| {
            row.iter()
                .map(|&index| {
                    let across = ((index % columns) as f32 + 0.5) / columns as f32;

                    (At::Tile(index), across)
                })
                .collect()
        })
        .collect();

    rows.push(vec![(At::Shuffle, 1.0)]);

    rows
}

// what a search last found, so the view, which runs every frame of a morph, searches again only
// when the query or the folder changed
#[derive(Default)]
struct Memo {
    query: String,
    images: std::sync::Arc<Vec<PathBuf>>,
    found: Vec<PathBuf>,
}

static MEMO: LazyLock<Mutex<Memo>> = LazyLock::new(Mutex::default);

/*
 * the pictures whose file name holds every word of the query, in any case, in the folder's order;
 * a query of only spaces finds them all
 */
fn found(query: &str) -> Vec<PathBuf> {
    let images = wallpaper::images();
    let mut memo = MEMO.locked();

    if memo.query != query || !std::sync::Arc::ptr_eq(&memo.images, &images) {
        let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();

        memo.found = images
            .iter()
            .filter(|path| {
                let name = name(path).to_lowercase();

                terms.iter().all(|term| name.contains(term.as_str()))
            })
            .cloned()
            .collect();
        memo.query = query.to_owned();
        memo.images = images;
    }

    memo.found.clone()
}

fn name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

// the view's own read of the visit, so this never reads IslandService again
pub fn surface(monitor: &str, visit: u64) -> Rectangle {
    let current = wallpaper::showing_on(&Output::Named(monitor.to_owned()));
    let pick = Pick::read().of(visit);
    let found = found(&pick.query);
    let all = wallpaper::images().len();
    let pick = pick.bounded(&found, &current);

    let grid: Box<dyn Widget> = if all == 0 {
        Box::new(missing(monitor))
    } else if found.is_empty() {
        Box::new(search::state(
            Icon::Picture,
            "No matching wallpapers",
            &format!("Nothing found for \u{201c}{}\u{201d}", pick.query.trim()),
            width(),
            room(),
        ))
    } else {
        Box::new(tiles(monitor, &found, &current, &pick))
    };

    let shape = view::largest();

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::Region, "wallpapers"))
        .child(
            Column::new(vec![
                Box::new(view::rise(
                    0,
                    search::field(Icon::Search, &pick.query, "Search wallpapers", width()),
                )) as Box<dyn Widget>,
                Box::new(view::rise(1, grid)),
                Box::new(view::rise(2, footer(monitor, &pick, &found, all))),
            ])
            .width(width())
            .gap(GAP),
        )
}

// the folder has no picture: it is not set, not there, or holds none
fn missing(monitor: &str) -> Rectangle {
    let (title, detail) = match wallpaper::directory() {
        None => (
            String::from("No wallpaper folder"),
            String::from("Set the wallpaper folder in Settings"),
        ),
        Some(directory) if !directory.is_dir() => (
            String::from("Wallpaper folder not found"),
            format!("{} is not a folder", directory.display()),
        ),
        Some(directory) => (
            String::from("No wallpapers"),
            format!("Add images to {}", directory.display()),
        ),
    };

    search::state_with(
        Icon::Picture,
        &title,
        &detail,
        Some(search::settings_pill(monitor, search::DATA)),
        width(),
        room(),
    )
}

/*
 * the rows of pictures scrolled `offset` down, clipped to the grid; a thumb in the inset says where
 * while they do not all fit
 */
fn tiles(monitor: &str, found: &[PathBuf], current: &[PathBuf], pick: &Pick) -> Stack {
    let count = found.len();
    let columns = columns();
    let (first, end) = shown(pick.offset, count);
    let ring = (!pick.shuffle).then_some(pick.selected).flatten();

    let rows: Vec<Box<dyn Widget>> = (first..end)
        .map(|row| {
            let pictures: Vec<Box<dyn Widget>> = (row * columns..((row + 1) * columns).min(count))
                .map(|index| {
                    let path = &found[index];

                    Box::new(tile(
                        monitor,
                        pick.visit,
                        path,
                        index,
                        ring == Some(index),
                        current.contains(path),
                    )) as Box<dyn Widget>
                })
                .collect();

            Box::new(Row::new(pictures).gap(TILE_GAP)) as Box<dyn Widget>
        })
        .collect();

    let listed = content(count).min(room());

    let viewport = Rectangle::new()
        .width(width())
        .height(listed)
        .clip()
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::List, "pictures"))
        .on_scroll({
            let monitor = monitor.to_owned();

            move |Scroll { y, .. }| wheel(&monitor, y)
        })
        .child(
            Rectangle::new()
                .width(width())
                .height(top(end - first))
                .align_child(Start, Start)
                .translate(0.0, top(first) - pick.offset)
                .child(Column::new(rows).width(width()).gap(TILE_GAP)),
        );

    let mut layers = children![viewport];

    let most = most(count);

    if most > 0.0 {
        let length = (room() * room() / content(count)).max(TARGET);
        let at = (room() - length) * pick.offset / most;

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
 * a picture: a quiet tile while it decodes, a crossed picture for one that cannot be, marked when it
 * is the wallpaper showing. Pressing it sets it
 */
fn tile(
    monitor: &str,
    visit: u64,
    path: &Path,
    index: usize,
    ring: bool,
    current: bool,
) -> Rectangle {
    let (width, height) = (tile_width(), tile_height());
    let picture = Image::cover(path).thumbnail(
        (width * SCALE).round() as u32,
        (height * SCALE).round() as u32,
    );

    let tile = Rectangle::new()
        .width(width)
        .height(height)
        .radius(radius::TILE)
        .fill(theme::island().surface_container);

    let tile = match picture.state() {
        ImageState::Ready => tile.fill(picture),
        ImageState::Loading => tile,
        ImageState::Failed => tile
            .align_child(Center, Center)
            .child(Icon::Picture.crossed(16.0).opacity(theme::DISABLED)),
    };

    let tile = if current {
        tile.padding(BADGE_INSET).align_child(End, Start).child(
            Rectangle::new()
                .width(BADGE)
                .height(BADGE)
                .radius(BADGE / 2.0)
                .fill(theme::island().primary)
                .align_child(Center, Center)
                .child(Icon::Check.on(14.0, theme::island().on_primary)),
        )
    } else {
        tile
    };

    let monitor = monitor.to_owned();
    let path = path.to_owned();

    tile.border_if(ring)
        .cursor(Cursor::Pointer)
        .identity(&path)
        .semantics(|| {
            Semantics::new(Role::Item, "picture")
                .content(name(&path))
                .selected(current)
        })
        .on_click(super::on_left(move || {
            run(&monitor, visit, Press::Set(path.clone(), index));
        }))
}

/*
 * what the ring is on, or how the last set went, how many pictures there are, of how many while
 * searching, and Shuffle, faded while there is nothing else to show
 */
fn footer(monitor: &str, pick: &Pick, found: &[PathBuf], all: usize) -> Row {
    let (said, failed) = match &pick.setting {
        Setting::Failed(why) => (format!("Not set: {why}"), true),
        Setting::Waiting(path) => (format!("Setting {}", name(path)), false),
        Setting::Idle if pick.shuffle => (String::from("A wallpaper at random"), false),
        Setting::Idle => (
            pick.selected
                .and_then(|index| found.get(index))
                .map(|path| name(path))
                .unwrap_or_default(),
            false,
        ),
    };

    let count = match all {
        _ if !pick.query.trim().is_empty() => format!("{} of {all}", found.len()),
        1 => String::from("1 wallpaper"),
        all => format!("{all} wallpapers"),
    };

    let label = Text::new("Shuffle")
        .size(theme::text::LABEL_SMALL)
        .color(theme::island().on_surface)
        .weight(theme::text::SEMIBOLD);

    let pill = Rectangle::new()
        .width(SHUFFLE)
        .height(FOOTER)
        .radius(FOOTER / 2.0)
        .align_child(Center, Center)
        .child(label);

    let pill = if pick.shuffle {
        pill.border(RING, theme::island().on_surface)
    } else {
        pill.border(1.0, theme::island().surface_container_high)
    };

    // another than the one showing needs two
    let pill = pill.semantics(|| Semantics::new(Role::Button, "shuffle").disabled(all <= 1));

    let pill = if all > 1 {
        let monitor = monitor.to_owned();
        let visit = pick.visit;

        pill.cursor(Cursor::Pointer)
            .on_click(super::on_left(move || run(&monitor, visit, Press::Shuffle)))
    } else {
        pill.opacity(theme::DISABLED)
    };

    let said = Text::new(said)
        .size(theme::text::LABEL_SMALL)
        .color(if failed {
            theme::island().on_surface
        } else {
            theme::island().on_surface_variant
        })
        .weight(if failed {
            theme::text::SEMIBOLD
        } else {
            theme::text::MEDIUM
        })
        .elide();

    Row::new(children![
        Column::new(children![said]).width(Parent),
        Text::new(count)
            .size(theme::text::LABEL_SMALL)
            .color(theme::island().on_surface_variant)
            .weight(theme::text::MEDIUM),
        pill,
    ])
    .width(width())
    .height(FOOTER)
    .gap(GAP)
    .align(Center)
}

/*
 * a key while this island shows Wallpapers; false for one it does not use, which the window's own
 * keys then get. A key it uses keeps the held island open for another hold
 */
pub fn key(monitor: &str, key: Key) -> bool {
    let visit = {
        let island = IslandService::read();

        if island.presentation(monitor) != Presentation::Expanded(Surface::Wallpapers) {
            return false;
        }

        island.visit()
    };

    // Enter on the empty state presses its way out
    if key == Key::Enter && wallpaper::images().is_empty() {
        search::open_settings(monitor, search::DATA);
        return true;
    }

    let current = wallpaper::showing_on(&Output::Named(monitor.to_owned()));
    let pick = Pick::read().of(visit);
    let found = found(&pick.query);

    let Some((mut pick, presses)) = pick.bounded(&found, &current).step(key, found.len()) else {
        return false;
    };

    let press = presses.then(|| pick.press(&found)).flatten();

    // a failed set is said until the next key
    if matches!(pick.setting, Setting::Failed(_)) {
        pick.setting = Setting::Idle;
    }

    store(pick);

    IslandService::write().attend(monitor, Instant::now());

    if let Some(press) = press {
        run(monitor, visit, press);
    }

    true
}

/*
 * sets the picture pressed, or one at random, the transition growing from what was pressed; the
 * Surface stays open on the grid, to try another
 */
fn run(monitor: &str, visit: u64, press: Press) {
    let (from, done, set) = {
        let monitor = monitor.to_owned();
        let mut pick = Pick::write();

        // a click before any key finds the last visit's state
        if pick.visit != visit {
            *pick = Pick::begin(visit);
        }

        pick.asked += 1;
        let set = pick.asked;

        let from = match &press {
            Press::Set(_, index) => at_tile(&monitor, &pick, *index),
            Press::Shuffle => at_shuffle(&monitor),
        };

        let done: wallpaper::Done = Box::new(move |result: Result<(), Unset>| {
            settled(visit, set, result.map_err(|unset| unset.brief()))
        });

        (from, done, set)
    };

    // the monitor's own wallpaper, the others as they are, unless one is set for all
    let output = wallpaper::picked_on(monitor);

    let started = match press {
        Press::Set(path, _) => wallpaper::set(path.clone(), output, from, Some(done))
            .map(|()| path)
            .map_err(|unset| String::from(unset.brief())),
        Press::Shuffle => wallpaper::shuffle(output, from, Some(done)),
    };

    let mut pick = Pick::write();

    if pick.visit == visit && pick.asked == set && pick.settled != set {
        pick.setting = match started {
            Ok(path) => Setting::Waiting(path),
            Err(why) => Setting::Failed(why),
        };
    }
}

// how the set asked in `visit` went, on the wallpaper's thread; nothing once that visit is over
fn settled(visit: u64, asked: u64, set: Result<(), &str>) {
    let mut pick: Write<Pick> = Pick::write();

    if pick.visit != visit || pick.asked != asked {
        return;
    }

    pick.settled = asked;

    pick.setting = match set {
        Ok(()) => Setting::Idle,
        Err(why) => Setting::Failed(why.to_owned()),
    };
}

// the middle of picture `index` on the screen, as a circle transition takes it
fn at_tile(monitor: &str, pick: &Pick, index: usize) -> Option<From> {
    let columns = columns();
    let x = INSET + (index % columns) as f32 * (tile_width() + TILE_GAP) + tile_width() / 2.0;
    let y = INSET + FIELD + GAP + top(index / columns) - pick.offset + tile_height() / 2.0;

    from(monitor, x, y)
}

// the middle of Shuffle on the screen
fn at_shuffle(monitor: &str) -> Option<From> {
    let shape = view::largest();
    let x = shape.width - INSET - SHUFFLE / 2.0;
    let y = shape.height - INSET - FOOTER / 2.0;

    from(monitor, x, y)
}

// a point of the body as awww takes it, counted up from the bottom
fn from(monitor: &str, x: f32, y: f32) -> Option<From> {
    let (x, y) = view::on_screen(monitor, (x, y))?;

    Some(From { x, y: 1.0 - y })
}

// down scrolls further down the grid
fn wheel(monitor: &str, lines: f32) {
    let visit = IslandService::read().visit();

    let current = wallpaper::showing_on(&Output::Named(monitor.to_owned()));
    let pick = Pick::read().of(visit);
    let found = found(&pick.query);

    store(pick.bounded(&found, &current).wheel(lines, found.len()));
}
