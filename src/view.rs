use std::time::Instant;

use amane::{
    Button, Center, Horizontal, InputArea, Key, Layer, LayerWindow, Monitor, Parent, Rectangle,
    Scroll, Service, Start, Text, Vertical, Zone, request_frame,
};

use crate::island::geometry::{self, Rect};
use crate::island::presentation::{Input, Presentation};
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

    // the spring is ours, not an Amane Animation, so the view asks for frames until it rests
    if !island.settled(&monitor.name, now) {
        request_frame();
    }

    let body = geometry::body(shape);
    let area = geometry::input_area(body);

    let clicked = monitor.name.clone();
    let moved = monitor.name.clone();
    let hovered = monitor.name.clone();
    let pressed = monitor.name.clone();
    let scrolled = monitor.name.clone();

    // the morph reveals content already laid out at its final size, clipped to the body's corners
    let mut body = Rectangle::new()
        .width(body.width)
        .height(body.height)
        .radius(shape.radius)
        .fill(theme::BODY)
        .clip()
        .align_child(Center, Start)
        .translate(body.x - area.x, body.y - area.y);

    // at most one shows at a time, the crossfade hands over through nothing
    if let Some((presentation, opacity)) = content.into_iter().flatten().next()
        && let Some(placeholder) = placeholder(presentation)
    {
        let size = geometry::shape(presentation);

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
        .input_region(vec![input_area(area)])
        .child(
            Rectangle::new()
                .width(Parent)
                .height(Parent)
                .align_child(Start, Start)
                .child(hover),
        )
}

// `amane ipc call island <verb> <monitor>`, runs on the draw thread; only posts
pub fn ipc(arguments: &[String]) -> String {
    match arguments {
        [verb, monitor] if verb == "open" => open(monitor),
        [verb, monitor] if verb == "collapse" => collapse(monitor),
        _ => return String::from("usage: island open|collapse <monitor>"),
    }

    String::new()
}

/*
 * stand-in content until the Surfaces and the Frame exist (#20, #27-#30): names what shows, so the
 * crossfade and the clipping can be seen. Short and fixed, so sized to its letters and centered;
 * Rest shows nothing
 */
fn placeholder(presentation: Presentation) -> Option<Text> {
    let (label, size) = match presentation {
        Presentation::Rest => return None,
        Presentation::Compact => (String::from("Compact"), 13.0),
        Presentation::Peek => (String::from("Peek"), 15.0),
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

// no press, so the island holds the keyboard until it collapses (#4)
fn open(monitor: &str) {
    if !IslandService::read().expanded(monitor) {
        IslandService::write().open(monitor, Instant::now());
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
