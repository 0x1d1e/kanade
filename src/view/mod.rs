//! The Island's window and the reserve strip: one window per output draws the body, its forms
//! (in the modules below), the Satellites, the cluster and the Dock merged into it.

use std::iter;
use std::rc::Rc;
use std::time::Instant;

use kanade_runtime::service::Service;
use kanade_runtime::{
    Button, Center, Horizontal, Layer, LayerWindow, Monitor, Parent, Rectangle, Scroll, Stack,
    Start, Vertical, Widget, Zone, request_frame,
};

use crate::autohide;
use crate::banners;
use crate::cluster::{self, Cluster};
use crate::config;
use crate::dock;
use crate::glass;
use crate::island::activity::{Lifetime, Priority};
use crate::island::arbiter::SATELLITES;
use crate::island::geometry::{self, Rect};
use crate::island::presentation::{Content, Input, Presentation};
use crate::island::service::IslandService;
use crate::look::Merge;
use crate::merge;
use crate::scene::{self, Scene, Stand};
use crate::shadow;
use crate::surfaces;
use crate::theme;

mod covered;
mod forms;
mod hud;
mod input;
mod media;
mod rest;
mod satellites;
mod shape;
mod status;
mod toast;

pub(crate) use self::covered::covered;
use self::covered::raise;
use self::forms::{small_form, split, surface};
use self::hud::hud;
pub(crate) use self::input::{claim, collapse, input_area, key, open, pin};
use self::input::{expand, hover, moved_to, route, set_armed, set_segment};
pub(crate) use self::media::{Change, state, tile};
use self::rest::{bare, has_battery, placeholder, rest};
pub(crate) use self::rest::{time_peek, time_peek_upright};
use self::satellites::satellites;
use self::shape::Room;
pub(crate) use self::shape::{canvas, hang, largest, peek, shape, upright};
pub(crate) use self::status::bar;
pub(crate) use self::toast::toast_tile;

// the Island's body, as liquid glass names its pane
const ISLAND: &str = "island";

/*
 * whether the Island hides when idle on every output, as `island.autohide` has it, or as a Rest with
 * nothing to show does: it then leaves its edge to the windows and stands in no merge with the Dock
 */
pub fn hides() -> bool {
    let config = config::get();

    config.island_place.autohide || config.rests().all(|rest| rest.bare(has_battery()))
}

/*
 * how much of its edge the Island keeps windows off, `island.reserve`: its small forms, with their
 * Satellites, and the gap on both sides, as a menu bar does; across a side edge, its width. A Peek
 * or a Surface grows over the windows. None while it autohides, as it then leaves the edge to them
 */
pub fn reserved() -> f32 {
    let config = config::get();
    let place = config.island_place;

    // merged, the Dock's plate stands between it and the edge, and its reserve is this one too
    let reserve = place.reserve || config.dock_place.reserve && merge::form().is_some();

    if reserve && !hides() {
        geometry::rest_reach(place.anchor.hang(), SATELLITES) + geometry::TOP + merge::apart()
    } else {
        0.0
    }
}

/*
 * keeps windows off the Island's strip of the output: an empty window a pixel wide on its edge, as
 * layer-shell reserves the thickness of a window held to one edge
 */
pub fn reserve(_monitor: &Monitor) -> LayerWindow {
    let place = config::get().island_place;
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

    let keyboard = island.keyboard(&monitor.name);
    let canvas = canvas(monitor);
    let place = config::get().island_place;
    let hang = place.anchor.hang();
    let stand = Stand { canvas, hang };

    /*
     * merged with the Dock (ADR 0031), this window draws both: the Island's canvas lies at
     * `shell.island` in it and the Dock's at `shell.dock`, under one pane of glass
     */
    let shell = merge::shell(monitor);
    let origin = shell.as_ref().map_or((0.0, 0.0), |shell| shell.island);

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
    let cluster = if covered { None } else { cluster::read() };
    let capturing = cluster.is_some();

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
    let glass_body = scene.glass(pose);

    let spot = glass::Spot::new(&monitor.name, ISLAND);
    let captured = capture(&island, &monitor.name, stand, &scene, glass_body, now);

    // hidden past its edge, it captures nothing there
    let liquid = glass::place(
        monitor,
        ISLAND,
        place.anchor,
        scene.window(),
        ((out > 0.0 || sliding) && (!covered || raised)).then_some(captured),
    );

    let backdrop = glass::shown(&spot, glass_body);

    let mut layers: Vec<Box<dyn Widget>> = Vec::new();
    let mut pane = None;

    match glass_pane(
        glass_body,
        body_rect,
        shape.radius,
        backdrop,
        plate.is_some(),
    ) {
        Glass::United(united) => pane = Some(united),
        Glass::Under(under) => layers.push(under),
    }

    layers.push(Box::new(holder));

    let tray = matches!(island.presentation(&monitor.name), Presentation::Tray(_));

    layers.extend(marks(cluster.filter(|_| !tray), body_rect, shape, hang));
    layers.extend(hud(&island, monitor, body_rect, hang, now).map(|hud| Box::new(hud) as _));

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

    let shadow = shadows(body_rect, shape.radius, plate, origin, liquid, out);

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

    layers.extend(pane);
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
 * the Dock, now the body's place is known: its plate in the window, and for the glass the plate a
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

        if let Some((content, opacity)) = content.into_iter().flatten().next()
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
            holder = holder.child(form.opacity(opacity));
        }
    }

    (holder, room)
}

/*
 * the cluster over the form, going with the body's trailing end, whole up to a Peek and gone by
 * the smallest Surface as the Satellites are; a Tray's slots fill the body, so it has none
 */
fn marks(
    cluster: Option<Cluster>,
    body: Rect,
    shape: geometry::Shape,
    hang: geometry::Hang,
) -> Option<Box<dyn Widget>> {
    let cluster = cluster?;
    let opacity = geometry::satellite_opacity(shape, hang);

    (opacity > 0.0)
        .then(|| Box::new(cluster.draw(body, hang.sideways()).opacity(opacity)) as Box<dyn Widget>)
}

/*
 * liquid glass casts none: it would gray the backdrop around it, which its rim then bends in.
 * Nor does a body hidden past its edge, where the shadow would still reach onto the screen.
 * Cast or not, it takes as many layers, so the body's buttons and sliders keep their place
 * among the targets mid-press or mid-drag; the Dock's plate, if merged, is cast in this
 * canvas's coordinates, from the canvas's `origin` in the window
 */
fn shadows(
    body: Rect,
    radius: f32,
    plate: Option<merge::Plate>,
    origin: (f32, f32),
    liquid: bool,
    out: f32,
) -> Vec<Box<dyn Widget>> {
    let mut shadow = Vec::new();

    if !liquid && out > 0.0 {
        shadow::draw(&mut shadow, body, radius);
    } else {
        shadow::none(&mut shadow);
    }

    if let Some(plate) = plate {
        if liquid {
            shadow::none(&mut shadow);
        } else {
            let rect = Rect {
                x: plate.rect.x - origin.0,
                y: plate.rect.y - origin.1,
                ..plate.rect
            };

            shadow::draw(&mut shadow, rect, plate.radius);
        }
    }

    shadow
}

/*
 * captured out to everywhere the body goes until its rim is shown, so it fits as the body
 * grows, and never samples the Island where the spring overshoots on the way; and to where it
 * was a quarter of that before, as the screen it is captured from lags this frame
 */
fn capture(
    island: &IslandService,
    monitor: &str,
    stand: Stand,
    scene: &Scene,
    body: glass::Body,
    now: Instant,
) -> glass::Body {
    let since = now.checked_sub(glass::LEAD / 4).unwrap_or(now);

    iter::once(since)
        .chain((1..=4).map(|step| now + glass::LEAD * step / 4))
        .fold(body, |captured, at| {
            let ahead = stand.pose(island.shape(monitor, at), autohide::at(monitor, at));

            glass::ahead(captured, scene.glass(ahead))
        })
}

// the glass under the Island's body
enum Glass {
    // its own layer under the body's content
    Under(Box<dyn Widget>),

    // merged, one pane in the window's coordinates under both the body and the Dock's plate
    United(Box<dyn Widget>),
}

/*
 * merged, one pane under both the Island's body and the Dock's plate, in the window's
 * coordinates, which cuts its tint and highlights to the two united; the body then has none
 * of its own
 */
fn glass_pane(
    glass_body: glass::Body,
    body: Rect,
    radius: f32,
    backdrop: Option<glass::Seen>,
    merged: bool,
) -> Glass {
    if !merged {
        return Glass::Under(Box::new(glass::layer(glass::Pane {
            width: body.width,
            height: body.height,
            radius,
            variant: glass::Variant::Clear,
            tone: None,
            light: None,
            backdrop,
            united: None,
        })));
    }

    let (x, y, width, height) = glass_body.bounds();

    Glass::United(Box::new(
        Rectangle::new()
            .width(width)
            .height(height)
            .align_child(Start, Start)
            .translate(x, y)
            .child(glass::layer(glass::Pane {
                width,
                height,
                radius,
                variant: glass::Variant::Clear,
                tone: None,
                light: None,
                backdrop,
                united: Some(scene::within(glass_body, (x, y))),
            })),
    ))
}

/*
 * exactly the input region, so leaving it is leaving the island; niri also sends the leave
 * when the region shrinks away from a still pointer (#3), which is what reports it here.
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
        .on_scroll(move |Scroll { y, .. }| route(&scrolled, Input::Wheel(y)));

    (target, track)
}
