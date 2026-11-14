//! Plan utilization reads only the Activity's two optional windows.

use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Arc, Canvas, Cap, Center, Column, Parent, Rectangle, Row, Shape, Stack, Text, Widget, children,
};

use crate::island::activity::Usage;
use crate::island::presentation::Presentation;
use crate::theme;

use super::forms::sized;
use super::shape::upright;

fn value(fraction: Option<f32>) -> String {
    fraction.map_or_else(
        || String::from("N/A"),
        |fraction| format!("{:.0}%", fraction.clamp(0.0, 1.0) * 100.0),
    )
}

fn ring(fraction: Option<f32>, side: f32) -> Canvas {
    let colors = theme::island();
    let mut shapes: Vec<Box<dyn kanade_runtime::Shape>> =
        vec![Box::new(Arc::new().stroke(2.0, colors.outline))];
    if let Some(fraction) = fraction {
        let ink = if fraction >= 0.9 {
            theme::signals().critical
        } else {
            colors.on_surface
        };
        shapes.push(Box::new(
            Arc::new()
                .sweep(360.0 * fraction.clamp(0.0, 1.0))
                .stroke(2.0, ink)
                .cap(Cap::Round),
        ));
    }
    Canvas::new().width(side).height(side).shapes(shapes)
}

// Concentric rings keep both windows visible even in a Satellite's 28 px body.
pub(super) fn mark(usage: &Usage) -> Box<dyn Widget> {
    Box::new(
        Rectangle::new()
            .width(22.0)
            .height(22.0)
            .align_child(Center, Center)
            .child(Stack::new(children![
                ring(usage.five_hour, 22.0),
                Rectangle::new()
                    .width(22.0)
                    .height(22.0)
                    .align_child(Center, Center)
                    .child(ring(usage.seven_day, 14.0))
            ]))
            .semantics({
                let label = format!(
                    "Usage: 5h {}, 7d {}",
                    value(usage.five_hour),
                    value(usage.seven_day)
                );
                move || Semantics::new(Role::Region, "usage").label(&label)
            }),
    )
}

pub(super) fn form(presentation: Presentation, usage: &Usage) -> Option<Rectangle> {
    if !matches!(presentation, Presentation::Compact | Presentation::Peek) {
        return None;
    }
    let window = |label: &str, fraction, side| {
        Row::new(children![
            ring(fraction, side),
            Column::new(children![
                Text::new(label)
                    .size(theme::text::LABEL_SMALL)
                    .color(theme::island().on_surface_variant)
                    .weight(theme::text::MEDIUM),
                Text::new(value(fraction))
                    .size(theme::text::BODY_LARGE)
                    .color(theme::island().on_surface)
                    .weight(theme::text::SEMIBOLD)
            ])
            .gap(1.0)
        ])
        .gap(7.0)
        .align(Center)
    };
    if presentation == Presentation::Compact && upright() {
        let window = |label: &str, fraction| {
            Box::new(
                Column::new(children![
                    ring(fraction, 22.0),
                    Text::new(label)
                        .size(theme::text::LABEL_SMALL)
                        .color(theme::island().on_surface_variant),
                    Text::new(value(fraction))
                        .size(theme::text::LABEL_SMALL)
                        .color(theme::island().on_surface)
                        .weight(theme::text::SEMIBOLD)
                ])
                .gap(3.0)
                .align(Center),
            ) as Box<dyn Widget>
        };
        return Some(super::standing::stacked(vec![
            window("5h", usage.five_hour),
            window("7d", usage.seven_day),
        ]));
    }
    Some(
        sized(presentation)
            .padding(8.0)
            .align_child(Center, Center)
            .child(
                Row::new(children![
                    window("5h", usage.five_hour, 22.0),
                    window("7d", usage.seven_day, 22.0)
                ])
                .width(Parent)
                .gap(10.0)
                .align(Center),
            ),
    )
}

pub(super) fn notice(presentation: Presentation, line: &str) -> Option<Rectangle> {
    if !matches!(presentation, Presentation::Compact | Presentation::Peek) {
        return None;
    }
    let ink = theme::signals().critical;
    let icon = crate::icon::Icon::Pulse.on(20.0, ink);
    if presentation == Presentation::Compact && upright() {
        return Some(super::standing::stacked(children![
            icon,
            Text::new("Usage")
                .size(theme::text::LABEL_SMALL)
                .color(ink)
                .weight(theme::text::SEMIBOLD)
        ]));
    }
    Some(
        sized(presentation)
            .padding(12.0)
            .align_child(Center, Center)
            .child(
                Row::new(children![
                    icon,
                    Text::new(line)
                        .size(theme::text::LABEL)
                        .color(ink)
                        .weight(theme::text::SEMIBOLD)
                        .elide()
                ])
                .width(Parent)
                .gap(10.0)
                .align(Center),
            ),
    )
}
