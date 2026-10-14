//! The Capture Surface: a screenshot of an area, the focused window or the focused output, and the
//! screen recording, each a tile; while one records, the last tile is Stop. It asks what
//! `kanade capture` asks, so a recording shows on the island and a screenshot by niri's
//! notification. A screenshot waits for the island to leave the screen first, or it would be in it.
//! The arrows move a ring between the tiles, Enter or Space presses it.

use std::thread;
use std::time::Duration;

use kanade_runtime::service::Service;
use kanade_runtime::{Key, Rectangle};

use super::pad::{self, Focus};
use crate::icon::Icon;
use crate::island::presentation::Surface;
use crate::island::service::IslandService;
use crate::later::Later;
use crate::sources::capture::{self, Mode};
use crate::sources::recording::{self, Request};

// the island's collapse and the compositor's frame after it, so a screenshot does not catch it
const CLEAR: Duration = Duration::from_millis(400);

// what the ring can be on
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum At {
    Shot(Mode),
    Record,
    Stop,
}

impl Service for Focus<At> {
    fn new() -> Self {
        Focus::default()
    }

    fn listen() {}
}

// the targets, the last one ending the recording while it runs
fn grid(recording: bool) -> Vec<Vec<(At, f32)>> {
    let row = [
        At::Shot(Mode::Area),
        At::Shot(Mode::Window),
        At::Shot(Mode::Output),
        if recording { At::Stop } else { At::Record },
    ];
    let count = row.len() as f32;

    vec![
        row.into_iter()
            .enumerate()
            .map(|(index, at)| (at, (index as f32 + 0.5) / count))
            .collect(),
    ]
}

/*
 * the tiles. `recording` says one runs, from the island (`recording::recording`). `open` says the
 * Surface is open rather than fading out, so only then does the ring show
 */
pub fn surface(open: bool, visit: u64, held: bool, recording: bool) -> Rectangle {
    let grid = grid(recording);
    let ring = open
        .then(|| Focus::<At>::read().of(visit, held).ring(&grid))
        .flatten();

    let width = pad::tile_width(grid[0].len());

    let row = grid[0]
        .iter()
        .map(|&(at, _)| {
            let (icon, name) = look(at);

            Box::new(pad::tile(icon, name, width, ring == Some(at), move || {
                click(at)
            })) as _
        })
        .collect();

    pad::frame("Capture", pad::tiles(row))
}

fn look(at: At) -> (Icon, &'static str) {
    match at {
        At::Shot(Mode::Area) => (Icon::Grid, "Area"),
        At::Shot(Mode::Window) => (Icon::Picture, "Window"),
        At::Shot(Mode::Output) => (Icon::Capture, "Screen"),
        At::Record => (Icon::Camera, "Record"),
        At::Stop => (Icon::Dismiss, "Stop"),
    }
}

// a key while this island shows the Capture Surface; false for one it does not use, as Escape
pub fn key(monitor: &str, key: Key) -> bool {
    pad::key(
        monitor,
        Surface::Capture,
        key,
        || grid(recording::recording(&IslandService::read()).is_some()),
        press,
    )
}

fn click(at: At) {
    pad::click(Surface::Capture, at, press);
}

/*
 * `at` pressed. Every press closes the Surface, which the capture then shows its result on: a
 * screenshot is taken once it is gone, off the draw thread; the recorder is told on it, which
 * must be it
 */
fn press(focus: Focus<At>, at: At) -> Focus<At> {
    match at {
        At::Shot(mode) => {
            pad::close();

            let taken = thread::Builder::new()
                .name(String::from("capture-surface"))
                .spawn(move || {
                    thread::sleep(CLEAR);

                    if let Err(why) = capture::screenshot(mode) {
                        eprintln!("capture: {why}");
                    }
                });

            if let Err(error) = taken {
                eprintln!("capture: {error}");
            }
        }
        At::Record => {
            pad::close();
            recording::request(Request::Start, Later::detached());
        }
        At::Stop => {
            pad::close();
            capture::act(recording::STOP, &stopping());
        }
    }

    focus
}

// the path of the recording to stop, none if it ended since the tile was drawn
fn stopping() -> String {
    recording::recording(&IslandService::read()).unwrap_or_default()
}
