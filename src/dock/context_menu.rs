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
    let (width, visible_rows) = size(monitor, count, separated, tall);
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
                .on_scroll(move |delta| scroll(&scroll_monitor, tall, delta.y))
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

fn size(monitor: &Monitor, count: usize, separated: bool) -> (f32, usize) {
    let width = WIDTH.min((monitor.width as f32 - 2.0 * SCREEN_GAP).max(1.0));
    let max_height = (monitor.height as f32 - 2.0 * SCREEN_GAP).max(ROW + 2.0 * INSET);
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
 * where the menu stands: out from the icon on the side away from the Dock's edge, then clear of the
 * Dock's strip on that edge, then within the output. The icon's rect is the one taken as the menu
 * opened, and an autohide that was still sliding out left it below the screen: the strip, read as it
 * stands, is what the menu goes above, not the screen's own edge
 */
fn position(monitor: &Monitor, open: &Open, width: f32, height: f32) -> (f32, f32) {
    place(
        open.at,
        open.edge,
        (monitor.width as f32, monitor.height as f32),
        (width, height),
        super::standing(monitor),
    )
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

fn scroll(monitor: &str, monitor_width: u32, monitor_height: u32, delta: f32) {
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
    let monitor = Monitor {
        name: monitor.to_owned(),
        width: monitor_width,
        height: monitor_height,
    };
    let visible = size(&monitor, count, separated).1;
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

fn move_selection(monitor: &str, screen_height: u32, down: bool) {
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
    let monitor = Monitor {
        name: open.monitor.clone(),
        width: WIDTH.ceil() as u32 + 2 * SCREEN_GAP as u32,
        height: screen_height,
    };
    let separated = !open.windows_only && count > 1;
    let visible = size(&monitor, count, separated).1;

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
