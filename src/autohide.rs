//! `island.autohide`: an idle Island slides out past its edge, as macOS's autohidden menu bar, and
//! back as the pointer touches the edge under it or anything shows on it. Idle is at Rest, with no
//! Satellite or Banner, nothing pinned, outside niri's overview, and the pointer away a moment. The
//! Island does not hide while something captures (ADR 0033). The slide is a spring of the Island's
//! motion, so it bounces as the body does and snaps under reduced motion. The Dock autohides alike,
//! and shares how long it takes.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::service::Service;

use crate::config;
use crate::island::geometry::{self, Canvas, Edge, Hang, Rect};
use crate::island::motion::{CRITICAL, Mode, Spring};

// it comes out, and goes once the pointer has been away DELAY
pub const SHOW: Duration = Duration::from_millis(380);
pub const HIDE: Duration = Duration::from_millis(300);
pub const DELAY: Duration = Duration::from_millis(350);

// how thick the strip on the edge is that brings it back
pub const TRIGGER: f32 = 2.0;

/*
 * written as an idle Island's DELAY ends, so it starts to hide having drawn nothing while it
 * waited, and as it starts to slide either way, so what hangs from it, as the Banners, follows; it
 * holds nothing, as the Island reads when it went idle from its own record
 */
pub struct Wakes;

impl Service for Wakes {
    fn new() -> Self {
        Wakes
    }

    fn listen() {}
}

// how far out the Island is on one output, 0 to 100
struct Out {
    spring: Spring<1>,

    // since when it has been idle, none while it is not
    idle: Option<Instant>,
}

/*
 * where an Island is now, whether it still moves, when to wake it, should it have just gone idle,
 * and whether it just started to slide
 */
#[derive(Debug, PartialEq)]
struct Step {
    out: f32,
    moving: bool,
    wake: Option<Instant>,
    slides: bool,
}

impl Out {
    // out when turned on, so it shows where it went before it hides
    fn new(motion: Mode) -> Self {
        Self {
            spring: Spring::new([100.0], motion),
            idle: None,
        }
    }

    fn step(&mut self, idle: bool, now: Instant, motion: Mode, damping: f32) -> Step {
        // a reload may have changed the motion: taken from where it stands
        if self.spring.mode() != motion {
            self.spring = Spring::new(self.spring.at(now), motion);
        }

        let went = idle && self.idle.is_none();

        self.idle = match (idle, self.idle) {
            (true, since) => since.or(Some(now)),
            (false, _) => None,
        };

        let hides = self.idle.is_some_and(|since| now >= since + DELAY);
        let target = if hides { 0.0 } else { 100.0 };
        let slides = self.spring.target() != [target];

        if slides {
            if hides {
                // going, it never passes the edge and swings back into view
                self.spring.damp(CRITICAL);
                self.spring.to([target], HIDE, now);
            } else {
                self.spring.damp(damping);
                self.spring.to([target], SHOW, now);
            }
        }

        Step {
            out: (self.spring.at(now)[0] / 100.0).max(0.0),
            moving: self.spring.mode() == Mode::Spring && !self.spring.settled(now),
            wake: went.then(|| now + DELAY),
            slides,
        }
    }
}

#[derive(Default)]
struct Outs {
    by: HashMap<String, Out>,

    // the outputs that left, until niri lists them again, so a late draw of one adds none back
    left: Vec<String>,
}

impl Outs {
    fn outputs(&mut self, present: &[String]) {
        self.left.retain(|left| !present.contains(left));

        let gone: Vec<String> = self
            .by
            .keys()
            .filter(|monitor| !present.contains(monitor))
            .cloned()
            .collect();

        for monitor in gone {
            self.by.remove(&monitor);
            self.left.push(monitor);
        }
    }

    fn left(&self, monitor: &str) -> bool {
        self.left.iter().any(|left| left == monitor)
    }
}

static OUT: Mutex<Option<Outs>> = Mutex::new(None);

/*
 * how far out the Island on `monitor` is, 0 hidden to 1 out, and whether it still moves; called
 * each time it draws, `idle` or not. A wake redraws every output's Island, each a step that
 * changes nothing but on the one it was for. `forced` hides it as autohide does, when the Island at
 * Rest has nothing to show
 */
pub fn out(monitor: &str, idle: bool, forced: bool, now: Instant) -> (f32, bool) {
    // reading subscribes the Island to its wakes
    drop(Wakes::read());

    let config = config::get();

    let mut outs = OUT.lock().unwrap_or_else(PoisonError::into_inner);

    if !(config.island_place.autohide || forced) {
        // forgets this output, not the others, which may be hiding for lack of a Rest to show
        if let Some(outs) = outs.as_mut() {
            outs.by.remove(monitor);
        }

        return (1.0, false);
    }

    let outs = outs.get_or_insert_with(Outs::default);

    if outs.left(monitor) {
        return (1.0, false);
    }

    let step = outs
        .by
        .entry(monitor.to_owned())
        .or_insert_with(|| Out::new(config.island.motion))
        .step(idle, now, config.island.motion, config.island.damping);

    // an Island no longer idle by then draws once more, for nothing
    if let Some(wake) = step.wake {
        thread::spawn(move || {
            thread::sleep(wake.saturating_duration_since(Instant::now()));
            drop(Wakes::write());
        });
    }

    // what hangs from it, drawn before this, follows from now; the Island draws once more
    if step.slides {
        thread::spawn(|| drop(Wakes::write()));
    }

    (step.out, step.moving)
}

// how far out the Island on `monitor` will be `at` a time to come, as far as it is heading now
pub fn at(monitor: &str, at: Instant) -> f32 {
    OUT.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|outs| outs.by.get(monitor))
        .map_or(1.0, |out| (out.spring.at(at)[0] / 100.0).max(0.0))
}

// whether the Island on `monitor` still slides, so what hangs from it follows it frame by frame
pub fn moving(monitor: &str, now: Instant) -> bool {
    OUT.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|outs| outs.by.get(monitor))
        .is_some_and(|out| out.spring.mode() == Mode::Spring && !out.spring.settled(now))
}

// niri's outputs, as one comes or goes: one replugged comes back out, as new
pub fn outputs(present: &[String]) {
    OUT.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(Outs::default)
        .outputs(present);
}

/*
 * the body `out` of the way out, slid past its edge the rest: hidden, it just clears the screen, so
 * it shows from the start of the way back. Its shadow, which reaches further, the Island fades
 */
pub fn slide(body: Rect, hang: Hang, out: f32) -> Rect {
    let thickness = if hang.sideways() {
        body.width
    } else {
        body.height
    };
    let away = (geometry::TOP + thickness) * (1.0 - out);

    match hang.edge {
        Edge::Top => Rect {
            y: body.y - away,
            ..body
        },
        Edge::Bottom => Rect {
            y: body.y + away,
            ..body
        },
        Edge::Left => Rect {
            x: body.x - away,
            ..body
        },
        Edge::Right => Rect {
            x: body.x + away,
            ..body
        },
    }
}

// the strip on the edge beside a body, which takes the pointer that brings it back
pub fn trigger(body: Rect, hang: Hang, canvas: Canvas) -> Rect {
    match hang.edge {
        Edge::Top => Rect {
            y: 0.0,
            height: TRIGGER,
            ..body
        },
        Edge::Bottom => Rect {
            y: canvas.height - TRIGGER,
            height: TRIGGER,
            ..body
        },
        Edge::Left => Rect {
            x: 0.0,
            width: TRIGGER,
            ..body
        },
        Edge::Right => Rect {
            x: canvas.width - TRIGGER,
            width: TRIGGER,
            ..body
        },
    }
}

/*
 * what takes the pointer, the body `out` of the way out: hidden, only the strip on the edge beside
 * it; else its area, and never less than that strip, so an Island just coming out, its area still
 * past the window, keeps the pointer that brought it
 */
pub fn area(body: Rect, hang: Hang, canvas: Canvas, out: f32) -> Rect {
    let strip = trigger(body, hang, canvas);

    if out <= 0.0 {
        return strip;
    }

    let area = geometry::input_area(slide(body, hang, out), canvas);

    let x = area.x.min(strip.x);
    let y = area.y.min(strip.y);

    Rect {
        x,
        y,
        width: (area.x + area.width).max(strip.x + strip.width) - x,
        height: (area.y + area.height).max(strip.y + strip.height) - y,
    }
}
