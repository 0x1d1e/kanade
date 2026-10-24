//! Column and window controls tied to the focused window (ADR 0132).
//!
//! Consume or expel, column width presets, maximize, float, and move to workspace.

use kanade_runtime::service::Service;
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Column, Cursor, Image, Key, Padding, Rectangle, Row, Start, Text, Widget,
};

use super::pad::{self, Focus};
use super::{Outline, on_left};
use crate::icon::Icon;
use crate::island::geometry;
use crate::island::presentation::Surface;
use crate::sources::icons;
use crate::sources::niri::{self, FocusedWindow, Niri, Seen, WorkspaceInfo};
use crate::sources::windows::{App, Windows};
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, radius};
use crate::view::rise;

const HEADER: f32 = TARGET;
const GAP: f32 = 14.0;
const BUTTON_WIDTH: f32 = (pad::WIDTH - 3.0 * 8.0) / 4.0; // 4 buttons per row with 8px gap

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum At {
    Consume,
    Expel,
    Maximize,
    Float,
    WidthThird,
    WidthHalf,
    WidthTwoThirds,
    WidthPreset,
    Workspace(u64),
}

impl Service for Focus<At> {
    fn new() -> Self {
        Focus::default()
    }

    fn listen() {}
}

fn node(at: At) -> &'static str {
    match at {
        At::Consume => "window_consume",
        At::Expel => "window_expel",
        At::Maximize => "window_maximize",
        At::Float => "window_float",
        At::WidthThird => "window_width_third",
        At::WidthHalf => "window_width_half",
        At::WidthTwoThirds => "window_width_two_thirds",
        At::WidthPreset => "window_width_preset",
        At::Workspace(_) => "window_workspace",
    }
}

fn relevant_workspaces(seen: &Seen, focused: Option<&FocusedWindow>) -> Vec<WorkspaceInfo> {
    let Some(focused) = focused else {
        return Vec::new();
    };

    let output = focused.workspace_id.and_then(|ws_id| {
        seen.workspaces
            .iter()
            .find(|ws| ws.id == ws_id)
            .and_then(|ws| ws.output.clone())
    });

    let filtered: Vec<WorkspaceInfo> = if let Some(output) = output {
        seen.workspaces
            .iter()
            .filter(|ws| ws.output.as_ref() == Some(&output))
            .cloned()
            .collect()
    } else {
        seen.workspaces.clone()
    };

    filtered
}

fn grid(focused: Option<&FocusedWindow>, workspaces: &[WorkspaceInfo]) -> Vec<Vec<(At, f32)>> {
    let Some(_) = focused else {
        return Vec::new();
    };

    let spread = |row: &[At]| -> Vec<(At, f32)> {
        let count = row.len() as f32;
        row.iter()
            .enumerate()
            .map(|(index, &at)| (at, (index as f32 + 0.5) / count))
            .collect()
    };

    let mut rows = Vec::new();

    // Row 0: Column controls & Toggles
    rows.push(spread(&[At::Consume, At::Expel, At::Maximize, At::Float]));

    // Row 1: Width Presets
    rows.push(spread(&[
        At::WidthThird,
        At::WidthHalf,
        At::WidthTwoThirds,
        At::WidthPreset,
    ]));

    // Row 2: Workspace pills
    let ws_targets: Vec<At> = workspaces.iter().map(|ws| At::Workspace(ws.id)).collect();

    if !ws_targets.is_empty() {
        rows.push(spread(&ws_targets));
    }

    rows
}

fn act(at: At) {
    let niri = Niri::read();
    let Some(focused) = niri.seen.focused_window.as_ref() else {
        return;
    };

    match at {
        At::Consume => {
            let _ = niri::consume_window_into_column();
        }
        At::Expel => {
            if focused.tile_count > 1 {
                let _ = niri::expel_window_from_column();
            }
        }
        At::Maximize => {
            let _ = niri::maximize_column();
        }
        At::Float => {
            let _ = niri::toggle_window_floating();
        }
        At::WidthThird => {
            let _ = niri::set_column_width(1.0 / 3.0);
        }
        At::WidthHalf => {
            let _ = niri::set_column_width(0.5);
        }
        At::WidthTwoThirds => {
            let _ = niri::set_column_width(2.0 / 3.0);
        }
        At::WidthPreset => {
            let _ = niri::switch_preset_column_width();
        }
        At::Workspace(id) => {
            let _ = niri::move_window_to_workspace(focused.id, id);
        }
    }
}

pub fn key(monitor: &str, key: Key) -> bool {
    let niri = Niri::read();
    let focused = niri.seen.focused_window.clone();
    let workspaces = relevant_workspaces(&niri.seen, focused.as_ref());

    pad::key(
        monitor,
        Surface::Window,
        key,
        || grid(focused.as_ref(), &workspaces),
        |focus, at| {
            act(at);
            focus
        },
    )
}

pub fn click(at: At) {
    pad::click(Surface::Window, at, |focus, at| {
        act(at);
        focus
    });
}

fn window_icon(window: &FocusedWindow) -> (Box<dyn Widget>, Option<String>) {
    let windows = Windows::read();
    let running = windows
        .running()
        .iter()
        .find(|app| app.windows.iter().any(|w| w.id.0 == window.id));

    let (icon_path, app_name) = match running {
        Some(running) => match &running.app {
            App::Desktop(entry) => (entry.icon_file.clone(), Some(entry.name.clone())),
            App::Unmatched(app_id) => (
                app_id.as_deref().and_then(|id| icons::find(id, None)),
                app_id.clone(),
            ),
        },
        None => {
            let icon_file = window
                .app_id
                .as_deref()
                .and_then(|app_id| icons::find(app_id, None));
            (icon_file, window.app_id.clone())
        }
    };

    let icon_widget: Box<dyn Widget> = if let Some(path) = icon_path {
        Box::new(
            Rectangle::new()
                .width(24.0)
                .height(24.0)
                .fill(Image::contain(path).thumbnail(48, 48)),
        )
    } else {
        Box::new(Icon::Columns.on(20.0, theme::island().on_surface))
    };

    (icon_widget, app_name)
}

fn frame(header: Row, body: impl Widget + 'static) -> Rectangle {
    let shape = geometry::PAD;

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::Region, "pad").label("Window"))
        .child(
            Column::new(vec![Box::new(rise(0, header)), Box::new(rise(1, body))])
                .width(pad::WIDTH)
                .gap(GAP),
        )
}

fn button(
    node: &'static str,
    label: &str,
    width: f32,
    active: bool,
    disabled: bool,
    ring: bool,
    press: impl Fn() + 'static,
) -> Rectangle {
    let roles = theme::island();
    let text_color = if disabled {
        roles.outline
    } else if active {
        roles.primary
    } else {
        roles.on_surface
    };

    let mut rect = Rectangle::new()
        .width(width)
        .height(34.0)
        .radius(radius::ROW)
        .padding(Padding {
            top: 0.0,
            right: 8.0,
            bottom: 0.0,
            left: 8.0,
        })
        .align_child(Center, Center)
        .semantics(|| {
            let mut sem = Semantics::new(Role::Button, node).label(label);
            if disabled {
                sem = sem.disabled(true);
            }
            sem
        })
        .child(
            Text::new(label)
                .size(theme::text::LABEL_SMALL)
                .color(text_color)
                .weight(theme::text::SEMIBOLD),
        );

    if !disabled {
        rect = rect.cursor(Cursor::Pointer).on_click(on_left(press));
    }

    let fill = if active {
        roles.surface_container_high
    } else if disabled {
        roles.surface
    } else {
        roles.surface_container
    };

    let rect = rect.fill(fill);
    if ring {
        rect.border_if(true)
    } else if active {
        rect.border(1.0, roles.primary)
    } else if disabled {
        rect.border(1.0, roles.surface_container)
    } else {
        rect.border(1.0, roles.surface_container_high)
    }
}

fn workspace_pill(
    node: &'static str,
    label: &str,
    active: bool,
    ring: bool,
    press: impl Fn() + 'static,
) -> Rectangle {
    let roles = theme::island();
    let text_color = if active {
        roles.on_primary
    } else {
        roles.on_surface
    };

    let pill_width = (label.len() as f32 * 8.0 + 24.0).max(36.0);

    let rect = Rectangle::new()
        .width(pill_width)
        .height(28.0)
        .padding(Padding {
            top: 0.0,
            right: 12.0,
            bottom: 0.0,
            left: 12.0,
        })
        .radius(14.0)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .semantics(|| Semantics::new(Role::Button, node).label(label))
        .on_click(on_left(press))
        .child(
            Text::new(label)
                .size(theme::text::LABEL_SMALL)
                .color(text_color)
                .weight(theme::text::SEMIBOLD),
        );

    let fill = if active {
        roles.primary
    } else {
        roles.surface_container
    };

    let rect = rect.fill(fill);
    if ring {
        rect.border_if(true)
    } else if active {
        rect
    } else {
        rect.border(1.0, roles.surface_container_high)
    }
}

pub fn surface(open: bool, visit: u64, held: bool) -> Rectangle {
    let niri = Niri::read();
    let Some(focused) = niri.seen.focused_window.as_ref() else {
        return pad::frame(
            "Window",
            pad::reading(Icon::Columns, "No focused window".into()),
        );
    };

    let workspaces = relevant_workspaces(&niri.seen, Some(focused));
    let grid = grid(Some(focused), &workspaces);

    let ring = open
        .then(|| Focus::<At>::read().of(visit, held).ring(&grid))
        .flatten();

    // Header: icon, window title, and optional app name
    let (icon_widget, app_name) = window_icon(focused);
    let title_text = if !focused.title.is_empty() {
        focused.title.clone()
    } else if let Some(app) = &app_name {
        app.clone()
    } else {
        String::from("Window")
    };

    let mut header_items: Vec<Box<dyn Widget>> = vec![icon_widget];
    let mut title_items: Vec<Box<dyn Widget>> = vec![Box::new(
        Text::new(&title_text)
            .size(theme::text::BODY)
            .weight(theme::text::SEMIBOLD)
            .color(theme::island().on_surface)
            .elide(),
    )];

    if let Some(app) = &app_name
        && *app != title_text
    {
        title_items.push(Box::new(
            Text::new(format!("— {app}"))
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .elide(),
        ));
    }

    header_items.push(Box::new(Row::new(title_items).gap(8.0).align(Center)));

    let header = Row::new(header_items)
        .width(pad::WIDTH)
        .height(HEADER)
        .gap(10.0)
        .align(Center);

    // Row 0: Column controls and toggles
    let expel_disabled = focused.tile_count <= 1;
    let row0 = Row::new(vec![
        Box::new(button(
            node(At::Consume),
            "Consume right",
            BUTTON_WIDTH,
            false,
            false,
            ring == Some(At::Consume),
            || click(At::Consume),
        )),
        Box::new(button(
            node(At::Expel),
            "Expel bottom",
            BUTTON_WIDTH,
            false,
            expel_disabled,
            ring == Some(At::Expel),
            || click(At::Expel),
        )),
        Box::new(button(
            node(At::Maximize),
            "Maximize",
            BUTTON_WIDTH,
            false,
            false,
            ring == Some(At::Maximize),
            || click(At::Maximize),
        )),
        Box::new(button(
            node(At::Float),
            "Float",
            BUTTON_WIDTH,
            focused.is_floating,
            false,
            ring == Some(At::Float),
            || click(At::Float),
        )),
    ])
    .gap(8.0);

    // Row 1: Width Presets
    let is_third = !focused.is_floating
        && focused
            .column_width
            .is_some_and(|w| (w - 1.0 / 3.0).abs() < 0.02);
    let is_half =
        !focused.is_floating && focused.column_width.is_some_and(|w| (w - 0.5).abs() < 0.02);
    let is_two_thirds = !focused.is_floating
        && focused
            .column_width
            .is_some_and(|w| (w - 2.0 / 3.0).abs() < 0.02);

    let row1 = Row::new(vec![
        Box::new(button(
            node(At::WidthThird),
            "⅓",
            BUTTON_WIDTH,
            is_third,
            false,
            ring == Some(At::WidthThird),
            || click(At::WidthThird),
        )),
        Box::new(button(
            node(At::WidthHalf),
            "½",
            BUTTON_WIDTH,
            is_half,
            false,
            ring == Some(At::WidthHalf),
            || click(At::WidthHalf),
        )),
        Box::new(button(
            node(At::WidthTwoThirds),
            "⅔",
            BUTTON_WIDTH,
            is_two_thirds,
            false,
            ring == Some(At::WidthTwoThirds),
            || click(At::WidthTwoThirds),
        )),
        Box::new(button(
            node(At::WidthPreset),
            "Preset",
            BUTTON_WIDTH,
            false,
            false,
            ring == Some(At::WidthPreset),
            || click(At::WidthPreset),
        )),
    ])
    .gap(8.0);

    // Row 2: Workspace label and pills
    let mut ws_pills = Vec::new();
    for ws in &workspaces {
        let is_current = focused.workspace_id == Some(ws.id);
        let label = ws.name.clone().unwrap_or_else(|| ws.index.to_string());
        let ws_id = ws.id;

        ws_pills.push(Box::new(workspace_pill(
            node(At::Workspace(ws_id)),
            &label,
            is_current,
            ring == Some(At::Workspace(ws_id)),
            move || click(At::Workspace(ws_id)),
        )) as Box<dyn Widget>);
    }

    let workspace_section = Column::new(vec![
        Box::new(
            Text::new("Move to workspace")
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant),
        ),
        Box::new(Row::new(ws_pills).gap(8.0).align(Center)),
    ])
    .gap(6.0)
    .align(Center);

    let body = Column::new(vec![
        Box::new(row0),
        Box::new(row1),
        Box::new(workspace_section),
    ])
    .gap(10.0)
    .align(Center);

    frame(
        header,
        Rectangle::new()
            .width(pad::WIDTH)
            .height(pad::BODY)
            .align_child(Center, Center)
            .child(body),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_window(tile_count: usize) -> FocusedWindow {
        FocusedWindow {
            id: 42,
            title: "Terminal".to_string(),
            app_id: Some("alacritty".to_string()),
            is_floating: false,
            tile_count,
            column_width: Some(0.5),
            workspace_id: Some(1),
        }
    }

    fn test_workspaces() -> Vec<WorkspaceInfo> {
        vec![
            WorkspaceInfo {
                id: 1,
                index: 1,
                name: None,
                output: Some("eDP-1".to_string()),
                is_active: true,
                is_focused: true,
            },
            WorkspaceInfo {
                id: 2,
                index: 2,
                name: Some("code".to_string()),
                output: Some("eDP-1".to_string()),
                is_active: false,
                is_focused: false,
            },
        ]
    }

    #[test]
    fn grid_empty_without_focused_window() {
        let workspaces = test_workspaces();
        assert!(grid(None, &workspaces).is_empty());
    }

    #[test]
    fn grid_targets_with_focused_window() {
        let window = test_window(2);
        let workspaces = test_workspaces();
        let g = grid(Some(&window), &workspaces);

        assert_eq!(g.len(), 3);
        assert_eq!(g[0].len(), 4);
        assert_eq!(g[1].len(), 4);
        assert_eq!(g[2].len(), 2);

        assert_eq!(g[0][0].0, At::Consume);
        assert_eq!(g[0][1].0, At::Expel);
        assert_eq!(g[0][2].0, At::Maximize);
        assert_eq!(g[0][3].0, At::Float);

        assert_eq!(g[1][0].0, At::WidthThird);
        assert_eq!(g[1][1].0, At::WidthHalf);
        assert_eq!(g[1][2].0, At::WidthTwoThirds);
        assert_eq!(g[1][3].0, At::WidthPreset);

        assert_eq!(g[2][0].0, At::Workspace(1));
        assert_eq!(g[2][1].0, At::Workspace(2));
    }

    #[test]
    fn focus_ring_step_navigation() {
        let window = test_window(2);
        let workspaces = test_workspaces();
        let g = grid(Some(&window), &workspaces);

        let focus = Focus::<At>::default().of(1, true);
        // Initially on first target
        assert_eq!(focus.ring(&g), Some(At::Consume));

        // Step right moves to Expel
        let (focus, act) = focus.step(Key::Right, &g).expect("step right");
        assert_eq!(act, None);
        assert_eq!(focus.ring(&g), Some(At::Expel));

        // Step down moves to WidthHalf (closest across)
        let (focus, act) = focus.step(Key::Down, &g).expect("step down");
        assert_eq!(act, None);
        assert_eq!(focus.ring(&g), Some(At::WidthHalf));

        // Press Enter acts on WidthHalf
        let (focus, act) = focus.step(Key::Enter, &g).expect("step enter");
        assert_eq!(act, Some(At::WidthHalf));
        assert_eq!(focus.ring(&g), Some(At::WidthHalf));
    }
}
