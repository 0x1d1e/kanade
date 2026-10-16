//! The Capture Surface: a screenshot of an area, the focused window or the focused output, a color
//! picked from the screen, and the screen recording, each a tile; while one records, the last tile
//! is Stop. Pills below record an area or the focused window and choose the recording's audio (none, the speakers' or the
//! microphone's, which is only ever chosen here, and stays chosen only while Kanade runs); while one
//! records, the pill pauses or resumes it. It asks what `kanade capture` asks, so a recording and a picked color show on the island
//! and a screenshot by niri's notification. A screenshot or a pick waits for the island to leave
//! the screen first, or it would be in it.
//! The arrows move a ring between the tiles, Enter or Space presses it.

use std::thread;
use std::time::Duration;

use kanade_runtime::service::Service;
use kanade_runtime::{Key, Rectangle};

use super::pad::{self, Focus};
use super::store;
use crate::icon::Icon;
use crate::island::presentation::Surface;
use crate::island::service::IslandService;
use crate::sources::capture::{self, Mode};
use crate::sources::recording::{self, Audio, Request, Standing, Start, Target};

// the island's collapse and the compositor's frame after it, so a screenshot does not catch it
pub const CLEAR: Duration = Duration::from_millis(400);

// what the ring can be on
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum At {
    Shot(Mode),
    Color,
    Record,
    Stop,

    // the pills: an area or the focused window recorded, the audio chosen, the recording paused or
    // taken up again
    Area,
    Window,
    Sound,
    Pause,
    Resume,
}

// the audio the next recording hears, as it was last chosen; never saved, so the microphone is
// never on from an earlier session
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Chosen(Audio);

impl Service for Chosen {
    fn new() -> Self {
        Chosen::default()
    }

    fn listen() {}
}

impl Service for Focus<At> {
    fn new() -> Self {
        Focus::default()
    }

    fn listen() {}
}

// the targets: the tiles, the last one ending the recording while it runs, then the pills
fn grid(standing: &Standing) -> Vec<Vec<(At, f32)>> {
    let recording = standing.path.is_some();
    let tiles = [
        At::Shot(Mode::Area),
        At::Shot(Mode::Window),
        At::Shot(Mode::Output),
        At::Color,
        if recording { At::Stop } else { At::Record },
    ];
    let pills = match (recording, standing.pausable, standing.paused) {
        (false, ..) => vec![At::Area, At::Window, At::Sound],
        (true, true, true) => vec![At::Resume],
        (true, true, false) => vec![At::Pause],
        (true, false, _) => Vec::new(),
    };

    let spread = |row: &[At]| -> Vec<(At, f32)> {
        let count = row.len() as f32;

        row.iter()
            .enumerate()
            .map(|(index, &at)| (at, (index as f32 + 0.5) / count))
            .collect()
    };

    let mut grid = vec![spread(&tiles)];

    if !pills.is_empty() {
        grid.push(spread(&pills));
    }

    grid
}

/*
 * the tiles. `standing` says how a recording stands, from the island (`recording::standing`). `open`
 * says the Surface is open rather than fading out, so only then does the ring show
 */
pub fn surface(open: bool, visit: u64, held: bool, standing: Standing) -> Rectangle {
    let mut grid = grid(&standing);
    let ring = open
        .then(|| Focus::<At>::read().of(visit, held).ring(&grid))
        .flatten();

    let width = pad::tile_width(grid[0].len());

    let tiles = grid
        .remove(0)
        .into_iter()
        .map(|(at, _)| {
            let (icon, name) = look(at);

            Box::new(pad::tile(
                node(at),
                icon,
                name,
                width,
                ring == Some(at),
                move || click(at),
            )) as _
        })
        .collect();

    let Some(pills) = grid.pop() else {
        return pad::frame("Capture", pad::tiles(tiles));
    };

    let pills = pad::pills(
        pills
            .into_iter()
            .map(|(at, _)| {
                Box::new(pad::pill(
                    node(at),
                    label(at),
                    ring == Some(at),
                    move || click(at),
                )) as _
            })
            .collect(),
    );

    pad::frame("Capture", pad::tiled(tiles, pills))
}

// what `kanade ui` names its tile
fn node(at: At) -> &'static str {
    match at {
        At::Shot(Mode::Area) => "area",
        At::Shot(Mode::Window) => "window",
        At::Shot(Mode::Output) => "screen",
        At::Color => "color",
        At::Record => "record",
        At::Stop => "stop",
        At::Area => "record-area",
        At::Window => "record-window",
        At::Sound => "audio",
        At::Pause => "pause",
        At::Resume => "resume",
    }
}

fn look(at: At) -> (Icon, &'static str) {
    match at {
        At::Shot(Mode::Area) => (Icon::Grid, "Area"),
        At::Shot(Mode::Window) => (Icon::Picture, "Window"),
        At::Shot(Mode::Output) => (Icon::Capture, "Screen"),
        At::Color => (Icon::Palette, "Color"),
        At::Record => (Icon::Camera, "Record"),
        At::Stop => (Icon::Dismiss, "Stop"),
        At::Area | At::Window | At::Sound | At::Pause | At::Resume => {
            unreachable!("a pill is not a tile")
        }
    }
}

// the words of a pill; the audio's is the one chosen
fn label(at: At) -> &'static str {
    match at {
        At::Area => "Rec Area",
        At::Window => "Rec Window",
        At::Sound => Chosen::read().0.choice(),
        At::Pause => "Pause",
        At::Resume => "Resume",
        _ => unreachable!("a tile is not a pill"),
    }
}

// a key while this island shows the Capture Surface; false for one it does not use, as Escape
pub fn key(monitor: &str, key: Key) -> bool {
    pad::key(
        monitor,
        Surface::Capture,
        key,
        || grid(&recording::standing(&IslandService::read())),
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

            spawn("capture-surface", move || {
                if let Err(why) = capture::screenshot(mode) {
                    eprintln!("capture: {why}");
                }
            });
        }
        At::Color => {
            pad::close();
            spawn("capture-color", || match capture::color() {
                // the picker left, which is no failure
                Ok(_) => {}
                Err(why) => eprintln!("capture: {why}"),
            });
        }
        At::Record => {
            pad::close();
            recording::press(Request::Start(Start {
                target: Target::Screen,
                audio: Chosen::read().0,
            }));
        }
        At::Area => {
            pad::close();
            recording::press(Request::Start(Start {
                target: Target::Area(None),
                audio: Chosen::read().0,
            }));
        }
        // the focused window, which the Surface is not: it is the island's, not a window of niri's
        At::Window => {
            pad::close();
            recording::press(Request::Start(Start {
                target: Target::Window(None),
                audio: Chosen::read().0,
            }));
        }
        // the Surface stays, to show what was chosen
        At::Sound => {
            // the read is let go before `store` writes the same service
            let next = Chosen::read().0.next();

            store(Chosen(next));
        }
        At::Stop => {
            pad::close();
            capture::act(recording::STOP, &stopping());
        }
        At::Pause => {
            pad::close();
            capture::act(recording::PAUSE, &stopping());
        }
        At::Resume => {
            pad::close();
            capture::act(recording::RESUME, &stopping());
        }
    }

    focus
}

// runs `take` on a thread of its own once the island is gone, so what it takes does not hold it
fn spawn(name: &str, take: impl FnOnce() + Send + 'static) {
    let spawned = thread::Builder::new()
        .name(String::from(name))
        .spawn(move || {
            thread::sleep(CLEAR);
            take();
        });

    if let Err(error) = spawned {
        eprintln!("capture: {error}");
    }
}

// the path of the recording to stop, pause or resume, none if it ended since the tile was drawn
fn stopping() -> String {
    recording::recording(&IslandService::read()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    // a press on the audio pill once read and wrote its service in one statement, which waits forever
    #[test]
    fn the_audio_pill_cycles_without_deadlocking() {
        let (done, pressed) = mpsc::channel();

        thread::spawn(move || {
            let before = Chosen::read().0;

            press(Focus::default(), At::Sound);

            done.send((before, Chosen::read().0)).ok();
        });

        let (before, after) = pressed
            .recv_timeout(Duration::from_secs(10))
            .expect("the press deadlocked");

        assert_eq!(after, before.next());
    }
}
