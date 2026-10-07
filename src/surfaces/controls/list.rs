//! What the Controls Surface's listing sub-surfaces, Wi-Fi, Bluetooth and Audio, share: a header
//! with a back chevron, a title and a radio's switch, then rows that scroll under it, or what it
//! means while there are none.

use amane::{
    Center, Column, Cursor, Padding, Parent, Rectangle, Row, Scroll, SpaceBetween, Stack, Start,
    Text, Widget, children,
};

use super::WIDTH;
use super::focus::{Act, At};
use crate::icon::Icon;
use crate::island::geometry;
use crate::sources::system::Radio;
use crate::surfaces::{RING, Ring};
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, DISABLED, radius};

pub const HEADER: f32 = TARGET;
pub const GAP: f32 = 14.0;

// where the rows go
pub const LIST: f32 = geometry::CONTROLS.height - 2.0 * INSET - HEADER - GAP;

pub const ROWS: usize = 4;
pub const ROW_GAP: f32 = 6.0;
pub const ROW: f32 = (LIST - (ROWS - 1) as f32 * ROW_GAP) / ROWS as f32;
pub const ROW_INSET: f32 = 12.0;

pub const ICON: f32 = 20.0;
pub const ICON_GAP: f32 = 12.0;

// the radio's switch in the header
const TOGGLE: f32 = 40.0;
const TOGGLE_KNOB: f32 = 18.0;
const TOGGLE_INSET: f32 = (TARGET - TOGGLE_KNOB) / 2.0;
const HALO: f32 = 4.0;

const PILL: f32 = 32.0;
const PILL_PADDING: f32 = 14.0;

// pixels per wheel line
const WHEEL: f32 = 40.0;

// how far `count` rows can scroll: none while they fit
pub fn most(count: usize) -> f32 {
    (content(count) - LIST).max(0.0)
}

fn content(count: usize) -> f32 {
    count as f32 * ROW + count.saturating_sub(1) as f32 * ROW_GAP
}

fn top(row: usize) -> f32 {
    row as f32 * (ROW + ROW_GAP)
}

// the least scroll from `offset` that shows `row` of `count` whole
pub fn reveal(offset: f32, row: usize, count: usize) -> f32 {
    let top = top(row);

    offset
        .clamp(0.0, most(count))
        .min(top)
        .max(top + ROW - LIST)
}

/*
 * where `count` rows show from: `offset`, moved just enough to show the `ringed` one whole, which
 * moves in the order as rows come, go and change while the ring stays on it
 */
pub fn scrolled(offset: f32, count: usize, ringed: Option<usize>) -> f32 {
    match ringed {
        Some(row) if row < count => reveal(offset, row, count),
        _ => offset.clamp(0.0, most(count)),
    }
}

// the rows any part of shows at `offset`, as a range, so only these are built
fn shown(offset: f32, count: usize) -> (usize, usize) {
    let step = ROW + ROW_GAP;
    let first = (offset / step).floor() as usize;
    let end = ((offset + LIST) / step).ceil() as usize;

    (first.min(count), end.min(count))
}

/*
 * the back chevron, `title` and the radio's switch, if it has one, the ring on the target `ring`
 * is. Its switch presses `At::Radio`, as Enter on it does
 */
pub fn header(title: &str, radio: Option<Radio>, ring: Option<&At>) -> Row {
    let title = Row::new(children![
        back(ring == Some(&At::Back)),
        Text::new(title)
            .size(theme::text::TITLE)
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::SEMIBOLD),
    ])
    .gap(8.0)
    .align(Center);

    let mut parts = children![title];

    if let Some(radio) = radio {
        parts.push(Box::new(toggle(radio, ring == Some(&At::Radio))));
    }

    Row::new(parts)
        .width(WIDTH)
        .height(HEADER)
        .justify(SpaceBetween)
        .align(Center)
}

// pressing it goes back a level, as Escape does
pub fn back(ring: bool) -> Rectangle {
    Rectangle::new()
        .width(TARGET)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .border_if(ring)
        .on_click(super::super::on_left(|| {
            super::click(Act::Press(At::Back));
        }))
        .child(Icon::Back.draw(18.0))
}

// the radio on or off, faded while missing
fn toggle(radio: Radio, ring: bool) -> Rectangle {
    let (track, knob, at) = if radio == Radio::On {
        (
            theme::ISLAND.primary,
            theme::ISLAND.on_primary,
            TOGGLE - TARGET,
        )
    } else {
        (
            theme::ISLAND.surface_container_high,
            theme::ISLAND.on_surface_variant,
            0.0,
        )
    };

    let toggle = Rectangle::new()
        .width(TOGGLE)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .fill(track)
        .padding(TOGGLE_INSET)
        .align_child(Start, Center)
        .child(
            Rectangle::new()
                .width(TOGGLE_KNOB)
                .height(TOGGLE_KNOB)
                .radius(TOGGLE_KNOB / 2.0)
                .fill(knob)
                .translate(at, 0.0),
        );

    // the ring around it, as it would not show on a filled track
    let toggle = Rectangle::new()
        .width(TOGGLE + 2.0 * HALO)
        .height(TARGET + 2.0 * HALO)
        .radius(TARGET / 2.0 + HALO)
        .align_child(Center, Center)
        .border_if(ring)
        .child(toggle);

    if radio == Radio::Missing {
        return toggle.opacity(DISABLED);
    }

    toggle
        .cursor(Cursor::Pointer)
        .on_click(super::super::on_left(|| {
            super::click(Act::Press(At::Radio));
        }))
}

// what it means, in the middle of where the rows go
pub fn state(icon: Icon, title: &str, detail: &str) -> Rectangle {
    let mut lines = children![
        icon.on(28.0, theme::ISLAND.on_surface_variant),
        Text::new(title)
            .size(theme::text::BODY)
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::SEMIBOLD),
    ];

    if !detail.is_empty() {
        lines.push(Box::new(
            Text::new(detail)
                .size(theme::text::LABEL_SMALL)
                .color(theme::ISLAND.on_surface_variant)
                .weight(theme::text::MEDIUM),
        ));
    }

    Rectangle::new()
        .width(WIDTH)
        .height(LIST)
        .align_child(Center, Center)
        .child(Column::new(lines).gap(6.0).align(Center))
}

/*
 * `count` rows scrolled `offset` down, clipped to the list, each built by `row` only while it
 * shows; a thumb in the inset says where while they do not all fit. The wheel scrolls them
 */
pub fn list(count: usize, offset: f32, row: impl Fn(usize) -> Box<dyn Widget>) -> Stack {
    scrolling(count, offset, super::scroll, row)
}

// `list`, the wheel scrolling them by `scroll`, in pixels, down further down
pub fn scrolling(
    count: usize,
    offset: f32,
    scroll: fn(f32),
    row: impl Fn(usize) -> Box<dyn Widget>,
) -> Stack {
    let (first, end) = shown(offset, count);

    let column = Column::new((first..end).map(row).collect())
        .width(WIDTH)
        .gap(ROW_GAP);

    let viewport = Rectangle::new()
        .width(WIDTH)
        .height(LIST)
        .clip()
        .align_child(Start, Start)
        .on_scroll(move |Scroll { y, .. }| scroll(y * WHEEL))
        .child(
            Rectangle::new()
                .width(WIDTH)
                .height(content(end - first))
                .align_child(Start, Start)
                .translate(0.0, top(first) - offset)
                .child(column),
        );

    let mut layers = children![viewport];

    let most = most(count);

    if most > 0.0 {
        let length = (LIST * LIST / content(count)).max(TARGET);
        let at = (LIST - length) * offset / most;

        layers.push(Box::new(
            Rectangle::new()
                .width(3.0)
                .height(length)
                .radius(radius::HAIRLINE)
                .fill(theme::ISLAND.surface_container_high)
                .translate(WIDTH + 7.0, at),
        ));
    }

    Stack::new(layers).width(WIDTH).height(LIST)
}

/*
 * one row: `leading`, its name with `status` under it, red when it is an error, then `trailing`,
 * filled while the ring is on it. Pressing it presses `press`, none leaving it no target
 */
pub fn row(
    leading: Box<dyn Widget>,
    name: &str,
    status: Option<(&str, bool)>,
    trailing: Vec<Box<dyn Widget>>,
    ring: bool,
    press: Option<At>,
) -> Rectangle {
    let mut lines = children![
        Text::new(name)
            .size(theme::text::BODY)
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::SEMIBOLD)
            .elide()
    ];

    if let Some((status, error)) = status {
        lines.push(Box::new(
            Text::new(status)
                .size(theme::text::LABEL_SMALL)
                .color(if error {
                    theme::SEMANTIC.critical
                } else {
                    theme::ISLAND.on_surface_variant
                })
                .weight(theme::text::MEDIUM)
                .elide(),
        ));
    }

    let row = frame(
        Row::new(vec![
            leading,
            Box::new(Column::new(lines).width(Parent).gap(1.0)),
            Box::new(Row::new(trailing).gap(ICON_GAP).align(Center)),
        ])
        .width(Parent)
        .gap(ICON_GAP)
        .align(Center),
        ring,
    );

    let Some(at) = press else {
        return row;
    };

    row.cursor(Cursor::Pointer)
        .on_click(super::super::on_left(move || {
            super::click(Act::Press(at.clone()));
        }))
}

// a row's shape around `content`, filled while the ring is on it
pub fn frame(content: Row, ring: bool) -> Rectangle {
    let row = Rectangle::new()
        .width(WIDTH)
        .height(ROW)
        .radius(radius::ROW)
        .padding(Padding {
            top: 0.0,
            right: ROW_INSET,
            bottom: 0.0,
            left: ROW_INSET,
        })
        .align_child(Start, Center)
        .child(content);

    if ring {
        row.fill(theme::ISLAND.surface_container).border_if(true)
    } else {
        row
    }
}

// a pill's words
pub fn label(text: &str) -> Text {
    Text::new(text)
        .size(theme::text::LABEL_SMALL)
        .color(theme::ISLAND.on_surface)
        .weight(theme::text::SEMIBOLD)
}

// outlined, the ring in place of the outline; none to press leaves it faded and not pressable
pub fn pill(child: Text, width: f32, ring: bool, act: Option<Act>) -> Rectangle {
    let pill = Rectangle::new()
        .width(width)
        .height(PILL)
        .radius(PILL / 2.0)
        .padding(Padding {
            top: 0.0,
            right: PILL_PADDING,
            bottom: 0.0,
            left: PILL_PADDING,
        })
        .align_child(Center, Center)
        .child(child);

    let pill = if ring {
        pill.border(RING, theme::ISLAND.on_surface)
    } else {
        pill.border(1.0, theme::ISLAND.surface_container_high)
    };

    match act {
        Some(act) => pill
            .cursor(Cursor::Pointer)
            .on_click(super::super::on_left(move || {
                super::click(act.clone());
            })),
        None => pill.opacity(DISABLED),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_rows_fit_and_more_scroll() {
        assert_eq!(most(ROWS), 0.0);
        assert!(most(ROWS + 1) > 0.0);
    }

    #[test]
    fn revealing_a_row_scrolls_the_least() {
        // already showing, nothing moves
        assert_eq!(reveal(0.0, 1, 10), 0.0);

        // below, it comes up to the bottom edge
        assert_eq!(reveal(0.0, ROWS, 10), top(ROWS) + ROW - LIST);

        // above, it comes down to the top edge
        assert_eq!(reveal(most(10), 0, 10), 0.0);
    }

    #[test]
    fn the_list_follows_the_ringed_row() {
        // the ring on the eighth, out of sight, brings it up
        assert_eq!(scrolled(0.0, 10, Some(7)), reveal(0.0, 7, 10));

        // with no ring on a row, the list stays where it was, as far as it can
        assert_eq!(scrolled(40.0, 10, None), 40.0);
        assert_eq!(scrolled(1e6, 10, None), most(10));
        assert_eq!(scrolled(40.0, 2, Some(1)), 0.0);
    }
}
