use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use crate::island::geometry::Rect;
use crate::look::Edge;
use crate::sources::niri::WindowId;
use crate::sources::windows::{self, Titled};
use crate::theme::{self, ThemeRoles, radius};
use kanade_runtime::service::Service;
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Button, Center, Column, Cursor, Full, Horizontal, Key, Keyboard, Layer, LayerWindow, Margin,
    Monitor, Monitors, Padding, Parent, Rectangle, Row, Shadow, Start, Text, Vertical, Widget,
    Zone, children,
};

use super::{Item, Press, carry_out, desktop};

const WIDTH: f32 = 264.0;
const ROW: f32 = 38.0;
const GAP: f32 = 4.0;
const INSET: f32 = 7.0;
const SCREEN_GAP: f32 = 8.0;
const SEPARATOR: f32 = 1.0;

// the close target at the end of a window's row
const CLOSE: f32 = 26.0;

// the newest open that asked for its titles: closing, or another open, supersedes the ones before it
static ASKED: AtomicU64 = AtomicU64::new(0);

// the hover list stays open while the pointer crosses from the icon onto it
static OVER_ICON: AtomicBool = AtomicBool::new(false);
static OVER_PANEL: AtomicBool = AtomicBool::new(false);
static CLOSE_TICK: AtomicU64 = AtomicU64::new(0);

const GRACE: Duration = Duration::from_millis(180);

#[derive(Default)]
struct Menu {
    open: Option<Open>,
}

#[derive(Clone)]
struct Open {
    monitor: String,
    item: Item,

    // the item's windows with the titles niri gave as the menu opened, kept for as long as it is
    windows: Vec<Titled>,
    at: Rect,
    edge: Edge,
    selected: Option<usize>,
    offset: usize,
    scroll: f32,

    // the hover list: window rows only, no pin and no actions
    windows_only: bool,

    // the pointer is on that list, so it may take the keyboard for Delete
    panel: bool,
}

impl Service for Menu {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {}
}

pub fn open(monitor: &str, item: &Item, at: Rect, edge: Edge) {
    if desktop(&item.app).is_none() && !item.pinned() {
        return;
    }

    CLOSE_TICK.fetch_add(1, Ordering::Relaxed);
    let (monitor, item) = (monitor.to_owned(), item.clone());
    let ticket = ASKED.fetch_add(1, Ordering::SeqCst) + 1;

    // off the view thread: niri is asked for the titles first, so the list is there as the menu is
    thread::spawn(move || {
        let ids: Vec<WindowId> = item.windows.iter().map(|window| window.id).collect();
        let windows = if ids.is_empty() {
            Vec::new()
        } else {
            windows::titled(&ids).unwrap_or_else(|error| {
                eprintln!("dock: no window titles: {error}");

                ids.iter()
                    .map(|id| Titled {
                        id: *id,
                        title: String::new(),
                    })
                    .collect()
            })
        };

        publish(
            ticket,
            Open {
                monitor,
                item,
                windows,
                at,
                edge,
                selected: None,
                offset: 0,
                scroll: 0.0,
                windows_only: false,
                panel: false,
            },
        );
    });
}

// the app's windows, asked of niri as the pointer comes onto a running icon; none with no windows,
// and the right-click menu wins if it is open
pub fn preview(monitor: &str, item: &Item, at: Rect, edge: Edge) {
    if item.windows.is_empty() {
        return;
    }

    if let Some(open) = Menu::read().open.clone() {
        // one menu for every output: a right-click is not replaced by a hover on another monitor
        if !open.windows_only {
            return;
        }
        if open.monitor == monitor && open.item == *item {
            OVER_ICON.store(true, Ordering::Relaxed);
            CLOSE_TICK.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }

    OVER_ICON.store(true, Ordering::Relaxed);
    CLOSE_TICK.fetch_add(1, Ordering::Relaxed);
    let (monitor, item) = (monitor.to_owned(), item.clone());
    let ticket = ASKED.fetch_add(1, Ordering::SeqCst) + 1;

    thread::spawn(move || {
        let ids: Vec<WindowId> = item.windows.iter().map(|window| window.id).collect();
        let windows = windows::titled(&ids).unwrap_or_else(|error| {
            eprintln!("dock: no window titles: {error}");
            ids.iter()
                .map(|id| Titled {
                    id: *id,
                    title: String::new(),
                })
                .collect()
        });

        if windows.is_empty() {
            return;
        }

        publish(
            ticket,
            Open {
                monitor,
                item,
                windows,
                at,
                edge,
                selected: None,
                offset: 0,
                scroll: 0.0,
                windows_only: true,
                panel: false,
            },
        );
    });
}

pub fn leave_icon() {
    OVER_ICON.store(false, Ordering::Relaxed);
    schedule_close();
}

fn hover_panel(inside: bool) {
    OVER_PANEL.store(inside, Ordering::Relaxed);

    let mut menu = Menu::write();
    match menu.open.as_mut() {
        Some(open) if open.windows_only && open.panel != inside => open.panel = inside,
        Some(_) => menu.quiet(),
        None => menu.quiet(),
    }

    if !inside {
        schedule_close();
    }
}

fn schedule_close() {
    let ticket = CLOSE_TICK.fetch_add(1, Ordering::Relaxed) + 1;

    thread::spawn(move || {
        thread::sleep(GRACE);

        if CLOSE_TICK.load(Ordering::Relaxed) != ticket {
            return;
        }
        if OVER_ICON.load(Ordering::Relaxed) || OVER_PANEL.load(Ordering::Relaxed) {
            return;
        }

        let mut menu = Menu::write();
        if menu.open.as_ref().is_some_and(|open| open.windows_only) {
            menu.open = None;
        } else {
            menu.quiet();
        }
    });
}

// shows the menu its titles were asked for, unless it was closed or another was asked since.
// A hover does not replace a right-click, whichever output that menu is on
fn publish(ticket: u64, open: Open) {
    let mut menu = Menu::write();

    if open.windows_only && menu.open.as_ref().is_some_and(|shown| !shown.windows_only) {
        menu.quiet();
        return;
    }

    // under the write, so a close that follows clears what this shows
    if ASKED.load(Ordering::SeqCst) == ticket {
        menu.open = Some(open);
    } else {
        menu.quiet();
    }
}

// what asked for titles and has not shown yet is not wanted any more
fn supersede() {
    ASKED.fetch_add(1, Ordering::SeqCst);
}

pub fn forget_gone(monitors: &Monitors) {
    // a menu asked for on an output that has left must not show, and this runs on a change of outputs only
    supersede();

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

    let count = row_count(&open);
    let separated = !open.windows_only && count > 1;
    let screen = (monitor.width as f32, monitor.height as f32);
    let dock = super::standing(monitor);
    let tall = tallest(screen, dock);
    let (width, visible_rows) = size(monitor.width as f32, tall, count, separated);
    let last_offset = count.saturating_sub(visible_rows);
    let offset = open.offset.min(last_offset);
    let end = (offset + visible_rows).min(count);
    let separator = !open.windows_only && offset == 0 && end > 1;
    let list_height =
        (end - offset) as f32 * (ROW + GAP) - GAP + if separator { SEPARATOR + GAP } else { 0.0 };
    let height = 2.0 * INSET + list_height;
    let (x, y) = place(open.at, open.edge, screen, (width, height), dock);
    let roles = theme::island();
    let content = rows(&open, offset, end, width - 2.0 * INSET, roles);
    let scroll_monitor = open.monitor.clone();
    let monitor_width = monitor.width;

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
        .semantics(|| Semantics::new(Role::Menu, "menu"))
        .child(
            Rectangle::new()
                .width(width - 2.0 * INSET)
                .height(list_height)
                .on_scroll(move |delta| scroll(&scroll_monitor, monitor_width, tall, delta.y))
                .child(Column::new(content).width(width - 2.0 * INSET).gap(GAP)),
        );

    if open.windows_only {
        let monitor_name = monitor.name.clone();
        let panel = panel.on_hover(hover_panel);

        return LayerWindow::new()
            .width(width)
            .height(height)
            .anchor_vertical(Vertical::Top)
            .anchor_horizontal(Horizontal::Left)
            .margin(Margin {
                top: y as i32,
                left: x as i32,
                ..Margin::default()
            })
            .layer(Layer::Overlay)
            .keyboard(if open.panel {
                Keyboard::Exclusive
            } else {
                Keyboard::None
            })
            .space(Zone::Ignore)
            .namespace("kanade-dock-menu")
            .on_key(move |key| key_press(&monitor_name, tall, key))
            .child(panel);
    }

    let root = Rectangle::new()
        .width(Parent)
        .height(Parent)
        .on_click(|_| close())
        .child(panel.translate(x, y));
    let monitor_name = monitor.name.clone();

    LayerWindow::new()
        .width(Full)
        .height(Full)
        .anchor_vertical(Vertical::Top)
        .anchor_horizontal(Horizontal::Left)
        .layer(Layer::Overlay)
        .keyboard(Keyboard::Exclusive)
        .space(Zone::Ignore)
        .namespace("kanade-dock-menu")
        .on_key(move |key| key_press(&monitor_name, tall, key))
        .child(root)
}

// what a row of the menu is, in the order they come: the pin, silence, the windows, the app's own actions
enum Entry<'a> {
    Pin,
    Silence,
    Window(&'a Titled),

    // by place in the entry's actions
    Action(usize),
}

fn entries(open: &Open) -> Vec<Entry<'_>> {
    if open.windows_only {
        return open.windows.iter().map(Entry::Window).collect();
    }

    let mut entries = vec![Entry::Pin];
    if silence_target(&open.item).is_some() {
        entries.push(Entry::Silence);
    }
    entries.extend(open.windows.iter().map(Entry::Window));
    if let Some(desktop) = desktop(&open.item.app) {
        entries.extend((0..desktop.actions.len()).map(Entry::Action));
    }
    entries
}

fn row(open: &Open, index: usize) -> Option<Entry<'_>> {
    entries(open).into_iter().nth(index)
}

fn row_count(open: &Open) -> usize {
    entries(open).len()
}

fn silence_target(item: &Item) -> Option<crate::sources::notifications::silence::App> {
    use crate::sources::windows::App;

    let (desktop, name) = match &item.app {
        App::Desktop(entry) => (entry.id.as_str(), entry.name.as_str()),
        App::Unmatched(Some(id)) => (id.as_str(), id.as_str()),
        App::Unmatched(None) => (item.pin.as_deref()?, ""),
    };

    crate::sources::notifications::silence::App::named(name, desktop)
}

fn size(width: f32, tall: f32, count: usize, separated: bool) -> (f32, usize) {
    let width = WIDTH.min((width - 2.0 * SCREEN_GAP).max(1.0));
    let max_height = (tall - 2.0 * SCREEN_GAP).max(ROW + 2.0 * INSET);
    let content_height = (max_height - 2.0 * INSET).max(ROW);
    let visible_rows = (0..=count)
        .rev()
        .find(|rows| {
            let rows = (*rows).max(1);
            let separators = if separated { SEPARATOR + GAP } else { 0.0 };
            rows as f32 * (ROW + GAP) - GAP + separators <= content_height
        })
        .unwrap_or(1)
        .min(count.max(1));
    (width, visible_rows)
}

/*
 * the room a menu may use on the far side of the Dock's strip, as it stands, grown icons included
 */
fn tallest(screen: (f32, f32), dock: Option<(Edge, Rect)>) -> f32 {
    let Some((edge, band)) = dock else {
        return screen.1;
    };

    match edge {
        Edge::Top => screen.1 - (band.bottom() + GAP),
        Edge::Bottom => band.y - GAP,

        // a side dock takes room along the output, not up it
        Edge::Left | Edge::Right => screen.1,
    }
}

fn place(
    at: Rect,
    edge: Edge,
    screen: (f32, f32),
    panel: (f32, f32),
    dock: Option<(Edge, Rect)>,
) -> (f32, f32) {
    let (width, height) = panel;
    let x = match edge {
        Edge::Left => at.x + at.width + GAP,
        Edge::Right => at.x - width - GAP,
        Edge::Top | Edge::Bottom => at.x + (at.width - width) / 2.0,
    };
    let y = match edge {
        Edge::Top => at.y + at.height + GAP,
        Edge::Bottom => at.y - height - GAP,
        Edge::Left | Edge::Right => at.y + (at.height - height) / 2.0,
    };

    let (mut left, mut right) = within(width, screen.0);
    let (mut low, mut high) = within(height, screen.1);

    // the Dock's strip, as far out as its icons grow, keeps the menu on the far side of it
    if let Some((band_edge, band)) = dock {
        match band_edge {
            Edge::Top => low = low.max(band.bottom() + GAP),
            Edge::Bottom => high = high.min(band.y - GAP - height),
            Edge::Left => left = left.max(band.right() + GAP),
            Edge::Right => right = right.min(band.x - GAP - width),
        }
    }

    // an output with no room for both keeps the menu on it, over the Dock rather than off the screen
    let (left, right) = if left > right {
        within(width, screen.0)
    } else {
        (left, right)
    };
    let (low, high) = if low > high {
        within(height, screen.1)
    } else {
        (low, high)
    };

    (x.clamp(left, right), y.clamp(low, high))
}

// the least and the most a menu `size` long stands at on an output `screen` long, a gap from its edges
fn within(size: f32, screen: f32) -> (f32, f32) {
    let inset = SCREEN_GAP.min(screen / 2.0);

    (inset, (screen - inset - size).max(inset))
}

fn rows(
    open: &Open,
    start: usize,
    end: usize,
    width: f32,
    roles: ThemeRoles,
) -> Vec<Box<dyn Widget>> {
    let mut rows = Vec::new();

    for index in start..end {
        let Some(entry) = row(open, index) else {
            break;
        };
        let (label, closable) = match &entry {
            Entry::Pin if open.item.pinned() => ("Unpin from Dock".to_owned(), false),
            Entry::Pin => ("Pin to Dock".to_owned(), false),
            Entry::Window(window) if window.title.is_empty() => {
                let name = desktop(&open.item.app).map_or("Window", |entry| entry.name.as_str());

                (name.to_owned(), true)
            }
            Entry::Window(window) => (window.title.clone(), true),
            Entry::Silence => {
                let silenced = silence_target(&open.item)
                    .is_some_and(|app| crate::sources::notifications::silence::covers_app(&app));
                (
                    if silenced {
                        "Unsilence".to_owned()
                    } else {
                        "Silence".to_owned()
                    },
                    false,
                )
            }
            Entry::Action(action) => (
                desktop(&open.item.app)
                    .map_or_else(String::new, |entry| entry.actions[*action].name.clone()),
                false,
            ),
        };

        let row = action_row(
            index,
            &label,
            closable,
            open.selected == Some(index),
            width,
            roles,
            open.monitor.clone(),
        )
        .semantics(|| {
            let item = match entry {
                Entry::Pin => Semantics::new(Role::MenuItem, "pin").label(label.as_str()),
                // a window's title is the user's, an action's name the app's
                Entry::Window(_) => {
                    Semantics::new(Role::MenuItem, "window").content(label.as_str())
                }
                Entry::Silence => Semantics::new(Role::MenuItem, "silence").label(label.as_str()),
                Entry::Action(_) => Semantics::new(Role::MenuItem, "action").label(label.as_str()),
            };

            item.selected(open.selected == Some(index))
        });

        rows.push(Box::new(row) as Box<dyn Widget>);

        if index == 0 && !open.windows_only && end > 1 {
            rows.push(Box::new(
                Rectangle::new()
                    .width(width)
                    .height(SEPARATOR)
                    .fill(roles.outline),
            ));
        }
    }

    rows
}

fn action_row(
    index: usize,
    label: &str,
    closable: bool,
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
    let click_monitor = monitor.clone();

    // the pointer's way to close a window, over the row's own click, which focuses it
    let mut parts = children![
        Text::new(label)
            .size(theme::text::BODY)
            .color(color)
            .elide()
    ];

    if closable {
        parts.push(Box::new(
            Rectangle::new()
                .width(CLOSE)
                .height(CLOSE)
                .radius(radius::ICON)
                .cursor(Cursor::Pointer)
                .identity(("dock-context-close", index))
                .semantics(|| Semantics::new(Role::Button, "close"))
                .on_click(move |button| {
                    if button == Button::Left {
                        close_window(&monitor, index);
                    }
                })
                .align_child(Center, Center)
                .child(Text::new("\u{d7}").size(theme::text::BODY).color(color)),
        ));
    }

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
        .child(Row::new(parts).width(Parent).align(Center))
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

fn scroll(monitor: &str, monitor_width: u32, tall: f32, delta: f32) {
    if delta == 0.0 {
        return;
    }

    let mut menu = Menu::write();
    let Some(open) = menu.open.as_mut().filter(|open| open.monitor == monitor) else {
        menu.quiet();
        return;
    };
    let count = row_count(open);
    let separated = !open.windows_only && count > 1;
    let visible = size(monitor_width as f32, tall, count, separated).1;
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

fn key_press(monitor: &str, tall: f32, key: Key) {
    match key {
        Key::Escape => close(),
        Key::Up => move_selection(monitor, tall, false),
        Key::Down => move_selection(monitor, tall, true),
        Key::Delete | Key::Backspace => {
            if let Some(selected) = selected(monitor) {
                close_window(monitor, selected);
            }
        }
        Key::Enter | Key::Space => {
            if let Some(selected) = selected(monitor) {
                activate(monitor, selected);
            }
        }
        _ => {}
    }
}

fn selected(monitor: &str) -> Option<usize> {
    Menu::read()
        .open
        .as_ref()
        .filter(|open| open.monitor == monitor)
        .and_then(|open| open.selected)
}

fn move_selection(monitor: &str, tall: f32, down: bool) {
    let mut menu = Menu::write();
    let Some(open) = menu.open.as_mut().filter(|open| open.monitor == monitor) else {
        menu.quiet();
        return;
    };
    let count = row_count(open);
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
    let separated = !open.windows_only && count > 1;
    let visible = size(WIDTH.ceil() + 2.0 * SCREEN_GAP, tall, count, separated).1;

    if next < open.offset {
        open.offset = next;
    } else if next >= open.offset + visible {
        open.offset = next + 1 - visible;
    }

    open.scroll = 0.0;
    open.selected = Some(next);
}

fn activate(monitor: &str, index: usize) {
    let Some(open) = take(monitor) else {
        return;
    };
    let item = open.item.clone();

    if let Some(Entry::Window(window)) = row(&open, index) {
        carry_out(Press::Focus(window.id));
        return;
    }

    if let Some(Entry::Silence) = row(&open, index) {
        if let Some(app) = silence_target(&item) {
            crate::sources::notifications::silence::toggle(app);
        }
        return;
    }

    if index == 0 {
        let Some(id) = item
            .pin
            .clone()
            .or_else(|| desktop(&item.app).map(|entry| entry.id.clone()))
        else {
            return;
        };
        let pinned = !item.pinned();

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
    let Some(Entry::Action(action)) = row(&open, index) else {
        return;
    };
    let action = &entry.actions[action];

    carry_out(Press::DesktopAction {
        launch: action.launch.clone(),
        id: action.id.clone(),
    });
}

// closes the window a row stands for, and the menu with it; the other rows close nothing
fn close_window(monitor: &str, index: usize) {
    let id = Menu::read()
        .open
        .as_ref()
        .filter(|open| open.monitor == monitor)
        .and_then(|open| match row(open, index) {
            Some(Entry::Window(window)) => Some(window.id),
            _ => None,
        });

    if let Some(id) = id {
        take(monitor);
        carry_out(Press::Close(id));
    }
}

// the open menu, closed, if it is this monitor's
fn take(monitor: &str) -> Option<Open> {
    supersede();

    let mut menu = Menu::write();

    match menu.open.take_if(|open| open.monitor == monitor) {
        Some(open) => Some(open),
        None => {
            menu.quiet();
            None
        }
    }
}

fn close() {
    supersede();

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

    const SCREEN: (f32, f32) = (1920.0, 1080.0);

    /*
     * how far off its edge the user's Dock reaches: `dock.size = "large"` with `magnification =
     * "large"`, so its icons grow 48 past its 75 deep strip at rest, and the spring swings it BOUNCE
     * further. The same as its window's height, which the strip is drawn in full: a menu that clears
     * the strip clears the window
     */
    fn strip_depth() -> f32 {
        let size = super::super::Size::of(crate::look::DockSize::Large);
        let grown = size.icon * (crate::look::Magnify::Large.most() - 1.0);

        super::super::depth(size) + grown + super::super::BOUNCE
    }

    fn band(edge: Edge, along: (f32, f32), off: (f32, f32)) -> (Edge, Rect) {
        let rect = match edge {
            Edge::Top | Edge::Bottom => Rect {
                x: along.0,
                y: off.0,
                width: along.1 - along.0,
                height: off.1 - off.0,
            },
            Edge::Left | Edge::Right => Rect {
                x: off.0,
                y: along.0,
                width: off.1 - off.0,
                height: along.1 - along.0,
            },
        };

        (edge, rect)
    }

    fn bottom_dock() -> (Edge, Rect) {
        band(
            Edge::Bottom,
            (760.0, 1160.0),
            (SCREEN.1 - strip_depth(), SCREEN.1),
        )
    }

    /*
     * mid-slide: the strip the menu clears is read at rest, and the slide's spring swings the strip
     * past its place. `strip_depth` is the rest depth, the icons grown, and BOUNCE, so it covers the
     * farthest the spring puts them; this is that arithmetic on the Dock's own slide, not a copy
     */
    #[test]
    fn the_strip_read_at_rest_covers_its_slide_and_the_icons_grown() {
        let size = super::super::Size::of(crate::look::DockSize::Large);
        let most = crate::look::Magnify::Large.most();
        let rest = super::super::strip_y(true, size, 1000.0, 1.0);

        // out from hidden, through held, and past it as far as the spring swings, the strip stands no
        // higher than this
        let highest = (0..=110)
            .map(|percent| super::super::strip_y(true, size, 1000.0, percent as f32 / 100.0))
            .fold(f32::MAX, f32::min);

        assert!(highest < rest, "the slide swings past its place");

        // its icons grow off the strip's far side, and the top of the tallest stands `rise` past it
        let rise = size.icon * (most - 1.0);
        let icon_top = highest - rise;

        // what a menu keeps clear of: the rest strip, the icons grown, and the spring once
        assert!(rest - strip_depth() <= icon_top);
    }

    #[test]
    fn the_strip_reaches_past_its_icons_grown_and_its_spring_once() {
        assert_eq!(strip_depth(), 131.0);
        assert_eq!(
            strip_depth(),
            super::super::Canvas::of(
                &[super::super::Piece::Icon],
                super::super::Size::of(crate::look::DockSize::Large),
                crate::look::Magnify::Large.most(),
            )
            .height
        );
    }

    // the regression: an autohide still sliding out left the icon's slot below the screen, and the menu
    // landed on the bottom edge over the strip, where the magnified icons grew into it
    #[test]
    fn a_menu_from_a_slot_off_the_screen_stays_clear_of_the_strip() {
        let at = Rect {
            x: 940.0,
            y: 1090.0,
            width: 56.0,
            height: 56.0,
        };
        let dock = bottom_dock();
        let (_, y) = place(at, Edge::Bottom, SCREEN, (264.0, 300.0), Some(dock));

        assert!(y + 300.0 <= dock.1.y - GAP, "menu {y} reaches the strip");
        assert!(y >= SCREEN_GAP);
    }

    // without the strip the same slot put the menu on the bottom edge, which is the overlap
    #[test]
    fn a_menu_from_a_slot_off_the_screen_without_a_dock_lands_on_the_edge() {
        let at = Rect {
            x: 940.0,
            y: 1090.0,
            width: 56.0,
            height: 56.0,
        };
        let (_, y) = place(at, Edge::Bottom, SCREEN, (264.0, 300.0), None);

        assert_eq!(y, SCREEN.1 - SCREEN_GAP - 300.0);
    }

    // a menu opened from a slot on the screen, off a strip that has slid out, keeps clear of it too
    #[test]
    fn a_menu_from_a_visible_slot_still_clears_the_strip() {
        let at = Rect {
            x: 940.0,
            y: 960.0,
            width: 56.0,
            height: 56.0,
        };
        let dock = bottom_dock();
        let (_, y) = place(at, Edge::Bottom, SCREEN, (264.0, 300.0), Some(dock));

        assert_eq!(y, dock.1.y - GAP - 300.0);
    }

    #[test]
    fn a_top_dock_keeps_the_menu_below_its_strip() {
        let at = Rect {
            x: 940.0,
            y: -20.0,
            width: 56.0,
            height: 56.0,
        };
        let dock = band(Edge::Top, (760.0, 1160.0), (0.0, strip_depth()));
        let (_, y) = place(at, Edge::Top, SCREEN, (264.0, 300.0), Some(dock));

        assert_eq!(y, dock.1.bottom() + GAP);
    }

    // an output with no room for both keeps the menu on it, over the Dock rather than off the screen
    #[test]
    fn a_strip_leaving_no_room_still_keeps_the_menu_on_the_output() {
        let at = Rect {
            x: 940.0,
            y: 1090.0,
            width: 56.0,
            height: 56.0,
        };
        let dock = band(Edge::Bottom, (760.0, 1160.0), (200.0, 1080.0));
        let (_, y) = place(at, Edge::Bottom, SCREEN, (264.0, 900.0), Some(dock));

        assert_eq!(y, SCREEN.1 - SCREEN_GAP - 900.0);
        assert!(y >= SCREEN_GAP);
    }

    #[test]
    fn a_side_dock_keeps_the_menu_off_it() {
        let at = Rect {
            x: -30.0,
            y: 400.0,
            width: 56.0,
            height: 56.0,
        };
        let dock = band(Edge::Left, (300.0, 700.0), (0.0, strip_depth()));
        let (x, _) = place(at, Edge::Left, SCREEN, (264.0, 300.0), Some(dock));

        assert_eq!(x, dock.1.right() + GAP);
    }

    // the whole point of the fix: rows are sized to the room past the strip, so an over-tall menu no
    // longer falls back onto the Dock
    #[test]
    fn tallest_leaves_room_past_the_strip_and_only_a_top_or_bottom_one_takes_any() {
        assert_eq!(tallest(SCREEN, None), SCREEN.1);

        let dock = bottom_dock();
        assert_eq!(tallest(SCREEN, Some(dock)), dock.1.y - GAP);

        let dock = band(Edge::Top, (760.0, 1160.0), (0.0, strip_depth()));
        assert_eq!(
            tallest(SCREEN, Some(dock)),
            SCREEN.1 - dock.1.bottom() - GAP
        );

        let dock = band(Edge::Left, (300.0, 700.0), (0.0, strip_depth()));
        assert_eq!(tallest(SCREEN, Some(dock)), SCREEN.1);

        let dock = band(Edge::Right, (300.0, 700.0), (1814.0, 1920.0));
        assert_eq!(tallest(SCREEN, Some(dock)), SCREEN.1);
    }

    // the user's 1920x1080 with a bottom dock: the room past the strip, not the whole output, sizes
    // the rows. A menu too tall for that room is cut to it rather than standing on the Dock
    #[test]
    fn rows_fit_the_room_past_the_strip() {
        let dock = bottom_dock();
        let tall = tallest(SCREEN, Some(dock));
        let (_, rows) = size(SCREEN.0, tall, 30, true);
        let list = rows as f32 * (ROW + GAP) - GAP + SEPARATOR + GAP;

        assert!(list + 2.0 * INSET <= tall - 2.0 * SCREEN_GAP);
        assert!(rows < size(SCREEN.0, SCREEN.1, 30, true).1);
    }

    // the whole of it: the rows the room past the strip allows make a panel that place() then keeps
    // clear of the strip and on the output. Sized to the whole output instead, that same panel stood
    // on the strip, which is the overlap
    #[test]
    fn a_menu_too_tall_for_the_output_still_clears_the_strip() {
        let at = Rect {
            x: 940.0,
            y: 1090.0,
            width: 56.0,
            height: 56.0,
        };
        let dock = bottom_dock();
        let panel = |rows: usize| 2.0 * INSET + rows as f32 * (ROW + GAP) - GAP + SEPARATOR + GAP;

        let tall = tallest(SCREEN, Some(dock));
        let (width, rows) = size(SCREEN.0, tall, 30, true);
        let height = panel(rows);
        let (_, y) = place(at, Edge::Bottom, SCREEN, (width, height), Some(dock));

        assert!(y + height <= dock.1.y - GAP, "menu {y} reaches the strip");
        assert!(y >= SCREEN_GAP);
        assert!(y + height <= SCREEN.1 - SCREEN_GAP);

        let (width, rows) = size(SCREEN.0, SCREEN.1, 30, true);
        let height = panel(rows);
        let (_, y) = place(at, Edge::Bottom, SCREEN, (width, height), Some(dock));

        assert!(y + height > dock.1.y - GAP, "the overlap this fixes");
    }

    fn menu_of(monitor: &str) -> Open {
        Open {
            monitor: monitor.to_owned(),
            item: Item {
                app: crate::sources::windows::App::Unmatched(None),
                windows: Vec::new(),
                pin: Some("test.desktop".to_owned()),
            },
            windows: Vec::new(),
            at: Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            edge: Edge::default(),
            selected: None,
            offset: 0,
            scroll: 0.0,
            windows_only: false,
            panel: false,
        }
    }

    // the menu is one global Service, so tests that publish it take this in turn
    fn menu_tests() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    // a menu dismissed while niri was asked for its titles came back when they arrived, and an older
    // one replaced a newer
    #[test]
    fn a_menu_closed_or_overtaken_before_its_titles_arrive_does_not_show() {
        let _lock = menu_tests();
        let closed = ASKED.fetch_add(1, Ordering::SeqCst) + 1;
        close();
        publish(closed, menu_of("closed"));
        assert!(Menu::read().open.is_none());

        let older = ASKED.fetch_add(1, Ordering::SeqCst) + 1;
        let newer = ASKED.fetch_add(1, Ordering::SeqCst) + 1;
        publish(newer, menu_of("newer"));
        publish(older, menu_of("older"));
        assert_eq!(
            Menu::read().open.as_ref().map(|open| open.monitor.as_str()),
            Some("newer")
        );

        close();
    }

    #[test]
    fn a_right_click_menu_is_not_replaced_by_a_hover_on_another_monitor() {
        let _lock = menu_tests();
        close();

        let ticket = ASKED.fetch_add(1, Ordering::SeqCst) + 1;
        publish(ticket, menu_of("left"));
        let mut hover = menu_of("right");
        hover.windows_only = true;
        let hover_ticket = ASKED.fetch_add(1, Ordering::SeqCst) + 1;
        publish(hover_ticket, hover);

        let open = Menu::read().open.clone().expect("right-click stays");
        assert_eq!(open.monitor, "left");
        assert!(!open.windows_only);

        close();

        let mut first = menu_of("left");
        first.windows_only = true;
        let ticket = ASKED.fetch_add(1, Ordering::SeqCst) + 1;
        publish(ticket, first);
        let mut second = menu_of("right");
        second.windows_only = true;
        let ticket = ASKED.fetch_add(1, Ordering::SeqCst) + 1;
        publish(ticket, second);
        assert_eq!(
            Menu::read().open.as_ref().map(|open| open.monitor.as_str()),
            Some("right")
        );

        close();
    }
}
