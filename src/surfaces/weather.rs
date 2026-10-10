//! The Weather Surface (#152, ADR 0017, docs/design.md Weather): the weather now on the left and
//! the days ahead on the right, from `sources::weather`. It opens only from IPC or a keybind and
//! has no targets, so Escape or a typed character collapses it. A forecast that is not the latest,
//! after a failed fetch or a suspend, stays shown with the time it is from; without one it says
//! why: no location set, the first fetch on its way, or the failure.

use chrono::{Datelike, NaiveDate};
use kanade_runtime::service::Service;
use kanade_runtime::{
    Center, Column, Padding, Rectangle, Row, SpaceBetween, Start, Text, Widget, children,
};

use crate::clock;
use crate::config::{self, Config};
use crate::icon::Icon;
use crate::island::geometry;
use crate::sources::weather::{self, Condition, Day, Forecast, Problem, State, Units, Weather};
use crate::theme::space::INSET;
use crate::theme::{self, radius};

// the content's height, which both sides fill
const HEIGHT: f32 = geometry::CONTROLS.height - 2.0 * INSET;
const WIDTH: f32 = geometry::CONTROLS.width - 2.0 * INSET;

// the weather now, left of the days
const NOW: f32 = 170.0;
const SIDE_GAP: f32 = 20.0;
const DAYS: f32 = WIDTH - NOW - SIDE_GAP;

// a row per day, as many as fill the height
const ROWS: usize = 5;
const ROW_GAP: f32 = 6.0;
const ROW: f32 = (HEIGHT - (ROWS - 1) as f32 * ROW_GAP) / ROWS as f32;
const ROW_INSET: f32 = 12.0;
const WEEKDAY: f32 = 48.0;
const CHANCE: f32 = 36.0;

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

// in the config of `monitor`, whose clock may be its own
pub fn surface(monitor: &str) -> Rectangle {
    let weather = Weather::read().clone();
    let config = config::on(monitor);
    let shape = geometry::CONTROLS;

    // one for a location since replaced is never shown, even before the fetch thread drops it
    let forecast = weather
        .forecast
        .as_ref()
        .filter(|forecast| Some(forecast.location) == config.location);

    let content: Box<dyn Widget> = match forecast {
        Some(forecast) => {
            // the days are the location's, named by its own date, the update by the machine's
            let there = forecast.today(weather::now()).unwrap_or_else(clock::today);

            Box::new(
                Row::new(children![
                    now(forecast, &weather, &config, clock::today()),
                    days(forecast, config.units, there),
                ])
                .gap(SIDE_GAP),
            )
        }
        None => Box::new(missing(&weather.state)),
    };

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .child(Row::new(vec![content]))
}

// the place, the temperature and sky now, the rest of now, and when it is from
fn now(forecast: &Forecast, weather: &Weather, config: &Config, today: NaiveDate) -> Column {
    let current = &forecast.current;
    let units = config.units;

    let place = Text::new(config.place.as_deref().unwrap_or("Weather"))
        .size(theme::text::TITLE)
        .color(theme::island().on_surface)
        .weight(theme::text::SEMIBOLD)
        .elide();

    let temperature = Row::new(children![
        Text::new(units.temperature(current.temperature))
            .size(theme::text::DISPLAY)
            .color(theme::island().on_surface)
            .weight(theme::text::MEDIUM),
        icon(current.condition, current.day).draw(36.0),
    ])
    .gap(12.0)
    .align(Center);

    let mut lines: Vec<Box<dyn Widget>> = vec![Box::new(
        Text::new(current.condition.name())
            .size(theme::text::BODY)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD)
            .elide(),
    )];
    let details = [
        current
            .feels
            .map(|feels| format!("Feels like {}", units.temperature(feels))),
        current
            .humidity
            .map(|humidity| format!("Humidity {}%", humidity.round())),
        current
            .wind
            .map(|wind| format!("Wind {}", units.speed(wind))),
    ];
    lines.extend(details.into_iter().flatten().map(|detail| {
        Box::new(
            Text::new(detail)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::MEDIUM)
                .elide(),
        ) as Box<dyn Widget>
    }));

    // Open-Meteo's terms ask its data be credited wherever it shows
    let said = Column::new(
        [
            footer(forecast, weather, config, today),
            String::from("Weather data by Open-Meteo"),
        ]
        .into_iter()
        .map(|line| {
            Box::new(
                Text::new(line)
                    .size(theme::text::LABEL_SMALL)
                    .color(theme::island().on_surface_variant)
                    .weight(theme::text::MEDIUM)
                    .elide(),
            ) as Box<dyn Widget>
        })
        .collect(),
    )
    .gap(2.0);

    Column::new(children![
        Column::new(children![place, temperature, Column::new(lines).gap(3.0)]).gap(4.0),
        said,
    ])
    .width(NOW)
    .height(HEIGHT)
    .justify(SpaceBetween)
}

// when the forecast is from; a stale one says why, so old weather never reads as now
fn footer(forecast: &Forecast, weather: &Weather, config: &Config, today: NaiveDate) -> String {
    let at = clock::local_time(forecast.at).map_or_else(String::new, |at| {
        let time = clock::time(at.time(), config.clock);

        if at.date() == today {
            time
        } else {
            format!("{} {time}", weekday(at.date()))
        }
    });

    match weather.problem() {
        Some(problem) => format!("{} \u{b7} last updated {at}", problem.brief()),
        None if weather.stale(weather::now()) => format!("Last updated {at}"),
        None => format!("Updated {at}"),
    }
}

// the days from today, a row each
fn days(forecast: &Forecast, units: Units, today: NaiveDate) -> Column {
    let rows: Vec<Box<dyn Widget>> = forecast
        .days
        .iter()
        .filter(|day| day.date >= today)
        .take(ROWS)
        .map(|day| Box::new(row(day, units, today)) as Box<dyn Widget>)
        .collect();

    if rows.is_empty() {
        return Column::new(children![
            Rectangle::new()
                .width(DAYS)
                .height(HEIGHT)
                .align_child(Center, Center)
                .child(
                    Text::new("No forecast")
                        .size(theme::text::BODY)
                        .color(theme::island().on_surface_variant)
                        .weight(theme::text::MEDIUM),
                ),
        ]);
    }

    Column::new(rows).width(DAYS).gap(ROW_GAP)
}

// the day's name and sky, its chance of rain or snow when there is much of one, its high and low
fn row(day: &Day, units: Units, today: NaiveDate) -> Rectangle {
    let name = if day.date == today {
        "Today"
    } else {
        weekday(day.date)
    };

    let chance = day
        .precipitation
        .filter(|&chance| chance >= 20.0)
        .map(|chance| format!("{}%", chance.round()))
        .unwrap_or_default();

    let sky = Row::new(children![
        Rectangle::new()
            .width(WEEKDAY)
            .height(ROW)
            .align_child(Start, Center)
            .child(
                Text::new(name)
                    .size(theme::text::LABEL)
                    .color(theme::island().on_surface)
                    .weight(theme::text::SEMIBOLD),
            ),
        icon(day.condition, true).draw(20.0),
        Rectangle::new()
            .width(CHANCE)
            .height(ROW)
            .align_child(Start, Center)
            .child(
                Text::new(chance)
                    .size(theme::text::LABEL_SMALL)
                    .color(theme::island().on_surface_variant)
                    .weight(theme::text::MEDIUM),
            ),
    ])
    .gap(8.0)
    .align(Center);

    let range = Row::new(children![
        Text::new(units.temperature(day.high))
            .size(theme::text::LABEL)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD),
        Text::new(units.temperature(day.low))
            .size(theme::text::LABEL)
            .color(theme::island().on_surface_variant)
            .weight(theme::text::MEDIUM),
    ])
    .gap(8.0)
    .align(Center);

    Rectangle::new()
        .width(DAYS)
        .height(ROW)
        .radius(radius::ROW)
        .fill(theme::island().surface_container)
        .padding(Padding {
            top: 0.0,
            right: ROW_INSET,
            bottom: 0.0,
            left: ROW_INSET,
        })
        .align_child(Start, Center)
        .child(
            Row::new(children![sky, range])
                .width(DAYS - 2.0 * ROW_INSET)
                .justify(SpaceBetween)
                .align(Center),
        )
}

// what stands for the sky; a clear or partly cloudy night shows no sun
pub fn icon(condition: Condition, day: bool) -> Icon {
    match condition {
        Condition::Clear if day => Icon::Sun,
        Condition::Clear => Icon::Moon,
        Condition::MainlyClear | Condition::PartlyCloudy if day => Icon::PartlyCloudy,
        Condition::MainlyClear | Condition::PartlyCloudy | Condition::Overcast => Icon::Cloud,
        Condition::Fog => Icon::Fog,
        Condition::Drizzle
        | Condition::FreezingDrizzle
        | Condition::Rain
        | Condition::FreezingRain
        | Condition::RainShowers => Icon::Rain,
        Condition::Snow | Condition::SnowShowers => Icon::Snow,
        Condition::Thunderstorm => Icon::Storm,
        Condition::Unknown => Icon::Cloud,
    }
}

fn weekday(date: NaiveDate) -> &'static str {
    WEEKDAYS[date.weekday().num_days_from_monday() as usize]
}

/*
 * why there is no forecast to show, across the whole Surface. Each line is short and fixed, so it
 * centers unelided; Open-Meteo's own words are in `kanade weather status`
 */
fn missing(state: &State) -> Rectangle {
    let (title, detail) = match state {
        State::Unset => ("No location", "Set weather.location in Settings"),
        State::Fetching | State::Fetched => ("Fetching the weather", "From Open-Meteo"),
        State::Failed(problem @ Problem::Offline(_)) => (problem.brief(), "Trying again soon"),
        State::Failed(problem @ Problem::Refused(_)) => {
            (problem.brief(), "See kanade weather status")
        }
    };

    Rectangle::new()
        .width(WIDTH)
        .height(HEIGHT)
        .align_child(Center, Center)
        .child(
            Column::new(children![
                Icon::Cloud.draw(28.0),
                Text::new(title)
                    .size(theme::text::BODY)
                    .color(theme::island().on_surface)
                    .weight(theme::text::SEMIBOLD),
                Text::new(detail)
                    .size(theme::text::LABEL_SMALL)
                    .color(theme::island().on_surface_variant)
                    .weight(theme::text::MEDIUM),
            ])
            .gap(6.0)
            .align(Center),
        )
}
