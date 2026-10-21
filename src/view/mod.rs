//! The Island's window and the reserve strip: one window per output draws the body, its forms
//! (in the modules below), the Satellites, the cluster and the Dock merged into it.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use kanade_runtime::MutexExt;
use kanade_runtime::service::{Service, Write};
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Button, Center, Horizontal, Layer, LayerWindow, Monitor, Parent, Rectangle, Scroll, Stack,
    Start, Vertical, Widget, Zone, request_frame,
};

use crate::autohide;
use crate::banners;
use crate::cluster::{self, Cluster};
use crate::config;
use crate::dock;
use crate::island::activity::{Lifetime, Priority};
use crate::island::arbiter::SATELLITES;
use crate::island::geometry::{self, Rect};
use crate::island::presentation::{Content, Input, Presentation};
use crate::island::service::IslandService;
use crate::look::{Anchor, Merge};
use crate::merge;
use crate::pane;
use crate::scene::{self, Scene, Stand};
use crate::shadow;
use crate::surfaces;
use crate::theme;

mod covered;
mod entrance;
mod forms;
mod hud;
mod input;
mod live;
mod media;
mod rest;
mod satellites;
mod shape;
mod standing;
mod status;
mod toast;

pub(crate) use self::covered::covered;
use self::covered::raise;
use self::entrance::Entrance;
pub(crate) use self::entrance::rise;
use self::forms::{small_form, split, surface};
use self::hud::hud;
pub(crate) use self::input::{claim, collapse, input_area, key, open, pin};
use self::input::{expand, hover, moved_to, route, set_armed, set_segment, wheel};
pub(crate) use self::media::{Change, state, tile};
pub(crate) use self::rest::time_peek;
use self::rest::{bare, has_battery, placeholder, rest};
use self::satellites::satellites;
use self::shape::Room;
pub(crate) use self::shape::{canvas, hang, largest, shape, tray_slots, upright};
pub(crate) use self::standing::time_peek as time_peek_upright;
pub(crate) use self::status::bar;
pub(crate) use self::toast::toast_tile;

/*
 * sizes the Island to the outputs, the settings and today's next event (ADR 0030), redrawing
 * nothing when they ask for what it already has
 */
pub(crate) fn resize(island: &mut Write<IslandService>) {
    let (wanted, holds) = shape::holds();

    if !island.resize(wanted, holds, hang(), Instant::now()) {
        island.quiet();
    }
}

// the Island's body and output size as last drawn, for positioning wallpaper actions
struct Placed {
    body: (f32, f32),
    screen: (f32, f32),
}

static PLACED: LazyLock<Mutex<HashMap<String, Placed>>> = LazyLock::new(Mutex::default);

fn placed(monitor: &Monitor, anchor: Anchor, window: (f32, f32), body: Rect) {
    let screen = (monitor.width, monitor.height);
    let (horizontal, vertical) = (anchor.horizontal(), anchor.vertical());
    let origin = (
        match horizontal {
            Horizontal::Left => 0.0,
            Horizontal::Middle => ((screen.0 as f32 - window.0.round()) / 2.0).floor(),
            Horizontal::Right => screen.0 as f32 - window.0.round(),
        },
        match vertical {
            Vertical::Top => 0.0,
            Vertical::Middle => ((screen.1 as f32 - window.1.round()) / 2.0).floor(),
            Vertical::Bottom => screen.1 as f32 - window.1.round(),
        },
    );
    let placed = Placed {
        body: (origin.0 + body.x, origin.1 + body.y),
        screen: (screen.0 as f32, screen.1 as f32),
    };
    PLACED.locked().insert(monitor.name.clone(), placed);
}

/*
 * a point of the body as last drawn, as a fraction of its output: 0 to 1 across from the left and
 * down from the top; none before it is drawn there
 */
pub(crate) fn on_screen(monitor: &str, (x, y): (f32, f32)) -> Option<(f32, f32)> {
    let all = PLACED.locked();
    let placed = all.get(monitor)?;

    (placed.screen.0 > 0.0 && placed.screen.1 > 0.0).then(|| {
        (
            (placed.body.0 + x) / placed.screen.0,
            (placed.body.1 + y) / placed.screen.1,
        )
    })
}

// the opacity the incoming content reaches before it takes input; the outgoing never does
const LIVE: f32 = 0.5;

/*
 * whether the Island hides when idle on every output, as `island.autohide` has it, or as a Rest with
 * nothing to show does: it then leaves its edge to the windows and stands in no merge with the Dock
 */
pub fn hides() -> bool {
    let config = config::get();

    config.island_placement().autohide || shape::rests().iter().all(|rest| rest.bare(has_battery()))
}

/*
 * how much of its edge the Island keeps windows off, `island.reserve`: its small forms, with their
 * Satellites, and the gap on both sides, as a menu bar does; across a side edge, its width. A Peek
 * or a Surface grows over the windows. None while it autohides, as it then leaves the edge to them.
 * A capture does not hold a hidden Island's strip (`hides` ignores privacy), so the reserve and the
 * Dock's merge never flicker as one starts and stops
 */
pub fn reserved() -> f32 {
    let config = config::get();
    let place = config.island_placement();
    let dock_place = config.dock_placement();
    let island_visible = !hides();
    let merged = merge::form().is_some();
    let dock_can_reserve = dock_place.reserve && !dock_place.autohide && merged;
    let dock_reach = (!island_visible && dock_can_reserve)
        .then(dock::extent)
        .flatten()
        .map_or(0.0, |extent| extent.depth);

    reservation_depth(
        island_visible,
        place.reserve,
        dock_can_reserve,
        geometry::rest_reach(place.anchor.hang(), SATELLITES) + geometry::TOP,
        dock_reach,
        merge::apart(),
    )
}

fn reservation_depth(
    island_visible: bool,
    island_reserve: bool,
    dock_can_reserve: bool,
    island_reach: f32,
    dock_reach: f32,
    apart: f32,
) -> f32 {
    if island_visible && (island_reserve || dock_can_reserve) {
        island_reach + apart
    } else if dock_can_reserve {
        dock_reach
    } else {
        0.0
    }
}

/*
 * keeps windows off the Island's strip of the output: an empty window a pixel wide on its edge, as
 * layer-shell reserves the thickness of a window held to one edge
 */
pub fn reserve(_monitor: &Monitor) -> LayerWindow {
    let place = config::get().island_placement();
    let reserved = reserved();

    // held to its edge alone, so the runtime reserves its thickness off that edge
    let (width, height, vertical, horizontal) = if place.anchor.sideways() {
        let horizontal = place.anchor.horizontal();
        (reserved.max(1.0), 1.0, Vertical::Middle, horizontal)
    } else {
        let vertical = place.anchor.vertical();
        (1.0, reserved.max(1.0), vertical, Horizontal::Middle)
    };

    LayerWindow::new()
        .width(width)
        .height(height)
        .anchor_vertical(vertical)
        .anchor_horizontal(horizontal)
        .layer(Layer::Bottom)
        .space(Zone::Reserve)
        .namespace("kanade-reserve")
        .visible(reserved > 0.0)
        .click_through()
        .child(Rectangle::new().width(Parent).height(Parent))
}

// one island window per monitor; the window stays put and only the body morphs inside it
pub fn island(monitor: &Monitor) -> LayerWindow {
    // reading subscribes this window to island changes
    let island = IslandService::read();

    let now = Instant::now();
    let _on = shape::On::take(monitor);

    let keyboard = island.keyboard(&monitor.name);
    let canvas = canvas(monitor);
    let place = config::get().island_placement();
    let hang = place.anchor.hang();

    /*
     * merged with the Dock (ADR 0031), this window draws both: the Island's canvas lies at
     * `shell.island` in it and the Dock's at `shell.dock`, under one pane
     */
    let shell = merge::shell(monitor);
    let origin = shell.as_ref().map_or((0.0, 0.0), |shell| shell.island);
    let edge_offset = shell
        .as_ref()
        .map_or(0.0, |shell| shell.edge_offset(canvas, hang.edge));
    let stand = Stand {
        canvas,
        hang,
        edge_offset,
    };

    let content = island.content(&monitor.name, now);

    /*
     * a fullscreen window covers the Island, which there waits on the Overlay layer, hidden, as niri
     * would send it frames only about once a second under the window: the OSD, or a Surface opened
     * from a keybind, then shows over the window at once (ADR 0021). The overview draws the Top
     * layer over every window
     */
    let covered = covered(monitor, || island.overview());
    let frame = island.frame(&monitor.name, now);
    // the OSD's own: Transient, so a debug post of any Feedback cannot hold it over the window
    let feedback = frame.primary.as_ref().is_some_and(|primary| {
        primary.priority() == Priority::Feedback
            && matches!(primary.lifetime(), Lifetime::Transient(_))
    });
    let raised = raise(
        &monitor.name,
        covered && (feedback || island.expanded(&monitor.name)),
        island.settled(&monitor.name, now),
    );

    // the spring is ours, not a runtime Animation, so the view asks for frames until it rests
    if !island.settled(&monitor.name, now) {
        request_frame();
    }

    // what captures stays shown, wherever the Island goes, but not over a fullscreen window (ADR 0033)
    let cluster = cluster::read(&monitor.name, covered, now);
    let capturing = cluster.is_some();

    if cluster.as_ref().is_some_and(|cluster| cluster.moving) {
        request_frame();
    }

    // `island.autohide`: idle, it slides past its edge, leaving a strip there that brings it back
    let idle = island.presentation(&monitor.name) == Presentation::Rest
        && frame.satellites.is_empty()
        && !island.inside(&monitor.name)
        && !island.pinned(&monitor.name)
        && !island.overview()
        && !banners::showing(&monitor.name)
        && !capturing;
    let (out, sliding) = autohide::out(&monitor.name, idle, bare(&monitor.name) && !capturing, now);

    if sliding {
        request_frame();
    }

    let pose = stand.pose(island.shape(&monitor.name, now), out);
    let shape = pose.shape;
    let still = island.under(&monitor.name);
    let scene = Scene::new(stand, shell.clone());
    let body_rect = stand.body(pose);
    let area = stand.area(pose, still);

    // folded, the pointer on the Island brings the Dock out rather than peeking or showing the tray
    let peeks = !shell
        .as_ref()
        .is_some_and(|shell| shell.form == Merge::Fold);
    let pressed = monitor.name.clone();

    let (holder, room) = form_holder(
        &island,
        monitor,
        content,
        (body_rect, hang),
        cluster.as_ref(),
        now,
    );

    let (laid, scene) = dock_in(
        monitor,
        shell.as_ref(),
        scene,
        (body_rect, shape.radius),
        area,
    );
    let plate = scene.plate();
    let pane_body = scene.pane(pose);

    // hidden past its edge, it has no pane fill
    let merged_dock_visible = shell
        .as_ref()
        .zip(laid.as_ref())
        .is_some_and(|(shell, laid)| shell.form != Merge::Fold || laid.held > 0.0);
    let blur_shown = (pane::blurs(out) || sliding || merged_dock_visible) && (!covered || raised);
    let mut layers: Vec<Box<dyn Widget>> = Vec::new();
    let mut pane = None;

    // a layer that is not there takes its place all the same, so the targets after keep theirs
    let nothing = || Box::new(Rectangle::new().width(0.0).height(0.0)) as Box<dyn Widget>;

    match body_pane(pane_body, body_rect, shape.radius, plate.is_some()) {
        Pane::United(united) => {
            pane = Some(united);
            layers.push(nothing());
        }
        Pane::Under(under) => layers.push(under),
    }

    layers.push(Box::new(holder));

    let presentation = island.presentation(&monitor.name);
    let sideways = hang.sideways() && cluster::at_end(presentation);

    layers.push(marks(cluster, body_rect, sideways).unwrap_or_else(nothing));
    layers.push(
        hud(&island, monitor, body_rect, hang, now).map_or_else(nothing, |hud| Box::new(hud) as _),
    );

    let mut body = Rectangle::new()
        .width(body_rect.width)
        .height(body_rect.height)
        .radius(shape.radius)
        .clip()
        .align_child(Start, Start)
        .translate(body_rect.x, body_rect.y)
        .child(Stack::new(layers));

    // a pinned island says why it stays open with nobody on it
    if island.pinned(&monitor.name) {
        body = body.border(1.0, theme::island().outline);
    }

    let (hover, track) = hover_target(&monitor.name, area, room, hang, canvas, peeks);

    let shadow = shadows(body_rect, shape.radius, plate, origin, out);

    // fading as the body slides out of the way, so it never pops at the edge
    let mut under: Vec<Box<dyn Widget>> = vec![Box::new(
        Rectangle::new()
            .width(canvas.width)
            .height(canvas.height)
            .align_child(Start, Start)
            .opacity(out.min(1.0))
            .child(Stack::new(shadow)),
    )];

    /*
     * the body grows over the Satellites as they fade, and they never take the pointer. They come
     * and go, but only about a body smaller than Media's, and rarely mid-press
     */
    if !island.overview() {
        under.extend(satellites(
            island.satellites(&monitor.name),
            body_rect,
            hang,
            canvas,
            shape,
            now,
        ));
    }

    let canvas_at = |layers| in_canvas(shell.as_ref().map(|_| origin), canvas, layers);

    let mut layers = vec![canvas_at(vec![
        Box::new(hover),
        Box::new(Stack::new(under)),
    ])];

    layers.push(pane.unwrap_or_else(nothing));
    layers.push(canvas_at(vec![Box::new(body)]));

    /*
     * the Dock's icons come last, as how many there are changes the targets after them and no
     * Island button should lose its place for it
     */
    layers.extend(
        laid.zip(shell.as_ref())
            .map(|(laid, shell)| dock_icons(laid, shell, origin, area, track)),
    );

    let window = scene.window();

    placed(monitor, place.anchor, window, scene.in_window(body_rect));

    // where the pointer reaches in this window: the Island's area, and the Dock's if merged
    let reach: Vec<_> = scene
        .reach(pose, still)
        .into_iter()
        .map(input_area)
        .collect();

    LayerWindow::new()
        .width(window.0)
        .height(window.1)
        .anchor_vertical(place.anchor.vertical())
        .anchor_horizontal(place.anchor.horizontal())
        .layer(if covered { Layer::Overlay } else { Layer::Top })
        .visible(!covered || raised)
        .space(Zone::Ignore)
        .namespace("kanade")
        // the panes' outlines, the body drawn alone or united with the Dock's plate
        .blur(pane::blurred(blur_shown.then_some(pane_body), (0.0, 0.0)))
        .keyboard(keyboard)
        .on_key(move |key| self::key(&pressed, key))
        // empty under niri's overview, so the pointer reaches the overview beneath
        .input_region(if island.overview() { vec![] } else { reach })
        .child(
            Rectangle::new()
                .width(Parent)
                .height(Parent)
                .align_child(Start, Start)
                .child(Stack::new(layers)),
        )
}

/*
 * the Dock, now the body's place is known: its plate in the window, and for the pane the plate a
 * body of any shape would have, as the icons go out to meet it; none unless merged
 */
fn dock_in(
    monitor: &Monitor,
    shell: Option<&merge::Shell>,
    scene: Scene,
    (body, radius): (Rect, f32),
    area: Rect,
) -> (Option<dock::Laid>, Scene) {
    let laid = shell.map(|shell| {
        if shell.among() {
            dock::between(
                monitor,
                shell,
                scene.in_window(body),
                radius,
                scene.in_window(area),
            )
        } else {
            dock::lay(monitor, true)
        }
    });

    let scene = match &laid {
        Some(laid) => scene.with_dock(laid.fit()),
        None => scene,
    };

    (laid, scene)
}

// the Island's canvas where it lies in a merged window, at `origin`; as it is, alone
fn in_canvas(
    origin: Option<(f32, f32)>,
    canvas: geometry::Canvas,
    layers: Vec<Box<dyn Widget>>,
) -> Box<dyn Widget> {
    let Some(origin) = origin else {
        return Box::new(Stack::new(layers));
    };

    Box::new(
        Rectangle::new()
            .width(canvas.width)
            .height(canvas.height)
            .align_child(Start, Start)
            .translate(origin.0, origin.1)
            .child(Stack::new(layers)),
    )
}

/*
 * the merged Dock's icons and their target, where the shell puts the Dock. The target over the
 * Island's own passes its moves on, those over the Island's `area`, to the Island's `track`
 */
fn dock_icons(
    laid: dock::Laid,
    shell: &merge::Shell,
    origin: (f32, f32),
    area: Rect,
    track: Rc<dyn Fn(f32, f32)>,
) -> Box<dyn Widget> {
    let (width, height) = laid.canvas;
    let dock = shell.dock;

    let island = Rc::new(move |x: f32, y: f32| {
        let (x, y) = (x + dock.0 - origin.0, y + dock.1 - origin.1);

        if x >= area.x && x < area.right() && y >= area.y && y < area.bottom() {
            track(x, y);
        }
    });
    let (target, icons) = laid.drawn(Some(island));

    Box::new(
        Rectangle::new()
            .width(width)
            .height(height)
            .align_child(Start, Start)
            .translate(dock.0, dock.1)
            .child(Stack::new(vec![Box::new(target), Box::new(icons)])),
    )
}

/*
 * the form the Island shows, laid out in a holder as big as the body, and the room the cluster takes
 * of it. The morph reveals content already laid out at its final size, clipped to the body's
 * corners. A small form lays out in what the cluster leaves of the body, from its start, so what
 * was centered stands to the leading side of the dots
 */
fn form_holder(
    island: &IslandService,
    monitor: &Monitor,
    content: [Option<(Content, f32)>; 2],
    (body, hang): (Rect, geometry::Hang),
    cluster: Option<&Cluster>,
    now: Instant,
) -> (Rectangle, f32) {
    let room = cluster
        .filter(|_| cluster::beside(island.presentation(&monitor.name)))
        .map_or(0.0, Cluster::room);
    let holder = Rectangle::new().width(body.width).height(body.height);
    let mut holder = if room > 0.0 && !hang.sideways() {
        holder.align_child(Start, Start)
    } else {
        holder.align_child(Center, Start)
    };

    // at most one shows at a time, the crossfade hands over through nothing
    {
        let _room = Room::take(room);
        // `Crossfade::shown` never shows both, so the first is the one leaving
        let leaving = content[0].is_some();
        let shown = content.into_iter().flatten().next();

        // a Surface coming in arrives a section at a time, one going out leaves as a whole
        let _entrance = Entrance::take(
            shown
                .as_ref()
                .map_or(1.0, |&(_, opacity)| if leaving { 1.0 } else { opacity }),
        );

        if let Some((content, opacity)) = shown
            && let Some(form) = split(
                &content,
                island.track(&monitor.name),
                island.swap(&monitor.name, now),
                now,
            )
            .or_else(|| small_form(&content, island.track(&monitor.name), now))
            .or_else(|| surface(&monitor.name, &content, island, now))
            .or_else(|| surfaces::tray::strip(&monitor.name, content.presentation))
            .or_else(|| rest(&monitor.name, &content))
            .or_else(|| placeholder(&content))
        {
            holder = holder.child(form.opacity(opacity).inert(leaving || opacity < LIVE));
        }
    }

    (holder, room)
}

// the cluster over the form, at the body's trailing end, or its top corner once a Surface is open
fn marks(cluster: Option<Cluster>, body: Rect, sideways: bool) -> Option<Box<dyn Widget>> {
    Some(Box::new(cluster?.draw(body, sideways)))
}

/* A body hidden past its edge has no shadow; the merged Dock's plate uses window coordinates. */
fn shadows(
    body: Rect,
    radius: f32,
    plate: Option<merge::Plate>,
    origin: (f32, f32),
    out: f32,
) -> Vec<Box<dyn Widget>> {
    let mut shadow = Vec::new();

    if out > 0.0 {
        shadow::draw(&mut shadow, body, radius);
    } else {
        shadow::none(&mut shadow);
    }

    if let Some(plate) = plate {
        let rect = Rect {
            x: plate.rect.x - origin.0,
            y: plate.rect.y - origin.1,
            ..plate.rect
        };

        shadow::draw(&mut shadow, rect, plate.radius);
    }

    shadow
}

// the pane under the Island's body
enum Pane {
    // its own layer under the body's content
    Under(Box<dyn Widget>),

    // merged, one pane in the window's coordinates under both the body and the Dock's plate
    United(Box<dyn Widget>),
}

/*
 * merged, one pane under both the Island's body and the Dock's plate, in the window's
 * coordinates, which cuts its fill to the two united; the body then has none of its own
 */
fn body_pane(pane_body: pane::Body, body: Rect, radius: f32, merged: bool) -> Pane {
    if !merged {
        return Pane::Under(Box::new(pane::layer(pane::Pane {
            width: body.width,
            height: body.height,
            radius,
            united: None,
        })));
    }

    let (x, y, width, height) = pane_body.bounds();

    Pane::United(Box::new(
        Rectangle::new()
            .width(width)
            .height(height)
            .align_child(Start, Start)
            .translate(x, y)
            .child(pane::layer(pane::Pane {
                width,
                height,
                radius,
                united: Some(scene::within(pane_body, (x, y))),
            })),
    ))
}

/*
 * exactly the input region, so leaving it is leaving the island; niri also sends the leave
 * when the region shrinks away from a still pointer, which is what reports it here.
 * Hover, click and arming all take this one target: the runtime hit-tests only on pointer events,
 * so a body that grows under a still pointer (Peek) would never report it on the body.
 * It draws nothing, and comes first: the runtime knows what the pointer is on by its place among
 * the targets, so the shadow's pieces and the Satellites, which come and go, must not move it,
 * or a leave after they did would go to another target. With it, what a pointer move there tracks,
 * which a Dock's target over it passes on
 */
fn hover_target(
    monitor: &str,
    area: Rect,
    room: f32,
    hang: geometry::Hang,
    canvas: geometry::Canvas,
    peeks: bool,
) -> (Rectangle, Rc<dyn Fn(f32, f32)>) {
    let (clicked, moved, hovered, scrolled) = (
        monitor.to_owned(),
        monitor.to_owned(),
        monitor.to_owned(),
        monitor.to_owned(),
    );

    // Escape disarms without the pointer leaving, the next move arms again
    let track: Rc<dyn Fn(f32, f32)> = Rc::new(move |x, y| {
        set_armed(&moved, true);

        set_segment(&moved, geometry::segment(x, y, room, hang, canvas));
        moved_to(&moved, x, y, canvas);
    });
    let tracked = track.clone();

    let target = Rectangle::new()
        .width(area.width)
        .height(area.height)
        .translate(area.x, area.y)
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::Button, "island"))
        .on_hover(move |inside| hover(&hovered, inside, peeks))
        .on_move(move |point| tracked(area.x + point.x, area.y + point.y))
        .on_click(move |button| match button {
            Button::Left => expand(&clicked),
            Button::Right => {
                let segment = IslandService::read().segment(&clicked);

                route(&clicked, Input::RightClick(segment));
            }
            _ => {}
        })
        .on_scroll(move |Scroll { y, .. }| {
            wheel(&scrolled, y);
            route(&scrolled, Input::Wheel(y));
        });

    (target, track)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_island_keeps_only_a_requested_merged_dock_reservation() {
        assert_eq!(
            reservation_depth(false, false, true, 90.0, 50.0, 12.0),
            50.0
        );
        assert_eq!(reservation_depth(false, true, false, 90.0, 0.0, 12.0), 0.0);
        assert_eq!(reservation_depth(false, false, false, 90.0, 0.0, 12.0), 0.0);
    }

    #[test]
    fn visible_merged_shell_reserves_its_combined_reach() {
        assert_eq!(reservation_depth(true, true, true, 80.0, 0.0, 12.0), 92.0);
        assert_eq!(reservation_depth(true, false, true, 80.0, 0.0, 12.0), 92.0);
        assert_eq!(reservation_depth(true, true, false, 80.0, 0.0, 0.0), 80.0);
        assert_eq!(reservation_depth(false, false, false, 90.0, 0.0, 12.0), 0.0);
        assert_eq!(reservation_depth(true, false, false, 80.0, 0.0, 12.0), 0.0);
    }
}
