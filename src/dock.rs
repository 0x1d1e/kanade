//! The Dock (#144, docs/design.md Later): the pinned apps, then the running apps that are not
//! pinned, on every monitor where `dock.edge` and `dock.align` put it, each with a dot while it
//! runs. A click focuses a running app's window, the next one if it has the focus already, or
//! launches an app that is not running. It reads `windows` (ADR 0014), which finds the pinned apps'
//! entries and every icon, so drawing reads no file, and the pointer over it: the icons near the
//! pointer grow as on macOS (`dock.magnification`) from their `dock.size`, and an autohidden Dock
//! comes out from its edge. It steps out of the way of the Island where it grows over it, as an
//! open Surface beside it, and fades back as it closes. It goes by its row at rest, so its own
//! icons grown under the pointer at the row's end may draw over the edge of an Island they near.
//! An autohide turned on or off slides it. It redraws when the apps, the pointer over it or the
//! config change, when an output
//! comes or goes, and when the Island on its edge changes; it asks for frames only while something
//! it draws moves: its springs, a fade or slide, or the Island moving near it.

use std::collections::HashMap;
use std::f32::consts::PI;
use std::rc::Rc;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::service::Service;
use kanade_runtime::{
    Button, Center, Column, Cursor, End, Horizontal, Layer, LayerWindow, Margin, Monitor, Monitors,
    Padding, Parent, Rectangle, Row as Line, Stack, Start, Vertical, Widget, Zone, request_frame,
};

use crate::autohide::{DELAY, HIDE, SHOW, TRIGGER};
use crate::config;
use crate::glass;
use crate::island::arbiter::SATELLITES;
use crate::island::geometry::{self, Hang, Rect, Shape};
use crate::island::motion::{CRITICAL, Mode, Spring};
use crate::island::service::IslandService;
use crate::look::{Along, Anchor, DockSize, Edge, Magnify, Merge, Placement};
use crate::merge;
use crate::modules;
use crate::scene;
use crate::sources::launch::Launch;
use crate::sources::niri::{self, Acted};
use crate::sources::windows::{self, App, DesktopEntry, Window, WindowId, Windows};
use crate::theme::{self, SEMANTIC, ThemeRoles, radius};
use crate::view;

// the dot under a running app's icon, and the room around it
const DOT: f32 = 4.0;
const FOCUSED_DOT: f32 = 12.0;
const BELOW: f32 = 4.0;

// under the icon at rest: from the icon to the strip's edge, the dot between
const DOT_ROOM: f32 = BELOW + DOT + BELOW;

// the Dock's pane of liquid glass on its output
const PANE: &str = "dock";

// from the monitor's edge
const EDGE_GAP: f32 = 4.0;

// between the pinned and the other running apps
const DIVIDER: f32 = 1.0;

// the side of the icon whose tile has the cards' radius::TILE corners; others round in proportion
const TILE_SIDE: f32 = 36.0;

/*
 * the Dock's measures at its `dock.size`, kept in dockbar's proportions: the gaps between the icons
 * and around them, and how far from the pointer an icon still grows, as its 200 px for 40 px icons;
 * with the icons pushed aside as they grow, about two neighbors on each side grow
 */
#[derive(Debug, Clone, Copy, PartialEq)]
struct Size {
    icon: f32,
    gap: f32,
    reach: f32,
}

impl Size {
    const fn of(size: DockSize) -> Self {
        let icon = size.icon();

        Self {
            icon,
            gap: (icon * 2.0 / 9.0).round(),
            reach: 5.0 * icon,
        }
    }

    // around the icons, as wide as between them
    const fn inset(self) -> f32 {
        self.gap
    }

    // the strip at rest, from its edge to the far side of its icons
    const fn body(self) -> f32 {
        self.inset() + self.icon + DOT_ROOM
    }
}

// how much of its way out the first icon of a folded Dock goes before the farthest starts, 0 to 1
const STAGGER: f32 = 0.3;

// how many times the icons' scales are laid out and measured again, past which none moves a pixel
const SETTLE_ROUNDS: usize = 8;

// how far past its height a Peek swings, as one growing into it, still leaving the Dock beside it
const SWING: f32 = 6.0;

// the Dock fading back in beside an Island closing past it
const FADE_IN: Duration = Duration::from_millis(220);

// how far past its place the Dock may swing coming out, kept in its window so none is cut off
const BOUNCE: f32 = 8.0;

// the icons grow fast under the pointer, as on macOS, and settle back a little slower
const GROW: Duration = Duration::from_millis(180);
const SHRINK: Duration = Duration::from_millis(260);

// one app the Dock shows
#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    app: App,

    // its open windows, by id; none for a pinned app not running
    windows: Vec<Window>,

    pinned: bool,
}

/*
 * the pinned apps in their order, each with its windows, then the running apps none pins in the
 * order they opened. A pinned id no entry has is left out, and `kanade status` names it
 */
fn items(windows: &Windows) -> Vec<Item> {
    let mut items: Vec<Item> = windows
        .pinned()
        .iter()
        .filter_map(|pinned| pinned.entry.clone())
        .map(|entry| Item {
            windows: windows
                .running()
                .iter()
                .find(|running| desktop(&running.app).is_some_and(|it| it.id == entry.id))
                .map(|running| running.windows.clone())
                .unwrap_or_default(),
            app: App::Desktop(entry),
            pinned: true,
        })
        .collect();

    for running in windows.running() {
        let pinned = desktop(&running.app)
            .is_some_and(|entry| windows.pinned().iter().any(|pinned| pinned.id == entry.id));

        if !pinned {
            items.push(Item {
                app: running.app.clone(),
                windows: running.windows.clone(),
                pinned: false,
            });
        }
    }

    items
}

fn desktop(app: &App) -> Option<&DesktopEntry> {
    match app {
        App::Desktop(entry) => Some(entry),
        App::Unmatched(_) => None,
    }
}

// what a click on an item does
#[derive(Debug, Clone, PartialEq, Eq)]
enum Press {
    Focus(WindowId),

    Launch(Launch),
}

/*
 * the window after the focused one, round to the first, so clicks go through an app's windows;
 * the first while another app has the focus; a launch while none is open. None for an app without
 * a window or a command
 */
fn press(item: &Item) -> Option<Press> {
    let Some(first) = item.windows.first() else {
        return desktop(&item.app)?.launch.clone().map(Press::Launch);
    };

    let next = item
        .windows
        .iter()
        .position(|window| window.focused)
        .and_then(|focused| item.windows.get(focused + 1))
        .unwrap_or(first);

    Some(Press::Focus(next.id))
}

// off the view thread, as niri's or the bus's answer may take its patience
fn carry_out(press: Press) {
    thread::spawn(move || {
        let done = match &press {
            Press::Focus(id) => {
                match niri::act(&format!(
                    r#"{{"Action":{{"FocusWindow":{{"id":{}}}}}}}"#,
                    id.0
                )) {
                    Ok(Acted::Done) => Ok(()),
                    Ok(Acted::Unknown(why)) => Err(format!("niri gave no clear answer: {why}")),
                    Err(error) => Err(error.to_string()),
                }
            }
            Press::Launch(launch) => launch.run(),
        };

        if let Err(error) = done {
            eprintln!("dock: {press:?}: {error}");
        }
    });
}

// pins the configured apps, at start and after a reload
pub fn pin() {
    windows::pin(config::get().pinned.clone());
}

// for `kanade status`
pub fn status() -> String {
    let windows = Windows::read();
    let items = items(&windows);

    let pinned = items.iter().filter(|item| item.pinned).count();
    let mut line = format!(
        "dock: {pinned} pinned, {} running",
        items.iter().filter(|item| !item.windows.is_empty()).count()
    );

    let missing: Vec<&str> = windows
        .pinned()
        .iter()
        .filter(|pinned| pinned.entry.is_none())
        .map(|pinned| pinned.id.as_str())
        .collect();

    if !missing.is_empty() {
        line.push_str(&format!(", no .desktop file for {}", missing.join(" ")));
    }

    line
}

/*
 * the Dock laid out for a monitor, in the coordinates of its own window: that is a window of its
 * own beside the Island, or part of the Island's, once they are merged (ADR 0031)
 */
pub(crate) struct Laid {
    monitor: String,
    axes: Axes,
    place: Placement,

    // whether any app shows
    items: bool,

    // the window's size, which holds the row at its widest and its icons at their largest
    pub(crate) canvas: (f32, f32),
    margin: Margin,

    // the strip now, where the glass goes, behind the icons, and its corners' radius
    pub(crate) strip: Rect,
    pub(crate) radius: f32,

    // how far the pointer holds the Dock out of the Island it is folded into, 0 to 1; 1 otherwise
    pub(crate) held: f32,

    // how far the icons grown under the pointer pushed each side of a Dock around the Island out
    pub(crate) swelled: (f32, f32),

    // how far, 0 to 1, it is out, and aside for the Island
    shown: f32,
    aside: f32,

    // the icons and the dividers between, over the strip
    layers: Vec<Box<dyn Widget>>,

    // where the pointer reaches it, in its window; none while it takes none
    pub(crate) area: Option<Rect>,
}

/*
 * what the merged shell needs of the Dock before it draws: its window's size, how far its strip
 * stands off the edge at rest, and for the forms that put the Island among its icons, how much of
 * the row lies before the Island and after it; none while no app shows. Reading subscribes to the
 * apps
 */
#[derive(Clone, Copy)]
pub struct Extent {
    pub width: f32,
    pub height: f32,
    pub depth: f32,

    // the strip at rest, from its edge to the far side of its icons, and its gap around them
    pub strip: f32,
    pub inset: f32,

    // how far the icons reach out from the Island's body to either side, with the gap between
    pub before: f32,
    pub after: f32,

    // how much further out either side reaches, and the strip's far side, with the icons grown
    pub swell: f32,
    pub rise: f32,
}

pub fn extent() -> Option<Extent> {
    let items = items(&Windows::read());

    if items.is_empty() {
        return None;
    }

    let config = config::get();
    let size = Size::of(config.dock_size);
    let pieces = pieces(&items);
    let most = config.magnify.most();
    let canvas = Canvas::of(&pieces, size, most);
    let wings = wings(&pieces, size, config.dock_place.anchor.along);
    let swell = |side: &[Piece], rest: &Row| (widest(side, size, most) - rest.width).max(0.0);

    Some(Extent {
        width: canvas.width,
        height: canvas.height,
        depth: depth(size),
        strip: size.body(),
        inset: size.inset(),
        before: wings.before,
        after: wings.after,
        swell: swell(&pieces[..wings.cut], &wings.left)
            .max(swell(&pieces[wings.cut..], &wings.right)),
        rise: size.icon * (most - 1.0),
    })
}

/*
 * where the Island stands in the row when it is its middle piece: `cut` pieces go before it, along
 * the Dock's side of its edge as the Dock sits there, all of them against an edge's end, half in
 * the middle. `before` and `after` are how far each side reaches from the body, with the gap
 */
struct Wings {
    cut: usize,
    left: Row,
    right: Row,
    before: f32,
    after: f32,
}

fn wings(pieces: &[Piece], size: Size, along: Along) -> Wings {
    let icons = pieces.iter().filter(|piece| **piece == Piece::Icon).count();
    let first = match along {
        Along::Left => 0,
        Along::Center => icons / 2,
        Along::Right => icons,
    };

    let mut cut = 0;
    let mut seen = 0;

    for (index, piece) in pieces.iter().enumerate() {
        if first > 0 && *piece == Piece::Icon {
            seen += 1;

            if seen == first {
                cut = index + 1;
                break;
            }
        }
    }

    let left = spans(&pieces[..cut], size, &vec![1.0; cut]);
    let right = spans(&pieces[cut..], size, &vec![1.0; pieces.len() - cut]);
    let reach = |row: &Row, any: bool| {
        if any {
            row.width - 2.0 * size.inset() + size.gap
        } else {
            0.0
        }
    };
    let (before, after) = (reach(&left, cut > 0), reach(&right, cut < pieces.len()));

    Wings {
        cut,
        left,
        right,
        before,
        after,
    }
}

/*
 * lays the Dock out on `monitor`. Merged, it never steps aside for the Island or hides, and its
 * window has no margin of its own: the shell puts it where it goes
 */
pub(crate) fn lay(monitor: &Monitor, merged: bool) -> Laid {
    // reading subscribes this window to the running apps and the pointer over its own output
    let items = items(&Windows::read());
    let pointers = Pointers::read_part(&monitor.name);

    let config = config::get();
    let place = config.dock_place;

    // merged, a Crown's icons grow over the Island's body just in from the plate, as they do over anything
    let most = config.magnify.most();
    let size = Size::of(config.dock_size);
    let now = Instant::now();

    // by tone, as the glass under them is tinted, not the palette's background
    let roles = theme::island();
    let pieces = pieces(&items);

    let pointer = pointers.by.get(&monitor.name);
    let autohide = autohides(place);

    // our springs, not runtime Animations, so the view asks for frames until those it shows rest
    if pointer.is_some_and(|pointer| {
        moving(&pointer.grow, now) || autohide && moving(&pointer.shown, now)
    }) {
        request_frame();
    }

    let grow = pointer.map_or(0.0, |pointer| pointer.grow(now));
    let shown = if autohide {
        pointer.map_or(0.0, |pointer| pointer.shown(now))
    } else {
        1.0
    };

    // laid out along its edge and across it, then turned to stand on a side edge
    let axes = Axes::of(place.anchor);
    let along = place.anchor.sits();
    let far = matches!(place.anchor.edge, Edge::Bottom | Edge::Right);

    let canvas = Canvas::of(&pieces, size, most);
    let rest = spans(&pieces, size, &vec![1.0; pieces.len()]);
    let rest_left = canvas.left(along, rest.width, None);

    let (covered, crossing) = if merged {
        (0.0, false)
    } else {
        covered(
            monitor,
            place.anchor,
            canvas.width,
            (rest_left, rest_left + rest.width),
            depth(size),
            now,
        )
    };

    let drawn = {
        let mut drawn = DRAWN.lock().unwrap_or_else(PoisonError::into_inner);
        let last = drawn.get(&monitor.name).copied();
        let next = Drawn::next(last, autohide, config.island.motion, shown, covered, now);

        drawn.insert(monitor.name.clone(), next);
        next
    };

    let (shown, aside) = (drawn.shown, drawn.aside);

    // the Island's spring growing past or back from its small forms, and the Dock easing after it
    if crossing || drawn.easing(covered, now) {
        request_frame();
    }

    let extra = grow * (most - 1.0);

    // where along the row at rest the pointer is, so the icon that grows most is the one under it
    let at = pointer.map(|pointer| {
        under(
            &pieces,
            size,
            canvas,
            along,
            pointer.x - rest_left,
            rest_left,
            extra,
        )
    });

    let scales = at.map_or_else(
        || vec![1.0; pieces.len()],
        |at| scales(&pieces, size, at, extra),
    );
    let row = spans(&pieces, size, &scales);

    // the point under the pointer stays under it as the row grows around it
    let held = at.map(|at| (rest_left + at, follow(&rest, &row, at)));
    let left = canvas.left(along, row.width, held);

    let strip_y = strip_y(far, size, canvas.height, shown);
    let lift = if merged { balance(far, size) } else { 0.0 };
    let strip = axes.rect(Rect {
        x: left,
        y: strip_y,
        width: row.width,
        height: size.body(),
    });
    let (window_width, window_height) = axes.size(canvas.width, canvas.height);

    let mut layers: Vec<Box<dyn Widget>> = Vec::new();
    let mut slots = items.iter();

    for ((piece, span), scale) in pieces.iter().zip(&row.spans).zip(&scales) {
        let x = left + span.x;

        match piece {
            Piece::Divider => {
                let at = axes.rect(Rect {
                    x,
                    y: strip_y + lift + if far { size.inset() } else { DOT_ROOM },
                    width: DIVIDER,
                    height: size.icon,
                });

                layers.push(Box::new(
                    Rectangle::new()
                        .width(at.width)
                        .height(at.height)
                        .fill(roles.outline)
                        .translate(at.x, at.y),
                ));
            }
            Piece::Icon => {
                let Some(item) = slots.next() else { break };
                let side = size.icon * scale;

                let at = axes.rect(Rect {
                    x,
                    y: slot_y(far, size, strip_y + lift, side),
                    width: side,
                    height: side + DOT_ROOM,
                });

                layers.push(Box::new(
                    slot(item, side, far, axes, &roles, true).translate(at.x, at.y),
                ));
            }
        }
    }

    let area = if aside > 0.0 {
        // nothing, so a click meant for the Island growing over the Dock reaches it
        None
    } else if shown <= 0.0 {
        // only a strip on the screen's edge, which brings the Dock back
        Some(axes.rect(Rect {
            x: rest_left,
            y: if far { canvas.height - TRIGGER } else { 0.0 },
            width: rest.width,
            height: TRIGGER,
        }))
    } else {
        /*
         * the strip down to the edge where it stands, and while it is hovered, up to its tallest
         * icon: a click just past the icons still reaches the window there
         */
        let tallest = if pointer.is_some_and(|pointer| pointer.inside) {
            scales.iter().copied().fold(1.0, f32::max)
        } else {
            1.0
        };
        let (y, height) = reached(
            far,
            size,
            canvas.height,
            strip_y + lift,
            size.icon * tallest,
        );

        Some(axes.rect(Rect {
            x: left.floor(),
            y,
            width: row.width.ceil() + 1.0,
            height,
        }))
    };

    Laid {
        monitor: monitor.name.clone(),
        axes,
        place,
        items: !items.is_empty(),
        canvas: (window_width, window_height),
        margin: margin(place.anchor),
        strip,
        radius: radius::CARD,
        held: 1.0,
        swelled: (0.0, 0.0),
        shown,
        aside,
        layers,
        area: area.filter(|_| !items.is_empty()),
    }
}

/*
 * the Dock laid out around the Island's `body` (with the corner `radius` it has), in the shell's
 * window (`merge::Shell`, ADR 0031): the Island is the middle piece of the row, its icons on
 * either side. Folded (`dock.merge = "fold"`), the icons come out of the body as `unfold` goes
 * from 0 to 1, with the pointer or while a small form shows, and go back into it as a Surface
 * opens. `reach` is where the pointer reaches the Island, which the Dock's hover takes in too
 * while folded
 */
pub(crate) fn between(
    monitor: &Monitor,
    shell: &merge::Shell,
    body: Rect,
    radius: f32,
    reach: Rect,
) -> Laid {
    let items = items(&Windows::read());
    let pointers = Pointers::read_part(&monitor.name);

    let config = config::get();
    let place = config.dock_place;
    let size = Size::of(config.dock_size);
    let now = Instant::now();
    let roles = theme::island();
    let pieces = pieces(&items);
    let far = place.anchor.edge == Edge::Bottom;
    let folds = shell.form == Merge::Fold;
    let lift = balance(far, size);

    let pointer = pointers.by.get(&monitor.name);

    if pointer
        .is_some_and(|pointer| moving(&pointer.grow, now) || folds && moving(&pointer.shown, now))
    {
        request_frame();
    }

    // the pointer's hold on it, then how much the Island being a small form leaves it open
    let held = if folds {
        pointer.map_or(0.0, |pointer| pointer.shown(now))
    } else {
        1.0
    };
    let unfold = shell.unfold(held, body.height);

    let wings = wings(&pieces, size, place.anchor.along);

    /*
     * the icons grow toward the pointer as they do alone, each side laid out from the body, which
     * stays where it is: what grows pushes the icons past it outward, and the plate with them
     */
    let extra = pointer.map_or(0.0, |pointer| pointer.grow(now))
        * (config.magnify.most() - 1.0)
        * unfold.min(1.0);
    let (grown_before, grown_after) = pointer.map_or_else(
        || (vec![1.0; wings.cut], vec![1.0; pieces.len() - wings.cut]),
        |pointer| {
            (
                swell(&pieces[..wings.cut], size, body.x - pointer.x, extra, true),
                swell(
                    &pieces[wings.cut..],
                    size,
                    pointer.x - body.right(),
                    extra,
                    false,
                ),
            )
        },
    );
    let (left, right) = (
        spans(&pieces[..wings.cut], size, &grown_before),
        spans(&pieces[wings.cut..], size, &grown_after),
    );
    let swelled = (
        left.width - wings.left.width,
        right.width - wings.right.width,
    );
    let plate = shell.plate(body, radius, unfold, swelled);

    // the icons nearest the Island come out first and go in last
    let icons_before = pieces[..wings.cut]
        .iter()
        .filter(|piece| **piece == Piece::Icon)
        .count();
    let icons_after = pieces[wings.cut..]
        .iter()
        .filter(|piece| **piece == Piece::Icon)
        .count();
    let stagger = icons_before.max(icons_after).saturating_sub(1).max(1) as f32;
    let alpha = |near: usize| {
        if folds {
            ((unfold - STAGGER * near as f32 / stagger) / (1.0 - STAGGER)).clamp(0.0, 1.0)
        } else {
            1.0
        }
    };

    let mut layers: Vec<Box<dyn Widget>> = Vec::new();
    let mut slots = items.iter();
    let axes = Axes::of(place.anchor);

    for (index, piece) in pieces.iter().enumerate() {
        let before = index < wings.cut;
        let (span, near, scale) = if before {
            let span = left.spans[index];
            let near = pieces[index + 1..wings.cut]
                .iter()
                .filter(|piece| **piece == Piece::Icon)
                .count();

            (span, near, grown_before[index])
        } else {
            let span = right.spans[index - wings.cut];
            let near = pieces[wings.cut..index]
                .iter()
                .filter(|piece| **piece == Piece::Icon)
                .count();

            (span, near, grown_after[index - wings.cut])
        };

        // where it stands with the icons all out, and how far out of the body it is
        let full = if before {
            body.x - left.width + span.x
        } else {
            body.right() + size.gap - size.inset() + span.x
        };
        let edge = if before { body.x } else { body.right() };
        let x = edge + unfold * (full - edge);
        let alpha = alpha(near);

        match piece {
            Piece::Divider => {
                if alpha <= 0.0 {
                    continue;
                }

                layers.push(Box::new(
                    Rectangle::new()
                        .width(DIVIDER)
                        .height(size.icon)
                        .fill(roles.outline)
                        .opacity(alpha)
                        .translate(
                            x,
                            shell.strip + lift + if far { size.inset() } else { DOT_ROOM },
                        ),
                ));
            }
            Piece::Icon => {
                let Some(item) = slots.next() else { break };

                if alpha <= 0.0 {
                    continue;
                }

                // growing out of the Island, from a little smaller
                let grown = size.icon * scale;
                let side = if folds {
                    grown * (0.7 + 0.3 * alpha)
                } else {
                    grown
                };
                let at = Rect {
                    x: x + (grown - side) / 2.0,
                    y: slot_y(far, size, shell.strip + lift, side),
                    width: side,
                    height: side + DOT_ROOM,
                };

                layers.push(Box::new(
                    slot(item, side, far, axes, &roles, alpha >= 0.5)
                        .opacity(alpha)
                        .translate(at.x, at.y),
                ));
            }
        }
    }

    // from the edge down to the plate's far side, or the tallest icon's, so a click just short of an icon reaches it
    let area = {
        let tallest = if pointer.is_some_and(|pointer| pointer.inside) {
            grown_before
                .iter()
                .chain(&grown_after)
                .copied()
                .fold(1.0, f32::max)
        } else {
            1.0
        };
        let top = slot_y(far, size, shell.strip + lift, size.icon * tallest);
        let (y, height) = if far {
            let y = plate.rect.y.min(top).floor().max(0.0);

            (y, shell.size.1 - y)
        } else {
            let bottom = plate
                .rect
                .bottom()
                .max(top + DOT_ROOM + size.icon * tallest);

            (0.0, bottom.ceil().min(shell.size.1))
        };
        let area = Rect {
            x: plate.rect.x.floor(),
            y,
            width: plate.rect.width.ceil() + 1.0,
            height,
        };

        if folds { area.around(reach) } else { area }
    };

    Laid {
        monitor: monitor.name.clone(),
        axes,
        place,
        items: !items.is_empty(),
        canvas: shell.size,
        margin: margin(place.anchor),
        strip: plate.rect,
        radius: plate.radius,
        held,
        swelled,
        shown: 1.0,
        aside: 0.0,
        layers,
        area: (!items.is_empty()).then_some(area),
    }
}

impl Laid {
    // what the shell's scene needs of the layout, apart from what it draws
    pub(crate) fn fit(&self) -> scene::Dock {
        scene::Dock {
            strip: self.strip,
            radius: self.radius,
            held: self.held,
            swelled: self.swelled,
            area: self.area,
        }
    }

    /*
     * what the pointer tells the Dock: a target as big as `area`, and the icons and dividers over
     * the glass, each drawn in the window's coordinates. The target takes the pointer where the
     * Dock does and no more. In the Island's window, which it overlaps, it takes the Island's moves
     * too, which it passes on to `island` as they are over the Dock's window
     */
    pub(crate) fn drawn(self, island: Option<Rc<dyn Fn(f32, f32)>>) -> (Rectangle, Stack) {
        let moved = self.monitor.clone();
        let hovered = self.monitor;
        let axes = self.axes;
        let area = self.area.unwrap_or(Rect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        });

        let hover = Rectangle::new()
            .width(area.width)
            .height(area.height)
            .translate(area.x, area.y)
            .align_child(Start, Start)
            .on_hover(move |inside| hover(&hovered, inside));

        let hover = hover.on_move(move |point| {
            let (x, y) = (area.x + point.x, area.y + point.y);

            pointed(&moved, axes.along(x, y));

            if let Some(island) = &island {
                island(x, y);
            }
        });

        (hover, Stack::new(self.layers))
    }
}

/*
 * the window of a Dock the Island's window draws, merged with it: nothing, but for stopping the
 * capture behind the plate it had before
 */
fn merged(monitor: &Monitor) -> LayerWindow {
    let place = config::get().dock_place;

    glass::place(monitor, PANE, place.anchor, (1.0, 1.0), None);

    LayerWindow::new()
        .width(1.0)
        .height(1.0)
        .anchor_vertical(place.anchor.vertical())
        .anchor_horizontal(place.anchor.horizontal())
        .layer(Layer::Top)
        .space(Zone::Ignore)
        .namespace("kanade-dock")
        .visible(false)
        .click_through()
        .child(Rectangle::new().width(1.0).height(1.0))
}

// one window per monitor, hidden while there are no items, or while the Island's window draws it
pub fn window(monitor: &Monitor) -> LayerWindow {
    if merge::form().is_some() {
        return merged(monitor);
    }

    let laid = lay(monitor, false);

    let place = laid.place;
    let (window_width, window_height) = laid.canvas;
    let margin = laid.margin;
    let strip_at = laid.strip;
    let (shown, aside, items, area) = (laid.shown, laid.aside, laid.items, laid.area);

    let glass_body = glass::Body {
        x: strip_at.x + margin.left as f32,
        y: strip_at.y + margin.top as f32,
        width: strip_at.width,
        height: strip_at.height,
        radius: radius::CARD,
        join: None,
    };

    /*
     * liquid glass knows the window's place with its margins as part of it; it captures nothing
     * while the Dock is out of sight, empty, under a fullscreen window, slid past its edge or
     * stepped aside
     */
    let spot = glass::Spot::new(&monitor.name, PANE);
    let fullscreen = view::covered(monitor, || IslandService::read().overview());
    let liquid = glass::place(
        monitor,
        PANE,
        place.anchor,
        (
            window_width + (margin.left + margin.right) as f32,
            window_height + (margin.top + margin.bottom) as f32,
        ),
        (items && shown > 0.0 && aside < 1.0 && !fullscreen).then_some(glass_body),
    );
    let backdrop = glass::shown(&spot, glass_body);

    let roles = theme::island();
    let mut strip = Rectangle::new()
        .width(strip_at.width)
        .height(strip_at.height)
        .radius(radius::CARD)
        .clip()
        .align_child(Start, Start)
        .translate(strip_at.x, strip_at.y)
        .child(glass::layer(glass::Pane {
            width: strip_at.width,
            height: strip_at.height,
            radius: radius::CARD,
            variant: glass::Variant::Clear,
            tone: None,
            light: None,
            backdrop,
            united: None,
        }));

    // the glass's own rim is its edge
    if !liquid {
        strip = strip.border(1.0, roles.outline);
    }

    let (target, icons) = laid.drawn(None);
    let hover = Rectangle::new()
        .width(window_width)
        .height(window_height)
        .align_child(Start, Start)
        .opacity(1.0 - aside)
        .child(Stack::new(vec![
            Box::new(target),
            Box::new(strip),
            Box::new(icons),
        ]));

    LayerWindow::new()
        .width(window_width)
        .height(window_height)
        .anchor_vertical(place.anchor.vertical())
        .anchor_horizontal(place.anchor.horizontal())
        .margin(margin)
        .layer(Layer::Top)
        // the reserving window keeps windows off the strip, so this one may cover its own
        .space(Zone::Ignore)
        .namespace("kanade-dock")
        .visible(items)
        .input_region(
            area.filter(|_| items)
                .map(view::input_area)
                .into_iter()
                .collect(),
        )
        .child(hover)
}

/*
 * keeps windows off the Dock's strip of the output, `dock.reserve`: an empty window a pixel wide on
 * its edge, as thick as the Dock at rest and, beside the Island, the room it leaves the Island, as
 * layer-shell reserves the thickness of a window held to one edge. The Island's own reserve on the
 * same edge counts toward it, so the two keep off the larger, and none is left to reserve when
 * the Island's covers it. The Dock's own window is taller, with room for the icons to grow.
 * Another bar's reserve on the same edge stacks under this one, while the Dock keeps its place
 * from the edge and so draws over that bar
 */
pub fn reserve(_monitor: &Monitor) -> LayerWindow {
    let items = !items(&Windows::read()).is_empty();
    let config = config::get();
    let place = config.dock_place;
    let body = Size::of(config.dock_size).body();
    let reserved = island_room(place.anchor) + body + EDGE_GAP - island_reserved(place.anchor);
    let (width, height) = Axes::of(place.anchor).size(1.0, reserved.max(1.0));

    // held to its edge alone, so the runtime reserves its thickness off that edge
    let (vertical, horizontal) = if place.anchor.sideways() {
        (Vertical::Middle, place.anchor.horizontal())
    } else {
        (place.anchor.vertical(), Horizontal::Middle)
    };

    LayerWindow::new()
        .width(width)
        .height(height)
        .anchor_vertical(vertical)
        .anchor_horizontal(horizontal)
        .layer(Layer::Bottom)
        .space(Zone::Reserve)
        .namespace("kanade-dock-reserve")
        .visible(
            place.reserve
                && !autohides(place)
                && items
                && reserved > 0.0
                && merge::form().is_none(),
        )
        .click_through()
        .child(Rectangle::new().width(Parent).height(Parent))
}

// off the screen's side the Dock sits against, as off its edge, and past the Island on its place
fn margin(anchor: Anchor) -> Margin {
    let side = EDGE_GAP as i32;
    let edge = island_room(anchor) as i32;

    let mut margin = match anchor.sits() {
        Along::Left => Margin {
            left: side,
            ..Margin::default()
        },
        Along::Right => Margin {
            right: side,
            ..Margin::default()
        },
        Along::Center => Margin::default(),
    };

    match anchor.edge {
        Edge::Top => margin.top = edge,
        Edge::Bottom => margin.bottom = edge,
        Edge::Left => margin.left = edge,
        Edge::Right => margin.right = edge,
    }

    margin
}

/*
 * beside the Island, on its edge and side or one of them centered, past where its small forms and
 * their hover reach, so neither covers the other. Whatever either's width, as the Dock widens with
 * every app opened and grows under the pointer: the room it reserves stays put rather than move
 * windows as it does. One on the other side of the edge sits on the edge, and steps aside should
 * the Island reach it
 */
fn island_room(anchor: Anchor) -> f32 {
    // merged, the Island's window holds it, and there is no beside
    if merge::form().is_some() {
        return 0.0;
    }

    room_for(config::get().island_place.anchor, anchor)
}

/*
 * what the Island's own reserve keeps off the Dock's edge already, as layer-shell stacks the
 * reserves on one edge: the Dock's then keeps off only the rest of its own, whether beside the
 * Island or along the edge from it
 */
fn island_reserved(anchor: Anchor) -> f32 {
    if config::get().island_place.anchor.edge == anchor.edge {
        view::reserved()
    } else {
        0.0
    }
}

/*
 * the Dock's strip at rest on `monitor`, and the edge it sits on, which what hangs from the Island,
 * as the Banners, keeps clear of; none while off, hiding under autohide, or with no app to show.
 * Its icons grown under the pointer may still reach past it. Reading subscribes to the apps
 */
pub fn strip(monitor: &Monitor) -> Option<(geometry::Edge, Rect)> {
    let items = items(&Windows::read());
    let config = config::get();
    let place = config.dock_place;

    if !modules::on("dock")
        || merge::form().is_some()
        || autohides(place)
        || items.is_empty()
        || monitor.width == 0
        || monitor.height == 0
    {
        return None;
    }

    let size = Size::of(config.dock_size);
    let pieces = pieces(&items);
    let canvas = Canvas::of(&pieces, size, config.magnify.most());
    let rest = spans(&pieces, size, &vec![1.0; pieces.len()]);
    let left = canvas.left(place.anchor.sits(), rest.width, None);
    let strip = dock_strip(
        (monitor.width as f32, monitor.height as f32),
        place.anchor,
        canvas.width,
        (left, left + rest.width),
        island_room(place.anchor),
        depth(size),
    );

    Some((place.anchor.hang().edge, strip))
}

// how far off its edge, past the Island beside it, the Dock's strip reaches at rest
fn depth(size: Size) -> f32 {
    EDGE_GAP + size.body()
}

// the Dock's icons in order, pinned ones apart from the rest
fn pieces(items: &[Item]) -> Vec<Piece> {
    let divided = items.iter().any(|item| item.pinned) && items.iter().any(|item| !item.pinned);
    let mut pieces = Vec::new();

    for (index, item) in items.iter().enumerate() {
        if divided && index > 0 && item.pinned != items[index - 1].pinned {
            pieces.push(Piece::Divider);
        }

        pieces.push(Piece::Icon);
    }

    pieces
}

fn room_for(island: Anchor, dock: Anchor) -> f32 {
    if beside(island, dock) {
        let peek = geometry::TOP + island.thickness(geometry::PEEK);

        // on a side edge the Banners line up beside the Island at rest
        let banners = match island.sideways() {
            true => {
                geometry::TOP
                    + island.thickness(geometry::upright(geometry::REST, island.hang()))
                    + crate::banners::GAP
                    + crate::banners::WIDTH
            }
            false => 0.0,
        };

        peek.max(geometry::rest_reach(island.hang(), SATELLITES))
            .max(banners)
            + geometry::HOVER_PADDING
    } else {
        0.0
    }
}

fn beside(island: Anchor, dock: Anchor) -> bool {
    island.edge == dock.edge
        && (island.sits() == dock.sits()
            || island.sits() == Along::Center
            || dock.sits() == Along::Center)
}

// how far a body of the Island on `anchor` reaches off its edge: its height, or its width on a side
/*
 * how far, 0 to 1, the Dock has faded out of the way of the Island, so it never covers it; and
 * whether the Island moves `near` it, so the fade wants frames, as its spring may swing past where
 * it heads or coast on, turned back mid growth. Only where the Island's body or Satellites reach
 * across the row at rest, now or where the body heads. Beside it, gone before an Island grown past
 * its small forms, as an open Surface, reaches the Dock, and not for the few pixels a Peek swings
 * past its height. On the other side of the edge, gone at once, so it is gone before the body
 * arrives; a Satellite sends it away as it touches the row. On another edge, gone at once where
 * the body or a Satellite reaches the strip `depth` off its edge. Its own magnification under the
 * pointer never sends it away
 */
fn covered(
    monitor: &Monitor,
    anchor: Anchor,
    canvas_width: f32,
    row: (f32, f32),
    depth: f32,
    now: Instant,
) -> (f32, bool) {
    let island_anchor = config::get().island_place.anchor;
    let room = room_for(island_anchor, anchor);
    let island = IslandService::read();
    let satellites = island.satellites(&monitor.name);

    // none under niri's overview, where the Island draws none
    let drawn = !island.overview();
    let shown = if drawn {
        satellites.shown(now)
    } else {
        Vec::new()
    };
    let island_canvas = view::canvas(monitor);
    let body = island.shape(&monitor.name, now);
    let heading = island.heading(&monitor.name, now);
    let moving = heading.is_some() || drawn && satellites.moving(now);

    // on another edge, by where both stand on the output; frames while the Island moves
    if island_anchor.edge != anchor.edge {
        let output = (monitor.width as f32, monitor.height as f32);
        let strip = dock_strip(output, anchor, canvas_width, row, room, depth);
        let hits = |shape: Shape| {
            let slots = shown.iter().map(|shown| (shown.slot, shown.presence));
            let reach = island_reach(
                geometry::bounded(shape, island_canvas),
                island_anchor.hang(),
                island_canvas,
                slots,
            );

            overlap(placed(reach, island_anchor, island_canvas, output), strip)
        };

        let covered = if hits(body) || heading.is_some_and(hits) {
            1.0
        } else {
            0.0
        };

        return (covered, moving);
    }

    // the output and the Island's window along their edge
    let axes = Axes::of(anchor);
    let width = axes.along(monitor.width as f32, monitor.height as f32);
    let island_width = axes.along(island_canvas.width, island_canvas.height);

    // whether the Island at a shape, with its Satellites as they are now, reaches across the row
    let reach = |shape: Shape| {
        let slots = shown.iter().map(|shown| (shown.slot, shown.presence));
        let span = island_span(
            geometry::bounded(shape, island_canvas),
            island_anchor.hang(),
            island_canvas,
            slots,
        );

        reaches(
            width,
            island_anchor,
            island_width,
            span,
            anchor,
            canvas_width,
            row,
        )
    };

    let covered = coverage(
        room,
        reach(body) || heading.is_some_and(reach),
        island_anchor.thickness(body),
        island_anchor,
    );

    (
        covered,
        moving
            && near(
                width,
                island_anchor,
                island_width,
                anchor,
                canvas_width,
                row,
            ),
    )
}

/*
 * how far the Dock `room` off the edge is aside for an Island body `height` tall, which `reaches`
 * across the row now or where it heads: beside it, as far as the body has grown toward it; across
 * the edge, at once. None for an Island anywhere else along the edge
 */
fn coverage(room: f32, reaches: bool, thickness: f32, island: Anchor) -> f32 {
    if !reaches {
        0.0
    } else if room > 0.0 {
        fade(thickness, island, room)
    } else {
        1.0
    }
}

/*
 * whether anything the Island draws on the Dock's edge may reach the row: its window, which holds
 * its body, Satellites and their bounces, does. Its spring may swing past where it heads, and its
 * dots pass where they slide, so while it moves the Dock watches it frame by frame, not ahead
 */
fn near(
    width: f32,
    island: Anchor,
    island_width: f32,
    dock: Anchor,
    canvas_width: f32,
    row: (f32, f32),
) -> bool {
    island.edge == dock.edge
        && reaches(
            width,
            island,
            island_width,
            (0.0, island_width),
            dock,
            canvas_width,
            row,
        )
}

/*
 * where along its edge, across its window, the Island's body and its Satellites, at their slots
 * and how far out, reach at a shape
 */
fn island_span(
    shape: Shape,
    hang: Hang,
    canvas: geometry::Canvas,
    slots: impl Iterator<Item = (f32, f32)>,
) -> (f32, f32) {
    let reach = island_reach(shape, hang, canvas, slots);

    if hang.sideways() {
        (reach.y, reach.y + reach.height)
    } else {
        (reach.x, reach.right())
    }
}

// what in its window the Island's body and its Satellites, at their slots and how far out, cover
fn island_reach(
    shape: Shape,
    hang: Hang,
    canvas: geometry::Canvas,
    slots: impl Iterator<Item = (f32, f32)>,
) -> Rect {
    let body = geometry::body(shape, hang, canvas);
    let shows = geometry::satellite_opacity(shape, hang) > 0.0;

    slots
        .filter(|_| shows)
        .map(|(slot, presence)| geometry::satellite(body, hang, canvas, slot, presence))
        .fold(body, |reach, dot| {
            let x = reach.x.min(dot.x);
            let y = reach.y.min(dot.y);

            Rect {
                x,
                y,
                width: reach.right().max(dot.right()) - x,
                height: (reach.y + reach.height).max(dot.y + dot.height) - y,
            }
        })
}

// a rect in the Island's window, on an output `output` wide and tall, as its window is anchored
fn placed(
    rect: Rect,
    island: Anchor,
    canvas: geometry::Canvas,
    (width, height): (f32, f32),
) -> Rect {
    let x = match island.horizontal() {
        Horizontal::Left => 0.0,
        Horizontal::Middle => (width - canvas.width) / 2.0,
        Horizontal::Right => width - canvas.width,
    };
    let y = match island.vertical() {
        Vertical::Top => 0.0,
        Vertical::Middle => (height - canvas.height) / 2.0,
        Vertical::Bottom => height - canvas.height,
    };

    Rect {
        x: x + rect.x,
        y: y + rect.y,
        ..rect
    }
}

/*
 * the Dock's strip at rest on the output: its `row` along its window `canvas_width` long, `room`
 * off its edge and `depth` deep
 */
fn dock_strip(
    (width, height): (f32, f32),
    dock: Anchor,
    canvas_width: f32,
    row: (f32, f32),
    room: f32,
    depth: f32,
) -> Rect {
    let axes = Axes::of(dock);
    let length = axes.along(width, height);
    let start = match dock.sits() {
        Along::Left => EDGE_GAP,
        Along::Center => (length - canvas_width) / 2.0,
        Along::Right => length - EDGE_GAP - canvas_width,
    };
    let from = match dock.edge {
        Edge::Top | Edge::Left => room,
        Edge::Bottom => height - room - depth,
        Edge::Right => width - room - depth,
    };

    axes.rect(Rect {
        x: start + row.0,
        y: from,
        width: row.1 - row.0,
        height: depth,
    })
}

fn overlap(a: Rect, b: Rect) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.y + b.height && b.y < a.y + a.height
}

/*
 * whether the Island, across `span` of its window `island_width` long on the same edge of an output
 * `width` long, reaches the Dock's `row` across its window `canvas_width` long. Along a side edge,
 * down it
 */
fn reaches(
    width: f32,
    island: Anchor,
    island_width: f32,
    span: (f32, f32),
    dock: Anchor,
    canvas_width: f32,
    row: (f32, f32),
) -> bool {
    // where a window `across` wide, `side` off the side it sits against, starts on the output
    let start = |across: f32, along: Along, side: f32| match along {
        Along::Left => side,
        Along::Center => (width - across) / 2.0,
        Along::Right => width - side - across,
    };

    let island_x = start(island_width, island.sits(), 0.0);
    let dock_x = start(canvas_width, dock.sits(), EDGE_GAP);

    island_x + span.0 < dock_x + row.1 && dock_x + row.0 < island_x + span.1
}

/*
 * what each monitor's Dock last drew, which its next frame eases from. The Dock's own memory of
 * its frames, not a Service, as nothing redraws for it: the view reads and writes it, and an
 * output leaving drops its own
 */
static DRAWN: LazyLock<Mutex<HashMap<String, Drawn>>> = LazyLock::new(Default::default);

#[derive(Clone, Copy, Debug)]
struct Drawn {
    // how far it had faded for the Island, 0 to 1
    aside: f32,

    // how far it was out, 0 to 1
    shown: f32,

    at: Instant,
    autohide: bool,

    // when `dock.autohide` last turned on or off, and how far out the Dock was then
    switched: Option<(Instant, f32)>,
}

impl Drawn {
    /*
     * an Island growing over the Dock puts it aside at once, never drawing over the Island; one
     * leaving it lets it fade back over FADE_IN, whatever the Island does meanwhile. An autohide
     * turned on or off slides the Dock over HIDE to where it now stands, out or away, from where
     * it stood, even mid slide; with reduced motion it moves there at once
     */
    fn next(
        last: Option<Self>,
        autohide: bool,
        motion: Mode,
        shown: f32,
        covered: f32,
        now: Instant,
    ) -> Self {
        let back = last.map_or(0.0, |last| {
            last.aside - now.duration_since(last.at).as_secs_f32() / FADE_IN.as_secs_f32()
        });

        let switched = match last {
            Some(last) if last.autohide != autohide => {
                (motion == Mode::Spring).then_some((now, last.shown))
            }
            Some(last) => last.switched,
            None => None,
        };

        let shown = switched.map_or(shown, |(since, from)| {
            let gone = (now.duration_since(since).as_secs_f32() / HIDE.as_secs_f32()).min(1.0);

            // eased as the springs are, slow off and slow in
            from + (shown - from) * (1.0 - (gone * PI).cos()) / 2.0
        });

        Self {
            aside: covered.max(back).clamp(0.0, 1.0),
            shown,
            at: now,
            autohide,
            switched,
        }
    }

    // whether the next frame differs: still fading back for the Island, or sliding for a switch
    fn easing(&self, covered: f32, now: Instant) -> bool {
        self.aside > covered
            || self
                .switched
                .is_some_and(|(since, _)| now.duration_since(since) < HIDE)
    }
}

/*
 * whether a spring still moves what it draws: one under reduced motion is where it heads at once,
 * though it counts the short fade it gives content as unsettled
 */
fn moving<const N: usize>(spring: &Spring<N>, now: Instant) -> bool {
    spring.mode() == Mode::Spring && !spring.settled(now)
}

/*
 * how far the Dock `room` off the edge has faded for an Island body `thickness` off its edge: out by the time
 * the body reaches it. An Island opening crosses the few pixels between in a frame or so, so the
 * Dock steps aside at once; closing, it moves away from the Dock, which fades back in over FADE_IN
 */
fn fade(thickness: f32, island: Anchor, room: f32) -> f32 {
    let from = island.thickness(geometry::PEEK) + SWING;
    let to = room + EDGE_GAP - geometry::TOP;

    ((thickness - from) / (to - from)).clamp(0.0, 1.0)
}

/*
 * whether the Dock hides, `dock.autohide`: not beside the Island, where the screen's edge under it
 * is the Island's, so the pointer could never reach the Dock's
 */
fn autohides(place: Placement) -> bool {
    place.autohide && island_room(place.anchor) == 0.0
}

// where the strip starts off the window's side away from the edge, slid toward the edge and past
// it as it hides
fn strip_y(bottom: bool, size: Size, height: f32, shown: f32) -> f32 {
    // coming out, it swings past its place by at most the room the window keeps for it
    let slide = ((1.0 - shown) * (size.body() + EDGE_GAP + 2.0)).max(-BOUNCE);

    if bottom {
        height - EDGE_GAP - size.body() + slide
    } else {
        EDGE_GAP - slide
    }
}

/*
 * how far a merged Dock's icons and dots stand further from its edge than the strip puts them: the
 * strip has the dot's room against the edge and a whole gap past the icons, so what they make looks
 * low on the plate by half the difference; moved away from the edge it sits level (ADR 0031)
 */
fn balance(bottom: bool, size: Size) -> f32 {
    let away = (size.inset() - BELOW) / 2.0;

    if bottom { -away } else { away }
}

/*
 * where a slot `side` across starts down the window, the strip starting at `strip_y`: it grows away
 * from the edge, its dot staying between it and the edge
 */
fn slot_y(bottom: bool, size: Size, strip_y: f32, side: f32) -> f32 {
    if bottom {
        strip_y + size.body() - DOT_ROOM - side
    } else {
        strip_y
    }
}

/*
 * down a window `height` tall, from its edge on the screen's side to the far side of the strip at
 * `strip_y` and of icons `side` across in it, in whole rows. Never less than the TRIGGER rows on
 * the edge, so a Dock just coming out, its strip still past the window, keeps the pointer that
 * brought it
 */
fn reached(bottom: bool, size: Size, height: f32, strip_y: f32, side: f32) -> (f32, f32) {
    if bottom {
        let far = strip_y
            .min(slot_y(bottom, size, strip_y, side))
            .floor()
            .clamp(0.0, height - TRIGGER);

        (far, height - far)
    } else {
        let far = (strip_y + size.body())
            .max(strip_y + DOT_ROOM + side)
            .ceil()
            .clamp(TRIGGER, height);

        (0.0, far)
    }
}

// an item's icon, `side` across, its dot between it and the edge while it runs, and its click
fn slot(
    item: &Item,
    side: f32,
    far: bool,
    axes: Axes,
    roles: &ThemeRoles,
    live: bool,
) -> Rectangle {
    let mut column: Vec<Box<dyn Widget>> = vec![mark(&item.app, side, roles)];

    if !item.windows.is_empty() {
        let focused = item.windows.iter().any(|window| window.focused);
        let urgent = item.windows.iter().any(|window| window.urgent);

        let color = if urgent {
            SEMANTIC.warning
        } else if focused {
            roles.on_surface
        } else {
            roles.on_surface_variant
        };

        let (width, height) = axes.size(if focused { FOCUSED_DOT } else { DOT }, DOT);
        let dot = Rectangle::new()
            .width(width)
            .height(height)
            .radius(DOT / 2.0)
            .fill(color);

        if far {
            column.push(Box::new(dot));
        } else {
            column.insert(0, Box::new(dot));
        }
    }

    let press = press(item);

    let (width, height) = axes.size(side, side + DOT_ROOM);
    let slot = Rectangle::new().width(width).height(height);

    // the dot `BELOW` off the edge, the icon past it
    let (near, away) = if far { (BELOW, 0.0) } else { (0.0, BELOW) };
    let slot = match (axes.sideways, far) {
        (false, true) => slot.align_child(Center, Start),
        (false, false) => slot.align_child(Center, End),
        (true, true) => slot.align_child(Start, Center),
        (true, false) => slot.align_child(End, Center),
    };
    let padding = if axes.sideways {
        Padding {
            left: away,
            right: near,
            ..Padding::default()
        }
    } else {
        Padding {
            top: away,
            bottom: near,
            ..Padding::default()
        }
    };
    let pieces: Box<dyn Widget> = if axes.sideways {
        Box::new(Line::new(column).gap(BELOW).align(Center))
    } else {
        Box::new(Column::new(column).gap(BELOW).align(Center))
    };

    let slot = slot.padding(padding).child(Stack::new(vec![pieces]));

    // one faded into the Island takes no click, which is the Island's
    if !live {
        return slot;
    }

    slot.cursor(Cursor::Pointer).on_click(move |button| {
        if button == Button::Left
            && let Some(press) = press.clone()
        {
            carry_out(press);
        }
    })
}

// the app's icon, `side` across, or its initial on the quiet tile
fn mark(app: &App, side: f32, roles: &ThemeRoles) -> Box<dyn Widget> {
    if let Some(path) = desktop(app).and_then(|entry| entry.icon_file.clone()) {
        /*
         * decoded once, at twice the largest any icon grows to, crisp at scale 2, rather than anew
         * at each size it passes through
         */
        let pixels = (DockSize::Large.icon() * Magnify::Large.most() * 2.0) as u32;

        return Box::new(
            Rectangle::new()
                .width(side)
                .height(side)
                .fill(kanade_runtime::Image::contain(path).thumbnail(pixels, pixels)),
        );
    }

    let name = match app {
        App::Desktop(entry) => Some(entry.name.as_str()),
        App::Unmatched(app_id) => app_id.as_deref(),
    };

    let initial = name
        .and_then(|name| name.chars().next())
        .map_or_else(|| String::from("?"), |char| char.to_uppercase().collect());

    // in half pixels, so a growing tile sets its letter at few sizes rather than one per frame
    let side = (side * 2.0).round() / 2.0;

    Box::new(view::tile(
        None,
        &initial,
        side,
        radius::TILE * side / TILE_SIDE,
        roles,
    ))
}

// what the Dock's row holds, left to right
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Piece {
    Icon,

    // between the pinned and the other running apps; it never grows
    Divider,
}

impl Piece {
    fn width(self, size: Size, scale: f32) -> f32 {
        match self {
            Piece::Icon => size.icon * scale,
            Piece::Divider => DIVIDER,
        }
    }
}

// where a piece sits, from the strip's left edge
#[derive(Debug, Clone, Copy, PartialEq)]
struct Span {
    x: f32,
    width: f32,
}

// the pieces laid out at their scales, and the strip around them
#[derive(Debug, Clone, PartialEq)]
struct Row {
    spans: Vec<Span>,
    width: f32,
}

fn spans(pieces: &[Piece], size: Size, scales: &[f32]) -> Row {
    let mut x = size.inset();
    let mut spans = Vec::with_capacity(pieces.len());

    for (index, (piece, scale)) in pieces.iter().zip(scales).enumerate() {
        if index > 0 {
            x += size.gap;
        }

        let width = piece.width(size, *scale);
        spans.push(Span { x, width });
        x += width;
    }

    Row {
        spans,
        width: x + size.inset(),
    }
}

/*
 * how many times its size each piece is with the pointer at `pointer` along the row at rest, from
 * the strip's left edge: an icon grows by how near it is, a cosine falling to none at its reach
 * taken over its width at rest around its center rather than at its center alone, so the one
 * centered under the pointer grows by just `extra` of its size. As dockbar does, nearness is
 * measured where each icon stands in the grown row: those the growing ones push aside are farther,
 * and grow less, so the swell is steep around the pointer rather than a broad hump
 */
fn scales(pieces: &[Piece], size: Size, pointer: f32, extra: f32) -> Vec<f32> {
    let rest = spans(pieces, size, &vec![1.0; pieces.len()]);

    // the cosine's integral from the pointer, flat beyond its reach
    let rise = |offset: f32| (PI / 2.0 * (offset / size.reach).clamp(-1.0, 1.0)).sin();
    let half = size.icon / 2.0;
    let centered = 2.0 * rise(half);

    let mut scales = vec![1.0; pieces.len()];

    // each round lays the row out at the last scales; it settles within a few, everywhere
    for _ in 0..SETTLE_ROUNDS {
        let grown = spans(pieces, size, &scales);
        // past the row's ends, as far past the grown row's
        let inside = pointer.clamp(0.0, rest.width);
        let pointer = follow(&rest, &grown, inside) + pointer - inside;

        scales = pieces
            .iter()
            .zip(&grown.spans)
            .map(|(piece, span)| match piece {
                Piece::Divider => 1.0,
                Piece::Icon => {
                    let center = span.x + span.width / 2.0 - pointer;
                    let near = (rise(center + half) - rise(center - half)) / centered;

                    1.0 + extra * near
                }
            })
            .collect();
    }

    scales
}

/*
 * how many times its size each piece of a side of the row is with the pointer `inward` of the
 * side's inner end, by the body: laid out outward from it, the nearest piece first, so a side to
 * the body's left is turned around and back. Behind the end, as over the body, the pointer still
 * reaches the pieces near it
 */
fn swell(pieces: &[Piece], size: Size, inward: f32, extra: f32, mirrored: bool) -> Vec<f32> {
    let mut outward = pieces.to_vec();

    if mirrored {
        outward.reverse();
    }

    let at = if inward <= 0.0 {
        inward
    } else {
        let none = Canvas {
            width: 0.0,
            height: 0.0,
        };

        under(&outward, size, none, Along::Left, inward, 0.0, extra)
    };
    let mut scales = scales(&outward, size, at, extra);

    if mirrored {
        scales.reverse();
    }

    scales
}

/*
 * where `at`, a point along the row at rest, lands in the grown row: as far through the same piece,
 * or the same gap or inset, so what was under the pointer still is
 */
fn follow(rest: &Row, grown: &Row, at: f32) -> f32 {
    let mut edges = vec![(0.0, 0.0)];

    for (rest, grown) in rest.spans.iter().zip(&grown.spans) {
        edges.push((rest.x, grown.x));
        edges.push((rest.x + rest.width, grown.x + grown.width));
    }

    edges.push((rest.width, grown.width));

    let at = at.clamp(0.0, rest.width);

    edges
        .windows(2)
        .find(|pair| at <= pair[1].0)
        .map_or(grown.width, |pair| {
            let ((rest_from, grown_from), (rest_to, grown_to)) = (pair[0], pair[1]);
            let through = if rest_to > rest_from {
                (at - rest_from) / (rest_to - rest_from)
            } else {
                0.0
            };

            grown_from + through * (grown_to - grown_from)
        })
}

/*
 * the point along the row at rest, from the strip's left edge, that the pointer is on once the row
 * has grown around it by `extra`, `pointer` being across the row at rest. Centered, the row grows
 * around the point under the pointer, which stays put; against a side it grows away from that
 * side, so the point is found that lands under the pointer
 */
fn under(
    pieces: &[Piece],
    size: Size,
    canvas: Canvas,
    along: Along,
    pointer: f32,
    rest_left: f32,
    extra: f32,
) -> f32 {
    let rest = spans(pieces, size, &vec![1.0; pieces.len()]);

    if along == Along::Center || extra == 0.0 {
        return pointer.clamp(0.0, rest.width);
    }

    // not clamped to the row at rest, which a row growing from a side reaches past
    let x = rest_left + pointer;

    // where the point `at` lands across the window; it moves on as `at` does
    let lands = |at: f32| {
        let grown = spans(pieces, size, &scales(pieces, size, at, extra));

        canvas.left(along, grown.width, None) + follow(&rest, &grown, at)
    };

    let (mut low, mut high) = (0.0, rest.width);

    for _ in 0..32 {
        let middle = (low + high) / 2.0;

        if lands(middle) < x {
            low = middle;
        } else {
            high = middle;
        }
    }

    (low + high) / 2.0
}

/*
 * the Dock laid out along its edge as `x` and off it as `y`, as on the top or bottom; on a side
 * edge each turned a quarter, so its row runs down the edge
 */
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Axes {
    sideways: bool,
}

impl Axes {
    fn of(anchor: Anchor) -> Self {
        Self {
            sideways: anchor.sideways(),
        }
    }

    // a size `along` its edge and `across` it, as a width and a height
    fn size(self, along: f32, across: f32) -> (f32, f32) {
        if self.sideways {
            (across, along)
        } else {
            (along, across)
        }
    }

    // a rect laid out along the edge as `x`, as it stands in the window
    fn rect(self, rect: Rect) -> Rect {
        if self.sideways {
            Rect {
                x: rect.y,
                y: rect.x,
                width: rect.height,
                height: rect.width,
            }
        } else {
            rect
        }
    }

    // of a point or size in the window, the part along the edge
    fn along(self, x: f32, y: f32) -> f32 {
        if self.sideways { y } else { x }
    }
}

// how wide the row is at its widest, with the pointer on a piece's middle or between two
fn widest(pieces: &[Piece], size: Size, most: f32) -> f32 {
    let row = spans(pieces, size, &vec![1.0; pieces.len()]);
    let middles = row.spans.iter().map(|span| span.x + span.width / 2.0);
    let gaps = row.spans.iter().map(|span| span.x - size.gap / 2.0);

    middles
        .chain(gaps)
        .chain([row.width - size.inset() + size.gap / 2.0])
        .map(|at| spans(pieces, size, &scales(pieces, size, at, most - 1.0)).width)
        .fold(row.width, f32::max)
}

// the Dock's window: as wide as the row at its widest, as tall as its icons at their largest
#[derive(Debug, Clone, Copy, PartialEq)]
struct Canvas {
    width: f32,
    height: f32,
}

impl Canvas {
    fn of(pieces: &[Piece], size: Size, most: f32) -> Self {
        let rest = spans(pieces, size, &vec![1.0; pieces.len()]).width;
        let widest = widest(pieces, size, most);

        Self {
            // room for it to grow to either side while centered under the pointer
            width: (rest + 2.0 * (widest - rest)).ceil(),
            height: (EDGE_GAP + size.body() + size.icon * (most - 1.0) + BOUNCE).ceil(),
        }
    }

    /*
     * where a row `width` wide starts: against the window's side, or centered; while `held`, the
     * pointer's x and the point of the row it is on, which then stays under it
     */
    fn left(self, along: Along, width: f32, held: Option<(f32, f32)>) -> f32 {
        match along {
            Along::Left => 0.0,
            Along::Right => self.width - width,
            Along::Center => held
                .map_or((self.width - width) / 2.0, |(x, on)| x - on)
                .clamp(0.0, (self.width - width).max(0.0)),
        }
    }
}

// the pointer over each output's Dock, by output name
#[derive(Default)]
pub struct Pointers {
    by: HashMap<String, Pointer>,
}

impl Service for Pointers {
    fn new() -> Self {
        Self::default()
    }

    // only the Dock's pointer events change it
    fn listen() {}
}

struct Pointer {
    // across the window, the last place the pointer was, kept as the icons shrink back after it
    x: f32,

    inside: bool,

    /*
     * whether the icons grow toward their magnification: from the first move after the pointer
     * comes in, as its x is only known then, until it leaves
     */
    growing: bool,

    // 0 to 100, how far the icons have grown toward their magnification
    grow: Spring<1>,

    // 0 to 100, how far an autohidden Dock is out
    shown: Spring<1>,

    // when it last left, so only the latest leave hides it
    left: Option<Instant>,
}

impl Pointer {
    fn new() -> Self {
        let timings = config::get().island;
        let mut grow = Spring::new([0.0], timings.motion);
        let shown = Spring::new([0.0], timings.motion);

        // the icons growing never pass their size; how the Dock comes and goes is set per move
        grow.damp(CRITICAL);

        Self {
            x: 0.0,
            inside: false,
            growing: false,
            grow,
            shown,
            left: None,
        }
    }

    fn grow(&self, now: Instant) -> f32 {
        (self.grow.at(now)[0] / 100.0).clamp(0.0, 1.0)
    }

    fn shown(&self, now: Instant) -> f32 {
        (self.shown.at(now)[0] / 100.0).max(0.0)
    }

    /*
     * a reload may have changed the motion since: the springs take it from where they stand, still,
     * so one switched mid move sets off again from there
     */
    fn retime(&mut self, now: Instant) {
        let timings = config::get().island;

        if self.grow.mode() != timings.motion {
            let grow = self.grow.target();
            let shown = self.shown.target();
            let damping = self.shown.damping();

            self.grow = Spring::new(self.grow.at(now), timings.motion);
            self.shown = Spring::new(self.shown.at(now), timings.motion);
            self.grow.damp(CRITICAL);
            self.shown.damp(damping);

            // still on their way where they were going; a spring at rest stays so
            if self.grow.at(now) != grow {
                self.grow
                    .to(grow, if grow[0] > 0.0 { GROW } else { SHRINK }, now);
            }

            if self.shown.at(now) != shown {
                self.shown
                    .to(shown, if shown[0] > 0.0 { SHOW } else { HIDE }, now);
            }
        }
    }
}

// read first, as a write redraws the Dock of its output
fn hover(monitor: &str, inside: bool) {
    let unchanged = |pointers: &Pointers| {
        pointers
            .by
            .get(monitor)
            .is_some_and(|pointer| pointer.inside == inside)
    };

    if unchanged(&Pointers::read()) {
        return;
    }

    let now = Instant::now();
    let mut pointers = Pointers::write();

    pointers.part(&monitor);

    let pointer = pointers
        .by
        .entry(monitor.to_owned())
        .or_insert_with(Pointer::new);

    pointer.inside = inside;
    pointer.retime(now);

    // folded, the Dock comes out of the Island with the pointer as an autohidden one does
    let autohide = autohides(config::get().dock_place) || merge::folds();

    /*
     * one always out stands shown while the pointer is in and hidden once it left, at once, so
     * an autohide turned on later keeps it out for a pointer still on it
     */
    if !autohide {
        let shown = if inside { 100.0 } else { 0.0 };
        pointer.shown = Spring::new([shown], pointer.shown.mode());
    }

    if inside {
        pointer.left = None;

        // back in before it hid, it is out or coming out already
        if autohide && pointer.shown.target() != [100.0] {
            pointer.shown.damp(config::get().island.damping);
            pointer.shown.to([100.0], SHOW, now);
        }
    } else {
        pointer.left = Some(now);
        pointer.growing = false;

        // a Dock that never grew, as with magnification off, has nothing to draw shrinking
        if pointer.grow.at(now) != [0.0] {
            pointer.grow.to([0.0], SHRINK, now);
        }

        if !autohide {
            return;
        }

        let monitor = monitor.to_owned();

        // an autohidden Dock waits a moment, so a pointer passing out and back keeps it
        thread::spawn(move || {
            thread::sleep(DELAY);

            let latest = |pointers: &Pointers| {
                pointers
                    .by
                    .get(&monitor)
                    .is_some_and(|pointer| pointer.left == Some(now))
            };

            if latest(&Pointers::read()) {
                let mut pointers = Pointers::write();

                if latest(&pointers) {
                    pointers.part(&monitor);

                    let now = Instant::now();
                    let pointer = pointers.by.get_mut(&monitor).expect("latest found it");

                    // going, it never passes the edge and swings back into view
                    pointer.retime(now);
                    pointer.shown.damp(CRITICAL);
                    pointer.shown.to([0.0], HIDE, now);
                } else {
                    pointers.quiet();
                }
            }
        });
    }
}

// an output that left keeps no hover or last frame, so one replugged under its name starts as new
pub fn forget_gone() {
    let monitors = Monitors::read();
    let here = |monitor: &String| monitors.has(monitor);

    // read first, as a write redraws every Dock
    if !Pointers::read().by.keys().all(here) {
        Pointers::write().by.retain(|monitor, _| here(monitor));
    }

    DRAWN
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|monitor, _| here(monitor));
}

fn pointed(monitor: &str, x: f32) {
    // without magnification nothing follows the pointer along the Dock
    if config::get().magnify == Magnify::Off {
        return;
    }

    let unchanged = |pointers: &Pointers| {
        pointers
            .by
            .get(monitor)
            .is_some_and(|pointer| pointer.x == x && pointer.growing == pointer.inside)
    };

    if unchanged(&Pointers::read()) {
        return;
    }

    let now = Instant::now();
    let mut pointers = Pointers::write();

    pointers.part(&monitor);

    let pointer = pointers
        .by
        .entry(monitor.to_owned())
        .or_insert_with(Pointer::new);

    pointer.x = x;

    if pointer.inside && !pointer.growing {
        pointer.growing = true;
        pointer.retime(now);
        pointer.grow.to([100.0], GROW, now);
    }
}
