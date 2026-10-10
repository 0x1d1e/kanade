//! What is behind a pane, captured from the compositor for a shader (ADR 0037).
//!
//! A view `watch`es a region of an output under a name of its own, each frame it draws; the
//! runtime copies the region with `wlr-screencopy` over its connection, again whenever the screen
//! changes, and a `Rectangle::backdrop` hands the newest copy to the rectangle's shader as a
//! texture. What the shader makes of it is the caller's: the runtime knows regions, not bodies.
//! With no copy, the shader sees a transparent texture.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use smithay_client_toolkit::reexports::calloop::ping::Ping;

use crate::service::Service;

// how far apart the light is read along the ring, in logical pixels
const STEP: f32 = 3.0;

// how far a channel of a copy may stray from the held one, of 255, and the copy still be the same:
// glass drawn over its own edge settles within a level or two, not exactly
const SAME: u8 = 2;

/// A pane of glass on an output, by a name of the caller's, like "island", "dock" or "banner-0".
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Spot {
    pub output: String,
    pub pane: String,
}

impl Spot {
    pub fn new(output: &str, pane: &str) -> Self {
        Self {
            output: output.to_owned(),
            pane: pane.to_owned(),
        }
    }
}

/// A rectangle in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// What to see behind a pane.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// Copied from the output, in its logical pixels and whole ones.
    pub region: Region,

    /// The outline, from the copy's corner, whose mean luma is the copy's light; where the copy
    /// ends short of it, as at the screen's edge, nothing is read.
    pub ring: Region,

    /// Handed back with the copy it was asked for, to the rows of the shader's values.
    pub values: Vec<[f32; 4]>,
}

/// A copy the compositor made: how bright it is around the pane, and what was asked.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    /// 0 to 1.
    pub light: f32,

    pub values: Vec<[f32; 4]>,
}

/// The newest copy of each pane, written as one lands, which redraws only the windows reading it.
#[derive(Debug, Default)]
struct Backdrops {
    seen: HashMap<Spot, Seen>,
}

/// What the newest copy of a pane shows, if there is one. A window that asks draws again as the
/// pane's next copy lands, even where none has yet, and for no other pane's.
pub fn seen(spot: &Spot) -> Option<Seen> {
    Backdrops::read_part(spot).seen.get(spot).cloned()
}

impl Service for Backdrops {
    fn new() -> Self {
        Self::default()
    }

    // written only by the runtime's capture events
    fn listen() {}
}

// a copy's pixels, rgba with the rows top first, which the gpu uploads once per generation
pub(crate) struct Pixels {
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

// what the views asked, for the runtime's event loop to carry out
pub(crate) enum Call {
    Watch(Spot, Option<Request>),
    Release(Spot),
    Pause(bool),
}

#[derive(Default)]
struct Asked {
    // what each pane last asked for, so a view asking each frame sends nothing new
    wanted: HashMap<Spot, Option<Request>>,

    paused: bool,
    calls: Vec<Call>,
}

static ASKED: Mutex<Option<Asked>> = Mutex::new(None);
static PING: OnceLock<Ping> = OnceLock::new();

// whether the compositor can be asked, known once the runtime has connected
static CAPABLE: AtomicBool = AtomicBool::new(false);

static PIXELS: Mutex<Option<HashMap<Spot, Arc<Pixels>>>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn asked() -> MutexGuard<'static, Option<Asked>> {
    ASKED.lock().unwrap_or_else(PoisonError::into_inner)
}

fn copies() -> MutexGuard<'static, Option<HashMap<Spot, Arc<Pixels>>>> {
    PIXELS.lock().unwrap_or_else(PoisonError::into_inner)
}

fn call(call: Call) {
    asked().get_or_insert_default().calls.push(call);

    if let Some(ping) = PING.get() {
        ping.ping();
    }
}

/// Whether the compositor can capture, known once the runtime has connected; a copy may still not
/// have landed.
pub fn capable() -> bool {
    CAPABLE.load(Ordering::Relaxed)
}

/// Asks to see `request` behind a pane, or none to stop looking while keeping the last copy. Called
/// by the views each time they draw, it only asks when the request changed.
pub fn watch(spot: &Spot, request: Option<Request>) {
    if !capable() {
        return;
    }

    {
        let mut guard = asked();
        let asked = guard.get_or_insert_default();

        if asked.wanted.get(spot) == Some(&request) {
            return;
        }

        asked.wanted.insert(spot.clone(), request.clone());
    }

    call(Call::Watch(spot.clone(), request));
}

/// Lets a pane go: its copy is dropped and nothing is captured for it until it is watched again.
pub fn release(spot: &Spot) {
    if !capable() {
        return;
    }

    {
        let mut guard = asked();

        if guard.get_or_insert_default().wanted.remove(spot).is_none() {
            return;
        }
    }

    call(Call::Release(spot.clone()));
}

/// Stops capturing every pane, keeping what each asked for, or captures them all again now, as
/// what is behind them may have changed meanwhile. Callable from any thread.
pub fn pause(paused: bool) {
    {
        let mut guard = asked();
        let asked = guard.get_or_insert_default();

        if asked.paused == paused {
            return;
        }

        asked.paused = paused;
    }

    call(Call::Pause(paused));
}

// the runtime connected, and whether the compositor has screencopy
pub(crate) fn connected(capable: bool, ping: Ping) {
    CAPABLE.store(capable, Ordering::Relaxed);

    let _ = PING.set(ping);

    // asked before the connection: the event loop carries those out at its first wake
    if capable
        && asked()
            .as_ref()
            .is_some_and(|asked| !asked.calls.is_empty())
        && let Some(ping) = PING.get()
    {
        ping.ping();
    }
}

// what the views asked since this last ran
pub(crate) fn take_calls() -> Vec<Call> {
    asked()
        .as_mut()
        .map(|asked| std::mem::take(&mut asked.calls))
        .unwrap_or_default()
}

// a capture that cannot finish: the same request asked again is carried out anew
pub(crate) fn failed(spot: &Spot) {
    if let Some(asked) = asked().as_mut() {
        asked.wanted.remove(spot);
    }
}

// the newest copy of a pane
pub(crate) fn pixels(spot: &Spot) -> Option<Arc<Pixels>> {
    copies().as_ref()?.get(spot).cloned()
}

// a pane's newest copy, and what was asked for it; its light is read here, once
pub(crate) fn arrived(spot: &Spot, request: &Request, width: u32, height: u32, rgba: Vec<u8>) {
    let light = light(&rgba, width, height, request);

    let mut backdrops = Backdrops::write();

    /*
     * a copy of a still screen is the one held: storing it again would redraw the readers, whose
     * next frame asks for another copy, round and round. The glass drawn at a copy's edge is itself
     * in the next copy, off by a level or two each time, so a copy within `SAME` of the held one is it
     */
    let same = backdrops.seen.get(spot).is_some_and(|seen| {
        (seen.light - light).abs() <= f32::from(SAME) / 255.0 && seen.values == request.values
    }) && copies()
        .as_ref()
        .and_then(|copies| copies.get(spot))
        .is_some_and(|held| {
            held.width == width
                && held.height == height
                && held.rgba.len() == rgba.len()
                && held
                    .rgba
                    .iter()
                    .zip(&rgba)
                    .all(|(a, b)| a.abs_diff(*b) <= SAME)
        });

    if same {
        backdrops.quiet();

        return;
    }

    let pixels = Pixels {
        generation: GENERATION.fetch_add(1, Ordering::Relaxed) + 1,
        width,
        height,
        rgba,
    };

    copies()
        .get_or_insert_default()
        .insert(spot.clone(), Arc::new(pixels));

    backdrops.part(spot);

    backdrops.seen.insert(
        spot.clone(),
        Seen {
            light,
            values: request.values.clone(),
        },
    );
}

// a pane's copy dropped, as it is let go or its output is gone
pub(crate) fn dropped(spots: impl Fn(&Spot) -> bool) {
    if let Some(asked) = asked().as_mut() {
        asked.wanted.retain(|spot, _| !spots(spot));
    }

    cleared(spots);
}

// copies dropped, the panes still asked for: what they showed is not to be shown again
pub(crate) fn cleared(spots: impl Fn(&Spot) -> bool) {
    if let Some(copies) = copies().as_mut() {
        copies.retain(|spot, _| !spots(spot));
    }

    let mut backdrops = Backdrops::write();

    let gone: Vec<Spot> = backdrops
        .seen
        .keys()
        .filter(|spot| spots(spot))
        .cloned()
        .collect();

    if gone.is_empty() {
        backdrops.quiet();
    }

    for spot in gone {
        backdrops.part(&spot);
        backdrops.seen.remove(&spot);
    }
}

/*
 * how bright the backdrop just around a pane is, 0 to 1: the mean luma of a ring's outline, for
 * the tint to keep the pane's content readable over it
 */
fn light(rgba: &[u8], width: u32, height: u32, request: &Request) -> f32 {
    let scale = (
        width as f32 / request.region.width,
        height as f32 / request.region.height,
    );

    let ring = request.ring;

    let mut total = 0.0;
    let mut count = 0.0;

    let mut along = |x: f32, y: f32| {
        // past the copy, a sample would only repeat its edge
        if x < 0.0 || y < 0.0 || x >= request.region.width || y >= request.region.height {
            return;
        }

        let column = ((x * scale.0) as usize).min(width as usize - 1);
        let row = ((y * scale.1) as usize).min(height as usize - 1);
        let at = (row * width as usize + column) * 4;

        let luma = 0.2126 * f32::from(rgba[at])
            + 0.7152 * f32::from(rgba[at + 1])
            + 0.0722 * f32::from(rgba[at + 2]);

        total += luma / 255.0;
        count += 1.0;
    };

    let (right, bottom) = (ring.x + ring.width, ring.y + ring.height);

    let mut x = ring.x;

    while x < right {
        along(x, ring.y);
        along(x, bottom);
        x += STEP;
    }

    let mut y = ring.y;

    while y < bottom {
        along(ring.x, y);
        along(right, y);
        y += STEP;
    }

    if count == 0.0 { 0.0 } else { total / count }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes;

    fn request(region: Region, ring: Region) -> Request {
        Request {
            region,
            ring,
            values: Vec::new(),
        }
    }

    const REGION: Region = Region {
        x: 0.0,
        y: 0.0,
        width: 40.0,
        height: 40.0,
    };

    // an even gray copy as big as its region
    fn gray(level: u8) -> Vec<u8> {
        [level, level, level, 255].repeat(40 * 40)
    }

    // the pane's own pixels, inside the ring, never count
    #[test]
    fn what_is_inside_the_ring_is_not_read() {
        let mut rgba = gray(0);

        for row in 10..30 {
            for column in 10..30 {
                rgba[(row * 40 + column) * 4..][..3].fill(255);
            }
        }

        let ring = Region {
            x: 5.0,
            y: 5.0,
            width: 30.0,
            height: 30.0,
        };

        assert_eq!(light(&rgba, 40, 40, &request(REGION, ring)), 0.0);
    }

    // a ring wholly past the copy, as at the screen's edge, has nothing to read
    #[test]
    fn a_ring_past_the_copy_reads_nothing() {
        let ring = Region {
            x: -10.0,
            y: -10.0,
            width: 60.0,
            height: 60.0,
        };

        assert_eq!(light(&gray(255), 40, 40, &request(REGION, ring)), 0.0);
    }

    // a copy of a screen that did not change is not news: it would redraw the readers, which ask again
    #[test]
    fn a_copy_that_equals_the_held_one_is_not_stored_again() {
        let spot = Spot::new("test-output", "same-copy");
        let ring = Region {
            x: 5.0,
            y: 5.0,
            width: 30.0,
            height: 30.0,
        };

        let asked = request(REGION, ring);

        arrived(&spot, &asked, 40, 40, gray(100));
        let first = pixels(&spot).expect("the copy is held").generation;

        arrived(&spot, &asked, 40, 40, gray(100));
        assert_eq!(pixels(&spot).unwrap().generation, first);

        // a level or two off is rounding, not news
        arrived(&spot, &asked, 40, 40, gray(102));
        assert_eq!(pixels(&spot).unwrap().generation, first);

        arrived(&spot, &asked, 40, 40, gray(103));
        let third = pixels(&spot).unwrap().generation;
        assert!(third > first);

        // one pixel far off among the rest is news, whatever the mean says
        let mut dot = gray(103);
        dot[0] = 200;

        arrived(&spot, &asked, 40, 40, dot);
        assert!(pixels(&spot).unwrap().generation > third);

        cleared(|dropped| *dropped == spot);
        assert!(pixels(&spot).is_none());
    }

    // a copy redraws the windows that read its pane, as they would not have seen it, and no other
    #[test]
    fn a_copy_draws_only_the_windows_that_read_its_pane() {
        let (here, there) = (
            Spot::new("test-output", "here"),
            Spot::new("test-output", "there"),
        );
        let asked = request(REGION, REGION);

        // the only test that takes what changed
        changes::take_read();
        let _ = changes::take();

        assert!(seen(&here).is_none());
        let reads_here = changes::take_read();

        assert!(seen(&there).is_none());
        let reads_there = changes::take_read();

        arrived(&here, &asked, 40, 40, gray(10));

        let changed = changes::take().expect("a copy is not a change of everything");
        assert!(changed.touches(&reads_here));
        assert!(!changed.touches(&reads_there));

        // the same copy again is nothing new
        arrived(&here, &asked, 40, 40, gray(10));
        assert!(!changes::take().unwrap().touches(&reads_here));

        cleared(|dropped| *dropped == here);

        let changed = changes::take().unwrap();
        assert!(changed.touches(&reads_here));
        assert!(!changed.touches(&reads_there));

        // nothing held, so nothing to tell
        cleared(|dropped| *dropped == here);
        assert!(!changes::take().unwrap().touches(&reads_here));
    }
}
