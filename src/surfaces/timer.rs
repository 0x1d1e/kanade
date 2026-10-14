//! The Timer Surface: a countdown to start with a tile each (`PRESETS`) or a pomodoro, and while
//! one runs its time left with Pause or Resume, Skip for a pomodoro, and Cancel. It asks the
//! timer (`sources::timer`) as `kanade timer` does, so the island shows it at once. The arrows
//! move a ring between the targets, Enter or Space presses it.

use std::time::{Duration, Instant};

use kanade_runtime::service::Service;
use kanade_runtime::{Key, Rectangle};

use super::pad::{self, Focus};
use crate::icon::Icon;
use crate::island::activity::{Countdown, Round};
use crate::island::presentation::Surface;
use crate::sources::timer::{self, Lengths, Request};

const MINUTE: u64 = 60;

// the countdowns a tile starts, by seconds; the CLI takes any length
const PRESETS: [(&str, u64); 4] = [
    ("5 min", 5 * MINUTE),
    ("10 min", 10 * MINUTE),
    ("25 min", 25 * MINUTE),
    ("1 hour", 60 * MINUTE),
];

// what the ring can be on
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum At {
    // the countdown of `PRESETS` at this place
    Preset(usize),
    Pomodoro,
    Pause,
    Resume,
    Skip,
    Cancel,
}

impl Service for Focus<At> {
    fn new() -> Self {
        Focus::default()
    }

    fn listen() {}
}

// the targets: the tiles, or what a running timer takes; a pomodoro alone has Skip
fn grid(running: Option<&(Countdown, Option<Round>)>) -> Vec<Vec<(At, f32)>> {
    let row: Vec<At> = match running {
        None => (0..PRESETS.len())
            .map(At::Preset)
            .chain([At::Pomodoro])
            .collect(),
        Some((countdown, round)) => {
            let mut row = vec![if countdown.paused.is_some() {
                At::Resume
            } else {
                At::Pause
            }];

            if round.is_some() {
                row.push(At::Skip);
            }

            row.push(At::Cancel);
            row
        }
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
 * the tiles to start one, or the one running. `open` says the Surface is open rather than fading
 * out, so only then does the ring show
 */
pub fn surface(open: bool, visit: u64, held: bool) -> Rectangle {
    let running = timer::standing();
    let grid = grid(running.as_ref());
    let ring = open
        .then(|| Focus::<At>::read().of(visit, held).ring(&grid))
        .flatten();

    match running {
        Some((countdown, round)) => pad::frame("Timer", reading(&countdown, round, ring)),
        None => pad::frame("Timer", tiles(ring)),
    }
}

fn tiles(ring: Option<At>) -> Rectangle {
    let width = pad::tile_width(PRESETS.len() + 1);

    let mut row: Vec<Box<dyn kanade_runtime::Widget>> = PRESETS
        .iter()
        .enumerate()
        .map(|(place, (name, _))| {
            let at = At::Preset(place);

            Box::new(pad::tile(
                Icon::Stopwatch,
                name,
                width,
                ring == Some(at),
                move || click(at),
            )) as _
        })
        .collect();

    row.push(Box::new(pad::tile(
        Icon::Pulse,
        "Pomodoro",
        width,
        ring == Some(At::Pomodoro),
        || click(At::Pomodoro),
    )));

    pad::tiles(row)
}

// the time left, then what the timer takes along the bottom
fn reading(
    countdown: &Countdown,
    round: Option<Round>,
    ring: Option<At>,
) -> impl kanade_runtime::Widget + use<> {
    let clock = timer::clock(countdown, Instant::now());

    let line = match (countdown.paused, round) {
        (Some(_), Some(round)) => format!("{} paused  {clock}", timer::phase(round.phase)),
        (Some(_), None) => format!("Paused  {clock}"),
        (None, Some(round)) => format!("{}  {clock}", timer::phase(round.phase)),
        (None, None) => clock,
    };

    let said = pad::reading(Icon::Stopwatch, line);

    let pills = pad::pills(
        grid(Some(&(*countdown, round)))
            .remove(0)
            .into_iter()
            .map(|(at, _)| Box::new(pad::pill(label(at), ring == Some(at), move || click(at))) as _)
            .collect(),
    );

    pad::stacked(said, pills)
}

fn label(at: At) -> &'static str {
    match at {
        At::Preset(place) => PRESETS[place].0,
        At::Pomodoro => "Pomodoro",
        At::Pause => "Pause",
        At::Resume => "Resume",
        At::Skip => "Skip",
        At::Cancel => "Cancel",
    }
}

// a key while this island shows the Timer Surface; false for one it does not use, as Escape
pub fn key(monitor: &str, key: Key) -> bool {
    pad::key(
        monitor,
        Surface::Timer,
        key,
        || grid(timer::standing().as_ref()),
        press,
    )
}

fn click(at: At) {
    pad::click(Surface::Timer, at, press);
}

/*
 * `at` pressed. The timer starts or changes on its own thread and the Surface follows it, so the
 * ring goes to the first target of whatever it then shows, as Pause becomes Resume
 */
fn press(focus: Focus<At>, at: At) -> Focus<At> {
    let request = match at {
        At::Preset(place) => Request::Start(Duration::from_secs(PRESETS[place].1)),
        At::Pomodoro => Request::Pomodoro(Lengths::default()),
        At::Pause => Request::Pause,
        At::Resume => Request::Resume,
        At::Skip => {
            timer::request(Request::Skip(None));

            return focus;
        }
        At::Cancel => Request::Cancel,
    };

    timer::request(request);

    Focus {
        ring: focus.ring.top(),
    }
}
