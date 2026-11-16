//! The Blur finish: the compositor blurs what lies behind each pane (ADR 0057, ADR 0153).
//!
//! Under niri the pane is the tinted solid's ramp, translucent, and the window asks
//! `ext-background-effect` (in the runtime) to blur behind the rects this module cuts from the
//! outline. Under Umbriel there is no such protocol: the pane is translucent, and Umbriel's own
//! `blur-kanade` layer rule blurs the layer surface where its alpha passes. The blur is the
//! compositor's either way, so Kanade copies nothing of the screen.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use kanade_runtime::{InputArea, MutexExt};

use super::Body;
use crate::config;
use crate::look::Finish;
use crate::sources::compositor::{self, Compositor};

// how opaque a pane's fill is over the blur: enough that light text reads over a bright window
pub const FILL: f32 = 0.78;

// whether the finish is translucent with what each compositor path offers: niri's protocol, or
// Umbriel's own layer rule. Either is enough, and neither leaves the opaque tinted solid behind
fn translucent_with(finish: Finish, protocol: bool, umbriel_rule: bool) -> bool {
    finish == Finish::Blur && (protocol || umbriel_rule)
}

/*
 * whether panes are translucent now: the finish is Blur and the compositor will show its blur
 * behind them, through `ext-background-effect` or, under Umbriel, its own layer rule. Without one
 * the finish is the tinted solid, opaque, as a translucent pane would show what lies behind it sharp
 */
pub fn translucent() -> bool {
    translucent_with(
        config::get().appearance.finish,
        kanade_runtime::blur_available(),
        compositor::active() == Compositor::Umbriel && umbriel_rule(),
    )
}

// whether the runtime may ask the protocol to blur behind the rects this module cuts; Umbriel has
// no such request, only its compositor's rule, so a finish there never sends one
fn behind_available() -> bool {
    config::get().appearance.finish == Finish::Blur && kanade_runtime::blur_available()
}

// how long a read of Umbriel's config stands before its metadata is asked again; an edit shows
// within this, and the file is re-read only when its path, mtime or length changed
const RULE_FRESH: Duration = Duration::from_millis(500);

#[derive(Default)]
struct RuleCache {
    path: Option<PathBuf>,
    modified: Option<(i64, i64, u64)>,
    active: bool,
    checked: Option<Instant>,
}

// pane draws are frequent, so the config is not parsed per draw: one cache, read again only when
// its metadata says the file may have changed
static RULE: LazyLock<Mutex<RuleCache>> = LazyLock::new(Mutex::default);

// whether Umbriel's config has the `blur-kanade` layer rule: none when the config is missing,
// unreadable or not TOML
fn umbriel_rule() -> bool {
    let Ok(path) = crate::umbriel_config::target(None) else {
        return false;
    };

    let mut cache = RULE.locked();
    let now = Instant::now();

    if cache.path.as_deref() == Some(path.as_path())
        && cache
            .checked
            .is_some_and(|checked| now.duration_since(checked) < RULE_FRESH)
    {
        return cache.active;
    }

    let modified = fs::metadata(&path)
        .ok()
        .map(|meta| (meta.mtime(), meta.mtime_nsec(), meta.len()));

    if cache.path.as_deref() == Some(path.as_path())
        && modified.is_some()
        && cache.modified == modified
    {
        cache.checked = Some(now);
        return cache.active;
    }

    let active = fs::read_to_string(&path)
        .is_ok_and(|text| crate::umbriel_config::rules::is_rule_active(&text, "blur-kanade"));

    cache.path = Some(path);
    cache.modified = modified;
    cache.active = active;
    cache.checked = Some(now);

    active
}

/*
 * whether a pane showing `shown` of itself, 1 whole and 0 gone, blurs what lies behind it: the whole
 * time any of it shows, and never once it is gone. The compositor's blur cannot fade, so a threshold
 * inside a fade pops it off and on mid-fade, under a pane that is still there to be read; gone is the
 * only moment at which it can go without being missed
 */
pub fn blurs(shown: f32) -> bool {
    shown > 0.0
}

/*
 * the rects of a window the compositor blurs behind, for panes whose outlines are `bodies` in
 * coordinates whose origin is the window's corner at `origin`; none unless panes blur. Whole pixels,
 * each wholly inside its outline, so no blur shows past a pane's edge
 */
pub fn behind(bodies: impl IntoIterator<Item = Body>, origin: (f32, f32)) -> Vec<InputArea> {
    if !behind_available() {
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
    let mut band: Option<Band> = None;

    for row in top..bottom {
        let spans = spans(&outline, left, right, row as f32 + 0.5);

        match &mut band {
            Some(band) if band.spans == spans => band.rows += 1,
            _ => {
                if let Some(band) = band.take() {
                    band.flush(&mut areas);
                }

                band = Some(Band {
                    spans,
                    y: row,
                    rows: 1,
                });
            }
        }
    }

    if let Some(band) = band {
        band.flush(&mut areas);
    }

    areas
}

// rows from `y` on that hold the same runs of pixels, each from and to
struct Band {
    spans: Vec<(i32, i32)>,
    y: i32,
    rows: i32,
}

impl Band {
    fn flush(self, areas: &mut Vec<InputArea>) {
        areas.extend(self.spans.into_iter().map(|(from, to)| InputArea {
            x: from,
            y: self.y,
            width: to - from,
            height: self.rows,
        }));
    }
}

/*
 * the runs of pixels along a row whose centers lie inside by half a pixel. Inspect each curved
 * edge pixel; skip only the solid rectangular core that is provably inside the outline.
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

        x = outline.solid_until(x, y).unwrap_or(x + 1).max(x + 1);
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

#[derive(Clone, Copy)]
struct Rect {
    center: (f32, f32),
    half: (f32, f32),
    corner: kanade_runtime::continuous::ContinuousCorner,
    shoulder: f32,
}

impl Outline {
    fn of(body: Body, origin: (f32, f32)) -> Self {
        let rect = |x: f32, y: f32, width: f32, height: f32, radius: f32| -> Rect {
            Rect {
                center: (x - origin.0 + width / 2.0, y - origin.1 + height / 2.0),
                half: (width / 2.0, height / 2.0),
                corner: kanade_runtime::continuous::ContinuousCorner::new(
                    radius,
                    width / 2.0,
                    height / 2.0,
                ),
                shoulder: (radius * 1.72).min(width / 2.0),
            }
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

    fn solid_until(&self, x: i32, y: f32) -> Option<i32> {
        std::iter::once(self.first)
            .chain(self.second.map(|(rect, _)| rect))
            .filter_map(|rect| {
                let (cx, cy) = rect.center;
                let (w, h) = rect.half;
                let end = cx + w - rect.shoulder - 0.5;
                (x as f32 + 0.5 >= cx - w + rect.shoulder
                    && x as f32 + 0.5 <= end
                    && (y - cy).abs() <= h - 0.5)
                    .then_some(end.floor() as i32)
            })
            .max()
    }
}

// `pane.wgsl`'s `rounded`
fn rounded(x: f32, y: f32, rect: Rect) -> f32 {
    rect.corner.distance(
        rect.half.0 - (x - rect.center.0).abs(),
        rect.half.1 - (y - rect.center.1).abs(),
    )
}

// `pane.wgsl`'s `united`: the exact circular fillet, and exactly the nearer side beyond it
fn united(a: f32, b: f32, blend: f32) -> f32 {
    if (a - b).abs() >= blend {
        return a.min(b);
    }

    let u = (blend - a).max(0.0);
    let v = (blend - b).max(0.0);

    blend.max(a.min(b)) - (u * u + v * v).sqrt()
}

#[cfg(test)]
mod tests {
    use super::{blurs, translucent_with, united};
    use crate::look::Finish;

    // the join is an exact circular fillet: its deepest point is (sqrt(2) - 1) of the radius, it is
    // symmetric, and it is the nearer contour wherever the two are farther apart than the radius
    #[test]
    fn a_united_outline_is_an_exact_circular_fillet() {
        let k = 9.0;

        assert!((united(0.0, 0.0, k) + (2.0f32.sqrt() - 1.0) * k).abs() < 1e-4);

        for (a, b) in [
            (0.0, 0.0),
            (0.0, k),
            (-2.0, 0.5),
            (1.5, 4.0),
            (-6.0, -1.0),
            (12.0, 13.0),
        ] {
            assert!((united(a, b, k) - united(b, a, k)).abs() < 1e-4);
        }

        for (a, b) in [(0.0, k), (-4.0, 6.0), (-20.0, -4.0), (12.0, 13.0)] {
            assert!((united(a, b, k) - a.min(b)).abs() < 1e-4);
        }

        // no radius, no fillet
        assert!((united(1.0, 2.0, 0.0) - 1.0).abs() < 1e-4);
    }

    // translucent through either compositor path, and opaque only when neither offers one
    #[test]
    fn the_finish_is_translucent_through_either_compositor_path() {
        // niri: the protocol alone
        assert!(translucent_with(Finish::Blur, true, false));
        // Umbriel: its layer rule alone, with no region request
        assert!(translucent_with(Finish::Blur, false, true));
        // neither: the opaque tinted solid
        assert!(!translucent_with(Finish::Blur, false, false));
        // both say the same
        assert!(translucent_with(Finish::Blur, true, true));

        // the solid finishes stay opaque whatever the compositor offers
        assert!(!translucent_with(Finish::TintedSolid, true, true));
        assert!(!translucent_with(Finish::Paper, false, true));
    }

    // blurred for as long as any of the pane shows, which is the whole of a fade, and not at all once
    // it is gone: a pane gone asks for no outline, so the window asks for no blur
    #[test]
    fn a_pane_blurs_while_any_of_it_shows() {
        assert!(!blurs(0.0));
        assert!(blurs(0.5));
        assert!(blurs(1.0));
    }
}
