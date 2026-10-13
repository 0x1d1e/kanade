//! What a pane asks the runtime to copy of the screen behind it, and what it gets back (ADR 0037).

use kanade_runtime::Monitor;
use kanade_runtime::backdrop::{self, Region, Request};
use kanade_runtime::{Horizontal, Vertical};

use crate::look::Anchor;

use super::pane::{COPY_ROWS, Seen};
use super::{Body, Spot};

// how much further out than its body a pane's copy reaches for the frost samples
const MARGIN: f32 = 50.0;

// how far out from its body the light of the backdrop is read, in logical pixels
const RING: f32 = 12.0;

// the lock screen holds the session, or let it go, as `kanade-lock` says (ADR 0025)
pub fn locked(locked: bool) {
    backdrop::pause(locked);
}

// the canvas's top left corner on its output, anchored as the window is
pub fn origin(anchor: Anchor, canvas: (f32, f32), screen: (u32, u32)) -> (f32, f32) {
    let (screen_width, screen_height) = (screen.0 as f32, screen.1 as f32);

    let x = match anchor.horizontal() {
        Horizontal::Left => 0.0,
        Horizontal::Middle => ((screen_width - canvas.0) / 2.0).floor(),
        Horizontal::Right => screen_width - canvas.0,
    };

    let y = match anchor.vertical() {
        Vertical::Top => 0.0,
        Vertical::Middle => ((screen_height - canvas.1) / 2.0).floor(),
        Vertical::Bottom => screen_height - canvas.1,
    };

    (x, y)
}

/*
 * what to copy around a body whose canvas has its corner at `origin` on a screen: its bounds and
 * `MARGIN` around them, within the screen, in whole pixels; none where nothing of it is on the screen
 */
fn request(body: Body, origin: (f32, f32), screen: (u32, u32)) -> Option<Request> {
    let (x, y, width, height) = body.bounds();
    let (x, y) = (origin.0 + x, origin.1 + y);

    let left = (x - MARGIN).floor().max(0.0);
    let top = (y - MARGIN).floor().max(0.0);
    let right = (x + width + MARGIN).ceil().min(screen.0 as f32);
    let bottom = (y + height + MARGIN).ceil().min(screen.1 as f32);

    if right <= left || bottom <= top {
        return None;
    }

    let region = Region {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    };

    let mut values = body.rows().to_vec();

    // the body's corner in the copy
    values.push([x - left, y - top, region.width, region.height]);

    Some(Request {
        region,
        ring: Region {
            x: x - RING - left,
            y: y - RING - top,
            width: width + 2.0 * RING,
            height: height + 2.0 * RING,
        },
        values,
    })
}

/*
 * where a frosted pane lies: its body in the window, `canvas` big and anchored as `anchor`, none
 * while it shows none; called each time the pane draws. Says whether the runtime can capture
 */
pub fn place(
    monitor: &Monitor,
    pane: &str,
    anchor: Anchor,
    canvas: (f32, f32),
    body: Option<Body>,
) -> bool {
    let spot = Spot::new(&monitor.name, pane);

    // rounded, as the runtime sizes the window
    let canvas = (canvas.0.round(), canvas.1.round());

    let screen = (monitor.width, monitor.height);

    backdrop::watch(
        &spot,
        body.and_then(|body| request(body, origin(anchor, canvas, screen), screen)),
    );

    backdrop::capable()
}

/*
 * what a pane shows of the captured area, if it is there; reading it redraws the pane once a copy
 * lands, even where it could not be placed yet
 */
pub fn shown(spot: &Spot) -> Option<Seen> {
    let seen = backdrop::seen(spot)?;

    let place = <[[f32; 4]; COPY_ROWS]>::try_from(seen.values.as_slice()).ok();

    Some(Seen {
        spot: spot.clone(),
        light: seen.light,
        place,
    })
}

// a pane let go, as a Banner's as it leaves: its copy is dropped, and nothing is captured until it is placed again
pub fn forget(spot: &Spot) {
    backdrop::release(spot);
}
