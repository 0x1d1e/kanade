//! The Blur finish: the compositor blurs what lies behind each pane, within its outline (ADR 0057).
//! The pane is the tinted solid's ramp, translucent; the window asks `ext-background-effect` (in
//! the runtime) to blur behind the rects this module cuts from the outline. The blur is the
//! compositor's, so Kanade copies nothing of the screen, and what lies behind is blurred whether it
//! is the wallpaper or a window only if the user's niri says so (`kanade doctor`).

use kanade_runtime::InputArea;

use super::Body;
use crate::config;
use crate::look::Finish;

// how opaque a pane's fill is over the blur: enough that light text reads over a bright window
pub const FILL: f32 = 0.78;

// whether panes are Blur now: the finish is, and the compositor can blur. Without the compositor's
// blur the finish is the tinted solid, opaque, as a translucent pane would show what lies behind it sharp
pub fn on() -> bool {
    config::get().appearance.finish == Finish::Blur && kanade_runtime::blur_available()
}

/*
 * the rects of a window the compositor blurs behind, for panes whose outlines are `bodies` in
 * coordinates whose origin is the window's corner at `origin`; none unless panes blur. Whole pixels,
 * each wholly inside its outline, so no blur shows past a pane's edge
 */
pub fn behind(bodies: impl IntoIterator<Item = Body>, origin: (f32, f32)) -> Vec<InputArea> {
    if !on() {
        return Vec::new();
    }

    bodies
        .into_iter()
        .flat_map(|body| strips(body, origin))
        .collect()
}

// a rect of the compositor's blur has no corners, so a rounded outline is a stack of strips
fn strips(body: Body, origin: (f32, f32)) -> Vec<InputArea> {
    let (x, y, width, height) = body.bounds();
    let (x, y) = (x - origin.0, y - origin.1);

    let (left, right) = (x.floor() as i32, (x + width).ceil() as i32);
    let (top, bottom) = (y.floor() as i32, (y + height).ceil() as i32);

    let outline = Outline::of(body, origin);

    let mut areas = Vec::new();

    // consecutive rows with the same spans are one rect: the straight sides are a few big ones
    let mut band: Option<(Vec<(i32, i32)>, i32, i32)> = None;

    for row in top..bottom {
        let spans = spans(&outline, left, right, row as f32 + 0.5);

        match &mut band {
            Some((held, _, rows)) if *held == spans => *rows += 1,
            _ => {
                flush(band.take(), &mut areas);
                band = Some((spans, row, 1));
            }
        }
    }

    flush(band, &mut areas);

    areas
}

fn flush(band: Option<(Vec<(i32, i32)>, i32, i32)>, areas: &mut Vec<InputArea>) {
    let Some((spans, y, rows)) = band else {
        return;
    };

    areas.extend(spans.into_iter().map(|(from, to)| InputArea {
        x: from,
        y,
        width: to - from,
        height: rows,
    }));
}

/*
 * the runs of pixels along a row at height `y` whose centers lie inside the outline by half a pixel,
 * from `left` to `right`. The distance is a bound, so a step of half of it never crosses the edge
 */
fn spans(outline: &Outline, left: i32, right: i32, y: f32) -> Vec<(i32, i32)> {
    let mut spans = Vec::new();
    let mut start = None;
    let mut x = left;

    while x < right {
        let d = outline.distance(x as f32 + 0.5, y);

        match (d <= -0.5, start) {
            (true, None) => start = Some(x),
            (false, Some(from)) => {
                spans.push((from, x));
                start = None;
            }
            _ => {}
        }

        x += ((d.abs() * 0.5) as i32).max(1);
    }

    if let Some(from) = start {
        spans.push((from, right));
    }

    spans
}

// the outline `pane.wgsl` fills, in the window's coordinates: one rounded rect, or two united
struct Outline {
    first: Rect,
    second: Option<(Rect, f32)>,
}

// center, half size and radius
type Rect = ((f32, f32), (f32, f32), f32);

// how squarely a corner is drawn, as in `pane.wgsl`
const SQUIRCLE: f32 = 4.0;

impl Outline {
    fn of(body: Body, origin: (f32, f32)) -> Self {
        let rect = |x: f32, y: f32, width: f32, height: f32, radius: f32| -> Rect {
            (
                (x - origin.0 + width / 2.0, y - origin.1 + height / 2.0),
                (width / 2.0, height / 2.0),
                radius.min(width / 2.0).min(height / 2.0),
            )
        };

        Self {
            first: rect(body.x, body.y, body.width, body.height, body.radius),
            second: body.join.map(|join| {
                (
                    rect(join.x, join.y, join.width, join.height, join.radius),
                    join.blend,
                )
            }),
        }
    }

    // `pane.wgsl`'s `outline`: negative inside
    fn distance(&self, x: f32, y: f32) -> f32 {
        let a = rounded(x, y, self.first);

        match self.second {
            Some((second, blend)) => united(a, rounded(x, y, second), blend),
            None => a,
        }
    }
}

// `pane.wgsl`'s `rounded`
fn rounded(x: f32, y: f32, ((cx, cy), (half_x, half_y), r): Rect) -> f32 {
    let (qx, qy) = ((x - cx).abs() - half_x + r, (y - cy).abs() - half_y + r);
    let (outer_x, outer_y) = (qx.max(0.0), qy.max(0.0));

    let t = ((r / half_x.min(half_y).max(1e-4) - 0.6) / 0.4).clamp(0.0, 1.0);
    let n = SQUIRCLE + (2.0 - SQUIRCLE) * t * t * (3.0 - 2.0 * t);
    let norm = (outer_x.powf(n) + outer_y.powf(n)).powf(1.0 / n);

    norm + qx.max(qy).min(0.0) - r
}

// `pane.wgsl`'s `united`
fn united(a: f32, b: f32, blend: f32) -> f32 {
    let pull = (blend - (a - b).abs()).max(0.0) / blend.max(1e-4);

    a.min(b) - pull * pull * blend * 0.25
}
