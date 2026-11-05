//! The field a password is typed into, on the Island: a dot per character typed, the caret after
//! the last one, and the lock that says what it is. Never a character of it (ADR 0146).

use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{Center, Padding, Rectangle, Row, Size, Start, Text, Widget, children};

use crate::icon::Icon;
use crate::sources::secret::Secret;
use crate::theme;
use crate::theme::space::TARGET;
use crate::theme::text;

const INSET: f32 = 16.0;
const LOCK: f32 = 16.0;
const LOCK_GAP: f32 = 8.0;
const DOTS: f32 = text::TITLE;
const CARET: f32 = 1.5;
const FIELD: f32 = 44.0;

// `secret` as the field shows it, `width` across, `placeholder` while nothing is typed
pub fn field(secret: &Secret, width: f32, placeholder: &str) -> Rectangle {
    // the caret stands at the end of what is typed, and the dots before it take what is left
    let room = width - 2.0 * INSET - TARGET - LOCK_GAP - CARET;

    let caret = Rectangle::new()
        .width(CARET)
        .height(DOTS + 4.0)
        .fill(theme::island().on_surface);

    let fits = |text: &Text| matches!(text.width(), Size::Fixed(natural) if natural <= room);

    let typed: Box<dyn Widget> = if secret.is_empty() {
        Box::new(
            Row::new(children![
                caret,
                Text::new(placeholder)
                    .size(DOTS)
                    .color(theme::island().on_surface_variant)
                    .weight(text::MEDIUM),
            ])
            .align(Center),
        )
    } else {
        let shown = dots(visible(secret.len(), fits));

        Box::new(Row::new(children![shown, caret]).align(Center))
    };

    Rectangle::new()
        .width(width)
        .height(FIELD)
        .radius(FIELD / 2.0)
        .fill(theme::island().surface_container)
        .padding(Padding {
            top: 0.0,
            right: INSET,
            bottom: 0.0,
            left: INSET,
        })
        .align_child(Start, Center)
        .semantics(|| {
            Semantics::new(Role::TextInput, "password")
                .secret()
                .focused(true)
        })
        .child(
            Row::new(vec![
                Box::new(
                    Rectangle::new()
                        .width(TARGET)
                        .height(TARGET)
                        .align_child(Center, Center)
                        .child(Icon::Lock.on(LOCK, theme::island().on_surface_variant)),
                ) as Box<dyn Widget>,
                typed,
            ])
            .gap(LOCK_GAP)
            .align(Center),
        )
}

// Auth's fixed field lane. Wi-Fi keeps its existing field and input behavior above.
pub fn auth(
    secret: &Secret,
    width: f32,
    focused: bool,
    checking: bool,
    failed: bool,
    caps: bool,
) -> Rectangle {
    let colors = theme::island();
    let caps = caps && !checking;
    let caps_width = if caps { 76.0 } else { 0.0 };
    let room = width - 32.0 - 18.0 - 12.0 - CARET - caps_width;
    let mut copy: Vec<Box<dyn Widget>> = Vec::new();
    if !checking && focused {
        copy.push(Box::new(
            Rectangle::new()
                .width(CARET)
                .height(20.0)
                .fill(colors.on_surface),
        ));
    }
    if checking || secret.is_empty() {
        copy.push(Box::new(
            Text::new(if checking {
                "Input unavailable"
            } else {
                "Password"
            })
            .size(if checking {
                text::LABEL
            } else {
                text::BODY_LARGE
            })
            .color(colors.on_surface_variant)
            .elide(),
        ));
    } else {
        let count = visible(
            secret.len(),
            |text| matches!(text.width(), Size::Fixed(n) if n <= room),
        );
        copy.clear();
        copy.push(Box::new(dots(count)));
        if focused {
            copy.push(Box::new(
                Rectangle::new()
                    .width(CARET)
                    .height(20.0)
                    .fill(colors.on_surface),
            ));
        }
    }
    let copy = Rectangle::new()
        .width(room + CARET)
        .height(24.0)
        .align_child(Start, Center)
        .clip()
        .child(Row::new(copy).gap(2.0).align(Center));
    let mut row: Vec<Box<dyn Widget>> = vec![
        Box::new(Icon::Lock.on(18.0, colors.on_surface_variant)),
        Box::new(copy),
    ];
    if caps {
        row.push(Box::new(
            Text::new("Caps Lock")
                .size(text::LABEL_SMALL)
                .color(colors.on_surface_variant),
        ));
    }
    let border = if failed {
        theme::signals().critical
    } else if focused && !checking {
        colors.on_surface
    } else {
        colors.outline
    };
    Rectangle::new()
        .width(width)
        .height(52.0)
        .radius(theme::radius::ROW)
        .fill(colors.surface_container)
        .border(if focused && !checking { 2.0 } else { 1.0 }, border)
        .padding(Padding {
            top: 0.0,
            right: 16.0,
            bottom: 0.0,
            left: 16.0,
        })
        .align_child(Start, Center)
        .semantics(move || {
            Semantics::new(Role::TextInput, "password")
                .secret()
                .focused(focused && !checking)
                .disabled(checking)
        })
        .on_click(super::on_left(|| {
            crate::sources::polkit::focus(crate::sources::polkit::Focus::Password)
        }))
        .child(Row::new(row).gap(12.0).align(Center))
}

// a dot per character typed
fn dots(count: usize) -> Text {
    Text::new("\u{2022}".repeat(count))
        .size(DOTS)
        .color(theme::island().on_surface)
        .weight(text::MEDIUM)
}

/*
 * the most of `count` dots that fit: a longer run of dots is always wider, so the count is halved
 * into rather than built and measured from the longest down, one build a letter typed
 */
fn visible(count: usize, fits: impl Fn(&Text) -> bool) -> usize {
    let (mut low, mut high) = (0usize, count);
    let mut shown = 0;

    while low <= high {
        let middle = low + (high - low) / 2;

        if fits(&dots(middle)) {
            shown = middle;
            low = middle + 1;
        } else {
            high = middle.saturating_sub(1);
        }
    }

    shown
}
