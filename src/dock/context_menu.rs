use crate::island::geometry::Rect;
use crate::look::Edge;
use crate::theme::{self, ThemeRoles, radius};
use kanade_runtime::service::Service;
use kanade_runtime::{
    Button, Center, Column, Cursor, Full, Horizontal, Key, Keyboard, Layer, LayerWindow, Monitor,
    Monitors, Padding, Parent, Rectangle, Shadow, Start, Text, Vertical, Widget, Zone,
};

use super::{Item, Press, carry_out, desktop};

const WIDTH: f32 = 264.0;
const ROW: f32 = 38.0;
const GAP: f32 = 4.0;
const INSET: f32 = 7.0;
const SCREEN_GAP: f32 = 8.0;
const SEPARATOR: f32 = 1.0;

#[derive(Default)]
struct Menu {
    open: Option<Open>,
}

#[derive(Clone)]
struct Open {
    monitor: String,
    item: Item,
    at: Rect,
    edge: Edge,
    selected: Option<usize>,
    offset: usize,
    scroll: f32,
}

impl Service for Menu {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {}
}

pub fn open(monitor: &str, item: &Item, at: Rect, edge: Edge) {
    if desktop(&item.app).is_none() {
        return;
    }

    Menu::write().open = Some(Open {
        monitor: monitor.to_owned(),
        item: item.clone(),
        at,
        edge,
        selected: None,
        offset: 0,
        scroll: 0.0,
    });
}

pub fn forget_gone(monitors: &Monitors) {
    let mut menu = Menu::write();

    if menu
        .open
        .as_ref()
        .is_some_and(|open| !monitors.has(&open.monitor))
    {
        menu.open = None;
    } else {
        menu.quiet();
    }
}

pub fn window(monitor: &Monitor) -> LayerWindow {
    let open = Menu::read()
        .open
        .as_ref()
        .filter(|open| open.monitor == monitor.name)
        .cloned();

    let Some(open) = open else {
        return LayerWindow::new()
            .width(1.0)
            .height(1.0)
            .anchor_vertical(Vertical::Top)
            .anchor_horizontal(Horizontal::Left)
            .layer(Layer::Overlay)
            .space(Zone::Ignore)
            .namespace("kanade-dock-menu")
            .visible(false)
            .click_through()
            .child(Rectangle::new().width(1.0).height(1.0));
    };

    let count = row_count(&open.item);
    let (width, visible_rows) = size(monitor, count);
    let last_offset = count.saturating_sub(visible_rows);
    let offset = open.offset.min(last_offset);
    let end = (offset + visible_rows).min(count);
    let separator = offset == 0 && end > 1;
    let list_height =
        (end - offset) as f32 * (ROW + GAP) - GAP + if separator { SEPARATOR + GAP } else { 0.0 };
    let height = 2.0 * INSET + list_height;
    let (x, y) = position(monitor, &open, width, height);
    let roles = theme::island();
    let content = rows(&open, offset, end, width - 2.0 * INSET, roles);
    let scroll_monitor = open.monitor.clone();
    let (monitor_width, monitor_height) = (monitor.width, monitor.height);

    let panel = Rectangle::new()
        .width(width)
        .height(height)
        .radius(radius::CARD)
        .fill(roles.surface)
        .border(1.0, roles.outline)
        .shadow(
            Shadow::drop(theme::SHADOW_AMBIENT)
                .blur(14.0)
                .offset(0.0, 5.0),
        )
        .padding(INSET)
        .align_child(Start, Start)
        .child(
            Rectangle::new()
                .width(width - 2.0 * INSET)
                .height(list_height)
                .on_scroll(move |delta| {
                    scroll(&scroll_monitor, monitor_width, monitor_height, delta.y)
                })
                .child(Column::new(content).width(width - 2.0 * INSET).gap(GAP)),
        );

    let root = Rectangle::new()
        .width(Parent)
        .height(Parent)
        .on_click(|_| close())
        .child(panel.translate(x, y));
    let monitor_name = monitor.name.clone();
    let height = monitor.height;

    LayerWindow::new()
        .width(Full)
        .height(Full)
        .anchor_vertical(Vertical::Top)
        .anchor_horizontal(Horizontal::Left)
        .layer(Layer::Overlay)
        .keyboard(Keyboard::Exclusive)
        .space(Zone::Ignore)
        .namespace("kanade-dock-menu")
        .on_key(move |key| key_press(&monitor_name, height, key))
        .child(root)
}

fn row_count(item: &Item) -> usize {
    1 + desktop(&item.app).map_or(0, |entry| entry.actions.len())
}

fn size(monitor: &Monitor, count: usize) -> (f32, usize) {
    let width = WIDTH.min((monitor.width as f32 - 2.0 * SCREEN_GAP).max(1.0));
    let max_height = (monitor.height as f32 - 2.0 * SCREEN_GAP).max(ROW + 2.0 * INSET);
    let content_height = (max_height - 2.0 * INSET).max(ROW);
    let visible_rows = (0..=count)
        .rev()
        .find(|rows| {
            let rows = (*rows).max(1);
            let separators = if count > 1 { SEPARATOR + GAP } else { 0.0 };
            rows as f32 * (ROW + GAP) - GAP + separators <= content_height
        })
        .unwrap_or(1)
        .min(count.max(1));
    (width, visible_rows)
}

fn position(monitor: &Monitor, open: &Open, width: f32, height: f32) -> (f32, f32) {
    let x = match open.edge {
        Edge::Left => open.at.x + open.at.width + GAP,
        Edge::Right => open.at.x - width - GAP,
        Edge::Top | Edge::Bottom => open.at.x + (open.at.width - width) / 2.0,
    };
    let y = match open.edge {
        Edge::Top => open.at.y + open.at.height + GAP,
        Edge::Bottom => open.at.y - height - GAP,
        Edge::Left | Edge::Right => open.at.y + (open.at.height - height) / 2.0,
    };

    (
        clamp(x, width, monitor.width as f32),
        clamp(y, height, monitor.height as f32),
    )
}

fn clamp(position: f32, size: f32, screen: f32) -> f32 {
    let inset = SCREEN_GAP.min(screen / 2.0);
    position.clamp(inset, (screen - inset - size).max(inset))
}

fn rows(
    open: &Open,
    start: usize,
    end: usize,
    width: f32,
    roles: ThemeRoles,
) -> Vec<Box<dyn Widget>> {
    let mut rows = Vec::new();

    if start == 0 {
        rows.push(Box::new(action_row(
            0,
            if open.item.pinned {
                "Unpin from Dock"
            } else {
                "Pin to Dock"
            },
            open.selected == Some(0),
            width,
            roles,
            open.monitor.clone(),
        )) as Box<dyn Widget>);
    }

    if start == 0 && end > 1 {
        rows.push(Box::new(
            Rectangle::new()
                .width(width)
                .height(SEPARATOR)
                .fill(roles.outline),
        ));
    }

    if let Some(entry) = desktop(&open.item.app) {
        for index in start.max(1)..end {
            let action = &entry.actions[index - 1];
            rows.push(Box::new(action_row(
                index,
                &action.name,
                open.selected == Some(index),
                width,
                roles,
                open.monitor.clone(),
            )));
        }
    }

    rows
}

fn action_row(
    index: usize,
    label: &str,
    selected: bool,
    width: f32,
    roles: ThemeRoles,
    monitor: String,
) -> Rectangle {
    let color = if selected {
        roles.on_primary
    } else {
        roles.on_surface
    };
    let label = label.to_owned();
    let hover_monitor = monitor.clone();
    let click_monitor = monitor;

    let mut row = Rectangle::new()
        .width(width)
        .height(ROW)
        .radius(radius::ICON);

    if selected {
        row = row.fill(roles.primary);
    }

    row.cursor(Cursor::Pointer)
        .identity(("dock-context-action", index))
        .on_hover(move |inside| {
            if inside {
                select(&hover_monitor, index);
            }
        })
        .on_click(move |button| {
            if button == Button::Left {
                activate(&click_monitor, index);
            }
        })
        .padding(Padding {
            left: 10.0,
            right: 10.0,
            ..Padding::default()
        })
        .align_child(Start, Center)
        .child(Text::new(label).size(theme::text::BODY).color(color))
}

fn select(monitor: &str, index: usize) {
    let mut menu = Menu::write();

    let Some(open) = menu.open.as_mut().filter(|open| open.monitor == monitor) else {
        menu.quiet();
        return;
    };

    if open.selected == Some(index) {
        menu.quiet();
    } else {
        open.selected = Some(index);
    }
}

fn scroll(monitor: &str, monitor_width: u32, monitor_height: u32, delta: f32) {
    if delta == 0.0 {
        return;
    }

    let mut menu = Menu::write();
    let Some(open) = menu.open.as_mut().filter(|open| open.monitor == monitor) else {
        menu.quiet();
        return;
    };
    let count = row_count(&open.item);
    let monitor = Monitor {
        name: monitor.to_owned(),
        width: monitor_width,
        height: monitor_height,
    };
    let visible = size(&monitor, count).1;
    let (next, remainder) = scrolled(open.offset, open.scroll, delta, count, visible);
    open.scroll = remainder;

    if open.offset == next {
        menu.quiet();
    } else {
        open.offset = next;
        open.selected = None;
    }
}

fn scrolled(offset: usize, pending: f32, delta: f32, count: usize, visible: usize) -> (usize, f32) {
    let pending = pending + delta;
    let steps = pending.trunc() as isize;

    if steps == 0 {
        return (offset, pending);
    }

    let next = (offset as isize + steps).clamp(0, count.saturating_sub(visible) as isize) as usize;

    if next == offset {
        (offset, 0.0)
    } else {
        (next, pending - steps as f32)
    }
}

fn key_press(monitor: &str, screen_height: u32, key: Key) {
    match key {
        Key::Escape => close(),
        Key::Up => move_selection(monitor, screen_height, false),
        Key::Down => move_selection(monitor, screen_height, true),
        Key::Enter | Key::Space => {
            let selected = Menu::read()
                .open
                .as_ref()
                .filter(|open| open.monitor == monitor)
                .and_then(|open| open.selected);

            if let Some(selected) = selected {
                activate(monitor, selected);
            }
        }
        _ => {}
    }
}

fn move_selection(monitor: &str, screen_height: u32, down: bool) {
    let mut menu = Menu::write();
    let Some(open) = menu.open.as_mut().filter(|open| open.monitor == monitor) else {
        menu.quiet();
        return;
    };
    let count = row_count(&open.item);
    if count == 0 {
        menu.quiet();
        return;
    }

    let next = match (open.selected, down) {
        (None, true) => 0,
        (None, false) => count - 1,
        (Some(current), true) => (current + 1) % count,
        (Some(current), false) => current.checked_sub(1).unwrap_or(count - 1),
    };
    let monitor = Monitor {
        name: open.monitor.clone(),
        width: WIDTH.ceil() as u32 + 2 * SCREEN_GAP as u32,
        height: screen_height,
    };
    let visible = size(&monitor, count).1;

    if next < open.offset {
        open.offset = next;
    } else if next >= open.offset + visible {
        open.offset = next + 1 - visible;
    }

    open.scroll = 0.0;
    open.selected = Some(next);
}

fn activate(monitor: &str, index: usize) {
    let selected = {
        let mut menu = Menu::write();
        if !menu
            .open
            .as_ref()
            .is_some_and(|open| open.monitor == monitor)
        {
            menu.quiet();
            return;
        }

        (
            menu.open.take().expect("the menu still matches").item,
            index,
        )
    };
    let (item, index) = selected;

    if index == 0 {
        let Some(entry) = desktop(&item.app) else {
            return;
        };
        let id = entry.id.clone();
        let pinned = !item.pinned;

        std::thread::spawn(move || {
            if let Err(error) = crate::settings::set_pinned(&id, pinned) {
                eprintln!("dock: could not update pinned apps: {error}");
            }
        });
        return;
    }

    let Some(entry) = desktop(&item.app) else {
        return;
    };
    let Some(action) = entry.actions.get(index - 1) else {
        return;
    };

    carry_out(Press::DesktopAction {
        launch: action.launch.clone(),
        id: action.id.clone(),
    });
}

fn close() {
    let mut menu = Menu::write();

    if menu.open.take().is_none() {
        menu.quiet();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fractional_touchpad_scroll_accumulates_until_a_row_turns() {
        let (offset, pending) = scrolled(0, 0.0, 0.2, 10, 3);
        assert_eq!(offset, 0);
        assert!((pending - 0.2).abs() < 0.001);

        let (offset, pending) = scrolled(offset, pending, 0.3, 10, 3);
        assert_eq!(offset, 0);
        assert!((pending - 0.5).abs() < 0.001);

        let (offset, pending) = scrolled(offset, pending, 0.6, 10, 3);
        assert_eq!(offset, 1);
        assert!((pending - 0.1).abs() < 0.001);
    }

    #[test]
    fn scrolling_past_a_menu_edge_does_not_save_whole_rows() {
        assert_eq!(scrolled(0, 0.0, -1.2, 10, 3), (0, 0.0));
        assert_eq!(scrolled(7, 0.0, 1.2, 10, 3), (7, 0.0));
    }
}
