//! The Emoji Surface: every emoji as a grid of tiles, scrolled with a title over each group,
//! searched as it is typed, with the emoji used lately in one row above the first group. It opens
//! only from IPC, a keybind or the Launcher, so it always holds the keyboard, like the Clipboard:
//! typing searches, the arrows and Tab move a ring over the tiles, and Enter copies what the ring is
//! on and closes the island once it is on the clipboard, or says it was not copied. Space types,
//! never presses.
//!
//! One scrolling grid with the groups' titles in it, not tabs: the whole table is one scroll away,
//! and a title is a heading rather than a mode to leave. `:` in the Launcher opens this Surface at
//! once (ADR 0135), so the table has one interaction: it is browsed here, not answered from a query.
//!
//! A tile draws the emoji's picture, written once per tile drawn from its color font
//! (`emoji::picture`), or the emoji as text where the font has no picture of it.

use std::time::Instant;

use kanade_runtime::service::{self, Service};
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Column, Cursor, End, Image, ImageState, Key, Rectangle, Row, Scroll, SpaceBetween,
    Stack, Start, Text, Widget, children,
};

use crate::icon::Icon;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::sources::clipboard;
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, radius};
use crate::view;

use super::grid::{self, Place};
use super::ring::Ring;
use super::search::{self, FIELD};
use super::visit::Visited;
use super::{Outline, store};

pub(crate) mod data;
pub(crate) mod picture;
mod recents;

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

// a tile and the gap after it; nine of them with their gaps are the content's width
const COLUMNS: usize = 9;
const CELL: f32 = 48.0;
const CELL_GAP: f32 = 6.0;

const _: () = assert!(
    COLUMNS as f32 * CELL + (COLUMNS - 1) as f32 * CELL_GAP
        == crate::island::geometry::CALENDAR.width - 2.0 * INSET
);

// a group's title, as tall as a target, as `pad::frame`'s header is
const HEADER: f32 = TARGET;

// the emoji drawn at twice their size, crisp at scale 2
const SCALE: f32 = 2.0;

// pixels per wheel line
const WHEEL: f32 = 40.0;

/*
 * the query, the ring and the scroll for one visit of the Surface (`IslandService::visit`), so every
 * opening starts at the top. Written by input, and by a copy telling how it went
 */
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Search {
    visit: u64,
    query: String,

    // in the tiles the query finds
    ring: Ring<At>,

    // how far the grid scrolled, in pixels
    offset: f32,

    copying: Copying,
}

// the copy, from its press until the emoji is on the clipboard or failed
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

impl Visited for Search {
    fn visit(&self) -> u64 {
        self.visit
    }

    fn begin(visit: u64) -> Self {
        Search {
            visit,
            ring: Ring::start(visit, false),
            ..Search::default()
        }
    }
}

impl Search {
    /*
     * the search after a key, and what it presses, none when the key is not for the Surface or the
     * ring was only shown by it. Space types, as in the Launcher and the Clipboard, so only Enter
     * presses
     */
    fn step(
        self,
        key: Key,
        grid: &Grid,
        targets: &[Vec<(At, f32)>],
    ) -> Option<(Search, Option<At>)> {
        match key {
            Key::Character(letter) => Some((self.typed(Some(letter)), None)),
            Key::Space => Some((self.typed(Some(' ')), None)),
            Key::Backspace => Some((self.typed(None), None)),
            Key::Enter
            | Key::Up
            | Key::Down
            | Key::Left
            | Key::Right
            | Key::Tab
            | Key::Home
            | Key::End => {
                let place = self.ring.place(targets, 0)?;
                let at = targets[place.row][place.column].0;
                let presses = (key == Key::Enter).then_some(at);

                // a ring that hid shows where it would be before any key presses or moves it, even
                // one that can do neither, so a press is never one the ring was not seen on
                if let Some(ring) = self.ring.revealed(targets, 0) {
                    return Some((Search { ring, ..self }, None));
                }

                Some(match grid::moved(place, key, targets) {
                    Some(Place { row, column }) => {
                        let at = targets[row][column].0;

                        (self.put(at, grid, targets), presses)
                    }
                    None => (self, presses),
                })
            }
            _ => None,
        }
    }

    // a letter typed, or the last one erased; a new query finds new emoji, so it starts at the top
    fn typed(mut self, letter: Option<char>) -> Search {
        match letter {
            Some(letter) => self.query.push(letter),
            None => {
                self.query.pop();
            }
        }

        self.ring = self.ring.top();
        self.offset = 0.0;

        self
    }

    // the ring on `at`, scrolled to show it whole
    fn put(mut self, at: At, grid: &Grid, targets: &[Vec<(At, f32)>]) -> Search {
        let most = most(grid);

        self.ring = self.ring.onto(at);
        self.offset = grid::find(targets, &at).map_or(self.offset, |place| {
            reveal(self.offset, place.row, grid).clamp(0.0, most)
        });

        self
    }

    // scrolled by the wheel, the ring kept on a row that shows whole
    fn wheel(mut self, lines: f32, grid: &Grid, targets: &[Vec<(At, f32)>]) -> Search {
        let most = most(grid);

        self.offset = (self.offset.clamp(0.0, most) + lines * WHEEL).clamp(0.0, most);

        if let Some(at) = self.ring.at
            && let Some(place) = grid::find(targets, &at)
            && let Some((first, last)) = whole(self.offset, grid)
        {
            let row = place.row.clamp(first, last);

            self.ring.at = Some(targets[row][place.column.min(targets[row].len() - 1)].0);
        }

        self
    }

    // the search after `count` tiles: nothing scrolls past them
    fn bounded(mut self, count: usize, grid: &Grid) -> Search {
        self.ring.at = self.ring.at.filter(|_| count > 0);
        self.offset = self.offset.clamp(0.0, most(grid));

        self
    }
}

// a tile the ring can be on: one of the recents, or one of the table's emoji
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum At {
    Recent(usize),
    Emoji(usize),
}

// where a tile sits across its row, from 0 to 1
fn across(column: usize, columns: usize) -> f32 {
    (column as f32 + 0.5) / columns as f32
}

// the ring's targets as rows: the recents' row, then the table's emoji a row at a time
fn targets(listed: &[Listed], recents: usize) -> Vec<Vec<(At, f32)>> {
    let mut rows: Vec<Vec<(At, f32)>> = Vec::new();

    if recents > 0 {
        rows.extend((0..recents).collect::<Vec<_>>().chunks(COLUMNS).map(|row| {
            row.iter()
                .enumerate()
                .map(|(column, &at)| (At::Recent(at), across(column, COLUMNS)))
                .collect()
        }));
    }

    rows.extend(row_ranges(listed).into_iter().map(|(start, end)| {
        (start..end)
            .enumerate()
            .map(|(column, at)| (At::Emoji(at), across(column, COLUMNS)))
            .collect()
    }));

    rows
}

// flat emoji indexes split into rows that fit the grid and never cross a group boundary
fn row_ranges(listed: &[Listed]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0;

    while start < listed.len() {
        let group = listed[start].group;
        let mut end = start + 1;

        while end < listed.len() && end - start < COLUMNS && listed[end].group == group {
            end += 1;
        }

        ranges.push((start, end));
        start = end;
    }

    ranges
}

// a line of the scrolled content: a group's title, or a row of tiles
struct Line {
    kind: Kind,
    top: f32,
    height: f32,
}

enum Kind {
    Title(&'static str),

    // the flat emoji indexes it draws
    Row(usize, usize),
    RecentRow(usize),
}

/*
 * the grid as it scrolls: its lines and where each stands, the height it asks for, and where each
 * row of targets sits, which is what a scroll is measured in. A title has its own height, so the
 * rows below keep their place
 */
struct Grid {
    lines: Vec<Line>,

    // each row of `targets`, its top and its height
    rows: Vec<(f32, f32)>,

    content: f32,
}

// how far the grid can scroll: none while it fits
fn most(grid: &Grid) -> f32 {
    (grid.content - room()).max(0.0)
}

// the least scroll from `offset` that shows row `row` whole
fn reveal(offset: f32, row: usize, grid: &Grid) -> f32 {
    match grid.rows.get(row) {
        Some(&(top, height)) => offset.min(top).max(top + height - room()),
        None => offset,
    }
}

// the first and last rows of targets that show whole at `offset`
fn whole(offset: f32, grid: &Grid) -> Option<(usize, usize)> {
    let first = grid.rows.iter().position(|&(top, _)| top >= offset - 0.5)?;
    let last = grid
        .rows
        .iter()
        .rposition(|&(top, height)| top + height <= offset + room() + 0.5)?;

    (first <= last).then_some((first, last))
}

// how tall `content` asks the body to be; the island caps it (ADR 0030)
fn asks(content: f32) -> f32 {
    2.0 * INSET + FIELD + content.min(room()) + FOOTER + 2.0 * GAP
}

// where a find sits in what is listed: the group it belongs to
#[derive(Debug, Clone, Copy, PartialEq)]
struct Listed {
    group: Option<usize>,
}

/*
 * what the query finds, in the table's order, each with the group it belongs to: every group's
 * emoji while the query is empty, else those that answer it. The recents are listed above them, in
 * their own order
 */
fn found(query: &str) -> (Vec<&'static emojis::Emoji>, Vec<Listed>) {
    let terms = data::terms(query);
    let terms: Vec<&str> = terms.iter().map(String::as_str).collect();

    let table = data::table();
    let mut found = Vec::new();
    let mut listed = Vec::new();

    for (at, (_, range)) in table.groups.iter().enumerate() {
        for entry in &table.entries[range.clone()] {
            if !entry.matches(&terms) {
                continue;
            }

            listed.push(Listed { group: Some(at) });
            found.push(entry.emoji);
        }
    }

    (found, listed)
}

// the emoji the recents hold, in their order, as far as the query finds them
fn recent(query: &str) -> Vec<&'static emojis::Emoji> {
    let terms = data::terms(query);
    let terms: Vec<&str> = terms.iter().map(String::as_str).collect();

    recents::with(|recent| {
        recent
            .iter()
            .filter_map(|emoji| emojis::get(emoji))
            .filter(|emoji| data::answers(emoji, &terms))
            .collect()
    })
}

// the grid the query's finds draw, with the recents in the row above them
fn layout(emoji: &[&'static emojis::Emoji], listed: &[Listed], recents: usize) -> Grid {
    debug_assert_eq!(emoji.len(), listed.len());

    let mut lines = Vec::new();
    let mut rows: Vec<(f32, f32)> = Vec::new();
    let mut top = 0.0;

    if recents > 0 {
        lines.push(Line {
            kind: Kind::Title("Recent"),
            top,
            height: HEADER,
        });
        top += HEADER;

        for _ in 0..recents.div_ceil(COLUMNS) {
            lines.push(Line {
                kind: Kind::RecentRow(rows.len()),
                top,
                height: CELL,
            });
            rows.push((top, CELL));
            top += CELL + CELL_GAP;
        }
    }

    // a title stands where its group starts; a row ends at a group transition
    let mut group: Option<usize> = None;

    for (start, end) in row_ranges(listed) {
        if let Some(of) = listed[start].group.filter(|of| group != Some(*of)) {
            let (each, _) = &data::table().groups[of];

            lines.push(Line {
                kind: Kind::Title(data::title(*each)),
                top,
                height: HEADER,
            });
            top += HEADER;
            group = Some(of);
        }

        lines.push(Line {
            kind: Kind::Row(start, end),
            top,
            height: CELL,
        });
        rows.push((top, CELL));
        top += CELL + CELL_GAP;
    }

    Grid {
        lines,
        rows,
        content: (top - CELL_GAP).max(0.0),
    }
}

/*
 * tells the island how tall the Emoji Surface asks to be: a new visit lists the whole table, the one
 * open what its query found. After each change of the search, never from a view
 */
pub fn fit() {
    let _ordered = super::fitting();
    let visit = IslandService::read().visit();

    let (fresh, open) = {
        let query = Search::read().of(visit).query;

        let (all, listed) = found("");
        let recents = recent("").len();

        let (finds, matching) = found(&query);
        let shown = recent(&query).len();

        (
            asks(layout(&all, &listed, recents).content),
            asks(layout(&finds, &matching, shown).content),
        )
    };

    super::ask(Surface::Emoji, fresh, Some((visit, open)));
}

// at start: the body follows the search from now on
pub fn start() {
    service::watch::<Search>(fit);
    fit();
}

// the view's own read of the visit, so this never reads IslandService again
pub fn surface(monitor: &str, visit: u64) -> Rectangle {
    let search = Search::read().of(visit);
    let (emoji, listed) = found(&search.query);
    let recents = recent(&search.query);
    let grid = layout(&emoji, &listed, recents.len());
    let targets = targets(&listed, recents.len());
    let search = search.bounded(targets.len(), &grid);

    let list: Box<dyn Widget> = if emoji.is_empty() {
        Box::new(search::state(
            Icon::Search,
            "No emoji",
            &format!("Nothing found for \u{201c}{}\u{201d}", search.query.trim()),
            width(),
            room(),
        ))
    } else {
        Box::new(list(monitor, &search, &emoji, &recents, &grid, &targets))
    };

    let shape = view::largest();

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::Region, "emoji"))
        .child(
            Column::new(vec![
                Box::new(view::rise(
                    0,
                    search::field(Icon::Search, &search.query, "Search emoji", width()),
                )) as Box<dyn Widget>,
                Box::new(view::rise(1, list)),
                Box::new(view::rise(2, footer(&search, emoji.len()))),
            ])
            .width(width())
            .gap(GAP),
        )
}

/*
 * the lines scrolled `offset` down, clipped to the grid, each drawn where it stands: only the ones
 * any part of shows are built, so only their pictures are asked for
 */
fn list(
    monitor: &str,
    search: &Search,
    emoji: &[&'static emojis::Emoji],
    recents: &[&'static emojis::Emoji],
    grid: &Grid,
    targets: &[Vec<(At, f32)>],
) -> Stack {
    let ring = search.ring.on(targets, 0);
    let end = search.offset + room();

    let shown: Vec<Box<dyn Widget>> = grid
        .lines
        .iter()
        .filter(|line| line.top + line.height > search.offset && line.top < end)
        .map(|line| {
            let at = line.top - search.offset;

            match line.kind {
                Kind::Title(title) => {
                    Box::new(title_row(title).translate(0.0, at)) as Box<dyn Widget>
                }
                Kind::Row(start, end) => Box::new(
                    tiles_row(
                        monitor,
                        search.visit,
                        &emoji[start..end],
                        start,
                        false,
                        ring,
                    )
                    .translate(0.0, at),
                ) as Box<dyn Widget>,
                Kind::RecentRow(row) => {
                    let start = row * COLUMNS;
                    let end = (start + COLUMNS).min(recents.len());

                    Box::new(
                        tiles_row(
                            monitor,
                            search.visit,
                            &recents[start..end],
                            start,
                            true,
                            ring,
                        )
                        .translate(0.0, at),
                    ) as Box<dyn Widget>
                }
            }
        })
        .collect();

    let listed = grid.content.min(room());

    let viewport = Rectangle::new()
        .width(width())
        .height(listed)
        .clip()
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::List, "emoji"))
        .on_scroll({
            let monitor = monitor.to_owned();

            move |Scroll { y, .. }| wheel(&monitor, y)
        })
        .child(Stack::new(shown).width(width()).height(listed));

    let mut layers = children![viewport];

    let most = most(grid);

    if most > 0.0 {
        let length = (room() * room() / grid.content).max(TARGET);
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

    Stack::new(layers).width(width()).height(listed)
}

// a group's title, at the height a target takes
fn title_row(title: &str) -> Rectangle {
    Rectangle::new()
        .width(width())
        .height(HEADER)
        .align_child(Start, End)
        .child(
            Text::new(title)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::SEMIBOLD),
        )
}

/*
 * one row of tiles, the first at `at` in the recents or in `emoji`: the ring on `selected`, and a
 * press copying it
 */
fn tiles_row(
    monitor: &str,
    visit: u64,
    emoji: &[&'static emojis::Emoji],
    at: usize,
    recent: bool,
    selected: Option<At>,
) -> Rectangle {
    let cells: Vec<Box<dyn Widget>> = emoji
        .iter()
        .enumerate()
        .map(|(column, emoji)| {
            let place = match recent {
                true => At::Recent(at + column),
                false => At::Emoji(at + column),
            };

            Box::new(tile(monitor, visit, emoji, selected == Some(place))) as Box<dyn Widget>
        })
        .collect();

    Rectangle::new()
        .width(width())
        .height(CELL)
        .child(Row::new(cells).gap(CELL_GAP))
}

/*
 * a tile: the emoji's picture on a plate, or the emoji as text where its font has no picture of it.
 * Pressing it copies it
 */
fn tile(monitor: &str, visit: u64, emoji: &'static emojis::Emoji, ring: bool) -> Rectangle {
    let plate = Rectangle::new()
        .width(CELL)
        .height(CELL)
        .radius(radius::TILE)
        .fill(theme::island().surface_container)
        .align_child(Center, Center);

    let picture = picture::picture(emoji.as_str())
        .map(|path| Image::contain(path).thumbnail((CELL * SCALE) as u32, (CELL * SCALE) as u32));

    let plate = match picture.as_ref().map(Image::state) {
        Some(ImageState::Ready) => plate.fill(picture.expect("a ready picture")),
        _ => plate.child(glyph(emoji)),
    };

    let monitor = monitor.to_owned();

    plate
        .border_if(ring)
        .cursor(Cursor::Pointer)
        .identity(("emoji", emoji.as_str()))
        .semantics(|| Semantics::new(Role::Button, "emoji").label(emoji.name()))
        .on_click(super::on_left(move || {
            copy(&monitor, visit, emoji.as_str())
        }))
}

// the emoji drawn as its own text, for one the font has no picture of
fn glyph(emoji: &emojis::Emoji) -> Text {
    Text::new(emoji.as_str())
        .size(CELL * 0.62)
        .color(theme::island().on_surface)
}

// how the last copy went, how many emoji there are, of how many while searching
fn footer(search: &Search, count: usize) -> Row {
    let (said, failed) = match search.copying {
        Copying::Failed => (String::from("Not copied"), true),
        Copying::Waiting => (String::from("Copying"), false),
        Copying::Idle => (String::from("Enter copies"), false),
    };

    let found = match search.query.trim().is_empty() {
        true => match count {
            1 => String::from("1 emoji"),
            count => format!("{count} emoji"),
        },
        false => format!("{count} of {}", data::table().len()),
    };

    Row::new(children![
        Text::new(said)
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
            }),
        Text::new(found)
            .size(theme::text::LABEL_SMALL)
            .color(theme::island().on_surface_variant)
            .weight(theme::text::MEDIUM),
    ])
    .width(width())
    .height(FOOTER)
    .justify(SpaceBetween)
    .align(Center)
}

/*
 * a key while this island shows the Emoji Surface; false for one it does not use, as Escape, which
 * the window's own keys then get to close it. A key it uses keeps the held island open for another
 * hold
 */
pub fn key(monitor: &str, key: Key) -> bool {
    let visit = {
        let island = IslandService::read();

        if island.presentation(monitor) != Presentation::Expanded(Surface::Emoji) {
            return false;
        }

        island.visit()
    };

    let search = Search::read().of(visit);
    let (emoji, listed) = found(&search.query);
    let recents = recent(&search.query);
    let grid = layout(&emoji, &listed, recents.len());
    let targets = targets(&listed, recents.len());

    let Some((mut search, pressed)) = search
        .bounded(targets.len(), &grid)
        .step(key, &grid, &targets)
    else {
        return false;
    };

    let pressed = pressed.and_then(|at| match at {
        At::Recent(at) => recents.get(at).copied(),
        At::Emoji(at) => emoji.get(at).copied(),
    });

    // a failed copy is said until the next key
    if search.copying == Copying::Failed {
        search.copying = Copying::Idle;
    }

    store(search);

    IslandService::write().attend(monitor, Instant::now());

    if let Some(emoji) = pressed {
        copy(monitor, visit, emoji.as_str());
    }

    true
}

/*
 * puts the emoji on the clipboard off the draw thread, since `wl-copy` blocks until it holds it, then
 * records it in the recents and closes the island; one it could not copy leaves the Surface open,
 * saying so. One copy at a time
 */
fn copy(monitor: &str, visit: u64, emoji: &'static str) {
    let search = Search::read().of(visit);

    if search.copying == Copying::Waiting {
        return;
    }

    store(Search {
        copying: Copying::Waiting,
        ..search
    });

    let started = std::thread::Builder::new()
        .name("emoji-copy".into())
        .spawn({
            let monitor = monitor.to_owned();

            move || {
                let done = clipboard::put(emoji).is_ok();

                copied(&monitor, visit, emoji, done);
            }
        });

    if let Err(error) = started {
        eprintln!("kanade: cannot copy an emoji ({})", error.kind());
        copied(monitor, visit, emoji, false);
    }
}

/*
 * how the copy pressed in `visit` went, on the copy's thread: the recents keep it, and the island
 * closes once it is on the clipboard; otherwise it stays open, saying it was not copied. Nothing once
 * that visit is over, so it never closes another
 */
fn copied(monitor: &str, visit: u64, emoji: &'static str, done: bool) {
    let open = {
        let island = IslandService::read();

        island.visit() == visit
            && island.presentation(monitor) == Presentation::Expanded(Surface::Emoji)
    };

    if done {
        recents::used(emoji);
    }

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

// down scrolls further down the grid
fn wheel(_monitor: &str, lines: f32) {
    let visit = IslandService::read().visit();
    let search = Search::read().of(visit);

    let (emoji, listed) = found(&search.query);
    let recents = recent(&search.query);
    let grid = layout(&emoji, &listed, recents.len());
    let targets = targets(&listed, recents.len());

    store(
        search
            .bounded(targets.len(), &grid)
            .wheel(lines, &grid, &targets),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // a search answers every word of it somewhere in a name or a shortcode, never only some, and the
    // groups are listed in the table's order
    #[test]
    fn a_search_matches_every_term_in_a_name_or_a_shortcode() {
        let terms = data::terms("thumbs up");
        let terms: Vec<&str> = terms.iter().map(String::as_str).collect();
        let thumbs = data::table()
            .entries
            .iter()
            .find(|entry| entry.emoji.as_str() == "\u{1f44d}")
            .expect("thumbs up");

        assert!(thumbs.matches(&[]));
        assert!(thumbs.matches(&terms));
        assert!(!thumbs.matches(&["thumbs", "nothing"]));

        let (all, listed) = found("");
        assert_eq!(all.len(), listed.len());
        assert_eq!(all.len(), data::table().len());
        assert_eq!(listed.first().and_then(|listed| listed.group), Some(0));

        let (thumbs_up, _) = found("thumbs up");
        assert!(thumbs_up.contains(&thumbs.emoji));
        assert!(thumbs_up.len() < 50);
        assert!(found("nothing at all").0.is_empty());
    }

    // a hidden ring is shown by the first key that would act or move, which does nothing else, and
    // the first row of targets is the one after the recents' row
    #[test]
    fn a_hidden_ring_is_shown_before_any_key_presses() {
        let grin = emojis::get("\u{1f600}").expect("grinning");
        let emoji = vec![grin; 3 * COLUMNS];
        let listed = vec![Listed { group: Some(0) }; emoji.len()];
        let grid = layout(&emoji, &listed, 0);
        let targets = targets(&listed, 0);
        let search = Search::begin(1).bounded(targets.len(), &grid);

        let (shown, pressed) = search.clone().step(Key::Enter, &grid, &targets).unwrap();
        assert_eq!(pressed, None);
        assert_eq!(shown.ring.on(&targets, 0), Some(At::Emoji(0)));

        let (moved, pressed) = shown.step(Key::Enter, &grid, &targets).unwrap();
        assert_eq!(pressed, Some(At::Emoji(0)));
        assert_eq!(moved.ring.at, Some(At::Emoji(0)));

        // Down moves a row, and a row is what a scroll is measured in
        let (down, pressed) = moved.step(Key::Down, &grid, &targets).unwrap();
        assert_eq!(pressed, None);
        assert_eq!(down.ring.at, Some(At::Emoji(COLUMNS)));
    }

    #[test]
    fn rows_stop_at_group_transitions_and_keep_flat_target_indexes() {
        let grin = emojis::get("\u{1f600}").expect("grinning");
        let emoji = vec![grin; 11];
        let listed = (0..11)
            .map(|at| Listed {
                group: Some(if at < 8 { 0 } else { 1 }),
            })
            .collect::<Vec<_>>();

        assert_eq!(row_ranges(&listed), [(0, 8), (8, 11)]);

        let targets = targets(&listed, 24);
        assert_eq!(targets.len(), 5);
        assert_eq!(targets[3].len(), 8);
        assert_eq!(targets[3][0].0, At::Emoji(0));
        assert_eq!(targets[3][7].0, At::Emoji(7));
        assert_eq!(targets[4][0].0, At::Emoji(8));
        assert_eq!(targets[4][2].0, At::Emoji(10));

        let grid = layout(&emoji, &listed, 24);
        let rows = grid
            .lines
            .iter()
            .filter_map(|line| match line.kind {
                Kind::Row(start, end) => Some((start, end)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(rows, [(0, 8), (8, 11)]);
    }

    // a title stands above the rows of its group, and the grid is as tall as its last row
    #[test]
    fn a_title_takes_its_own_place_above_its_group() {
        let grin = emojis::get("\u{1f600}").expect("grinning");
        let emoji = vec![grin; 2 * COLUMNS];
        let listed = vec![Listed { group: Some(0) }; emoji.len()];
        let grid = layout(&emoji, &listed, 0);

        assert_eq!(grid.rows.len(), 2);
        assert_eq!(grid.rows[0].0, HEADER);
        assert_eq!(grid.rows[1].0, HEADER + CELL + CELL_GAP);
        assert_eq!(grid.content, HEADER + 2.0 * CELL + CELL_GAP);

        // the recents take their own title and row above the first group
        let grid = layout(&emoji, &listed, 2);

        assert_eq!(grid.rows.len(), 3);
        assert_eq!(grid.rows[0].0, HEADER);
        assert_eq!(grid.rows[1].0, 2.0 * HEADER + CELL + CELL_GAP);

        // 24 recents wrap to three target rows, all included in the content height.
        let grid = layout(&emoji, &listed, 24);
        let targets = targets(&listed, 24);
        assert_eq!(grid.rows.len(), 5);
        assert_eq!(targets.len(), 5);
        assert_eq!(grid.rows[3].0, 2.0 * HEADER + 3.0 * (CELL + CELL_GAP));
        assert_eq!(grid.rows[4].0, 2.0 * HEADER + 4.0 * (CELL + CELL_GAP));
    }
}
