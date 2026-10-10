//! Banners (ADRs 0007, 0023): each notification as it arrives, on the focused output, hanging from
//! the Island on its edge and side: each comes out of the Island's body, as its twin stretching
//! into a card, and goes back into it. UI over `notifications`, which keeps the bus, the history
//! and DND; closing a Banner leaves the notification in the history. Plain text only, and an
//! action runs only on a click, never from the keyboard.

mod motion;
mod stack;

use std::cell::RefCell;
use std::collections::HashMap;
use std::iter;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{LazyLock, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crate::sources::notifications::{Notification, Notifications, Urgency};
use kanade_runtime::service::Service;
use kanade_runtime::{
    Button, Center, Column, Cursor, Horizontal, InputArea, Layer, LayerWindow, Monitor, Padding,
    Rectangle, Row, Size, Stack as Layers, Start, Text, Vertical, Widget, Zone, children,
    request_frame,
};

use motion::Motion;
pub use stack::Banner;
use stack::{MOST, Stack};

use crate::icon::Icon;
use crate::island::geometry::{self, Canvas, Edge, Hang, Rect, Shape, Side};
use crate::island::service::IslandService;
use crate::look::{Anchor, Entrance};
use crate::merge;
use crate::scene::Stand;
use crate::shadow::{self, ShadowStyle};
use crate::sources::notifications;
use crate::theme::space::TARGET;
use crate::theme::{self, ThemeRoles, radius};
use crate::{autohide, config, dock, glass, modules, view};

pub(crate) const WIDTH: f32 = 360.0;
pub(crate) const GAP: f32 = 8.0;
const INSET: f32 = 12.0;

const TILE: f32 = 36.0;
const TILE_GAP: f32 = 10.0;

// past this many a Banner shows only the first ones; the sender's own window has the rest
const MOST_ACTIONS: usize = 3;
const ACTION_WIDTH: f32 = 120.0;
const PILL_PADDING: f32 = 10.0;

// the body wraps to at most this many lines, elided past them
const BODY_LINES: usize = 2;

/*
 * how far out of the Island a morphing Banner is when it has parted from the body and shows whole,
 * and when it takes the pointer and what it says starts to show
 */
const PARTS: f32 = 0.3;
const TAKES: f32 = 0.6;

// how small a fading Banner starts
const FADE_SCALE: f32 = 0.92;

// how each output's Banners move, kept between draws
static MOTIONS: LazyLock<Mutex<HashMap<String, Motion>>> = LazyLock::new(Mutex::default);

// per output, which pane of liquid glass each Banner draws in
static PANES: LazyLock<Mutex<HashMap<String, Panes>>> = LazyLock::new(Mutex::default);

// the tallest a Banner gets, as its text lays out the same for good
static TALLEST: OnceLock<f32> = OnceLock::new();

/*
 * a deadline change nudges listen(), which otherwise sleeps until the next deadline; one pending
 * nudge is enough, since listen() reads the deadline again when it wakes
 */
static NUDGE: LazyLock<(SyncSender<()>, Mutex<Receiver<()>>)> = LazyLock::new(|| {
    let (sender, receiver) = mpsc::sync_channel(1);

    (sender, Mutex::new(receiver))
});

/*
 * written by the notifications and niri sources, and by a Banner's own pointer input; a write
 * wakes every Banners window even when nothing changed, so every writer checks first
 */
pub struct Banners {
    stack: Stack,

    // from niri; none while unknown or without niri, which counts every monitor as focused
    focused_output: Option<String>,
}

impl Service for Banners {
    fn new() -> Self {
        Self {
            stack: Stack::default(),
            focused_output: None,
        }
    }

    // sleeps until a Banner's time runs out or a nudge, so no Banner showing never wakes
    fn listen() {
        let receiver = NUDGE.1.lock().unwrap_or_else(PoisonError::into_inner);

        loop {
            // no deadline waits forever, recv_timeout falls back to recv on overflow
            let wait = Self::read()
                .stack
                .deadline()
                .map_or(Duration::MAX, |deadline| {
                    deadline.saturating_duration_since(Instant::now())
                });

            if receiver.recv_timeout(wait) != Err(RecvTimeoutError::Timeout) {
                continue;
            }

            let now = Instant::now();

            if Self::read()
                .stack
                .deadline()
                .is_some_and(|deadline| deadline <= now)
            {
                Self::write().stack.expire(now);
            }
        }
    }
}

impl Banners {
    pub fn arrive(&mut self, banner: Banner, dnd: bool, now: Instant) {
        self.stack.arrive(banner, dnd, now);
        nudge();
    }

    pub fn contains(&self, id: u32) -> bool {
        self.stack.contains(id)
    }

    pub fn close(&mut self, id: u32, now: Instant) {
        self.stack.close(id, now);
        nudge();
    }

    // Banners follow the focus to its output; the pointer is not on the new one yet
    pub fn focus(&mut self, output: Option<String>, now: Instant) {
        self.focused_output = output;
        self.stack.hover(false, now);
        nudge();
    }

    // newest first, and only on the focused output
    fn shown_on(&self, monitor: &str) -> Vec<Banner> {
        let focused = self
            .focused_output
            .as_deref()
            .is_none_or(|focused| focused == monitor);

        if focused {
            self.stack.shown().cloned().collect()
        } else {
            Vec::new()
        }
    }
}

/*
 * whether any Banner shows on `monitor`, so the Island it hangs from stays out under autohide
 * rather than sliding them from under the pointer; reading subscribes to them
 */
pub fn showing(monitor: &str) -> bool {
    modules::on("banners") && !Banners::read().shown_on(monitor).is_empty()
}

// niri's outputs, as one comes or goes: one unplugged mid-way forgets its Banners' places
pub fn outputs(present: &[String]) {
    MOTIONS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|monitor, _| present.contains(monitor));
    PANES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|monitor, _| present.contains(monitor));
}

fn nudge() {
    // full means a nudge is already pending, which is all listen() needs
    let _ = NUDGE.0.try_send(());
}

impl Banner {
    pub fn of(notification: &Notification) -> Banner {
        Banner {
            id: notification.id(),
            toast: notifications::toast(notification),
            urgency: notification.urgency(),
            default: notification.has_default_action(),
            actions: notification
                .actions()
                .iter()
                .take(MOST_ACTIONS)
                .map(|action| (action.key().to_owned(), action.label().to_owned()))
                .collect(),
        }
    }
}

// DND came on: only Critical Banners stay
pub fn silence(now: Instant) {
    if modules::on("banners") && Banners::read().stack.silences() {
        Banners::write().stack.silence(now);
        nudge();
    }
}

/*
 * one window per monitor, over the Island's and laid out as it, and taller by room for the
 * Banners hanging from it, or on a side edge wider by room for them beside it; shown on the focused one while any Banner shows or goes back in, so it
 * draws nothing else
 */
pub fn window(monitor: &Monitor) -> LayerWindow {
    // reading subscribes this window to the Banners
    let shown = Banners::read().shown_on(&monitor.name);

    let now = Instant::now();
    let config = config::get();
    let anchor = config.island_place.anchor;
    let hang = anchor.hang();

    let canvas = view::canvas(monitor);
    let stand = Stand { canvas, hang };
    let tallest = *TALLEST.get_or_init(tallest);
    let reach = ShadowStyle::AMBIENT.reach() as f32;

    let drawing = !shown.is_empty()
        || MOTIONS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(&monitor.name);

    // merged with the Dock, the Island's body stands further from its edge, past the Dock's plate;
    // reading subscribes this window to the apps, so only while it draws
    let apart = if drawing { merge::apart() } else { 0.0 };
    let full = canvas.height + apart + MOST as f32 * (tallest + GAP) + reach;

    // beside an Island on a side edge, past its largest body
    let width = if hang.sideways() {
        let wide = canvas.width + GAP + WIDTH + reach;

        if monitor.width > 0 {
            wide.min(monitor.width as f32).max(canvas.width)
        } else {
            wide
        }
    } else {
        canvas.width
    }
    .round();

    /*
     * never past the output, nor so short a window cannot be made, when its size is unknown; whole
     * pixels, as the runtime rounds the window's, so the Island's body lies where `shift` says.
     * Beside an Island on a side edge, as tall as the output, the two centered on it
     */
    let height = if monitor.height > 0 && hang.sideways() {
        monitor.height as f32
    } else if monitor.height > 0 {
        full.min(monitor.height as f32).max(canvas.height)
    } else {
        full
    }
    .round();

    // the Island's window is at the edge, or in the middle of a side, so its body lies this far into this one
    let shift = Shift {
        x: if hang.edge == Edge::Right {
            width - canvas.width
        } else {
            0.0
        },
        y: match hang.edge {
            Edge::Top => apart,
            Edge::Bottom => height - canvas.height - apart,
            Edge::Left | Edge::Right => ((height - canvas.height) / 2.0).round(),
        },
    };

    // reading subscribes this window to the apps, so only while it draws
    let dock = if drawing {
        dock::strip(monitor)
            .map(|(edge, strip)| (edge, within(strip, monitor, anchor, (width, height))))
    } else {
        None
    };
    let lane = Lane::of(hang, (width, height), rest_body(hang, canvas, shift), dock);

    let heights: HashMap<u32, f32> = shown
        .iter()
        .map(|banner| (banner.id, measured(banner)))
        .collect();

    /*
     * those that would hang past the column's end wait in the stack: past the Island's body as far
     * as it reaches now or where it heads, so they go before a Surface opening pushes them off the
     * output and come back once it closes. Reading subscribes this window to the Island
     */
    let from = if shown.is_empty() {
        lane.from(rest_body(hang, canvas, shift), hang).1
    } else {
        let island = IslandService::read();
        let from = |shape: Shape| {
            let body = stand.body(stand.pose(shape, 1.0));

            lane.from(shift.of(body), hang).1
        };
        let now_from = from(island.shape(&monitor.name, now));
        let heading_from = island.heading(&monitor.name, now).map_or(now_from, from);

        if hang.edge == Edge::Bottom {
            now_from.min(heading_from)
        } else {
            now_from.max(heading_from)
        }
    };
    let shown = fitting(shown, &heights, from, lane.end, hang);

    let (placed, later, moving) = {
        let mut motions = MOTIONS.lock().unwrap_or_else(PoisonError::into_inner);
        let motion = motions.entry(monitor.name.clone()).or_default();

        motion.follow(
            &shown,
            |banner| heights.get(&banner.id).copied().unwrap_or_default(),
            GAP,
            config.island.motion,
            config.island.damping,
            now,
        );

        let mut placed = motion.placed(now);

        // as it heads now, to place each card ahead by its own pane's lead
        let later = motion.clone();
        let moving = motion.moving(now);

        if motion.is_empty() {
            motions.remove(&monitor.name);
        }

        /*
         * oldest first, so a new one comes out over the others, and one gone is let go from among
         * them rather than from the start, as fewer of the others' targets move
         */
        placed.sort_by_key(|placed| placed.banner.id);

        (placed, later, moving)
    };

    // each Banner's pane of liquid glass, and those let go, which the next Banner takes afresh
    let (panes, freed) = {
        let mut all = PANES.lock().unwrap_or_else(PoisonError::into_inner);
        let panes = all.entry(monitor.name.clone()).or_default();
        let ids: Vec<u32> = placed.iter().map(|placed| placed.banner.id).collect();
        let freed = panes.follow(&ids);

        (panes.by.clone(), freed)
    };

    for pane in freed {
        glass::forget(&glass::Spot::new(&monitor.name, &self::pane(pane)));
    }

    let mut cards: Vec<Box<dyn Widget>> = Vec::new();
    let mut areas = Vec::new();

    if !placed.is_empty() {
        // reading subscribes this window to the Island, which the Banners hang from, and its slide
        let island = IslandService::read();
        drop(autohide::Wakes::read());

        // under a fullscreen window the cards are not seen, so capture nothing
        let covered = view::covered(monitor, || island.overview());

        // the body moving, they follow it
        if moving || !island.settled(&monitor.name, now) || autohide::moving(&monitor.name, now) {
            request_frame();
        }

        // how a Banner draws `at` a time, as far as it is heading now
        let look = |placed: &motion::Placed, at: Instant| {
            let pose = stand.pose(
                island.shape(&monitor.name, at),
                autohide::at(&monitor.name, at),
            );
            let body = shift.of(stand.body(pose));
            let (x, base) = lane.from(body, hang);
            let card_height = heights
                .get(&placed.banner.id)
                .copied()
                .unwrap_or_else(|| measured(&placed.banner));
            let card = Rect {
                x,
                y: if hang.edge == Edge::Bottom {
                    base - placed.slot - card_height
                } else {
                    base + placed.slot
                },
                width: WIDTH,
                height: card_height,
            };

            entering(
                config.entrance,
                body,
                pose.shape.radius,
                card,
                hang,
                (width, height),
                placed.out,
            )
        };

        let roles = theme::island();

        for placed in &placed {
            let card_height = heights
                .get(&placed.banner.id)
                .copied()
                .unwrap_or_else(|| measured(&placed.banner));
            let now_look = look(placed, now);
            let at = now_look.at;
            let body = liquid_body(at, now_look.radius);

            let spot = glass::Spot::new(&monitor.name, &pane(panes[&placed.banner.id]));

            /*
             * out to everywhere the card goes until its rim shows, and to where it was a quarter of
             * that before, as the Island's body is
             */
            let since = now.checked_sub(glass::LEAD / 4).unwrap_or(now);
            let captured = iter::once(since)
                .chain((1..=4).map(|step| now + glass::LEAD * step / 4))
                .filter_map(|at| {
                    let ahead = later
                        .placed(at)
                        .into_iter()
                        .find(|ahead| ahead.banner.id == placed.banner.id)?;
                    let look = look(&ahead, at);

                    Some(liquid_body(look.at, look.radius))
                })
                .fold(body, glass::ahead);

            // another material stops the glass whole, so the pane is told only where it is
            let liquid = glass::place(
                monitor,
                &spot.pane,
                anchor,
                (width, height),
                (now_look.seen > 0.0 && !covered).then_some(captured),
            );
            let backdrop = glass::shown(&spot, body);

            /*
             * liquid glass casts none, as it would gray what its rim bends; cast or not, it takes as
             * many layers, so the targets of the cards after it keep their place
             */
            let mut shadow = Vec::new();

            if liquid {
                shadow::none(&mut shadow);
            } else {
                shadow::draw(&mut shadow, at, now_look.radius);
            }

            cards.push(Box::new(
                Layers::new(children![
                    Rectangle::new()
                        .width(width)
                        .height(height)
                        .align_child(Start, Start)
                        .opacity(now_look.shadow)
                        .child(Layers::new(shadow)),
                    drawn(&placed.banner, &now_look, backdrop, card_height, &roles),
                ])
                .width(width)
                .height(height),
            ));

            // nearly in place, it takes the pointer; going, it takes none
            if placed.out >= TAKES && !placed.leaving {
                areas.push(outward(at));
            }
        }
    }

    LayerWindow::new()
        .width(width)
        .height(height)
        .anchor_vertical(anchor.vertical())
        .anchor_horizontal(anchor.horizontal())
        .layer(Layer::Top)
        .space(Zone::Ignore)
        .namespace("kanade-banners")
        .visible(!placed.is_empty())
        .input_region(areas)
        .child(
            // the input region is the Banners, so this is on while the pointer is on any of them
            Rectangle::new()
                .width(width)
                .height(height)
                .align_child(Start, Start)
                .on_hover(hover)
                .child(Layers::new(cards).width(width).height(height)),
        )
}

// a pane of liquid glass on an output, as a Banner draws in
fn pane(index: usize) -> String {
    format!("banner-{index}")
}

/*
 * which pane each Banner on an output draws in: its own while it shows, so one going never hands
 * its rims to another, and the lowest free one for one new
 */
#[derive(Debug, Default)]
struct Panes {
    by: HashMap<u32, usize>,
}

impl Panes {
    // follows the Banners `ids` drawn now; says the panes let go, even one another Banner took
    fn follow(&mut self, ids: &[u32]) -> Vec<usize> {
        let mut freed: Vec<usize> = self
            .by
            .iter()
            .filter(|(id, _)| !ids.contains(id))
            .map(|(_, pane)| *pane)
            .collect();

        self.by.retain(|id, _| ids.contains(id));

        for id in ids {
            if !self.by.contains_key(id) {
                let free = (0..)
                    .find(|pane| !self.by.values().any(|taken| taken == pane))
                    .unwrap_or_default();

                self.by.insert(*id, free);
            }
        }

        freed.sort_unstable();
        freed
    }
}

/*
 * how tall a Banner lays out, kept while it shows as it was, so a frame lays out none of the text
 * again
 */
fn measured(banner: &Banner) -> f32 {
    thread_local! {
        static MEASURED: RefCell<HashMap<u32, (Banner, f32)>> = RefCell::new(HashMap::new());
    }

    MEASURED.with_borrow_mut(|measured| {
        if let Some((known, height)) = measured.get(&banner.id)
            && known == banner
        {
            return *height;
        }

        // the most a stack shows, and those going, so this stays small
        if measured.len() > 4 * MOST {
            measured.clear();
        }

        let height = card_height(banner);
        measured.insert(banner.id, (banner.clone(), height));

        height
    })
}

fn liquid_body(at: Rect, radius: f32) -> glass::Body {
    glass::Body {
        x: at.x,
        y: at.y,
        width: at.width,
        height: at.height,
        radius,
        join: None,
    }
}

// what takes the pointer for a card mid-way: whole pixels out to its edge, as it draws
fn outward(at: Rect) -> InputArea {
    let (left, top) = (at.x.floor(), at.y.floor());

    InputArea {
        x: left as i32,
        y: top as i32,
        width: ((at.x + at.width).ceil() - left) as i32,
        height: ((at.y + at.height).ceil() - top) as i32,
    }
}

// how far into this window the Island's canvas lies
#[derive(Debug, Clone, Copy, PartialEq)]
struct Shift {
    x: f32,
    y: f32,
}

impl Shift {
    // a rect in the Island's canvas, in this window
    fn of(self, rect: Rect) -> Rect {
        Rect {
            x: rect.x + self.x,
            y: rect.y + self.y,
            ..rect
        }
    }
}

// the Island's body at rest, in this window
fn rest_body(hang: Hang, canvas: Canvas, shift: Shift) -> Rect {
    shift.of(geometry::body(
        geometry::upright(geometry::REST, hang),
        hang,
        canvas,
    ))
}

/*
 * the Banners `shown`, nearest the Island first, that hang between `from` and the column's `end`;
 * the first always, clipped if it must be
 */
fn fitting(
    shown: Vec<Banner>,
    heights: &HashMap<u32, f32>,
    from: f32,
    end: f32,
    hang: Hang,
) -> Vec<Banner> {
    // none past the end, where the Island's body already reaches
    let room = if hang.edge == Edge::Bottom {
        from - end
    } else {
        end - from
    }
    .max(0.0);
    let mut reached = 0.0;

    shown
        .into_iter()
        .enumerate()
        .take_while(|(index, banner)| {
            reached += heights.get(&banner.id).copied().unwrap_or_default();
            let fits = *index == 0 || reached <= room;
            reached += GAP;

            fits
        })
        .map(|(_, banner)| banner)
        .collect()
}

/*
 * where in this window the Banners line up. Under a top Island they hang down, over a bottom one
 * up, at `x` across the window, from no nearer the Island's edge than `clear`, to no farther than
 * `end`. Beside one on a side edge they hang down from level with its top, no nearer that edge
 * than `x` (no farther, on the right), from no higher than `clear` to no lower than `end`. Kept
 * clear of the Dock's strip: past it on the Island's edge, short of it on the far one, and aside
 * of it on an edge across the column, wherever the column would cover it from the Island at rest
 * to its end, so the column never steps aside as the Island grows
 */
#[derive(Debug, Clone, Copy, PartialEq)]
struct Lane {
    x: f32,
    clear: f32,
    end: f32,
}

impl Lane {
    fn of(hang: Hang, size: (f32, f32), rest: Rect, dock: Option<(Edge, Rect)>) -> Self {
        if hang.sideways() {
            return Self::beside(hang, size, rest, dock);
        }

        let (width, height) = size;
        let up = hang.edge == Edge::Bottom;
        let mut lane = Lane {
            x: column_x(hang, width),
            clear: if up { height - GAP } else { GAP },
            end: if up { 0.0 } else { height },
        };

        let Some((edge, strip)) = dock else {
            return lane;
        };

        let across = lane.x < strip.right() && strip.x < lane.x + WIDTH;
        let (top, bottom) = if up {
            (lane.end, rest.y)
        } else {
            (rest.y + rest.height, lane.end)
        };
        let along = top < strip.y + strip.height && strip.y < bottom;

        match (edge, up) {
            (Edge::Top, false) if across => {
                lane.clear = lane.clear.max(strip.y + strip.height + GAP)
            }
            (Edge::Top, true) if across => lane.end = lane.end.max(strip.y + strip.height + GAP),
            (Edge::Bottom, false) if across => lane.end = lane.end.min(strip.y - GAP),
            (Edge::Bottom, true) if across => lane.clear = lane.clear.min(strip.y - GAP),
            (Edge::Left, _) if across && along => lane.x = strip.right() + GAP,
            (Edge::Right, _) if across && along => lane.x = strip.x - GAP - WIDTH,
            _ => {}
        }

        lane.x = lane.x.clamp(0.0, (width - WIDTH).max(0.0));
        lane
    }

    // beside an Island on a side edge, the column at rest past its body
    fn beside(
        hang: Hang,
        (width, height): (f32, f32),
        rest: Rect,
        dock: Option<(Edge, Rect)>,
    ) -> Self {
        let left = hang.edge == Edge::Left;
        let mut lane = Lane {
            x: if left { 0.0 } else { width - WIDTH },
            clear: GAP,
            end: height,
        };

        let Some((edge, strip)) = dock else {
            return lane;
        };

        let (top, bottom) = (rest.y, lane.end);
        let along = top < strip.y + strip.height && strip.y < bottom;

        match edge {
            Edge::Left if left && along => lane.x = strip.right() + GAP,
            Edge::Right if !left && along => lane.x = strip.x - GAP - WIDTH,
            _ => {}
        }

        let (x, _) = lane.from(rest, hang);
        let across = x < strip.right() && strip.x < x + WIDTH;

        match edge {
            Edge::Top if across => lane.clear = lane.clear.max(strip.y + strip.height + GAP),
            Edge::Bottom if across => lane.end = lane.end.min(strip.y - GAP),
            _ => {}
        }

        lane
    }

    /*
     * where the first Banner hangs from, its `x` and the `y` the column starts at: past the
     * Island's `body` as it is now, so they follow it as it grows
     */
    fn from(self, body: Rect, hang: Hang) -> (f32, f32) {
        match hang.edge {
            Edge::Top => (self.x, (body.y + body.height + GAP).max(self.clear)),
            Edge::Bottom => (self.x, (body.y - GAP).min(self.clear)),
            Edge::Left => ((body.right() + GAP).max(self.x), body.y.max(self.clear)),
            Edge::Right => ((body.x - GAP - WIDTH).min(self.x), body.y.max(self.clear)),
        }
    }
}

// lined up with the Island's side, or centered on it, under a top Island and over a bottom one
fn column_x(hang: Hang, width: f32) -> f32 {
    match hang.across() {
        Side::Start => geometry::TOP,
        Side::Middle => (width - WIDTH) / 2.0,
        Side::End => width - geometry::TOP - WIDTH,
    }
}

// a rect on the output as it lies in this window, `size` and laid out as the Island's on `anchor`
fn within(rect: Rect, monitor: &Monitor, anchor: Anchor, (width, height): (f32, f32)) -> Rect {
    let x = match anchor.horizontal() {
        Horizontal::Left => 0.0,
        Horizontal::Middle => ((monitor.width as f32 - width) / 2.0).round(),
        Horizontal::Right => monitor.width as f32 - width,
    };
    let y = match anchor.vertical() {
        Vertical::Top => 0.0,
        Vertical::Middle => ((monitor.height as f32 - height) / 2.0).round(),
        Vertical::Bottom => monitor.height as f32 - height,
    };

    Rect {
        x: rect.x - x,
        y: rect.y - y,
        ..rect
    }
}

// how a Banner draws `out` of the way in
struct Look {
    at: Rect,
    radius: f32,

    // how much what it says shows, the whole of it, and its shadow
    says: f32,
    seen: f32,
    shadow: f32,
}

/*
 * a Banner `out` of the way to its `card`, as `banners.entrance` says: out of the Island's `body`,
 * in from the edge, or fading in place; past its card as the spring swings
 */
fn entering(
    entrance: Entrance,
    body: Rect,
    body_radius: f32,
    card: Rect,
    hang: Hang,
    size: (f32, f32),
    out: f32,
) -> Look {
    let shown = out.clamp(0.0, 1.0);

    match entrance {
        /*
         * the body's twin, of its glass, showing as it parts from the body so it never covers
         * what the body shows, and what it says, laid out at its full size, once it is nearly out
         */
        Entrance::Morph => Look {
            at: morph(body, card, out),
            radius: mix(body_radius, radius::CARD, shown),
            says: ((out - TAKES) / (1.0 - TAKES)).clamp(0.0, 1.0),
            seen: (out / PARTS).clamp(0.0, 1.0),
            shadow: shown,
        },

        // from just past the edge, behind which it hides, the window `size` wide and tall
        Entrance::Drop => {
            let (width, height) = size;
            let at = match hang.edge {
                Edge::Top => Rect {
                    y: card.y - (card.y + card.height) * (1.0 - out),
                    ..card
                },
                Edge::Bottom => Rect {
                    y: card.y + (height - card.y) * (1.0 - out),
                    ..card
                },
                Edge::Left => Rect {
                    x: card.x - card.right() * (1.0 - out),
                    ..card
                },
                Edge::Right => Rect {
                    x: card.x + (width - card.x) * (1.0 - out),
                    ..card
                },
            };

            Look {
                at,
                radius: radius::CARD,
                says: 1.0,
                seen: 1.0,

                // none past the edge, where it would show as a sliver
                shadow: shown,
            }
        }

        // a little small, growing to its card as it shows
        Entrance::Fade => {
            let scale = mix(FADE_SCALE, 1.0, out);
            let width = card.width * scale;
            let tall = card.height * scale;

            Look {
                at: Rect {
                    x: card.x + (card.width - width) / 2.0,
                    y: card.y + (card.height - tall) / 2.0,
                    width,
                    height: tall,
                },
                radius: radius::CARD,
                says: 1.0,
                seen: shown,
                shadow: shown,
            }
        }
    }
}

// a Banner's card as it morphs between the body's twin and its card
fn morph(body: Rect, card: Rect, out: f32) -> Rect {
    Rect {
        x: mix(body.x, card.x, out),
        y: mix(body.y, card.y, out),
        width: mix(body.width, card.width, out).max(0.0),
        height: mix(body.height, card.height, out).max(0.0),
    }
}

fn mix(from: f32, to: f32, amount: f32) -> f32 {
    from + (to - from) * amount
}

/*
 * the card as `look` says, what it says laid out at its full `height`, clipped to where it is, of
 * the Island's glass, with what liquid glass sees behind it
 */
fn drawn(
    banner: &Banner,
    look: &Look,
    backdrop: Option<glass::Seen>,
    height: f32,
    roles: &ThemeRoles,
) -> Rectangle {
    let at = look.at;

    let layers: Vec<Box<dyn Widget>> = vec![
        Box::new(glass::layer(glass::Pane {
            width: at.width,
            height: at.height,
            radius: look.radius,
            variant: glass::Variant::Regular,
            tone: None,
            light: None,
            backdrop,
            united: None,
        })),
        Box::new(card(banner, height, roles).opacity(look.says)),
    ];

    Rectangle::new()
        .width(at.width)
        .height(at.height)
        .radius(look.radius)
        .clip()
        .align_child(Start, Start)
        .translate(at.x, at.y)
        .opacity(look.seen)
        .child(Layers::new(layers))
}

fn hover(inside: bool) {
    let now = Instant::now();

    // its own statement, so the read is released before the write
    let changes = Banners::read().stack.hovered() != inside;

    if changes && Banners::write().stack.hover(inside, now) {
        nudge();
    }
}

fn close(id: u32) {
    let now = Instant::now();

    if Banners::read().contains(id) {
        Banners::write().close(id, now);
    }
}

// what a Banner says: the sender's picture, who sent it, the summary, the body and a close, then
// the actions
fn card(banner: &Banner, height: f32, roles: &ThemeRoles) -> Rectangle {
    let words = words();

    let sender = Text::new(&banner.toast.app)
        .size(theme::text::LABEL_SMALL)
        .color(roles.on_surface_variant)
        .weight(theme::text::MEDIUM)
        .elide();

    // said in words before the sender, so it never rests on color alone or elides away
    let from: Box<dyn Widget> = if banner.critical() {
        Box::new(
            Row::new(children![
                Text::new("Critical")
                    .size(theme::text::LABEL_SMALL)
                    .color(theme::SEMANTIC.critical)
                    .weight(theme::text::SEMIBOLD),
                sender,
            ])
            .gap(6.0),
        )
    } else {
        Box::new(sender)
    };

    let mut lines: Vec<Box<dyn Widget>> = vec![from, Box::new(summary(banner, roles))];

    if !banner.toast.body.is_empty() {
        lines.push(Box::new(body(banner, roles)));
    }

    let top = Row::new(children![
        view::toast_tile(&banner.toast, TILE, radius::TILE, roles),
        Column::new(lines).width(words).gap(1.0),
        dismiss(banner.id, roles),
    ])
    .gap(TILE_GAP)
    .align(Start);

    let mut parts = children![top];

    if !banner.actions.is_empty() {
        parts.push(Box::new(actions(banner, words + TARGET, roles)));
    }

    let id = banner.id;
    let default = banner.default;

    Rectangle::new()
        .width(WIDTH)
        .height(height)
        .padding(INSET)
        .align_child(Start, Start)
        .cursor(Cursor::Pointer)
        .on_click(on_left(move || {
            // the daemon runs the default action; without one the click only puts the Banner away
            if default {
                Notifications::click(id);
            }

            close(id);
        }))
        .child(Column::new(parts).gap(GAP))
}

fn summary(banner: &Banner, roles: &ThemeRoles) -> Text {
    Text::new(&banner.toast.summary)
        .size(theme::text::BODY)
        .color(roles.on_surface)
        .weight(theme::text::SEMIBOLD)
        .elide()
}

fn body(banner: &Banner, roles: &ThemeRoles) -> Text {
    let text = || {
        Text::new(&banner.toast.body)
            .size(theme::text::LABEL)
            .color(roles.on_surface_variant)
            .weight(theme::text::MEDIUM)
            .elide()
    };

    // a wrapped Text is as tall as its most lines, so one that fits a line never wraps
    let wrapped = text().wrap().max_lines(BODY_LINES);

    if wrapped.height_in(words()) > tall(&text()) {
        wrapped
    } else {
        text()
    }
}

// what the text column takes, between the picture and the close
fn words() -> f32 {
    WIDTH - 2.0 * INSET - TILE - TARGET - 2.0 * TILE_GAP
}

fn card_height(banner: &Banner) -> f32 {
    let roles = theme::island();

    // the sender line is as tall whether it says Critical or not
    let sender = Text::new(&banner.toast.app)
        .size(theme::text::LABEL_SMALL)
        .weight(theme::text::MEDIUM);

    let mut text = tall(&sender) + 1.0 + tall(&summary(banner, &roles));

    if !banner.toast.body.is_empty() {
        text += 1.0 + tall(&body(banner, &roles));
    }

    let actions = if banner.actions.is_empty() {
        0.0
    } else {
        GAP + TARGET
    };

    2.0 * INSET + text.max(TILE) + actions
}

// how tall a Text lays out in the text column
fn tall(text: &Text) -> f32 {
    match text.height() {
        Size::Fixed(height) => height,
        _ => text.height_in(words()),
    }
}

// the tallest a Banner gets: a body of every line, and actions
fn tallest() -> f32 {
    let mut banner = Banner {
        id: 0,
        toast: Default::default(),
        urgency: Urgency::Normal,
        default: false,
        actions: vec![(String::new(), String::new())],
    };
    banner.toast.body = "Ag ".repeat(400);

    card_height(&banner)
}

// a pill per action, as wide as fits, under the text
fn actions(banner: &Banner, width: f32, roles: &ThemeRoles) -> Row {
    let count = banner.actions.len() as f32;
    let each = ((width - 6.0 * (count - 1.0)) / count).min(ACTION_WIDTH);
    let id = banner.id;

    let pills = banner
        .actions
        .iter()
        .map(|(key, label)| {
            let key = key.clone();

            Box::new(
                Rectangle::new()
                    .width(each)
                    .height(TARGET)
                    .radius(TARGET / 2.0)
                    .padding(Padding {
                        top: 0.0,
                        right: PILL_PADDING,
                        bottom: 0.0,
                        left: PILL_PADDING,
                    })
                    .border(1.0, roles.surface_container_high)
                    .align_child(Center, Center)
                    .cursor(Cursor::Pointer)
                    .on_click(on_left(move || {
                        // the daemon closes the notification unless its sender keeps it
                        Notifications::invoke(id, &key);
                        close(id);
                    }))
                    .child(label_in(label, each - 2.0 * PILL_PADDING, roles)),
            ) as Box<dyn Widget>
        })
        .collect();

    // under the text, not the picture
    Row::new(children![
        Rectangle::new().width(TILE).height(TARGET),
        Row::new(pills).gap(6.0),
    ])
    .gap(TILE_GAP)
}

// centered while it fits, elided only when it does not, since elided text fills its width
fn label_in(label: &str, width: f32, roles: &ThemeRoles) -> Text {
    let text = Text::new(label)
        .size(theme::text::LABEL_SMALL)
        .color(roles.on_surface)
        .weight(theme::text::SEMIBOLD);

    match text.width() {
        Size::Fixed(natural) if natural <= width => text,
        _ => text.elide(),
    }
}

// a round target with a cross; it puts the Banner away, the notification stays in the history
fn dismiss(id: u32, roles: &ThemeRoles) -> Rectangle {
    Rectangle::new()
        .width(TARGET)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(on_left(move || close(id)))
        .child(Icon::Dismiss.on(20.0, roles.on_surface_variant))
}

// only the left button acts, so a stray press of another never runs an action
fn on_left(press: impl Fn() + 'static) -> impl Fn(Button) + 'static {
    move |button| {
        if button == Button::Left {
            press();
        }
    }
}
