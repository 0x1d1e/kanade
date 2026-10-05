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
    let armed = island.armed(&monitor.name);
    let shape = island.shape(&monitor.name);

    let body = geometry::body(shape);
    let area = geometry::input_area(body);

    let clicked = monitor.name.clone();
    let entered = monitor.name.clone();
    let moved = monitor.name.clone();
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
        })
        .on_hover(move |inside| set_armed(&entered, inside))
        // Escape disarms without the pointer leaving, the next move arms again
        .on_move(move |_| set_armed(&moved, true));

    /*
     * exactly the input region, so leaving it is leaving the island; niri also sends the leave
     * when the region shrinks away from a still pointer (#3), which is what reports it here
     */
    let hover = Rectangle::new()
        .width(area.width)
        .height(area.height)
        .translate(area.x, area.y)
        .align_child(Start, Start)
        .on_hover(move |inside| {
            if !inside {
                set_expanded(&hovered, false);
                set_armed(&hovered, false);
            }
        })
        .child(body);

    /*
     * Escape needs focus, and OnDemand only takes it on a press, so it must be on before the
     * press that expands; Exclusive would keep the keyboard from overlays opened later (niri
     * gives it to the first mapped exclusive surface)
     */
    let keyboard = if expanded || armed {
        Keyboard::OnDemand
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

// a write wakes the window even when nothing changed, so only write a real change
fn set_expanded(monitor: &str, expanded: bool) {
    if IslandService::read().expanded(monitor) != expanded {
        IslandService::write().set_expanded(monitor, expanded);
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
