//! What a Liquid Glass pane asks the runtime to copy of the screen around it, and what it gets back
//! (ADRs 0037, 0052). A pane of any other finish asks for nothing.

use std::collections::HashMap;
use std::sync::Mutex;

use kanade_runtime::backdrop::{self, Region, Request};
use kanade_runtime::{Horizontal, Vertical};
use kanade_runtime::{Monitor, MutexExt};

use crate::look::Anchor;

use super::glass::{COPY_ROWS, NEAR, Seen};
use super::{Body, Spot, glass};

// how far out the edge reads the screen at most, as a share of the output's height (the studio's
// refraction distance), and the widest a channel of the dispersion stretches it
const REACH: f32 = 0.02;
const SPREAD: f32 = 1.02;

// how much further out than its body a pane's copy reaches, past the edge's widest read
fn margin(screen: (u32, u32)) -> f32 {
    (REACH * screen.1 as f32 * SPREAD).ceil() + 4.0
}

// how far out from its body the light of the backdrop is read, in logical pixels
const RING: f32 = 12.0;

/*
 * where each captured pane lies on its output, as x, y, width and height: a pane's copy holds the
 * others' pixels as part of the screen, so a pane tells the shader where they are, to read none of
 * them (ADR 0052)
 */
static LYING: Mutex<Option<HashMap<Spot, [f32; 4]>>> = Mutex::new(None);

// how far a neighbour's rect is grown, to cover its rounded corners and its edge's anti-aliasing
const SHADOW: f32 = 2.0;

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
 * the rects wholly inside a body's outline, `x` and `y` being its corner in the copy: each of its
 * rects less what its rounded corners cut off, which is at most 30% of the radius at a corner
 */
fn inside(body: Body, x: f32, y: f32) -> Vec<Region> {
    let rect = |at: (f32, f32), width: f32, height: f32, radius: f32| {
        let inset = radius.min(width / 2.0).min(height / 2.0) * 0.3 + 1.0;

        Region {
            x: at.0 + inset,
            y: at.1 + inset,
            width: (width - 2.0 * inset).max(0.0),
            height: (height - 2.0 * inset).max(0.0),
        }
    };

    // `body` is placed in its canvas, `x` and `y` is where its bounds start
    let (bx, by, _, _) = body.bounds();
    let at = |rx: f32, ry: f32| (x + rx - bx, y + ry - by);

    let mut rects = vec![rect(
        at(body.x, body.y),
        body.width,
        body.height,
        body.radius,
    )];

    if let Some(join) = body.join {
        rects.push(rect(
            at(join.x, join.y),
            join.width,
            join.height,
            join.radius,
        ));
    }

    rects
}

/*
 * what to copy around a body whose canvas has its corner at `origin` on a screen: its bounds and
 * `margin` around them, within the screen, in whole pixels; none where nothing of it is on the screen
 */
fn request(
    body: Body,
    origin: (f32, f32),
    screen: (u32, u32),
    others: &[[f32; 4]],
) -> Option<Request> {
    let (x, y, width, height) = body.bounds();
    let (x, y) = (origin.0 + x, origin.1 + y);

    let margin = margin(screen);

    let left = (x - margin).floor().max(0.0);
    let top = (y - margin).floor().max(0.0);
    let right = (x + width + margin).ceil().min(screen.0 as f32);
    let bottom = (y + height + margin).ceil().min(screen.1 as f32);

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

    // the other panes on the copy, the largest first, in its pixels
    let mut near: Vec<(f32, [f32; 4])> = others
        .iter()
        .filter_map(|&[ox, oy, ow, oh]| {
            let (ox, oy) = (ox - SHADOW - left, oy - SHADOW - top);
            let (ow, oh) = (ow + 2.0 * SHADOW, oh + 2.0 * SHADOW);

            let covered = (ox + ow).min(region.width) - ox.max(0.0);
            let reaching = (oy + oh).min(region.height) - oy.max(0.0);

            (covered > 0.0 && reaching > 0.0).then_some((covered * reaching, [ox, oy, ow, oh]))
        })
        .collect();

    near.sort_by(|a, b| b.0.total_cmp(&a.0));
    near.truncate(NEAR);

    // what the shader reads none of: the screen inside the body, and the panes it skips
    let mut unread: Vec<Region> = inside(body, x - left, y - top);

    unread.extend(near.iter().map(|&(_, [x, y, width, height])| Region {
        x,
        y,
        width,
        height,
    }));

    values.extend(
        near.into_iter()
            .map(|(_, rect)| rect)
            .chain(std::iter::repeat([0.0; 4]))
            .take(NEAR),
    );

    // how far out the edge reads at most, in pixels, which is 0 while there is no copy
    values.push([REACH * screen.1 as f32, 0.0, 0.0, 0.0]);

    Some(Request {
        region,
        ring: Region {
            x: x - RING - left,
            y: y - RING - top,
            width: width + 2.0 * RING,
            height: height + 2.0 * RING,
        },
        unread,
        values,
    })
}

/*
 * where a glass pane lies: its body in the window, `canvas` big and anchored as `anchor`, none
 * while it shows none; called each time the pane draws. Says whether it is captured, which is the runtime able to and the finish Liquid Glass
 */
pub fn place(
    monitor: &Monitor,
    pane: &str,
    anchor: Anchor,
    canvas: (f32, f32),
    body: Option<Body>,
) -> bool {
    let spot = Spot::new(&monitor.name, pane);

    // any other finish lets go of its copy, as when the setting turns
    let glass = glass();
    let body = body.filter(|_| glass);

    // rounded, as the runtime sizes the window
    let canvas = (canvas.0.round(), canvas.1.round());

    let screen = (monitor.width, monitor.height);

    let origin = origin(anchor, canvas, screen);

    let others = {
        let mut lying = LYING.locked();
        let lying = lying.get_or_insert_default();

        match body {
            Some(body) => {
                let (x, y, width, height) = body.bounds();

                lying.insert(spot.clone(), [origin.0 + x, origin.1 + y, width, height]);
            }
            None => {
                lying.remove(&spot);
            }
        }

        let mut others: Vec<(&Spot, [f32; 4])> = lying
            .iter()
            .filter(|(other, _)| other.output == spot.output && **other != spot)
            .map(|(other, rect)| (other, *rect))
            .collect();

        // a stable order, so the same panes ask for the same copy
        others.sort_by(|a, b| a.0.cmp(b.0));
        others.into_iter().map(|(_, rect)| rect).collect::<Vec<_>>()
    };

    backdrop::watch(
        &spot,
        body.and_then(|body| request(body, origin, screen, &others)),
    );

    glass && backdrop::capable()
}

/*
 * what a pane shows of the captured area, if it is there; reading it redraws the pane once a copy
 * lands, even where it could not be placed yet
 */
pub fn shown(spot: &Spot) -> Option<Seen> {
    if !glass() {
        return None;
    }

    let seen = backdrop::seen(spot)?;

    let place = <[[f32; 4]; COPY_ROWS]>::try_from(seen.values.as_slice()).ok();

    Some(Seen {
        spot: spot.clone(),
        place,
    })
}

// a pane let go, as a Banner's as it leaves: its copy is dropped, and nothing is captured until it is placed again
pub fn forget(spot: &Spot) {
    if let Some(lying) = LYING.locked().as_mut() {
        lying.remove(spot);
    }

    backdrop::release(spot);
}
