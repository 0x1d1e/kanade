//! What the Surfaces made of tiles and pills share (Session, Timer, Capture): a title over a row of
//! tiles or a reading with pills below, the keyboard ring over their targets, and the keys and clicks
//! that press them. The arrows move the ring, Enter or Space presses it.

use std::time::Instant;

use kanade_runtime::service::Service;
use kanade_runtime::{
    Center, Column, Cursor, Key, Padding, Rectangle, Row, Start, Text, Widget, children,
};

use super::grid::moved;
use super::ring::Ring;
use super::{Outline, store};
use crate::icon::Icon;
use crate::island::geometry;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, radius};
use crate::view::rise;

pub const WIDTH: f32 = geometry::PAD.width - 2.0 * INSET;
const HEADER: f32 = TARGET;
const GAP: f32 = 14.0;

// where the tiles or the reading go
pub const BODY: f32 = geometry::PAD.height - 2.0 * INSET - HEADER - GAP;

const TILE: f32 = 92.0;
const TILE_GAP: f32 = 8.0;
const KNOB: f32 = 40.0;
const NOTE: f32 = 16.0;

pub const PILL: f32 = 32.0;
const PILL_WIDTH: f32 = 140.0;
pub const PILL_GAP: f32 = 8.0;

// the pills sit below where the tiles were, so a double click on a tile never presses one
const _: () = assert!(BODY - PILL >= (BODY + TILE) / 2.0);

/*
 * the ring of one visit over the targets of a Surface, `A` being what it can be on. A ring kept
 * from an earlier visit is over
 */
#[derive(Debug, Clone, PartialEq)]
pub struct Focus<A> {
    pub ring: Ring<A>,
}

impl<A> Default for Focus<A> {
    fn default() -> Self {
        Focus {
            ring: Ring::default(),
        }
    }
}

impl<A: Clone + PartialEq> Focus<A> {
    // this visit's focus; one kept from an earlier visit is over
    pub fn of(&self, visit: u64, held: bool) -> Focus<A> {
        if self.ring.current(visit) {
            self.clone()
        } else {
            Focus {
                ring: Ring::start(visit, held),
            }
        }
    }

    pub fn ring(&self, grid: &[Vec<(A, f32)>]) -> Option<A> {
        self.ring.on(grid, 0)
    }

    /*
     * the focus after a key and what it presses, none when the key is not the Surface's, as
     * Escape, which closes it. A key while the ring hides only shows it, so nothing is pressed
     * that the ring was not seen on
     */
    pub fn step(self, key: Key, grid: &[Vec<(A, f32)>]) -> Option<(Focus<A>, Option<A>)> {
        let place = self.ring.place(grid, 0)?;
        let at = grid[place.row][place.column].0.clone();

        let act = matches!(key, Key::Enter | Key::Space).then_some(at);
        let to = moved(place, key, grid);

        if act.is_none() && to.is_none() {
            return None;
        }

        if let Some(ring) = self.ring.revealed(grid, 0) {
            return Some((Focus { ring }, None));
        }

        let ring = match to {
            Some(to) => self.ring.onto(grid[to.row][to.column].0.clone()),
            None => self.ring,
        };

        Some((Focus { ring }, act))
    }
}

/*
 * a key while this island shows `surface`; false for one it does not use, as Escape, which the
 * window's own keys then get to close it. `press` is given the focus after the key and what it
 * pressed, and gives the focus that leaves
 */
pub fn key<A>(
    monitor: &str,
    surface: Surface,
    key: Key,
    grid: impl FnOnce() -> Vec<Vec<(A, f32)>>,
    press: impl FnOnce(Focus<A>, A) -> Focus<A>,
) -> bool
where
    A: Clone + PartialEq,
    Focus<A>: Service + PartialEq,
{
    let (visit, held) = {
        let island = IslandService::read();

        if island.presentation(monitor) != Presentation::Expanded(surface) {
            return false;
        }

        (island.visit(), island.held(monitor))
    };

    let focus = Focus::<A>::read().of(visit, held);

    let Some((focus, at)) = focus.step(key, &grid()) else {
        return false;
    };

    let focus = match at {
        Some(at) => press(focus, at),
        None => focus,
    };

    store(focus);

    IslandService::write().attend(monitor, Instant::now());

    true
}

// only while open: two clicks queued before the next frame both reach the targets that frame drew
pub fn click<A>(surface: Surface, at: A, press: impl FnOnce(Focus<A>, A) -> Focus<A>)
where
    A: Clone + PartialEq,
    Focus<A>: Service + PartialEq,
{
    let visit = {
        let island = IslandService::read();

        if island.surface() != Some(surface) {
            return;
        }

        island.visit()
    };
    let focus = Focus {
        ring: Focus::<A>::read().of(visit, false).ring.hidden(),
    };

    store(press(focus, at));
}

// the Surface done with
pub fn close() {
    let monitor = IslandService::read().expanded_on().map(str::to_owned);

    if let Some(monitor) = monitor {
        crate::view::collapse(&monitor);
    }
}

// the title over `body`, which is `BODY` tall
pub fn frame(title: &str, body: impl Widget + 'static) -> Rectangle {
    let shape = geometry::PAD;

    let header = Row::new(children![
        Text::new(title)
            .size(theme::text::TITLE)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD)
    ])
    .width(WIDTH)
    .height(HEADER)
    .align(Center);

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .child(
            Column::new(vec![Box::new(rise(0, header)), Box::new(rise(1, body))])
                .width(WIDTH)
                .gap(GAP),
        )
}

// `count` tiles side by side, as wide as they all fit
pub fn tile_width(count: usize) -> f32 {
    (WIDTH - (count - 1) as f32 * TILE_GAP) / count as f32
}

// the tiles in the middle of the body
pub fn tiles(row: Vec<Box<dyn Widget>>) -> Rectangle {
    Rectangle::new()
        .width(WIDTH)
        .height(BODY)
        .align_child(Center, Center)
        .child(Row::new(row).gap(TILE_GAP))
}

// its icon in a knob, then its name; filled while the ring is on it
pub fn tile(
    icon: Icon,
    name: &str,
    width: f32,
    ring: bool,
    press: impl Fn() + 'static,
) -> Rectangle {
    let knob = Rectangle::new()
        .width(KNOB)
        .height(KNOB)
        .radius(KNOB / 2.0)
        .fill(theme::island().surface_container_high)
        .align_child(Center, Center)
        .child(icon.on(18.0, theme::island().on_surface));

    let tile = Rectangle::new()
        .width(width)
        .height(TILE)
        .radius(radius::TILE)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(super::on_left(press))
        .child(
            Column::new(children![
                knob,
                Text::new(name)
                    .size(theme::text::LABEL_SMALL)
                    .color(theme::island().on_surface)
                    .weight(theme::text::SEMIBOLD)
            ])
            .gap(10.0)
            .align(Center),
        );

    if ring {
        tile.fill(theme::island().surface_container).border_if(true)
    } else {
        tile
    }
}

// outlined, the ring in place of the outline
pub fn pill(label: &str, ring: bool, press: impl Fn() + 'static) -> Rectangle {
    let pill = Rectangle::new()
        .width(PILL_WIDTH)
        .height(PILL)
        .radius(PILL / 2.0)
        .padding(Padding {
            top: 0.0,
            right: 14.0,
            bottom: 0.0,
            left: 14.0,
        })
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(super::on_left(press))
        .child(
            Text::new(label)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface)
                .weight(theme::text::SEMIBOLD),
        );

    if ring {
        pill.border_if(true)
    } else {
        pill.border(1.0, theme::island().surface_container_high)
    }
}

// pills side by side along the bottom of the body
pub fn pills(row: Vec<Box<dyn Widget>>) -> Row {
    Row::new(row).gap(PILL_GAP)
}

/*
 * a mark over a line, centered in what the body leaves above the pills; the line is the reading,
 * as the time left or what is doing
 */
pub fn reading(icon: Icon, line: String) -> Rectangle {
    noted(icon, line, Vec::new())
}

// a reading with short `notes` under it, each on a line of its own, cut where it does not fit
pub fn noted(icon: Icon, line: String, notes: Vec<String>) -> Rectangle {
    let mut column: Vec<Box<dyn Widget>> = vec![
        Box::new(icon.on(28.0, theme::island().on_surface_variant)),
        Box::new(
            Text::new(line)
                .size(theme::text::BODY)
                .color(theme::island().on_surface)
                .weight(theme::text::SEMIBOLD),
        ),
    ];

    column.extend(notes.into_iter().map(|note| {
        Box::new(
            Rectangle::new()
                .width(WIDTH - 2.0 * INSET)
                .height(NOTE)
                .align_child(Center, Center)
                .child(
                    Text::new(note)
                        .size(theme::text::LABEL_SMALL)
                        .color(theme::island().on_surface_variant)
                        .elide(),
                ),
        ) as Box<dyn Widget>
    }));

    Rectangle::new()
        .width(WIDTH)
        .height(BODY - PILL)
        .align_child(Center, Center)
        .child(Column::new(column).gap(4.0).align(Center))
}

// the reading and its pills, one under the other
pub fn stacked(reading: Rectangle, pills: Row) -> Column {
    Column::new(children![reading, pills])
        .width(WIDTH)
        .align(Center)
}
