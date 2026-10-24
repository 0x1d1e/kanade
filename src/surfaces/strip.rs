//! The niri strip minimap (ADR 0133): every workspace of the focused monitor as a strip of
//! columns, each column its windows stacked, widths and heights as niri tiles them. A viewport
//! frame marks what the monitor shows of the active workspace's strip; off-screen columns dim.
//! Clicking a tile focuses that window and collapses the island; arrows move a 2D ring across
//! columns and stacked tiles, Enter focuses, Escape collapses.

use kanade_runtime::service::{self, Service};
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Column, Cursor, Image, ImageState, Key, Monitors, Padding, Parent, Rectangle, Row,
    ScrollArea, Stack, Start, Text, Widget, children,
};

use super::pad::{self, Focus};
use super::{Outline, on_left};
use crate::icon::Icon;
use crate::island::geometry::DEFAULT_LARGEST;
use crate::island::presentation::Surface;
use crate::island::service::IslandService;
use crate::sources::icons;
use crate::sources::niri::{self, Niri, Seen, StripWindow, WorkspaceInfo};
use crate::theme::space::INSET;
use crate::theme::{self, radius};
use crate::view;

// the body's width, a list Surface's
const WIDTH: f32 = DEFAULT_LARGEST.width - 2.0 * INSET;

// a column never draws narrower than this, so a sliver of a tile stays readable
const MIN_COLUMN: f32 = 60.0;

// a workspace's header, the strip under it, and the gaps between workspaces and columns
const HEADER: f32 = 22.0;
const STRIP: f32 = 96.0;
const WORKSPACE_GAP: f32 = 14.0;
const COLUMN_GAP: f32 = 6.0;
const TILE_INSET: f32 = 6.0;
const TILE_GAP: f32 = 6.0;

// what one workspace takes of the height, header and strip together
const WORKSPACE: f32 = HEADER + STRIP;

// the height the body asks for at `workspaces`, clamped to the room the largest body has
fn height_for(workspaces: usize) -> f32 {
    let room = DEFAULT_LARGEST.height - 2.0 * INSET;
    let content =
        workspaces as f32 * WORKSPACE + workspaces.saturating_sub(1) as f32 * WORKSPACE_GAP;

    2.0 * INSET + content.min(room)
}

// the same either way, since workspaces change under the Surface wherever it is
fn asks(workspaces: usize) -> f32 {
    height_for(workspaces)
}

/*
 * tells the island how tall the Strip asks to be. After each change of niri's windows or
 * workspaces, never from a view
 */
pub fn fit() {
    let _ordered = super::fitting();
    let visit = IslandService::read().visit();

    let workspaces = strip_workspaces(&Niri::read().seen).len();
    let height = asks(workspaces);

    super::ask(Surface::Strip, height, Some((visit, height)));
}

// at start: the body follows niri's layout from now on
pub fn start() {
    service::watch::<Niri>(fit);
    fit();
}

// what a target the ring can be on
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum At {
    Window(u64),
}

impl Service for Focus<At> {
    fn new() -> Self {
        Focus::default()
    }

    fn listen() {}
}

// the workspaces of the focused monitor, ordered by index; while focus is unknown, all of them
fn strip_workspaces(seen: &Seen) -> Vec<WorkspaceInfo> {
    let mine = |output: &Option<String>| match &seen.focused_output {
        Some(focused) => output.as_deref() == Some(focused),
        None => true,
    };

    let mut workspaces: Vec<WorkspaceInfo> = seen
        .workspaces
        .iter()
        .filter(|workspace| mine(&workspace.output))
        .cloned()
        .collect();

    workspaces.sort_by_key(|workspace| (workspace.index, workspace.id));

    workspaces
}

// the windows of one workspace, floating ones out, each within its column and row
fn windows_of<'a>(windows: &'a [StripWindow], workspace: &WorkspaceInfo) -> Vec<&'a StripWindow> {
    let mut mine: Vec<&StripWindow> = windows
        .iter()
        .filter(|window| window.workspace_id == Some(workspace.id) && !window.is_floating)
        .collect();

    mine.sort_by_key(|window| (window.col, window.row, window.id));

    mine
}

// the floating windows of one workspace, said in the header
fn floating_of(windows: &[StripWindow], workspace: &WorkspaceInfo) -> usize {
    windows
        .iter()
        .filter(|window| window.workspace_id == Some(workspace.id) && window.is_floating)
        .count()
}

// one workspace's strip as the view lays it out: the columns in order, each with its tiles
#[derive(Debug, Clone, PartialEq)]
struct StripColumn {
    windows: Vec<u64>,
    col: Option<u64>,

    // the widest tile in the column, what niri says the column takes
    width: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
struct Strip {
    workspace: u64,
    columns: Vec<StripColumn>,
    floating: usize,
}

// the strips of the workspaces given, in their order
fn strips(windows: &[StripWindow], workspaces: &[WorkspaceInfo]) -> Vec<Strip> {
    workspaces
        .iter()
        .map(|workspace| {
            let mut columns: Vec<StripColumn> = Vec::new();

            for window in windows_of(windows, workspace) {
                let open = columns
                    .last()
                    .is_some_and(|column| column.col == window.col);

                if open {
                    let column = columns.last_mut().expect("just checked");
                    column.windows.push(window.id);
                    column.width = match (column.width, window.tile_width) {
                        (Some(was), Some(now)) => Some(was.max(now)),
                        (was, now) => was.or(now),
                    };
                } else {
                    columns.push(StripColumn {
                        windows: vec![window.id],
                        col: window.col,
                        width: window.tile_width,
                    });
                }
            }

            Strip {
                workspace: workspace.id,
                columns,
                floating: floating_of(windows, workspace),
            }
        })
        .collect()
}

/*
 * the strip's viewport the monitor frames, as its left and right edge across the columns, in the
 * columns' own units from the strip's left: the frame is the monitor's width against the strip's
 * whole width, centered on the focused window's column, never past the strip's ends
 */
fn viewport(strip: &Strip, focused: Option<u64>, screen_width: f64) -> Option<(f32, f32)> {
    if strip.columns.is_empty() {
        return None;
    }

    let width_of = |column: &StripColumn| column.width.unwrap_or(1.0).max(1.0);

    let total: f64 = strip.columns.iter().map(width_of).sum();
    let end = total + strip.columns.len().saturating_sub(1) as f64 * COLUMN_GAP as f64;

    let at = strip
        .columns
        .iter()
        .position(|column| focused.is_some_and(|id| column.windows.contains(&id)))
        .unwrap_or(0);

    // the frame covers the monitor's share of the strip, at least the focused column
    let frame = screen_width.min(end).max(width_of(&strip.columns[at]));

    let left_of = |at: usize| -> f64 {
        strip.columns[..at].iter().map(width_of).sum::<f64>() + at as f64 * COLUMN_GAP as f64
    };

    // centered on the focused column, clamped so the frame never runs off either end
    let center = left_of(at) + width_of(&strip.columns[at]) / 2.0;
    let left = (center - frame / 2.0).clamp(0.0, (end - frame).max(0.0));

    Some((left as f32, (left + frame) as f32))
}

// where each column starts across, in pixels, beside the width it draws at
fn laid(strip: &Strip) -> Vec<(f32, f32, &StripColumn)> {
    let width_of = |column: &StripColumn| column.width.unwrap_or(1.0).max(1.0);

    let total: f64 = strip
        .columns
        .iter()
        .map(|column| width_of(column) + COLUMN_GAP as f64)
        .sum::<f64>()
        - COLUMN_GAP as f64;
    let room = WIDTH - 2.0 * TILE_INSET;
    let scale = f64::from(room) / total.max(1.0);

    let mut laid = Vec::new();
    let mut across: f32 = 0.0;

    for column in &strip.columns {
        let remaining = (room - across).max(0.0);
        let share = ((width_of(column) * scale) as f32)
            .max(MIN_COLUMN)
            .min(remaining.max(MIN_COLUMN));

        laid.push((across, share, column));
        across += share + COLUMN_GAP;
    }

    laid
}

// the ring's grid: rows across workspaces and stacked tiles in columns
fn ring_grid(strips: &[Strip]) -> Vec<Vec<(At, f32)>> {
    let mut grid = Vec::new();

    for strip in strips {
        let columns = laid(strip);
        let max_depth = strip
            .columns
            .iter()
            .map(|c| c.windows.len())
            .max()
            .unwrap_or(0);

        for depth in 0..max_depth {
            let mut row = Vec::new();

            for &(across, width, column) in &columns {
                if let Some(&id) = column.windows.get(depth) {
                    let middle = (across + width / 2.0) / WIDTH;
                    row.push((At::Window(id), middle));
                }
            }

            if !row.is_empty() {
                grid.push(row);
            }
        }
    }

    grid
}

// the view's own read of the visit, so this never reads IslandService again
pub fn surface(open: bool, visit: u64, held: bool) -> Rectangle {
    let niri = Niri::read();
    let workspaces = strip_workspaces(&niri.seen);
    let strips = strips(&niri.seen.windows, &workspaces);
    let grid = ring_grid(&strips);

    let ring = open
        .then(|| Focus::<At>::read().of(visit, held).ring(&grid))
        .flatten();

    let shape = view::largest();
    let region = || Semantics::new(Role::Region, "strip").label("Strip");

    if strips.is_empty() {
        return Rectangle::new()
            .width(shape.width)
            .height(shape.height)
            .padding(INSET)
            .align_child(Start, Start)
            .semantics(region)
            .child(
                Text::new("No workspaces on this monitor")
                    .size(theme::text::BODY)
                    .color(theme::island().on_surface_variant),
            );
    }

    let room = shape.height - 2.0 * INSET;

    let listed = Column::new(
        strips
            .iter()
            .map(|strip| Box::new(workspace(&niri.seen, strip, ring)) as Box<dyn Widget>)
            .collect(),
    )
    .width(WIDTH)
    .gap(WORKSPACE_GAP);

    let natural = workspaces.len() as f32 * WORKSPACE
        + workspaces.len().saturating_sub(1) as f32 * WORKSPACE_GAP;

    // workspaces past the room scroll inside the body
    let body: Box<dyn Widget> = if natural > room {
        Box::new(
            ScrollArea::new("strip workspaces", listed)
                .width(WIDTH)
                .height(room),
        )
    } else {
        Box::new(listed)
    };

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .semantics(region)
        .child(body)
}

// one workspace's header and strip
fn workspace(seen: &Seen, strip: &Strip, ring: Option<At>) -> Column {
    let roles = theme::island();
    let info = seen
        .workspaces
        .iter()
        .find(|workspace| workspace.id == strip.workspace);

    let index = info.map_or(strip.workspace, |info| info.index as u64);
    let name = info
        .and_then(|info| info.name.clone())
        .unwrap_or_else(|| format!("Workspace {index}"));

    let mut title_children = children![
        Text::new(format!("{index} · {name}"))
            .size(theme::text::LABEL_SMALL)
            .color(match info {
                Some(info) if info.is_focused => roles.primary,
                Some(info) if info.is_active => roles.on_surface,
                _ => roles.on_surface_variant,
            },)
            .weight(theme::text::SEMIBOLD)
            .elide(),
    ];

    if strip.floating > 0 {
        title_children.push(Box::new(
            Text::new(format!("{} floating", strip.floating))
                .size(theme::text::LABEL_SMALL)
                .color(roles.on_surface_variant),
        ));
    }

    let header = Row::new(title_children)
        .width(WIDTH)
        .height(HEADER)
        .align(Center)
        .gap(COLUMN_GAP);

    Column::new(vec![
        Box::new(header) as Box<dyn Widget>,
        Box::new(strip_view(seen, strip, ring)),
    ])
    .width(WIDTH)
}

// the horizontal strip of one workspace: its columns and the viewport frame over the active one
fn strip_view(seen: &Seen, strip: &Strip, ring: Option<At>) -> Stack {
    let roles = theme::island();
    let active = seen
        .focused_window
        .as_ref()
        .is_some_and(|focused| focused.workspace_id == Some(strip.workspace));

    let focused = seen
        .windows
        .iter()
        .find(|window| window.is_focused)
        .map(|window| window.id);

    let monitors = Monitors::read();
    let screen_width = seen
        .focused_output
        .as_ref()
        .and_then(|output| monitors.all().iter().find(|m| &m.name == output))
        .map_or(1920.0, |monitor| monitor.width as f64);

    let frame = active
        .then(|| viewport(strip, focused, screen_width))
        .flatten();

    let width_of = |column: &StripColumn| column.width.unwrap_or(1.0).max(1.0);
    let unscaled_end: f64 = strip.columns.iter().map(width_of).sum::<f64>()
        + strip.columns.len().saturating_sub(1) as f64 * COLUMN_GAP as f64;

    let laid_columns = laid(strip);
    let drawn_end = laid_columns
        .last()
        .map_or(0.0, |&(across, width, _)| across + width);

    let scale = if unscaled_end > 0.0 {
        drawn_end / unscaled_end as f32
    } else {
        1.0
    };

    let windows_by_id: std::collections::HashMap<u64, &StripWindow> =
        seen.windows.iter().map(|w| (w.id, w)).collect();

    let mut layers: Vec<Box<dyn Widget>> = Vec::new();
    let mut unscaled_left = 0.0;

    // the columns, each as wide as its share of the strip, dimmed where the monitor is not
    for (across, width, column) in laid_columns {
        let col_unscaled_width = width_of(column);
        let col_unscaled_right = unscaled_left + col_unscaled_width;

        let on_screen = frame.is_none_or(|(left, right)| {
            col_unscaled_right > f64::from(left) && unscaled_left < f64::from(right)
        });

        unscaled_left = col_unscaled_right + COLUMN_GAP as f64;

        let col_windows: Vec<&StripWindow> = column
            .windows
            .iter()
            .filter_map(|&id| windows_by_id.get(&id).copied())
            .collect();

        let count = col_windows.len();
        let tile_gap = 3.0;
        let inner_height = STRIP - TILE_GAP;
        let total_gaps = count.saturating_sub(1) as f32 * tile_gap;
        let usable_height = (inner_height - total_gaps).max(count as f32 * 16.0);
        let total_th: f64 = col_windows
            .iter()
            .map(|w| w.tile_height.unwrap_or(1.0).max(1.0))
            .sum();

        let tiles: Vec<Box<dyn Widget>> = col_windows
            .iter()
            .map(|window| {
                let share = (window.tile_height.unwrap_or(1.0).max(1.0) / total_th.max(1.0)) as f32;
                let tile_h = (usable_height * share).max(16.0);
                Box::new(tile(window, ring == Some(At::Window(window.id)), tile_h))
                    as Box<dyn Widget>
            })
            .collect();

        layers.push(Box::new(
            Rectangle::new()
                .width(width)
                .height(STRIP)
                .radius(radius::TILE)
                .fill(roles.surface_container)
                .padding(TILE_GAP / 2.0)
                .align_child(Start, Start)
                .child(
                    Column::new(tiles)
                        .width(Parent)
                        .height(Parent)
                        .gap(tile_gap),
                )
                .translate(TILE_INSET + across, 0.0)
                .opacity(if on_screen { 1.0 } else { 0.55 }),
        ));
    }

    // the illuminated frame over what the monitor shows, active workspace only
    if let Some((left, right)) = frame {
        let frame_left = left * scale;
        let frame_width = (right - left) * scale;

        layers.push(Box::new(
            Rectangle::new()
                .width(frame_width)
                .height(STRIP)
                .radius(radius::TILE)
                .border(1.5, roles.primary)
                .fill(theme::faded(roles.primary, 0.06))
                .translate(TILE_INSET + frame_left, 0.0),
        ));
    }

    Stack::new(layers).width(WIDTH).height(STRIP)
}

// a window's tile: its app's icon beside its title, on a card the ring can sit on
fn tile(window: &StripWindow, ring: bool, height: f32) -> Rectangle {
    let roles = theme::island();
    let focused = window.is_focused;

    let (icon, label) = match &window.app_id {
        Some(app_id) => (icons::find(app_id, None), app_id.clone()),
        None => (None, String::from("(no app)")),
    };

    let title = if window.title.is_empty() {
        label
    } else {
        window.title.clone()
    };

    let mark = match &icon {
        Some(path) => {
            let picture = Image::contain(path).thumbnail(48, 48);

            match picture.state() {
                ImageState::Ready => Rectangle::new().width(16.0).height(16.0).fill(picture),
                _ => Icon::Columns.on(16.0, roles.on_surface_variant),
            }
        }
        None => Icon::Columns.on(16.0, roles.on_surface_variant),
    };

    let press = window.id;

    let mut row_children = children![
        mark,
        Text::new(&title)
            .size(theme::text::LABEL_SMALL)
            .color(roles.on_surface)
            .weight(if focused {
                theme::text::SEMIBOLD
            } else {
                theme::text::MEDIUM
            })
            .elide(),
    ];

    if window.is_urgent {
        row_children.push(Box::new(
            Rectangle::new()
                .width(6.0)
                .height(6.0)
                .radius(3.0)
                .fill(theme::signals().warning),
        ));
    }

    // the ring shows over whatever else the tile says of itself, fill kept under it
    let card = Rectangle::new()
        .width(Parent)
        .height(height)
        .radius(radius::TILE)
        .fill(if focused {
            theme::mix(roles.surface_container_high, roles.primary, 0.10)
        } else {
            roles.surface_container_high
        })
        .padding(Padding {
            top: 0.0,
            right: TILE_INSET,
            bottom: 0.0,
            left: TILE_INSET,
        })
        .align_child(Start, Center)
        .cursor(Cursor::Pointer)
        .identity(("strip tile", window.id))
        .semantics(|| {
            Semantics::new(Role::Button, "strip_tile")
                .label(&title)
                .content(window.title.as_str())
        })
        .on_click(on_left(move || click(At::Window(press))))
        .child(Row::new(row_children).gap(TILE_GAP).align(Center));

    if ring {
        card.border_if(true)
    } else if focused {
        card.border(1.0, roles.primary)
    } else if window.is_urgent {
        card.border(1.0, theme::signals().warning)
    } else {
        card
    }
}

// focuses the window and collapses the Surface if not pinned, niri asked off the draw thread
fn focus_and_close(id: u64, monitor: Option<&str>) {
    let island = IslandService::read();
    let target = monitor.or_else(|| island.expanded_on());
    let is_pinned = target.is_some_and(|m| island.pinned(m));

    if !is_pinned {
        pad::close();
    }

    // off the view thread, as niri's answer may take its patience
    let _ = std::thread::Builder::new()
        .name("strip-focus".into())
        .spawn(move || match niri::focus_window(id) {
            Ok(niri::Acted::Done) => {}
            Ok(niri::Acted::Unknown(why)) => {
                eprintln!("strip: focus {id}: no clear answer: {why}")
            }
            Err(error) => eprintln!("strip: focus {id}: {error}"),
        });
}

/*
 * a key while this island shows the Strip; false for one it does not use, as Escape, which the
 * window's own keys then get to close it. A key while the ring hides only shows it, so nothing is
 * pressed that the ring was not seen on
 */
pub fn key(monitor: &str, key: Key) -> bool {
    let niri = Niri::read();
    let workspaces = strip_workspaces(&niri.seen);
    let grid = ring_grid(&strips(&niri.seen.windows, &workspaces));

    pad::key(
        monitor,
        Surface::Strip,
        key,
        || grid.clone(),
        |focus, At::Window(id)| {
            focus_and_close(id, Some(monitor));
            focus
        },
    )
}

// the ring moved by a click lands hidden, as the pointer is what moves now
pub fn click(at: At) {
    pad::click(Surface::Strip, at, |focus, At::Window(id)| {
        focus_and_close(id, None);
        focus
    });
}

#[cfg(test)]
mod tests {
    use super::super::grid;
    use super::*;

    fn window(id: u64, workspace: u64, col: u64, row: u64, width: f64) -> StripWindow {
        StripWindow {
            id,
            title: String::new(),
            app_id: None,
            workspace_id: Some(workspace),
            is_floating: false,
            is_focused: false,
            is_urgent: false,
            col: Some(col),
            row: Some(row),
            tile_width: Some(width),
            tile_height: Some(900.0),
        }
    }

    fn info(id: u64, index: u32) -> WorkspaceInfo {
        WorkspaceInfo {
            id,
            index,
            name: None,
            output: Some("eDP-1".into()),
            is_active: index == 1,
            is_focused: index == 1,
        }
    }

    #[test]
    fn columns_hold_their_stacked_tiles_in_order() {
        let windows = vec![
            window(1, 1, 0, 1, 600.0),
            window(2, 1, 0, 0, 600.0),
            window(3, 1, 1, 0, 900.0),
        ];
        let workspaces = vec![info(1, 1)];

        let strips = strips(&windows, &workspaces);

        assert_eq!(strips.len(), 1);
        assert_eq!(strips[0].columns.len(), 2);
        assert_eq!(strips[0].columns[0].windows, vec![2, 1]);
        assert_eq!(strips[0].columns[0].width, Some(600.0));
        assert_eq!(strips[0].columns[1].windows, vec![3]);
        assert_eq!(strips[0].columns[1].width, Some(900.0));
    }

    #[test]
    fn floating_windows_are_counted_not_placed() {
        let mut floating = window(9, 1, 0, 0, 600.0);
        floating.is_floating = true;

        let windows = vec![window(1, 1, 0, 0, 600.0), floating];
        let workspaces = vec![info(1, 1)];

        let strips = strips(&windows, &workspaces);

        assert_eq!(strips[0].columns.len(), 1);
        assert_eq!(strips[0].columns[0].windows, vec![1]);
        assert_eq!(strips[0].floating, 1);
    }

    #[test]
    fn the_viewport_frames_the_focused_column_and_its_neighbors() {
        // gaps between the columns count toward the strip's width, as they are drawn
        let strip = Strip {
            workspace: 1,
            columns: vec![
                StripColumn {
                    windows: vec![1],
                    col: Some(0),
                    width: Some(960.0),
                },
                StripColumn {
                    windows: vec![2],
                    col: Some(1),
                    width: Some(1920.0),
                },
                StripColumn {
                    windows: vec![3],
                    col: Some(2),
                    width: Some(960.0),
                },
            ],
            floating: 0,
        };

        // the strip is 3852 units wide, so a 3852 px screen frames it all
        let (left, right) = viewport(&strip, Some(2), 3852.0).unwrap();
        assert!(left <= 1.0, "{left}");
        assert!(right >= 3851.0, "{right}");

        // half a strip's width of screen frames the focused column and its neighbor
        let (left, right) = viewport(&strip, Some(2), 1926.0).unwrap();
        assert!((right - left - 1926.0).abs() < 2.0, "{left} {right}");
        assert!(left > 0.0 && right < 3852.0, "{left} {right}");

        // a narrow screen still frames the focused column whole
        let (left, right) = viewport(&strip, Some(2), 300.0).unwrap();
        assert!(right - left >= 1920.0, "{left} {right}");

        // no focused window: the frame centers on the first column
        let (left, _) = viewport(&strip, None, 1926.0).unwrap();
        assert!(left <= 1.0, "{left}");

        // nothing to frame
        assert_eq!(
            viewport(
                &Strip {
                    workspace: 1,
                    columns: vec![],
                    floating: 0,
                },
                Some(2),
                1920.0,
            ),
            None
        );
    }

    #[test]
    fn the_height_follows_the_workspace_count_and_stays_in_the_body() {
        assert_eq!(height_for(1), 2.0 * INSET + WORKSPACE);
        assert_eq!(height_for(2), 2.0 * INSET + 2.0 * WORKSPACE + WORKSPACE_GAP);

        // three and more clamp at the room the largest body has
        let room = DEFAULT_LARGEST.height - 2.0 * INSET;
        assert_eq!(
            height_for(3),
            2.0 * INSET + room.min(3.0 * WORKSPACE + 2.0 * WORKSPACE_GAP)
        );
        assert!(height_for(8) <= DEFAULT_LARGEST.height);
    }

    #[test]
    fn the_ring_steps_across_columns_and_workspaces() {
        let windows = vec![
            window(1, 1, 0, 0, 600.0),
            window(2, 1, 0, 1, 600.0),
            window(3, 1, 1, 0, 900.0),
            window(4, 2, 0, 0, 600.0),
        ];
        let workspaces = vec![info(1, 1), info(2, 2)];

        let grid = ring_grid(&strips(&windows, &workspaces));

        // workspace 1: row 0 has win 1, win 3; row 1 has stacked win 2
        // workspace 2: row 2 has win 4
        assert_eq!(grid.len(), 3);
        assert_eq!(grid[0].len(), 2);
        assert_eq!(grid[1].len(), 1);
        assert_eq!(grid[2].len(), 1);

        // Left from the second column (win 3) lands on the first's (win 1)
        let place = grid::Place { row: 0, column: 1 };
        let moved = grid::moved(place, Key::Left, &grid).unwrap();
        assert_eq!(grid[moved.row][moved.column].0, At::Window(1));

        // Right comes back to win 3
        let moved = grid::moved(moved, Key::Right, &grid).unwrap();
        assert_eq!(grid[moved.row][moved.column].0, At::Window(3));

        // Down from win 1 steps to stacked win 2 in the same column
        let place_win1 = grid::Place { row: 0, column: 0 };
        let moved = grid::moved(place_win1, Key::Down, &grid).unwrap();
        assert_eq!(grid[moved.row][moved.column].0, At::Window(2));

        // Down from win 2 steps to win 4 in the next workspace
        let moved = grid::moved(moved, Key::Down, &grid).unwrap();
        assert_eq!(grid[moved.row][moved.column].0, At::Window(4));
    }

    #[test]
    fn the_workspaces_of_the_focused_output_only() {
        let mut other = info(3, 1);
        other.output = Some("HDMI-A-1".into());

        let seen = Seen {
            focused_output: Some("eDP-1".into()),
            workspaces: vec![other, info(2, 2), info(1, 1)],
            ..Seen::default()
        };

        let workspaces = strip_workspaces(&seen);

        assert_eq!(
            workspaces.iter().map(|w| w.id).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn a_hidden_ring_is_shown_by_a_key_before_any_key_presses() {
        let windows = vec![window(1, 1, 0, 0, 600.0)];
        let workspaces = vec![info(1, 1)];

        let grid = ring_grid(&strips(&windows, &workspaces));
        let hidden = Focus::<At>::default().of(1, false);

        let (shown, act) = hidden.step(Key::Enter, &grid).unwrap();

        assert_eq!(act, None);
        assert_eq!(shown.ring(&grid), Some(At::Window(1)));
        assert_eq!(
            shown.step(Key::Enter, &grid).unwrap().1,
            Some(At::Window(1))
        );
    }

    #[test]
    fn laid_never_produces_negative_widths_with_many_columns() {
        let columns: Vec<StripColumn> = (0..20)
            .map(|i| StripColumn {
                windows: vec![i],
                col: Some(i),
                width: Some(960.0),
            })
            .collect();
        let strip = Strip {
            workspace: 1,
            columns,
            floating: 0,
        };
        let laid_cols = laid(&strip);
        assert_eq!(laid_cols.len(), 20);
        for (_, width, _) in laid_cols {
            assert!(width >= MIN_COLUMN, "width {width} < MIN_COLUMN");
        }
    }

    #[test]
    fn urgent_window_indicator() {
        let mut urgent = window(10, 1, 0, 0, 600.0);
        urgent.is_urgent = true;
        let card = tile(&urgent, false, 90.0);
        assert_eq!(card.width(), kanade_runtime::Size::Parent);
    }
}
