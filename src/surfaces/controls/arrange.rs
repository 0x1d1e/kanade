//! Arranging the Controls tiles (ADR 0067): the pencil in the header opens a draft of
//! `controls.tiles`. Choose a tile, move it earlier or later, give it another size, hide it, or bring
//! a hidden one back; Done saves the draft, and Escape or the back chevron drops it. The draft lives
//! in the Surface's focus, so leaving the Surface drops it too. An edit that would not fit the
//! Surface's rows is not offered.

use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Column, Cursor, Padding, Rectangle, Row, SpaceBetween, Start, Text, Widget, children,
};
use toml::Value;

use super::focus::{Act, At, Focus, Subsurface};
use super::{HALO, ICON_GAP, KNOB_INSET, SWITCH, SWITCH_GAP, WIDTH, click, icon, list};
use crate::config;
use crate::icon::Icon;
use crate::reload;
use crate::settings;
use crate::surfaces::{Outline, on_left};
use crate::theme::{self, DISABLED};
use crate::tiles::{self, Placed, Size, Tile};

// the toolbar's and tray's buttons: round ones, and the words' pills
const BUTTON: f32 = 32.0;
const SIZE: f32 = 52.0;
const HIDE: f32 = 64.0;
const DONE: f32 = 56.0;
const GAP: f32 = 6.0;

// the tray's label
const HIDDEN: f32 = 52.0;

/*
 * what the toolbar offers for the chosen tile, left to right with each one's width and whether it
 * is the size the tile has: the moves and sizes and hiding that fit
 */
fn toolbar(draft: &[Placed], chosen: usize) -> Vec<(At, f32, bool)> {
    let Some(placed) = draft.get(chosen) else {
        return Vec::new();
    };

    let mut bar = Vec::new();

    if tiles::moved(draft, chosen, -1).is_some() {
        bar.push((At::Earlier, BUTTON, false));
    }

    if tiles::moved(draft, chosen, 1).is_some() {
        bar.push((At::Later, BUTTON, false));
    }

    for &size in placed.tile.sizes() {
        if size == placed.size || tiles::resized(draft, chosen, size).is_some() {
            bar.push((At::Resize(size), SIZE, size == placed.size));
        }
    }

    if tiles::removed(draft, chosen).is_some() {
        bar.push((At::Remove, HIDE, false));
    }

    bar
}

// the hidden tiles, each with whether there is room to bring it back
fn tray(draft: &[Placed]) -> Vec<(Tile, bool)> {
    tiles::missing(draft)
        .into_iter()
        .map(|tile| (tile, tiles::added(draft, tile).is_some()))
        .collect()
}

// the targets under the header: the tiles, the toolbar, and the tiles that can come back
pub fn rows(draft: &[Placed], chosen: usize) -> Vec<Vec<(At, f32)>> {
    let mut rows = Vec::new();

    for row in tiles::pack(draft) {
        let widths = tiles::widths(&row, WIDTH, SWITCH_GAP, SWITCH);
        let mut left = 0.0;
        let mut targets = Vec::new();

        for (placed, width) in row.iter().zip(widths) {
            targets.push((At::Pick(placed.tile), (left + width / 2.0) / WIDTH));
            left += width + SWITCH_GAP;
        }

        rows.push(targets);
    }

    let mut left = 0.0;
    let mut bar = Vec::new();

    for (at, width, _) in toolbar(draft, chosen) {
        bar.push((at, (left + width / 2.0) / WIDTH));
        left += width + GAP;
    }

    if !bar.is_empty() {
        rows.push(bar);
    }

    let mut left = HIDDEN + GAP;
    let mut back = Vec::new();

    for (tile, fits) in tray(draft) {
        if fits {
            back.push((At::Add(tile), (left + BUTTON / 2.0) / WIDTH));
        }

        left += BUTTON + GAP;
    }

    if !back.is_empty() {
        rows.push(back);
    }

    rows
}

pub fn view(draft: &[Placed], chosen: usize, ring: Option<&At>) -> Column {
    let on = |at: At| ring == Some(&at);

    let header = Row::new(children![
        Row::new(children![
            list::back(on(At::Back)),
            Text::new("Edit Controls")
                .size(theme::text::TITLE)
                .color(theme::island().on_surface)
                .weight(theme::text::SEMIBOLD),
        ])
        .gap(8.0)
        .align(Center),
        pill("Done", DONE, true, on(At::Done), Some(At::Done)),
    ])
    .width(WIDTH)
    .height(list::HEADER)
    .justify(SpaceBetween)
    .align(Center);

    let mut index = 0;
    let mut grid: Vec<Box<dyn Widget>> = Vec::new();

    for row in tiles::pack(draft) {
        let widths = tiles::widths(&row, WIDTH, SWITCH_GAP, SWITCH);
        let mut faces: Vec<Box<dyn Widget>> = Vec::new();

        for (placed, width) in row.iter().zip(widths) {
            faces.push(Box::new(face(
                placed,
                width,
                index == chosen,
                on(At::Pick(placed.tile)),
            )));
            index += 1;
        }

        grid.push(Box::new(Row::new(faces).gap(SWITCH_GAP)));
    }

    let bar: Vec<Box<dyn Widget>> = toolbar(draft, chosen)
        .into_iter()
        .map(|(at, width, current)| {
            let ring = on(at.clone());

            Box::new(match at {
                At::Earlier => round(Icon::Back, ring, at),
                At::Later => round(Icon::Forward, ring, at),
                At::Resize(size) => pill(label(size), width, current, ring, Some(at)),
                _ => pill("Hide", width, false, ring, Some(at)),
            }) as Box<dyn Widget>
        })
        .collect();

    let mut hidden: Vec<Box<dyn Widget>> = children![
        Text::new("Hidden")
            .size(theme::text::LABEL_SMALL)
            .color(theme::island().on_surface_variant)
            .weight(theme::text::MEDIUM)
    ];

    for (tile, fits) in tray(draft) {
        let button = round(icon(tile), on(At::Add(tile)), At::Add(tile)).semantics(|| {
            Semantics::new(Role::Button, "new")
                .label(tile.label())
                .disabled(!fits)
        });

        hidden.push(Box::new(if fits {
            button
        } else {
            button.opacity(DISABLED)
        }));
    }

    Column::new(children![
        header,
        Column::new(grid).gap(SWITCH_GAP),
        Row::new(bar).gap(GAP).height(BUTTON).align(Center),
        Row::new(hidden).gap(GAP).height(BUTTON).align(Center)
    ])
    .width(WIDTH)
    .gap(SWITCH_GAP)
}

// what `kanade ui` names a button of the arrangement
fn node(at: &At) -> &'static str {
    match at {
        At::Earlier => "earlier",
        At::Later => "later",
        At::Resize(Size::Half) => "half",
        At::Resize(Size::Full) => "full",
        At::Resize(Size::Icon) => "icon",
        At::Remove => "hide",
        At::Add(_) => "new",
        At::Done => "done",
        _ => "button",
    }
}

fn label(size: Size) -> &'static str {
    match size {
        Size::Half => "Half",
        Size::Full => "Full",
        Size::Icon => "Icon",
    }
}

// a tile as it is: the chosen one filled, the rest as a switch off
fn face(placed: &Placed, width: f32, chosen: bool, ring: bool) -> Rectangle {
    let (fill, ink) = if chosen {
        (theme::island().primary, theme::island().on_primary)
    } else {
        (
            theme::island().surface_container_high,
            theme::island().on_surface,
        )
    };

    let tile = placed.tile;
    let face = Rectangle::new()
        .width(width)
        .height(SWITCH)
        .radius(SWITCH / 2.0)
        .fill(fill)
        .border_if(ring)
        .cursor(Cursor::Pointer)
        .semantics(|| {
            Semantics::new(Role::Button, "tile")
                .label(tile.label())
                .selected(chosen)
        })
        .on_click(on_left(move || click(Act::Press(At::Pick(tile)))));

    if placed.size == Size::Icon {
        return face
            .align_child(Center, Center)
            .child(icon(tile).on(18.0, ink));
    }

    let right = 16.0;
    let words = width - (KNOB_INSET - HALO) - 18.0 - ICON_GAP - right;

    face.padding(Padding {
        top: 0.0,
        right,
        bottom: 0.0,
        left: KNOB_INSET - HALO + 7.0,
    })
    .align_child(Start, Center)
    .child(
        Row::new(children![
            icon(tile).on(18.0, ink),
            Column::new(children![
                Text::new(tile.label())
                    .size(theme::text::LABEL)
                    .color(ink)
                    .weight(theme::text::SEMIBOLD)
                    .elide()
            ])
            .width(words - 7.0),
        ])
        .gap(ICON_GAP)
        .align(Center),
    )
}

fn round(icon: Icon, ring: bool, at: At) -> Rectangle {
    Rectangle::new()
        .width(BUTTON)
        .height(BUTTON)
        .radius(BUTTON / 2.0)
        .fill(theme::island().surface_container_high)
        .align_child(Center, Center)
        .border_if(ring)
        .cursor(Cursor::Pointer)
        .semantics(|| Semantics::new(Role::Button, node(&at)))
        .on_click(on_left(move || click(Act::Press(at.clone()))))
        .child(icon.draw(16.0))
}

// words in a pill, filled while `current`
fn pill(text: &'static str, width: f32, current: bool, ring: bool, at: Option<At>) -> Rectangle {
    let (fill, ink) = if current {
        (theme::island().primary, theme::island().on_primary)
    } else {
        (
            theme::island().surface_container_high,
            theme::island().on_surface,
        )
    };

    let pill = Rectangle::new()
        .width(width)
        .height(BUTTON)
        .radius(BUTTON / 2.0)
        .fill(fill)
        .align_child(Center, Center)
        .border_if(ring)
        .child(
            Text::new(text)
                .size(theme::text::LABEL_SMALL)
                .color(ink)
                .weight(theme::text::SEMIBOLD),
        );

    match at {
        Some(at) => pill
            .cursor(Cursor::Pointer)
            .semantics(|| {
                Semantics::new(Role::Button, node(&at))
                    .label(text)
                    .selected(current)
            })
            .on_click(on_left(move || click(Act::Press(at.clone())))),
        None => pill,
    }
}

// the draft after `at` was pressed, an edit that does not apply leaving it as it is
pub fn press(focus: Focus, at: At) -> Focus {
    let Some(Subsurface::Arrange { draft, chosen }) = focus.sub.clone() else {
        return focus;
    };

    let last = |list: &[Placed]| list.len().saturating_sub(1);

    match at {
        At::Pick(tile) => match draft.iter().position(|placed| placed.tile == tile) {
            Some(at) => focus.arranged(draft, at),
            None => focus,
        },
        At::Earlier => match tiles::moved(&draft, chosen, -1) {
            Some(draft) => focus.arranged(draft, chosen - 1),
            None => focus,
        },
        At::Later => match tiles::moved(&draft, chosen, 1) {
            Some(draft) => focus.arranged(draft, chosen + 1),
            None => focus,
        },
        At::Resize(size) => match tiles::resized(&draft, chosen, size) {
            Some(draft) => focus.arranged(draft, chosen),
            None => focus,
        },
        At::Remove => match tiles::removed(&draft, chosen) {
            Some(draft) => {
                let chosen = chosen.min(last(&draft));

                focus.arranged(draft, chosen)
            }
            None => focus,
        },
        At::Add(tile) => match tiles::added(&draft, tile) {
            Some(draft) => {
                let chosen = last(&draft);

                focus.arranged(draft, chosen)
            }
            None => focus,
        },
        // a draft that did not save stays, so the layout is not left looking saved
        At::Done => match save(&draft) {
            Ok(()) => focus.out(),
            Err(why) => {
                eprintln!("kanade: controls: tiles not saved: {why}");
                focus
            }
        },
        _ => focus,
    }
}

/*
 * the draft into the settings file, or no override when it is what the files below give, which are
 * the layout without it
 */
fn save(draft: &[Placed]) -> Result<(), String> {
    if draft == config::get().tiles {
        return Ok(());
    }

    let (below, _) = config::layers(false);
    let value = (draft != below.tiles).then(|| {
        Value::Array(
            draft
                .iter()
                .map(|placed| Value::String(placed.to_string()))
                .collect(),
        )
    });

    settings::set(&["controls", "tiles"], value).map_err(|why| why.to_string())?;

    match reload::reload() {
        reload::Outcome::Valid(_) => Ok(()),
        reload::Outcome::Invalid(_) => Err(String::from(
            "saved, but the config did not reload, see `kanade status`",
        )),
    }
}
