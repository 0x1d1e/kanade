use std::time::Instant;

use amane::{
    Button, Center, Horizontal, InputArea, Key, Layer, LayerWindow, Monitor, Parent, Rectangle,
    Scroll, Service, Stack, Start, Text, Vertical, Widget, Zone, request_frame,
};

use crate::island::activity::{Activity, Frame, Kind};
use crate::island::geometry::{self, Rect, Shape};
use crate::island::presentation::{Content, Input, Presentation};
use crate::island::service::IslandService;
use crate::theme;

// one island window per monitor; the window stays put and only the body morphs inside it
pub fn island(monitor: &Monitor) -> LayerWindow {
    // reading subscribes this window to island changes
    let island = IslandService::read();

    let now = Instant::now();

    let keyboard = island.keyboard(&monitor.name);
    let shape = island.shape(&monitor.name, now);
    let content = island.content(&monitor.name, now);
    let frame = island.frame(&monitor.name, now);

    // the spring is ours, not an Amane Animation, so the view asks for frames until it rests
    if !island.settled(&monitor.name, now) {
        request_frame();
    }

    let body_rect = geometry::body(shape);
    let area = geometry::input_area(body_rect);

    let clicked = monitor.name.clone();
    let moved = monitor.name.clone();
    let hovered = monitor.name.clone();
    let pressed = monitor.name.clone();
    let scrolled = monitor.name.clone();

    // the morph reveals content already laid out at its final size, clipped to the body's corners
    let mut body = Rectangle::new()
        .width(body_rect.width)
        .height(body_rect.height)
        .radius(shape.radius)
        .fill(theme::BODY)
        .clip()
        .align_child(Center, Start)
        .translate(body_rect.x - area.x, body_rect.y - area.y);

    // at most one shows at a time, the crossfade hands over through nothing
    if let Some((content, opacity)) = content.into_iter().flatten().next()
        && let Some(placeholder) = placeholder(&content)
    {
        let size = geometry::shape(content.presentation);

        body = body.child(
            Rectangle::new()
                .width(size.width)
                .height(size.height)
                .opacity(opacity)
                .align_child(Center, Center)
                .child(placeholder),
        );
    }

    /*
     * exactly the input region, so leaving it is leaving the island; niri also sends the leave
     * when the region shrinks away from a still pointer (#3), which is what reports it here.
     * Hover, click and arming all take this one target: Amane hit-tests only on pointer events,
     * so a body that grows under a still pointer (Peek) would never report it on the body
     */
    let hover = Rectangle::new()
        .width(area.width)
        .height(area.height)
        .translate(area.x, area.y)
        .align_child(Start, Start)
        .on_hover(move |inside| hover(&hovered, inside))
        // Escape disarms without the pointer leaving, the next move arms again
        .on_move(move |_| set_armed(&moved, true))
        .on_click(move |button| match button {
            Button::Left => expand(&clicked),
            Button::Right => route(&clicked, Input::RightClick),
            _ => {}
        })
        .on_scroll(move |Scroll { y, .. }| route(&scrolled, Input::Wheel(y)))
        .child(body);

    // the body grows over the Satellites as they fade, and they never take the pointer
    let mut layers = if island.overview() {
        Vec::new()
    } else {
        satellites(&frame, body_rect, shape)
    };

    layers.push(Box::new(hover));

    if let Some(badge) = queued(&frame, body_rect, shape) {
        layers.push(Box::new(badge));
    }

    LayerWindow::new()
        .width(geometry::CANVAS_WIDTH)
        .height(geometry::CANVAS_HEIGHT)
        .anchor_vertical(Vertical::Top)
        .anchor_horizontal(Horizontal::Middle)
        .layer(Layer::Overlay)
        .space(Zone::Ignore)
        .namespace("kanade")
        .keyboard(keyboard)
        .on_key(move |key| {
            if key == Key::Escape {
                collapse(&pressed);

                // OnDemand would keep the focus the press gave while the pointer rests on the pill
                set_armed(&pressed, false);
            }
        })
        // empty under niri's overview, so the pointer reaches the overview beneath
        .input_region(if island.overview() {
            vec![]
        } else {
            vec![input_area(area)]
        })
        .child(
            Rectangle::new()
                .width(Parent)
                .height(Parent)
                .align_child(Start, Start)
                .child(Stack::new(layers)),
        )
}

// the Satellites, then the ones past the cap as one "+N"
fn satellites(frame: &Frame, body: Rect, shape: Shape) -> Vec<Box<dyn Widget>> {
    let opacity = geometry::satellite_opacity(shape);

    if opacity == 0.0 {
        return Vec::new();
    }

    let labels = frame
        .satellites
        .iter()
        .map(|activity| abbreviation(activity.kind()).to_owned())
        .chain((frame.overflow > 0).then(|| format!("+{}", frame.overflow)));

    labels
        .enumerate()
        .map(|(index, label)| {
            let at = geometry::satellite(body, index);

            Box::new(dot(at, label).opacity(opacity)) as Box<dyn Widget>
        })
        .collect()
}

/*
 * the Transients an open Surface keeps back, as a count in its top right corner; it fades in as
 * the Satellites fade out
 */
fn queued(frame: &Frame, body: Rect, shape: Shape) -> Option<Rectangle> {
    let opacity = 1.0 - geometry::satellite_opacity(shape);

    if frame.queued.is_empty() || opacity == 0.0 {
        return None;
    }

    let inset = 12.0;
    let at = Rect {
        x: body.x + body.width - inset - geometry::SATELLITE,
        y: body.y + inset,
        width: geometry::SATELLITE,
        height: geometry::SATELLITE,
    };

    Some(dot(at, frame.queued.len().to_string()).opacity(opacity))
}

fn dot(at: Rect, label: String) -> Rectangle {
    Rectangle::new()
        .width(at.width)
        .height(at.height)
        .radius(at.width / 2.0)
        .fill(theme::DOT)
        .align_child(Center, Center)
        .translate(at.x, at.y)
        .child(Text::new(label).size(12.0).color(theme::FG).weight(600))
}

// stand-in for each Kind's glyph (#21-#33), distinct per Kind
fn abbreviation(kind: Kind) -> &'static str {
    match kind {
        Kind::Media => "M",
        Kind::Notification => "N",
        Kind::Volume => "V",
        Kind::Brightness => "Br",
        Kind::Workspace => "W",
        Kind::Battery => "Ba",
        Kind::Network => "Nw",
        Kind::Bluetooth => "Bt",
        Kind::ScreenCast => "S",
        Kind::Timer => "T",
        Kind::Privacy => "P",
    }
}

/*
 * stand-in content until the sources and the Surfaces draw their own (#21-#33): names the Activity
 * a small form shows, or the open Surface, so the crossfade and the clipping can be seen. Short,
 * so sized to its letters and centered; Rest shows nothing
 */
fn placeholder(content: &Content) -> Option<Text> {
    let name = |activity: &Option<Activity>| {
        activity.as_ref().map_or_else(String::new, |activity| {
            format!("{} {}", activity.kind().name(), activity.id().key())
        })
    };

    let (label, size) = match content.presentation {
        Presentation::Rest => return None,
        Presentation::Compact => (name(&content.activity), 13.0),
        Presentation::Peek => (name(&content.activity), 15.0),
        Presentation::Expanded(surface) => (format!("{surface:?}"), 17.0),
    };

    Some(Text::new(label).size(size).color(theme::FG).weight(500))
}

// a write wakes the window even when nothing changed, so only write a real change
fn expand(monitor: &str) {
    if !IslandService::read().expanded(monitor) {
        IslandService::write().input(monitor, Input::Click, Instant::now());
    }
}

fn collapse(monitor: &str) {
    if IslandService::read().expanded(monitor) {
        IslandService::write().input(monitor, Input::Collapse, Instant::now());
    }
}

// in arms and starts the hover delay, out disarms and starts the grace; IslandService::listen times both
fn hover(monitor: &str, inside: bool) {
    if IslandService::read().inside(monitor) != inside {
        IslandService::write().hover(monitor, inside, Instant::now());
    }
}

// right click and wheel reach no Presentation yet, so nothing is written for them (#27, #31)
fn route(monitor: &str, input: Input) {
    if input.decides() {
        IslandService::write().input(monitor, input, Instant::now());
    }
}

fn set_armed(monitor: &str, armed: bool) {
    if IslandService::read().armed(monitor) != armed {
        IslandService::write().set_armed(monitor, armed);
    }
}

// geometry already rounded it to whole pixels
fn input_area(area: Rect) -> InputArea {
    InputArea {
        x: area.x as i32,
        y: area.y as i32,
        width: area.width as i32,
        height: area.height as i32,
    }
}
