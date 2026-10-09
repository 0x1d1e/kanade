//! The Session Surface (#156): lock, sleep, restart, power off and log out, each a tile. A restart,
//! power off or log out counts down first (`sources::session`), and while one does the Surface
//! says what is left, with Cancel and Now. The arrows move a ring between them, Enter or Space
//! presses it.

use std::time::Instant;

use amane::{
    Center, Column, Cursor, Key, Padding, Rectangle, Row, Service, Start, Text, Widget, children,
};

use super::Ring;
use super::grid::{Place, find, moved};
use crate::icon::Icon;
use crate::island::activity::{Ending, Leave};
use crate::island::geometry;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::modules;
use crate::sources::session::{self, Counting};
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, radius};
use crate::view;

const WIDTH: f32 = geometry::CONTROLS.width - 2.0 * INSET;
const HEADER: f32 = TARGET;
const GAP: f32 = 14.0;

// where the tiles or the countdown go
const BODY: f32 = geometry::CONTROLS.height - 2.0 * INSET - HEADER - GAP;

const TILE: f32 = 92.0;
const TILE_GAP: f32 = 8.0;
const KNOB: f32 = 40.0;

const PILL: f32 = 32.0;
const PILL_WIDTH: f32 = 140.0;
const PILL_GAP: f32 = 8.0;

// the pills sit below where the tiles were, so a double click on a tile never presses Now
const _: () = assert!(BODY - PILL >= (BODY + TILE) / 2.0);

// what the ring can be on
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum At {
    Tile(Leave),
    // the countdown drawn, so a press meant for one since replaced does nothing
    Cancel(Counting),
    Now(Counting),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Focus {
    visit: u64,

    // none is the first target
    at: Option<At>,

    // the ring shows: from the start when opened from the keyboard, else from the first key
    shown: bool,
}

impl Service for Focus {
    fn new() -> Self {
        Focus::default()
    }

    fn listen() {}
}

impl Focus {
    // this visit's focus; one kept from an earlier visit is over
    fn of(&self, visit: u64, held: bool) -> Focus {
        if self.visit == visit {
            self.clone()
        } else {
            Focus {
                visit,
                shown: held,
                at: None,
            }
        }
    }

    // on its target, or the first when that is gone or unchosen
    fn place(&self, grid: &[Vec<(At, f32)>]) -> Option<Place> {
        self.at
            .and_then(|at| find(grid, &at))
            .or_else(|| (!grid.is_empty()).then_some(Place { row: 0, column: 0 }))
    }

    fn ring(&self, grid: &[Vec<(At, f32)>]) -> Option<At> {
        self.shown
            .then(|| self.place(grid))
            .flatten()
            .map(|place| grid[place.row][place.column].0)
    }

    /*
     * the focus after a key and what it presses, none when the key is not the Surface's, as
     * Escape, which closes it. A key while the ring hides only shows it, so nothing is pressed
     * that the ring was not seen on
     */
    fn step(self, key: Key, grid: &[Vec<(At, f32)>]) -> Option<(Focus, Option<At>)> {
        let place = self.place(grid)?;
        let at = grid[place.row][place.column].0;

        if !self.shown {
            let shown = matches!(
                key,
                Key::Left
                    | Key::Right
                    | Key::Up
                    | Key::Down
                    | Key::Home
                    | Key::End
                    | Key::Enter
                    | Key::Space
            );

            return shown.then_some((
                Focus {
                    at: Some(at),
                    shown: true,
                    ..self
                },
                None,
            ));
        }

        if matches!(key, Key::Enter | Key::Space) {
            return Some((self, Some(at)));
        }

        let to = moved(place, key, grid)?;

        Some((
            Focus {
                at: Some(grid[to.row][to.column].0),
                ..self
            },
            None,
        ))
    }
}

// the tiles, the lock's only while its Module is on
fn leaves() -> Vec<Leave> {
    let mut leaves = Vec::new();

    if modules::on("lock") {
        leaves.push(Leave::Lock);
    }

    leaves.extend([
        Leave::Sleep,
        Leave::Ending(Ending::Restart),
        Leave::Ending(Ending::PowerOff),
        Leave::Ending(Ending::LogOut),
    ]);
    leaves
}

// the targets: the tiles, or Cancel and Now while one counts down
fn grid(counting: Option<&Counting>) -> Vec<Vec<(At, f32)>> {
    let row: Vec<At> = match counting {
        Some(counting) => vec![At::Cancel(*counting), At::Now(*counting)],
        None => leaves().into_iter().map(At::Tile).collect(),
    };
    let count = row.len() as f32;

    vec![
        row.into_iter()
            .enumerate()
            .map(|(index, at)| (at, (index as f32 + 0.5) / count))
            .collect(),
    ]
}

/*
 * the tiles, or what counts down. `open` says the Surface is open rather than fading out, so only
 * then does the ring show
 */
pub fn surface(open: bool, visit: u64, held: bool) -> Rectangle {
    let shape = geometry::CONTROLS;
    let counting = session::counting();
    let grid = grid(counting.as_ref());
    let ring = open
        .then(|| Focus::read().of(visit, held).ring(&grid))
        .flatten();

    let header = Row::new(children![
        Text::new("Session")
            .size(theme::text::TITLE)
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::SEMIBOLD)
    ])
    .width(WIDTH)
    .height(HEADER)
    .align(Center);

    let body: Box<dyn Widget> = match counting {
        Some(counting) => Box::new(countdown(&counting, ring)),
        None => Box::new(tiles(ring)),
    };

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .child(
            Column::new(vec![Box::new(header), body])
                .width(WIDTH)
                .gap(GAP),
        )
}

fn tiles(ring: Option<At>) -> Rectangle {
    let leaves = leaves();
    let width = (WIDTH - (leaves.len() - 1) as f32 * TILE_GAP) / leaves.len() as f32;

    let row = leaves
        .into_iter()
        .map(|leave| Box::new(tile(leave, width, ring == Some(At::Tile(leave)))) as _)
        .collect();

    Rectangle::new()
        .width(WIDTH)
        .height(BODY)
        .align_child(Center, Center)
        .child(Row::new(row).gap(TILE_GAP))
}

// its icon in a knob, then its name; filled while the ring is on it
fn tile(leave: Leave, width: f32, ring: bool) -> Rectangle {
    let knob = Rectangle::new()
        .width(KNOB)
        .height(KNOB)
        .radius(KNOB / 2.0)
        .fill(theme::ISLAND.surface_container_high)
        .align_child(Center, Center)
        .child(icon(leave).on(18.0, theme::ISLAND.on_surface));

    let tile = Rectangle::new()
        .width(width)
        .height(TILE)
        .radius(radius::TILE)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(super::on_left(move || click(At::Tile(leave))))
        .child(
            Column::new(children![
                knob,
                Text::new(name(leave))
                    .size(theme::text::LABEL_SMALL)
                    .color(theme::ISLAND.on_surface)
                    .weight(theme::text::SEMIBOLD)
            ])
            .gap(10.0)
            .align(Center),
        );

    if ring {
        tile.fill(theme::ISLAND.surface_container).border_if(true)
    } else {
        tile
    }
}

// what is left, then Cancel and Now along the bottom
fn countdown(counting: &Counting, ring: Option<At>) -> Column {
    let left = session::left(&counting.countdown, Instant::now());
    let leave = Leave::Ending(counting.end);

    let said = Rectangle::new()
        .width(WIDTH)
        .height(BODY - PILL)
        .align_child(Center, Center)
        .child(
            Column::new(children![
                icon(leave).on(28.0, theme::ISLAND.on_surface_variant),
                Text::new(format!("{} in {left} s", session::doing(counting.end)))
                    .size(theme::text::BODY)
                    .color(theme::ISLAND.on_surface)
                    .weight(theme::text::SEMIBOLD)
            ])
            .gap(6.0)
            .align(Center),
        );

    let pills = Row::new(children![
        pill("Cancel", At::Cancel(*counting), ring),
        pill(session::at_once(counting.end), At::Now(*counting), ring)
    ])
    .gap(PILL_GAP);

    Column::new(children![said, pills])
        .width(WIDTH)
        .align(Center)
}

// outlined, the ring in place of the outline
fn pill(label: &str, at: At, ring: Option<At>) -> Rectangle {
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
        .on_click(super::on_left(move || click(at)))
        .child(
            Text::new(label)
                .size(theme::text::LABEL_SMALL)
                .color(theme::ISLAND.on_surface)
                .weight(theme::text::SEMIBOLD),
        );

    if ring == Some(at) {
        pill.border_if(true)
    } else {
        pill.border(1.0, theme::ISLAND.surface_container_high)
    }
}

pub fn icon(leave: Leave) -> Icon {
    match leave {
        Leave::Lock => Icon::Lock,
        Leave::Sleep => Icon::Moon,
        Leave::Ending(Ending::Restart) => Icon::Restart,
        Leave::Ending(Ending::PowerOff) => Icon::Power,
        Leave::Ending(Ending::LogOut) => Icon::LogOut,
    }
}

pub fn name(leave: Leave) -> &'static str {
    match leave {
        Leave::Lock => "Lock",
        Leave::Sleep => "Sleep",
        Leave::Ending(Ending::Restart) => "Restart",
        Leave::Ending(Ending::PowerOff) => "Power off",
        Leave::Ending(Ending::LogOut) => "Log out",
    }
}

/*
 * a key while this island shows the Session Surface; false for one it does not use, as Escape,
 * which the window's own keys then get to close it
 */
pub fn key(monitor: &str, key: Key) -> bool {
    let (visit, held) = {
        let island = IslandService::read();

        if island.presentation(monitor) != Presentation::Expanded(Surface::Session) {
            return false;
        }

        (island.visit(), island.held(monitor))
    };

    let counting = session::counting();
    let focus = Focus::read().of(visit, held);

    let Some((focus, at)) = focus.step(key, &grid(counting.as_ref())) else {
        return false;
    };

    let focus = match at {
        Some(at) => press(focus, at),
        None => focus,
    };

    set(focus);

    IslandService::write().attend(monitor, Instant::now());

    true
}

// only while open, so a second click as it fades out never asks again
fn click(at: At) {
    let visit = {
        let island = IslandService::read();

        if island.surface() != Some(Surface::Session) {
            return;
        }

        island.visit()
    };
    let focus = Focus {
        shown: false,
        ..Focus::read().of(visit, false)
    };

    set(press(focus, at));
}

/*
 * `at` pressed, giving where that leaves the focus. A restart, power off or log out keeps the
 * Surface open on its countdown, the ring on Cancel, and Cancel brings the ring back to its tile,
 * so a key repeated only counts down again; the rest close it, as does a countdown already gone.
 * One refused shows its refusal on the island, which closes the Surface
 */
fn press(focus: Focus, at: At) -> Focus {
    match at {
        At::Tile(leave) => {
            let requested = session::request(leave);

            if let (Leave::Ending(_), Some(counting)) =
                (leave, requested.as_ref().ok().and(session::counting()))
            {
                return Focus {
                    at: Some(At::Cancel(counting)),
                    ..focus
                };
            }

            // refused, its refusal closed the Surface already
            if requested.is_err() {
                return focus;
            }
        }
        At::Cancel(counting) => {
            session::act(session::CANCEL, &counting.serial.to_string());

            return Focus {
                at: Some(At::Tile(Leave::Ending(counting.end))),
                ..focus
            };
        }
        At::Now(counting) => session::act(session::NOW, &counting.serial.to_string()),
    }

    close();
    focus
}

// the Surface done with
fn close() {
    let monitor = IslandService::read().expanded_on().map(str::to_owned);

    if let Some(monitor) = monitor {
        view::collapse(&monitor);
    }
}

// a write wakes the window even when nothing changed, so only write a real change
fn set(focus: Focus) {
    if *Focus::read() != focus {
        *Focus::write() = focus;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiles() -> Vec<Vec<(At, f32)>> {
        vec![vec![
            (At::Tile(Leave::Sleep), 0.25),
            (At::Tile(Leave::Ending(Ending::Restart)), 0.75),
        ]]
    }

    #[test]
    fn the_first_key_only_shows_the_ring() {
        let (focus, at) = Focus::default().step(Key::Enter, &tiles()).unwrap();

        assert_eq!(at, None);
        assert_eq!(focus.ring(&tiles()), Some(At::Tile(Leave::Sleep)));

        let (focus, _) = focus.step(Key::Right, &tiles()).unwrap();
        let (_, at) = focus.step(Key::Space, &tiles()).unwrap();

        assert_eq!(at, Some(At::Tile(Leave::Ending(Ending::Restart))));
    }

    #[test]
    fn escape_is_the_windows() {
        assert_eq!(Focus::default().step(Key::Escape, &tiles()), None);
    }
}
