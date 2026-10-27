//! The forms standing along a side edge, where the island hangs left or right: Rest, the Compact
//! of each Kind and the Tray strip's lead. Their bodies are short, so each is a column of marks, a
//! part a line, and no words, which wait for the Peek. Their lengths are `geometry`'s `upright`
//! constants. A form's horizontal layout lives with its Kind.

use std::time::Instant;

use kanade_runtime::service::Service;
use kanade_runtime::{
    Center, Color, Column, End, Justify, Padding, Parent, Rectangle, Start, Text, Widget, children,
};

use crate::clock;
use crate::config;
use crate::icon::Icon;
use crate::island::activity::{self, Countdown, Workspace};
use crate::island::geometry;
use crate::island::presentation::Presentation;
use crate::modules;
use crate::sources::timer;
use crate::sources::weather::Weather;
use crate::surfaces;
use crate::theme;

use super::forms::sized;
use super::rest::{Charge, overflows, shown};
use super::shape::{peek, shape};
use super::status::{battery_icon, charge_tone, timer_tone};

/*
 * a Compact standing along a side edge: its parts down its length from the round end, centered
 * across it, a lead tile concentric with the end. Its words wait for the Peek
 */
pub(super) fn stacked(parts: Vec<Box<dyn Widget>>) -> Rectangle {
    // a lone part sits in the middle of the pill rather than leaving the rest of it empty
    let justify = if parts.len() == 1 {
        Justify::Center
    } else {
        Justify::SpaceBetween
    };

    sized(Presentation::Compact)
        .padding(Padding {
            top: STACK_INSET,
            bottom: STACK_INSET + 5.0,
            ..Padding::default()
        })
        .align_child(Center, Start)
        .child(
            Column::new(parts)
                .height(Parent)
                .justify(justify)
                .align(Center),
        )
}

// a lead tile's inset from a small form's round end
const STACK_INSET: f32 = 7.0;

// the side of a lead tile in an upright Compact
pub(super) fn stacked_tile() -> f32 {
    shape(Presentation::Compact).width - 2.0 * STACK_INSET
}

/*
 * the stopwatch at the round end, the countdown at the other, a part a line, the minutes over the
 * seconds, as the clock
 */
pub(super) fn timer(countdown: &Countdown, now: Instant) -> Rectangle {
    let mut clock: Vec<Box<dyn Widget>> = Vec::new();

    for part in timer::clock(countdown, now).split(':') {
        clock.push(Box::new(
            Text::new(part)
                .tabular()
                .size(theme::text::LABEL)
                .color(timer_tone(countdown))
                .weight(theme::text::SEMIBOLD),
        ));
    }

    stacked(children![
        Icon::Stopwatch.draw(18.0),
        Column::new(clock).gap(3.0).align(Center),
    ])
}

// the battery's icon over its percent
pub(super) fn battery(charge: &activity::Charge) -> Rectangle {
    let tone = charge_tone(charge);

    stacked(children![
        battery_icon(22.0, charge.percent, tone),
        Text::new(format!("{}%", charge.percent))
            .tabular()
            .size(theme::text::LABEL_SMALL)
            .color(tone)
            .weight(theme::text::SEMIBOLD),
    ])
}

// the most workspaces a pager down a Compact holds
const PAGER: u32 = 8;

// the pager alone, in the middle; its number and "of N" past what it fits
pub(super) fn workspace(workspace: &Workspace) -> Rectangle {
    let dot = 6.0;
    let mut parts: Vec<Box<dyn Widget>> = Vec::new();

    let number = |text: String, tone: Color| {
        Text::new(text)
            .size(theme::text::LABEL)
            .color(tone)
            .weight(theme::text::SEMIBOLD)
    };

    if workspace.count > PAGER {
        parts.push(Box::new(number(
            workspace.index.to_string(),
            theme::island().on_surface,
        )));
        parts.push(Box::new(
            Text::new(format!("of {}", workspace.count))
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::SEMIBOLD),
        ));

        return stacked(parts);
    }

    let (mark, gap) = (dot * 8.0 / 3.0, dot * 5.0 / 6.0);
    let dots = (1..=workspace.count)
        .map(|index| {
            let focused = index == workspace.index;

            Box::new(
                Rectangle::new()
                    .width(dot)
                    .height(if focused { mark } else { dot })
                    .radius(dot / 2.0)
                    .fill(if focused {
                        theme::island().on_surface
                    } else {
                        theme::island().on_surface_variant
                    }),
            ) as Box<dyn Widget>
        })
        .collect();

    parts.push(Box::new(Column::new(dots).gap(gap).align(Center)));

    stacked(parts)
}

// the icon, a bar rising with the level, then its number
pub(super) fn level(icon: Icon, percent: u8, tone: Color) -> Rectangle {
    let length = shape(Presentation::Compact).height - 100.0;

    stacked(children![
        icon.draw(20.0),
        rising(length, f32::from(percent) / 100.0, tone),
        Text::new(percent.to_string())
            .tabular()
            .size(theme::text::LABEL_SMALL)
            .color(tone)
            .weight(theme::text::SEMIBOLD),
    ])
}

// `bar` standing, filled from the bottom, for a level along a side edge
fn rising(length: f32, fraction: f32, tone: Color) -> Rectangle {
    let width = 6.0;
    let filled = length * fraction.clamp(0.0, 1.0);

    let track = Rectangle::new()
        .width(width)
        .height(length)
        .radius(width / 2.0)
        .fill(theme::island().surface_container_high)
        .align_child(Center, End);

    // shorter than its round ends it would draw as a misshapen dot
    if filled >= width {
        track.child(
            Rectangle::new()
                .width(width)
                .height(filled)
                .radius(width / 2.0)
                .fill(tone),
        )
    } else {
        track
    }
}

// Rest standing along a side edge: the hours over the minutes, then the battery under them
pub(super) fn rest(monitor: &str, level: Option<Charge>) -> Rectangle {
    let column = |level: Option<Charge>| {
        let mut items: Vec<Box<dyn Widget>> = Vec::new();

        if config::on(monitor).rest.clock {
            items.push(Box::new(stacked_time(monitor, theme::text::LABEL)));
        }

        if let Some(charge) = level {
            items.push(Box::new(battery_stack(charge)));
        }

        Column::new(items).gap(8.0).align(Center)
    };

    // as along the top: the battery goes first where the dots leave the rest no room
    let full = column(level);
    let room = shape(Presentation::Rest).height - 2.0 * geometry::REST_MARGIN;
    let column = if level.is_some() && overflows(Widget::height(&full), room) {
        column(None)
    } else {
        full
    };

    sized(Presentation::Rest)
        .align_child(Center, Center)
        .child(column)
}

// the battery's icon over its percent, under a bolt while it charges
fn battery_stack(charge: Charge) -> Column {
    let tone = theme::island().on_surface_variant;
    let mut parts: Vec<Box<dyn Widget>> = Vec::new();

    if charge.charging {
        parts.push(Box::new(Icon::Bolt.on(geometry::BOLT, tone)));
    }

    parts.push(Box::new(battery_icon(20.0, charge.percent, tone)));
    parts.push(Box::new(
        Text::new(format!("{}%", charge.percent))
            .tabular()
            .size(theme::text::LABEL_SMALL)
            .color(tone)
            .weight(theme::text::SEMIBOLD),
    ));

    Column::new(parts).gap(3.0).align(Center)
}

// the local time a part a line, the hours over the minutes and any AM or PM; a digit alone gets a 0
fn stacked_time(monitor: &str, size: f32) -> Column {
    let time = clock::now(config::on(monitor).clock);
    let parts = time
        .split([':', ' '])
        .map(|part| {
            Box::new(
                Text::new(format!("{part:0>2}"))
                    .tabular()
                    .size(size)
                    .color(theme::island().on_surface)
                    .weight(theme::text::SEMIBOLD),
            ) as Box<dyn Widget>
        })
        .collect();

    Column::new(parts).align(Center)
}

/*
 * the time's Peek standing along a side edge, which leads the upright Tray strip: the hours over
 * the minutes, then the weather and the battery as the settings ask, and a line before the slots
 */
pub(crate) fn time_peek(monitor: &str, slots: bool) -> Rectangle {
    let config = config::on(monitor);
    let island = theme::island();
    let mut parts = children![stacked_time(monitor, theme::text::BODY)];

    let weather = (config.rest.weather && modules::on("weather")).then(Weather::read);

    if let Some(current) = weather
        .as_ref()
        .and_then(|weather| weather.forecast.as_ref())
        .map(|forecast| &forecast.current)
    {
        parts.push(Box::new(
            Column::new(children![
                surfaces::weather::icon(current.condition, current.day)
                    .on(14.0, island.on_surface_variant),
                Text::new(config.units.temperature(current.temperature))
                    .size(theme::text::LABEL_SMALL)
                    .color(island.on_surface_variant)
                    .weight(theme::text::MEDIUM),
            ])
            .gap(2.0)
            .align(Center),
        ));
    }

    let level = shown(config.rest.peek_battery);

    if let Some(charge) = level {
        parts.push(Box::new(battery_stack(charge)));
    }

    let content = Rectangle::new()
        .width(Parent)
        .height(peek().height())
        .align_child(Center, Center)
        .child(Column::new(parts).gap(6.0).align(Center));

    if !slots {
        return content;
    }

    // the line sits at the lead's foot, just before the first slot
    let line = Rectangle::new()
        .width(20.0)
        .height(1.0)
        .fill(island.on_surface_variant)
        .opacity(0.4);

    Rectangle::new()
        .width(Parent)
        .height(peek().height() + geometry::UPRIGHT_DIVIDER)
        .align_child(Center, Start)
        .child(
            Column::new(vec![Box::new(content) as Box<dyn Widget>, Box::new(line)])
                .gap((geometry::UPRIGHT_DIVIDER - 1.0) / 2.0)
                .align(Center),
        )
}
