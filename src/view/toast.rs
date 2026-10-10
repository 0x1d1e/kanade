//! The notification toast forms.

use kanade_runtime::{
    Center, Column, Padding, Parent, Rectangle, Row, Stack, Start, Text, Widget, children,
};

use crate::island::activity::{Priority, Toast};
use crate::island::presentation::{Content, Presentation};
use crate::theme::{self, ThemeRoles};

use super::forms::{sized, stacked, stacked_tile};
use super::media::tile;
use super::shape::{shape, upright};

// the sender's picture, or its initial on the quiet tile
pub(crate) fn toast_tile(toast: &Toast, side: f32, radius: f32, roles: &ThemeRoles) -> Stack {
    let sender = if toast.app.is_empty() {
        &toast.summary
    } else {
        &toast.app
    };
    let initial = sender
        .chars()
        .next()
        .map_or_else(String::new, |initial| initial.to_uppercase().collect());

    tile(toast.image.as_deref(), &initial, side, radius, roles)
}

pub(super) fn critical(content: &Content) -> bool {
    content
        .activity
        .as_ref()
        .is_some_and(|activity| activity.priority() == Priority::Critical)
}

// the summary, said Critical in words first as the Notifications Surface does, never color alone
fn summary(toast: &Toast, critical: bool, size: f32, weight: u16) -> Box<dyn Widget> {
    let summary = Text::new(&toast.summary)
        .size(size)
        .color(theme::island().on_surface)
        .weight(weight)
        .elide();

    if critical {
        Box::new(
            Row::new(children![
                Text::new("Critical")
                    .size(size)
                    .color(theme::SEMANTIC.critical)
                    .weight(theme::text::SEMIBOLD),
                summary,
            ])
            .width(Parent)
            .gap(6.0),
        )
    } else {
        Box::new(summary)
    }
}

// picture, then the summary, laid out like the media Compact
pub(super) fn toast_compact(toast: &Toast, critical: bool) -> Rectangle {
    if upright() {
        let mut parts = children![toast_tile(
            toast,
            stacked_tile(),
            theme::radius::ART_COMPACT,
            &theme::island(),
        )];

        // Critical as a mark under the picture, which its Peek says in words
        if critical {
            parts.push(Box::new(
                Text::new("!")
                    .size(theme::text::LABEL)
                    .color(theme::SEMANTIC.critical)
                    .weight(theme::text::SEMIBOLD),
            ));
        }

        return stacked(parts);
    }

    let shape = shape(Presentation::Compact);
    let inset = 7.0;

    sized(Presentation::Compact)
        .padding(Padding {
            top: 0.0,
            right: 15.0,
            bottom: 0.0,
            left: inset,
        })
        .align_child(Start, Center)
        .child(
            Row::new(vec![
                Box::new(toast_tile(
                    toast,
                    shape.height - 2.0 * inset,
                    theme::radius::ART_COMPACT,
                    &theme::island(),
                )),
                summary(toast, critical, theme::text::LABEL, theme::text::MEDIUM),
            ])
            .width(Parent)
            .gap(9.0)
            .align(Center),
        )
}

// the Compact with the body under the summary, or the sender when the summary is not its name
pub(super) fn toast_peek(toast: &Toast, critical: bool) -> Rectangle {
    let shape = shape(Presentation::Peek);
    let inset = 7.0;

    let mut lines = vec![summary(
        toast,
        critical,
        theme::text::BODY,
        theme::text::SEMIBOLD,
    )];

    let second = if toast.body.is_empty() && toast.app != toast.summary {
        &toast.app
    } else {
        &toast.body
    };

    // with nothing more to say the summary centers alone
    if !second.is_empty() {
        lines.push(Box::new(
            Text::new(second)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::MEDIUM)
                .elide(),
        ));
    }

    sized(Presentation::Peek)
        .padding(Padding {
            top: 0.0,
            right: 20.0,
            bottom: 0.0,
            left: inset,
        })
        .align_child(Start, Center)
        .child(
            Row::new(children![
                toast_tile(
                    toast,
                    shape.height - 2.0 * inset,
                    theme::radius::ART_PEEK,
                    &theme::island(),
                ),
                Column::new(lines).width(Parent).gap(1.0),
            ])
            .width(Parent)
            .gap(11.0)
            .align(Center),
        )
}
