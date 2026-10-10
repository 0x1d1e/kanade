//! The Island at Rest, its time Peek and the Tray strip's lead.

use kanade_runtime::service::Service;
use kanade_runtime::{
    Center, Column, Padding, Parent, Rectangle, Row, Start, Text, Widget, children,
};

use crate::clock;
use crate::config;
use crate::island::activity::Activity;
use crate::island::geometry;
use crate::island::presentation::{Content, Presentation};
use crate::modules;
use crate::sources::battery::BatteryLevel;
use crate::sources::calendar;
use crate::sources::weather::{self, Weather};
use crate::surfaces;
use crate::theme;

use super::forms::sized;
use super::satellites::abbreviation;
use super::shape::{peek, upright};
use super::status::battery_icon;

/*
 * the local time, as this monitor's `clock` reads it, if `rest.clock` shows it, then with
 * `rest.battery` the battery's percent; an Activity's small form crossfades over it, and only a
 * window that draws it redraws when the minute turns. With neither it draws nothing, and the
 * Island hides (`bare`)
 */
pub(super) fn rest(monitor: &str, content: &Content) -> Option<Rectangle> {
    if content.presentation != Presentation::Rest {
        return None;
    }

    let config = config::on(monitor);

    // reading the level only when shown keeps the Rest of one without it from waking on a change
    let level = (config.rest.battery && modules::on("battery"))
        .then(|| *BatteryLevel::read())
        .and_then(|level| level.percent);

    if upright() {
        return Some(rest_upright(monitor, level));
    }

    let mut items: Vec<Box<dyn Widget>> = Vec::new();

    if config.rest.clock {
        items.push(Box::new(
            Text::new(clock::now(config.clock))
                .size(theme::text::LABEL)
                .color(theme::island().on_surface)
                .weight(theme::text::SEMIBOLD),
        ));
    }

    if let Some(percent) = level {
        items.push(Box::new(battery_readout(percent)));
    }

    Some(
        sized(Presentation::Rest)
            .align_child(Center, Center)
            .child(Row::new(items).gap(8.0).align(Center)),
    )
}

// the battery's percent and its icon, in the Island's quiet tone
fn battery_readout(percent: u8) -> Row {
    let tone = theme::island().on_surface_variant;

    Row::new(children![
        Text::new(format!("{percent}%"))
            .size(theme::text::LABEL_SMALL)
            .color(tone)
            .weight(theme::text::SEMIBOLD),
        battery_icon(20.0, percent, tone),
    ])
    .gap(4.0)
    .align(Center)
}

/*
 * whether Rest on `monitor` has nothing to show, no clock and no battery: the Island then slides
 * past its edge when idle, as `island.autohide` does, and comes back as the pointer touches it
 */
pub(super) fn bare(monitor: &str) -> bool {
    config::on(monitor).rest.bare(has_battery())
}

pub(super) fn has_battery() -> bool {
    modules::on("battery") && BatteryLevel::read().present()
}

// Rest standing along a side edge: the hours over the minutes, then the battery under them
fn rest_upright(monitor: &str, level: Option<u8>) -> Rectangle {
    let mut items: Vec<Box<dyn Widget>> = Vec::new();

    if config::on(monitor).rest.clock {
        items.push(Box::new(stacked_time(monitor, theme::text::LABEL)));
    }

    if let Some(percent) = level {
        items.push(Box::new(battery_stacked(percent)));
    }

    sized(Presentation::Rest)
        .align_child(Center, Center)
        .child(Column::new(items).gap(10.0).align(Center))
}

// the battery's icon over its percent
fn battery_stacked(percent: u8) -> Column {
    let tone = theme::island().on_surface_variant;

    Column::new(children![
        battery_icon(20.0, percent, tone),
        Text::new(percent.to_string())
            .size(theme::text::LABEL_SMALL)
            .color(tone)
            .weight(theme::text::SEMIBOLD),
    ])
    .gap(3.0)
    .align(Center)
}

// the local time a part a line, the hours over the minutes and any AM or PM
fn stacked_time(monitor: &str, size: f32) -> Column {
    let time = clock::now(config::on(monitor).clock);
    let parts = time
        .split([':', ' '])
        .map(|part| {
            Box::new(
                Text::new(part)
                    .size(size)
                    .color(theme::island().on_surface)
                    .weight(theme::text::SEMIBOLD),
            ) as Box<dyn Widget>
        })
        .collect();

    Column::new(parts).align(Center)
}

/*
 * the time's Peek, which leads the Tray strip: the time, with the battery's percent as the settings
 * ask, over the date and, as they ask, the weather and today's next event. An autohidden Island
 * shows this as it comes out, so the battery is seen here. Each source is read only when shown, so
 * an island without it never wakes on its changes. `slots` says whether slots follow, which sets
 * the room after it
 */
pub(crate) fn time_peek(monitor: &str, slots: bool) -> Rectangle {
    let config = config::on(monitor);
    let island = theme::island();
    let today = clock::today();

    let mut line = children![
        Text::new(today.format("%a %-d %b").to_string())
            .size(theme::text::LABEL_SMALL)
            .color(island.on_surface_variant)
            .weight(theme::text::MEDIUM),
    ];

    if config.rest.weather && modules::on("weather") {
        let weather = Weather::read();

        if let Some(current) = weather.forecast.as_ref().map(|forecast| &forecast.current) {
            line.push(Box::new(
                Row::new(children![
                    surfaces::weather::icon(current.condition, current.day)
                        .on(12.0, island.on_surface_variant),
                    Text::new(config.units.temperature(current.temperature))
                        .size(theme::text::LABEL_SMALL)
                        .color(island.on_surface_variant)
                        .weight(theme::text::MEDIUM),
                ])
                .gap(3.0)
                .align(Center),
            ));
        }
    }

    // the next of today's timed events not over yet, the one under way included
    if config.rest.agenda && modules::on("calendar") {
        let now = weather::now();
        let events = calendar::upcoming(today);

        if let Some(next) = events
            .iter()
            .find(|event| !event.all_day && event.span.1.max(event.span.0 + 1) > now)
        {
            line.push(Box::new(
                Text::new(format!(
                    "{} {}",
                    clock::time(next.start.time(), config.clock),
                    next.title
                ))
                .size(theme::text::LABEL_SMALL)
                .color(island.on_surface)
                .weight(theme::text::MEDIUM)
                .elide(),
            ));
        }
    }

    let mut head = children![
        Text::new(clock::now(config.clock))
            .size(theme::text::LABEL)
            .color(island.on_surface)
            .weight(theme::text::SEMIBOLD),
    ];

    let level = (config.rest.peek_battery && modules::on("battery"))
        .then(|| *BatteryLevel::read())
        .and_then(|level| level.percent);

    if let Some(percent) = level {
        head.push(Box::new(battery_readout(percent)));
    }

    // the room after it holds the slots, or ends the strip as round as it began
    let after = if slots {
        geometry::TIME_GAP
    } else {
        geometry::TIME_INSET
    };

    Rectangle::new()
        .width(geometry::TIME_INSET + peek().width() + after)
        .height(Parent)
        .padding(Padding {
            left: geometry::TIME_INSET,
            right: after,
            ..Padding::default()
        })
        .align_child(Start, Center)
        .child(
            Column::new(children![
                Row::new(head).gap(geometry::HEAD_GAP).align(Center),
                Row::new(line).gap(geometry::LINE_GAP).align(Center),
            ])
            .gap(2.0),
        )
}

/*
 * the time's Peek standing along a side edge, which leads the upright Tray strip: the hours over
 * the minutes, then the weather and the battery as the settings ask
 */
pub(crate) fn time_peek_upright(monitor: &str) -> Rectangle {
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

    let level = (config.rest.peek_battery && modules::on("battery"))
        .then(|| *BatteryLevel::read())
        .and_then(|level| level.percent);

    if let Some(percent) = level {
        parts.push(Box::new(battery_stacked(percent)));
    }

    Rectangle::new()
        .width(Parent)
        .height(peek().height())
        .align_child(Center, Center)
        .child(Column::new(parts).gap(6.0).align(Center))
}

/*
 * stand-in content until the sources and the Surfaces draw their own (#21-#33): names the Activity
 * a small form shows, or the open Surface, so the crossfade and the clipping can be seen. Short,
 * so sized to its letters and centered
 */
pub(super) fn placeholder(content: &Content) -> Option<Rectangle> {
    let name = |activity: &Option<Activity>| {
        activity.as_ref().map_or_else(String::new, |activity| {
            format!("{} {}", activity.kind().name(), activity.id().key())
        })
    };

    let (label, size) = match content.presentation {
        Presentation::Rest | Presentation::Tray(_) => return None,
        // an upright Compact has room for the Kind's letters only
        Presentation::Compact if upright() => (
            content
                .activity
                .as_ref()
                .map_or("", |activity| abbreviation(activity.kind()))
                .to_owned(),
            theme::text::LABEL,
        ),
        Presentation::Compact => (name(&content.activity), theme::text::LABEL),
        // `split` draws both segments, each its own form
        Presentation::Split => return None,
        Presentation::Peek => (name(&content.activity), theme::text::BODY_LARGE),
        Presentation::Expanded(surface) => (format!("{surface:?}"), theme::text::TITLE_LARGE),
    };

    Some(
        sized(content.presentation)
            .align_child(Center, Center)
            .child(
                Text::new(label)
                    .size(size)
                    .color(theme::island().on_surface)
                    .weight(theme::text::MEDIUM),
            ),
    )
}
