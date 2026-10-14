//! The Island at Rest, its time Peek and the Tray strip's lead.

use std::collections::HashMap;

use chrono::NaiveDate;
use kanade_runtime::service::Service;
use kanade_runtime::{
    Center, Column, Monitor, Monitors, Padding, Parent, Rectangle, Row, Size, Start, Text, Widget,
    children,
};

use crate::clock::{self, Hours};
use crate::config::{self, Config};
use crate::icon::Icon;
use crate::island::activity::Activity;
use crate::island::geometry::{self, Shape};
use crate::island::presentation::{Content, Presentation};
use crate::modules;
use crate::sources::battery::BatteryLevel;
use crate::sources::calendar;
use crate::sources::weather::Weather;
use crate::surfaces;
use crate::theme;

use super::forms::sized;
use super::satellites::abbreviation;
use super::shape::{peek, shape, upright};
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
    let level = shown(config.rest.battery);

    if upright() {
        return Some(rest_upright(monitor, level));
    }

    let row = |level: Option<Charge>| {
        let mut items: Vec<Box<dyn Widget>> = Vec::new();

        if config.rest.clock {
            items.push(Box::new(
                Text::new(clock::now(config.clock))
                    .size(theme::text::LABEL)
                    .color(theme::island().on_surface)
                    .weight(theme::text::SEMIBOLD),
            ));
        }

        if let Some(charge) = level {
            items.push(Box::new(battery_readout(charge)));
        }

        Row::new(items).gap(8.0).align(Center)
    };

    // the dots take room from the form: the battery is the first to go where the rest no longer fits
    let full = row(level);
    let room = shape(Presentation::Rest).width - 2.0 * geometry::REST_MARGIN;
    let row = if level.is_some() && overflows(Widget::width(&full), room) {
        row(None)
    } else {
        full
    };

    Some(
        sized(Presentation::Rest)
            .align_child(Center, Center)
            .child(row),
    )
}

// whether a laid-out length, as the widgets know it, is longer than the `room` it goes in
fn overflows(length: Size, room: f32) -> bool {
    matches!(length, Size::Fixed(length) if length > room)
}

// the battery as Rest and its Peek show it
#[derive(Clone, Copy)]
struct Charge {
    percent: u8,

    // on the mains, so a bolt comes before the percent
    charging: bool,
}

// the battery as `enabled` asks and the machine has; reading it only then keeps a Rest without it from waking on a change
fn shown(enabled: bool) -> Option<Charge> {
    let level = (enabled && modules::on("battery")).then(|| *BatteryLevel::read())?;

    Some(Charge {
        percent: level.percent?,
        charging: level.charging,
    })
}

// the battery's percent and its icon, in the Island's quiet tone, after a bolt while it charges
fn battery_readout(charge: Charge) -> Row {
    let tone = theme::island().on_surface_variant;
    let mut parts: Vec<Box<dyn Widget>> = Vec::new();

    if charge.charging {
        parts.push(Box::new(Icon::Bolt.on(geometry::BOLT, tone)));
    }

    parts.push(Box::new(
        Text::new(format!("{}%", charge.percent))
            .size(theme::text::LABEL_SMALL)
            .color(tone)
            .weight(theme::text::SEMIBOLD),
    ));
    parts.push(Box::new(battery_icon(20.0, charge.percent, tone)));

    Row::new(parts).gap(4.0).align(Center)
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
fn rest_upright(monitor: &str, level: Option<Charge>) -> Rectangle {
    let column = |level: Option<Charge>| {
        let mut items: Vec<Box<dyn Widget>> = Vec::new();

        if config::on(monitor).rest.clock {
            items.push(Box::new(stacked_time(monitor, theme::text::LABEL)));
        }

        if let Some(charge) = level {
            items.push(Box::new(battery_stacked(charge)));
        }

        Column::new(items).gap(10.0).align(Center)
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
fn battery_stacked(charge: Charge) -> Column {
    let tone = theme::island().on_surface_variant;
    let mut parts: Vec<Box<dyn Widget>> = Vec::new();

    if charge.charging {
        parts.push(Box::new(Icon::Bolt.on(geometry::BOLT, tone)));
    }

    parts.push(Box::new(battery_icon(20.0, charge.percent, tone)));
    parts.push(Box::new(
        Text::new(format!("{}%", charge.percent))
            .size(theme::text::LABEL_SMALL)
            .color(tone)
            .weight(theme::text::SEMIBOLD),
    ));

    Column::new(parts).gap(3.0).align(Center)
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

    if config.rest.agenda
        && modules::on("calendar")
        && let Some(next) = next_event(today, clock::unix_seconds(), config.clock)
    {
        line.push(Box::new(agenda_line(next).color(island.on_surface).elide()));
    }

    let mut head = children![
        Text::new(clock::now(config.clock))
            .size(theme::text::LABEL)
            .color(island.on_surface)
            .weight(theme::text::SEMIBOLD),
    ];

    let level = shown(config.rest.peek_battery);

    if let Some(charge) = level {
        head.push(Box::new(battery_readout(charge)));
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

// the next of today's timed events not over yet, the one under way included: its start, then its title
fn next_event(today: NaiveDate, now: i64, hours: Hours) -> Option<String> {
    next_event_in(&calendar::upcoming(today), now, hours)
}

fn next_event_in(events: &[calendar::Occurrence], now: i64, hours: Hours) -> Option<String> {
    events
        .iter()
        .find(|event| !event.all_day && event.span.1.max(event.span.0 + 1) > now)
        .map(|next| format!("{} {}", clock::time(next.start.time(), hours), next.title))
}

fn agenda_line(label: String) -> Text {
    Text::new(label)
        .size(theme::text::LABEL_SMALL)
        .weight(theme::text::MEDIUM)
}

/*
 * the width the time's Peek gives today's next event on each output showing it: its Tray strip is
 * as wide as its lead needs. Written by `measure_agenda`, which the Island's sizes follow, never
 * by a view
 */
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Agenda(HashMap<String, u16>);

impl Agenda {
    // the width on this output, none for no event or the setting off
    pub fn on(&self, monitor: &str) -> u16 {
        self.0.get(monitor).copied().unwrap_or(0)
    }
}

impl Service for Agenda {
    fn new() -> Self {
        Agenda::default()
    }

    fn listen() {}
}

/*
 * measures the next event's line on each output that shows it, to the most the strip gives it in
 * the largest body that output holds (`fits`). It arms the clock's minute timer when a connected
 * output shows the agenda, even if no window draws the time, so an ended event stops taking room
 */
pub(crate) fn measure_agenda(fits: &HashMap<String, Shape>) {
    let now = clock::unix_seconds();
    let config = config::get();
    let monitors = Monitors::read();
    let agenda_enabled = wants_agenda_clock(modules::on("calendar"), &config, monitors.all());
    let today = if agenda_enabled {
        // Agenda width depends on time even when no Rest or Tray currently draws the clock.
        let _ = clock::now(config.clock);
        clock::local_time(now).map(|time| time.date())
    } else {
        None
    };
    let events = today.map(calendar::upcoming);
    let widths = events.as_deref().map_or_else(HashMap::new, |events| {
        agenda_widths(monitors.all(), &config, fits, now, events)
    });

    let next = Agenda(widths);

    if *Agenda::read() != next {
        *Agenda::write() = next;
    }
}

fn wants_agenda_clock(calendar: bool, config: &Config, monitors: &[Monitor]) -> bool {
    calendar
        && monitors
            .iter()
            .any(|monitor| config.on(&monitor.name).rest.agenda)
}

fn agenda_widths(
    monitors: &[Monitor],
    config: &Config,
    fits: &HashMap<String, Shape>,
    now: i64,
    events: &[calendar::Occurrence],
) -> HashMap<String, u16> {
    monitors
        .iter()
        .filter_map(|monitor| {
            let on = config.on(&monitor.name);
            let label = next_event_in(events, now, on.clock).filter(|_| on.rest.agenda)?;
            let most = geometry::agenda_most(fits.get(&monitor.name)?.width);

            match agenda_line(label).width() {
                Size::Fixed(width) => Some((monitor.name.clone(), (width.ceil() as u16).min(most))),
                _ => None,
            }
        })
        .collect()
}

/*
 * the time's Peek standing along a side edge, which leads the upright Tray strip: the hours over
 * the minutes, then the weather and the battery as the settings ask, and a line before the slots
 */
pub(crate) fn time_peek_upright(monitor: &str, slots: bool) -> Rectangle {
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
        parts.push(Box::new(battery_stacked(charge)));
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

/*
 * stand-in content until the sources and the Surfaces draw their own: names the Activity
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_clock_agenda_width_clears_after_an_event_ends() {
        let monitor = Monitor {
            name: "eDP-1".into(),
            width: 1920,
            height: 1080,
        };
        let mut config = Config::default();
        config.rest.clock = false;
        config.rest.agenda = true;
        let fits = HashMap::from([(
            monitor.name.clone(),
            Shape {
                width: 900.0,
                height: 600.0,
                radius: 28.0,
            },
        )]);
        let start = NaiveDate::from_ymd_opt(2026, 10, 11)
            .unwrap()
            .and_hms_opt(10, 0, 0)
            .unwrap();
        let event = calendar::Occurrence {
            title: "Meeting".into(),
            location: None,
            start,
            end: start + chrono::Duration::minutes(1),
            span: (1_000, 1_060),
            all_day: false,
            added: None,
        };

        let before = agenda_widths(
            std::slice::from_ref(&monitor),
            &config,
            &fits,
            1_000,
            std::slice::from_ref(&event),
        );
        assert!(before.get(&monitor.name).is_some_and(|width| *width > 0));

        let after = agenda_widths(
            std::slice::from_ref(&monitor),
            &config,
            &fits,
            1_060,
            std::slice::from_ref(&event),
        );
        assert!(after.is_empty());
    }

    #[test]
    fn agenda_timer_demand_follows_visible_agenda_outputs_not_clock_visibility() {
        let monitor = Monitor {
            name: "eDP-1".into(),
            width: 1920,
            height: 1080,
        };
        let mut config = Config::default();
        config.rest.clock = false;
        config.rest.agenda = true;

        assert!(wants_agenda_clock(
            true,
            &config,
            std::slice::from_ref(&monitor)
        ));
        assert!(!wants_agenda_clock(true, &config, &[]));
        assert!(!wants_agenda_clock(
            false,
            &config,
            std::slice::from_ref(&monitor)
        ));

        config.rest.agenda = false;
        assert!(!wants_agenda_clock(
            true,
            &config,
            std::slice::from_ref(&monitor)
        ));
    }
}
