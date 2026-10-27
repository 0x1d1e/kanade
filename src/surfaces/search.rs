//! What a Surface searched as it is typed shows: the field the query goes in and the state that
//! stands where nothing is listed. Drawn as the Clipboard and the Launcher draw their own.

use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Column, Cursor, Padding, Rectangle, Row, Size, Start, Text, Widget, children,
};

use crate::icon::Icon;
use crate::theme::space::TARGET;
use crate::theme::{self};

pub const FIELD: f32 = 44.0;
const FIELD_INSET: f32 = 16.0;
const ICON_GAP: f32 = 12.0;
const ICON_GLYPH: f32 = 16.0;

// the typed text's size, and the caret after it
const QUERY: f32 = theme::text::TITLE;
const CARET: f32 = 1.5;

/*
 * `width` across: `icon`, what is typed with the caret after it, or `placeholder` faded; a
 * query wider than the field shows its end, where the typing is
 */
pub fn field(icon: Icon, query: &str, placeholder: &str, width: f32) -> Rectangle {
    let room = width - 2.0 * FIELD_INSET - TARGET - ICON_GAP - CARET;

    let caret = Rectangle::new()
        .width(CARET)
        .height(QUERY + 4.0)
        .fill(theme::island().on_surface);

    let typed: Box<dyn Widget> = if query.is_empty() {
        Box::new(
            Row::new(children![
                caret,
                Text::new(placeholder)
                    .size(QUERY)
                    .color(theme::island().on_surface_variant)
                    .weight(theme::text::MEDIUM),
            ])
            .align(Center),
        )
    } else {
        Box::new(Row::new(children![tail(query, room), caret]).align(Center))
    };

    Rectangle::new()
        .width(width)
        .height(FIELD)
        .radius(FIELD / 2.0)
        .fill(theme::island().surface_container)
        .padding(Padding {
            top: 0.0,
            right: FIELD_INSET,
            bottom: 0.0,
            left: FIELD_INSET,
        })
        .align_child(Start, Center)
        .semantics(|| {
            Semantics::new(Role::TextInput, "search")
                .text(query)
                .focused(true)
        })
        .child(
            Row::new(vec![
                Box::new(
                    Rectangle::new()
                        .width(TARGET)
                        .height(TARGET)
                        .align_child(Center, Center)
                        .child(icon.on(ICON_GLYPH, theme::island().on_surface_variant)),
                ) as Box<dyn Widget>,
                typed,
            ])
            .gap(ICON_GAP)
            .align(Center),
        )
}

// the end of `query` that fits in `width`, an ellipsis before it when the start is cut
pub(super) fn tail(query: &str, width: f32) -> Text {
    let text = |shown: &str| {
        Text::new(shown)
            .size(QUERY)
            .color(theme::island().on_surface)
            .weight(theme::text::MEDIUM)
    };
    let fits = |text: &Text| matches!(text.width(), Size::Fixed(natural) if natural <= width);

    let whole = text(query);

    if fits(&whole) {
        return whole;
    }

    let letters: Vec<char> = query.chars().collect();
    let shown = |cut: usize| -> String {
        std::iter::once('\u{2026}')
            .chain(letters[cut..].iter().copied())
            .collect()
    };

    /*
     * each letter left out narrows the text, so the cut that fits is halved into: a probe measures
     * its candidate once, where trying every cut in turn measured the same letters again and again
     */
    let (mut low, mut high) = (1usize, letters.len());
    let mut cut = letters.len();

    while low <= high {
        let middle = low + (high - low) / 2;

        if fits(&text(&shown(middle))) {
            cut = middle;
            high = middle - 1;
        } else {
            low = middle + 1;
        }
    }

    text(&shown(cut))
}

// what it means, `width` by `height` where the list goes: its icon, a title and a detail line
pub fn state(icon: Icon, title: &str, detail: &str, width: f32, height: f32) -> Rectangle {
    state_with(icon, title, detail, None, width, height)
}

// the same, with an action under the detail, like `settings_pill`
pub fn state_with(
    icon: Icon,
    title: &str,
    detail: &str,
    action: Option<Rectangle>,
    width: f32,
    height: f32,
) -> Rectangle {
    let mut lines = children![
        icon.draw(ICON_GLYPH),
        Text::new(title)
            .size(theme::text::BODY)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD),
    ];

    if !detail.is_empty() {
        let text = Text::new(detail)
            .size(theme::text::LABEL_SMALL)
            .color(theme::island().on_surface_variant)
            .weight(theme::text::MEDIUM);

        // centred while it fits, elided only when a long one does not, since elided text fills its
        // width
        lines.push(Box::new(match text.width() {
            Size::Fixed(natural) if natural <= width => text,
            _ => text.elide(),
        }));
    }

    if let Some(action) = action {
        lines.push(Box::new(action));
    }

    Rectangle::new()
        .width(width)
        .height(height)
        .align_child(Center, Center)
        .child(Column::new(lines).gap(6.0).align(Center))
}

// the Settings page that holds the wallpaper folder, the calendars and the weather location
pub const DATA: &str = "data";

const PILL_WIDTH: f32 = 132.0;
const PILL_HEIGHT: f32 = 28.0;

/*
 * an empty state's way out: a pill that opens Settings on `page` and puts the island away. The
 * Surface presses it on Enter too, through `open_settings`
 */
pub fn settings_pill(monitor: &str, page: &'static str) -> Rectangle {
    let monitor = monitor.to_owned();

    super::pill_face(
        Rectangle::new()
            .width(PILL_WIDTH)
            .height(PILL_HEIGHT)
            .radius(PILL_HEIGHT / 2.0),
    )
    .align_child(Center, Center)
    .cursor(Cursor::Pointer)
    .semantics(|| Semantics::new(Role::Button, "settings"))
    .on_click(super::on_left(move || open_settings(&monitor, page)))
    .child(
        Text::new("Open Settings")
            .size(theme::text::LABEL_SMALL)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD),
    )
}

pub fn open_settings(monitor: &str, page: &'static str) {
    crate::settings::open(Some(page));
    crate::view::collapse(monitor);
}
