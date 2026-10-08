//! The Dock (#144, docs/design.md Later): the pinned apps, then the running apps that are not
//! pinned, bottom centre on every monitor, each with a dot while it runs. A click focuses a running
//! app's window, the next one if it has the focus already, or launches an app that is not running.
//! It reads only `windows` (ADR 0014), which finds the pinned apps' entries and every icon, so
//! drawing reads no file; it draws only when the running or pinned apps change.

use std::thread;

use amane::{
    Button, Center, Column, Cursor, Horizontal, Layer, LayerWindow, Margin, Monitor, Padding,
    Rectangle, Row, Service, Start, Vertical, Widget, Zone,
};

use crate::config;
use crate::island::geometry::Rect;
use crate::sources::launch::Launch;
use crate::sources::niri::{self, Acted};
use crate::sources::windows::{self, App, DesktopEntry, Window, WindowId, Windows};
use crate::theme::{self, SEMANTIC, ThemeRoles, radius};
use crate::view;

const ICON: f32 = 36.0;
const GAP: f32 = 8.0;

// around the icons, and the room under them for the dot
const INSET: f32 = 8.0;
const DOT: f32 = 4.0;
const FOCUSED_DOT: f32 = 12.0;
const BELOW: f32 = 4.0;

const BODY: f32 = INSET + ICON + BELOW + DOT + BELOW;

// from the monitor's bottom edge; with BODY under the OSD's 72, so the OSD never covers the Dock
const BOTTOM: f32 = 4.0;

// between the pinned and the other running apps
const DIVIDER: f32 = 1.0;

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

// one window per monitor, as wide as its items, hidden while there are none
pub fn window(_monitor: &Monitor) -> LayerWindow {
    // reading subscribes this window to the running apps
    let items = items(&Windows::read());

    let roles = theme::roles();
    let divided = items.iter().any(|item| item.pinned) && items.iter().any(|item| !item.pinned);

    let mut row: Vec<Box<dyn Widget>> = Vec::new();

    for (index, item) in items.iter().enumerate() {
        if divided && index > 0 && item.pinned != items[index - 1].pinned {
            row.push(Box::new(
                Rectangle::new()
                    .width(DIVIDER)
                    .height(ICON)
                    .fill(roles.outline),
            ));
        }

        row.push(Box::new(slot(item, &roles)));
    }

    let count = row.len() as f32;
    let width = INSET * 2.0
        + ICON * items.len() as f32
        + if divided { DIVIDER } else { 0.0 }
        + GAP * (count - 1.0).max(0.0);

    let body = Rect {
        x: 0.0,
        y: 0.0,
        width,
        height: BODY,
    };

    LayerWindow::new()
        .width(width)
        .height(BODY)
        .anchor_vertical(Vertical::Bottom)
        .anchor_horizontal(Horizontal::Middle)
        .margin(Margin {
            bottom: BOTTOM as i32,
            ..Margin::default()
        })
        .layer(Layer::Top)
        .space(Zone::Reserve)
        .namespace("kanade-dock")
        .visible(!items.is_empty())
        .input_region(vec![view::input_area(body)])
        .child(
            Rectangle::new()
                .width(width)
                .height(BODY)
                .radius(radius::CARD)
                .fill(roles.surface)
                .border(1.0, roles.outline)
                .padding(Padding {
                    top: INSET,
                    right: INSET,
                    bottom: 0.0,
                    left: INSET,
                })
                .align_child(Start, Start)
                .child(Row::new(row).gap(GAP).align(Center)),
        )
}

// an item's icon, its dot under it while it runs, and its click
fn slot(item: &Item, roles: &ThemeRoles) -> Rectangle {
    let mut column: Vec<Box<dyn Widget>> = vec![mark(&item.app, roles)];

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

        column.push(Box::new(
            Rectangle::new()
                .width(if focused { FOCUSED_DOT } else { DOT })
                .height(DOT)
                .radius(DOT / 2.0)
                .fill(color),
        ));
    }

    let press = press(item);

    Rectangle::new()
        .width(ICON)
        .height(BODY - INSET)
        .align_child(Center, Start)
        .cursor(Cursor::Pointer)
        .on_click(move |button| {
            if button == Button::Left
                && let Some(press) = press.clone()
            {
                carry_out(press);
            }
        })
        .child(Column::new(column).gap(BELOW).align(Center))
}

// the app's icon, or its initial on the quiet tile
fn mark(app: &App, roles: &ThemeRoles) -> Box<dyn Widget> {
    if let Some(path) = desktop(app).and_then(|entry| entry.icon_file.clone()) {
        // decoded at twice its size, crisp at scale 2
        let pixels = (ICON * 2.0) as u32;

        return Box::new(
            Rectangle::new()
                .width(ICON)
                .height(ICON)
                .fill(amane::Image::contain(path).thumbnail(pixels, pixels)),
        );
    }

    let name = match app {
        App::Desktop(entry) => Some(entry.name.as_str()),
        App::Unmatched(app_id) => app_id.as_deref(),
    };

    let initial = name
        .and_then(|name| name.chars().next())
        .map_or_else(|| String::from("?"), |char| char.to_uppercase().collect());

    Box::new(view::tile(None, &initial, ICON, radius::TILE, roles))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crate::sources::launch::Fields;
    use crate::sources::windows::{Pinned, Running};

    fn entry(id: &str, exec: Option<&str>) -> DesktopEntry {
        let file = format!("{id}.desktop");

        DesktopEntry {
            launch: exec.and_then(|exec| launch(&file, exec)),
            id: file,
            name: id.to_owned(),
            icon: None,
            icon_file: None,
        }
    }

    fn launch(id: &str, exec: &str) -> Option<Launch> {
        Launch::of(&Fields {
            id,
            name: id,
            icon: None,
            file: Path::new(id),
            exec: Some(exec),
            path: None,
            terminal: false,
            dbus_activatable: false,
        })
    }

    fn window(id: u64, focused: bool) -> Window {
        Window {
            id: WindowId(id),
            app_id: None,
            focused,
            urgent: false,
        }
    }

    fn running(app: App, windows: Vec<Window>) -> Running {
        Running { app, windows }
    }

    // each item as its name, its window ids and whether it is pinned
    fn shown(items: &[Item]) -> Vec<String> {
        items
            .iter()
            .map(|item| {
                let name = match &item.app {
                    App::Desktop(entry) => entry.id.clone(),
                    App::Unmatched(app_id) => app_id.clone().unwrap_or_default(),
                };
                let ids: Vec<String> = item.windows.iter().map(|w| w.id.0.to_string()).collect();
                let pin = if item.pinned { "pinned " } else { "" };

                format!("{pin}{name} {}", ids.join(","))
            })
            .collect()
    }

    #[test]
    fn pinned_apps_come_first_and_running_ones_join_them() {
        let firefox = entry("firefox", Some("firefox"));
        let kitty = entry("kitty", Some("kitty"));
        let zed = entry("zed", Some("zed"));

        let windows = Windows::with(
            vec![
                running(App::Desktop(kitty.clone()), vec![window(1, false)]),
                running(
                    App::Unmatched(Some(String::from("steam_app_1"))),
                    vec![window(2, false)],
                ),
                running(
                    App::Desktop(zed.clone()),
                    vec![window(3, true), window(4, false)],
                ),
            ],
            vec![
                Pinned {
                    id: zed.id.clone(),
                    entry: Some(zed),
                },
                Pinned {
                    id: String::from("gone.desktop"),
                    entry: None,
                },
                Pinned {
                    id: firefox.id.clone(),
                    entry: Some(firefox),
                },
            ],
        );

        assert_eq!(
            shown(&items(&windows)),
            [
                "pinned zed.desktop 3,4",
                "pinned firefox.desktop ",
                "kitty.desktop 1",
                "steam_app_1 2",
            ]
        );
        assert_eq!(items(&Windows::default()), []);
    }

    #[test]
    fn a_click_launches_focuses_or_goes_to_the_next_window() {
        let item = |windows: Vec<Window>, exec: Option<&str>| Item {
            app: App::Desktop(entry("zed", exec)),
            windows,
            pinned: true,
        };

        assert_eq!(
            press(&item(vec![], Some("zed --new"))),
            launch("zed.desktop", "zed --new").map(Press::Launch)
        );
        assert_eq!(press(&item(vec![], None)), None);

        // another app has the focus: the first window
        assert_eq!(
            press(&item(vec![window(3, false), window(5, false)], None)),
            Some(Press::Focus(WindowId(3)))
        );

        // this one has it: the next, round to the first
        assert_eq!(
            press(&item(vec![window(3, true), window(5, false)], None)),
            Some(Press::Focus(WindowId(5)))
        );
        assert_eq!(
            press(&item(vec![window(3, false), window(5, true)], None)),
            Some(Press::Focus(WindowId(3)))
        );

        let unmatched = Item {
            app: App::Unmatched(None),
            windows: vec![window(7, false)],
            pinned: false,
        };
        assert_eq!(press(&unmatched), Some(Press::Focus(WindowId(7))));
    }
}
