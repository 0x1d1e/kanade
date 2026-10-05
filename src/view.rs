use amane::{
    Button, Horizontal, InputArea, Key, Keyboard, Layer, LayerWindow, Monitor, Parent, Rectangle,
    Service, Start, Vertical, Zone,
};

use crate::island::geometry::{self, Rect};
use crate::island::service::IslandService;
use crate::theme;

// one island window per monitor; the window stays put and only the body morphs inside it
pub fn island(monitor: &Monitor) -> LayerWindow {
    // reading subscribes this window to island changes
    let island = IslandService::read();

    let expanded = island.expanded(&monitor.name);
    let shape = island.shape(&monitor.name);

    let body = geometry::body(shape);
    let area = geometry::input_area(body);

    let clicked = monitor.name.clone();
    let hovered = monitor.name.clone();
    let pressed = monitor.name.clone();

    let body = Rectangle::new()
        .width(body.width)
        .height(body.height)
        .radius(shape.radius)
        .fill(theme::BODY)
        .translate(body.x - area.x, body.y - area.y)
        .on_click(move |button| {
            if button == Button::Left {
                set_expanded(&clicked, true);
            }
        });

    // exactly the input region, so leaving it is leaving the island
    let hover = Rectangle::new()
        .width(area.width)
        .height(area.height)
        .translate(area.x, area.y)
        .align_child(Start, Start)
        .on_hover(move |inside| {
            if !inside {
                set_expanded(&hovered, false);
            }
        })
        .child(body);

    /*
     * Escape needs focus; OnDemand only focuses on a press, and the press that expands lands
     * while the mode is still None, so Exclusive takes it until the island collapses
     */
    let keyboard = if expanded {
        Keyboard::Exclusive
    } else {
        Keyboard::None
    };

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
                set_expanded(&pressed, false);
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

// a write wakes the window even when nothing changed, so only write a real change
fn set_expanded(monitor: &str, expanded: bool) {
    if IslandService::read().expanded(monitor) != expanded {
        IslandService::write().set_expanded(monitor, expanded);
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
