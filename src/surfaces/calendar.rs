//! The Calendar Surface (#150, docs/design.md Calendar): a month on the left and the agenda of the
//! chosen day on the right, from the local calendars (`sources::calendar`). It opens only from IPC
//! or a keybind, so it holds the keyboard: the arrow keys move the chosen day, by a day or a week,
//! `n` and `p` turn to the next and the previous month, Home or `t` goes back to today, and Tab
//! moves the ring into the agenda, where Up and Down walk its events, and back. Weeks start on
//! Monday (ADR 0015). It says when there are no calendars and when the day has no events.

use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::Instant;

use amane::{
    Center, Column, Cursor, Key, Padding, Parent, Rectangle, Row, Scroll, Service, SpaceBetween,
    Start, Text, Widget, children,
};
use chrono::{Datelike, Days, Months, NaiveDate, NaiveTime};

use crate::clock;
use crate::config;
use crate::icon::Icon;
use crate::island::geometry;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::sources::calendar::{Calendars, Occurrence};
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, DISABLED, radius};

use super::Ring;

// the content's height, which both sides fill
const HEIGHT: f32 = geometry::EXPANDED_MAX.height - 2.0 * INSET;

// a day of the month, and the six weeks any month fits in
const CELL: f32 = 32.0;
const WEEKS: usize = 6;
const MONTH: f32 = 7.0 * CELL;

const HEADER: f32 = 28.0;
const WEEKDAYS: f32 = 18.0;
const GAP: f32 = 8.0;
const DOT: f32 = 4.0;

// the agenda, right of the month
const SIDE_GAP: f32 = 20.0;
const AGENDA: f32 = geometry::EXPANDED_MAX.width - 2.0 * INSET - MONTH - SIDE_GAP;
const FOOTER: f32 = 16.0;
const LIST: f32 = HEIGHT - HEADER - FOOTER - 2.0 * GAP;

const ROWS: usize = 4;
const ROW_GAP: f32 = 6.0;
const ROW: f32 = (LIST - (ROWS - 1) as f32 * ROW_GAP) / ROWS as f32;
const ROW_INSET: f32 = 10.0;

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const DAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

/*
 * the chosen day and where the ring is for one visit of the Surface (`IslandService::visit`), so
 * every opening starts on today. Written by input only, never by the view
 */
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Browse {
    visit: u64,

    // none until a key moves it: today, whichever day that is by then
    day: Option<NaiveDate>,

    // the ring is in the agenda, on its `selected` event
    agenda: bool,
    selected: usize,

    // the first of the events the agenda shows
    first: usize,
}

impl Service for Browse {
    fn new() -> Self {
        Browse::default()
    }

    fn listen() {}
}

impl Browse {
    // this visit's browse; one kept from an earlier visit is over
    fn of(&self, visit: u64) -> Browse {
        if self.visit == visit {
            *self
        } else {
            Browse {
                visit,
                ..Browse::default()
            }
        }
    }

    fn day(&self, today: NaiveDate) -> NaiveDate {
        self.day.unwrap_or(today)
    }

    /*
     * the browse after a key, none when the key is not for the Surface. `events` counts the chosen
     * day's events, before the key. A key that moves the day takes the ring back to the month
     */
    fn step(self, key: Key, today: NaiveDate, events: usize) -> Option<Browse> {
        let day = self.day(today);
        let to = |day: Option<NaiveDate>| {
            Some(Browse {
                day: Some(day.unwrap_or(today)),
                agenda: false,
                selected: 0,
                first: 0,
                ..self
            })
        };

        match key {
            Key::Left => to(day.pred_opt()),
            Key::Right => to(day.succ_opt()),
            Key::Up if !self.agenda => to(day.checked_sub_days(Days::new(7))),
            Key::Down if !self.agenda => to(day.checked_add_days(Days::new(7))),
            Key::Character('n') => to(day.checked_add_months(Months::new(1))),
            Key::Character('p') => to(day.checked_sub_months(Months::new(1))),
            Key::Home | Key::Character('t') => to(Some(today)),
            Key::Up => Some(self.select(self.selected.saturating_sub(1), events)),
            Key::Down => Some(self.select(self.selected + 1, events)),
            Key::Tab | Key::Enter | Key::Escape if self.agenda => Some(Browse {
                agenda: false,
                ..self
            }),
            // an empty agenda has nothing to ring, but the key is still the Surface's
            Key::Tab | Key::Enter => Some(Browse {
                agenda: events > 0,
                ..self.select(0, events)
            }),
            _ => None,
        }
    }

    // the ring on event `at` of `events`, the agenda showing it
    fn select(self, at: usize, events: usize) -> Browse {
        let selected = at.min(events.saturating_sub(1));
        let first = self
            .first
            .min(selected)
            .min(events.saturating_sub(ROWS))
            .max((selected + 1).saturating_sub(ROWS));

        Browse {
            selected,
            first,
            ..self
        }
    }

    // within `events`, after the calendars changed under it
    fn bounded(self, events: usize) -> Browse {
        Browse {
            agenda: self.agenda && events > 0,
            ..self.select(self.selected, events)
        }
    }
}

/*
 * the events of the six weeks a month shows, so the view, which runs every frame of a morph,
 * expands the calendars again only when they or the month changed
 */
#[derive(Default)]
struct Memo {
    generation: u64,
    start: NaiveDate,
    events: Vec<Occurrence>,
}

static MEMO: LazyLock<Mutex<Memo>> = LazyLock::new(Mutex::default);

// the events from `start` on, for the six weeks the month shows
fn events(calendars: &Calendars, start: NaiveDate) -> Vec<Occurrence> {
    let mut memo = MEMO.lock().unwrap_or_else(PoisonError::into_inner);

    if memo.generation != calendars.generation || memo.start != start {
        let end = start + Days::new(7 * WEEKS as u64);

        memo.events = calendars.occurrences(start, end);
        memo.generation = calendars.generation;
        memo.start = start;
    }

    memo.events.clone()
}

// the Monday on or before the first of `day`'s month, where its grid starts
fn grid_start(day: NaiveDate) -> NaiveDate {
    let first = day.with_day(1).unwrap_or(day);

    first - Days::new(u64::from(first.weekday().num_days_from_monday()))
}

// the chosen day's events, in the order they happen
fn on(events: &[Occurrence], day: NaiveDate) -> Vec<Occurrence> {
    events
        .iter()
        .filter(|event| event.on(day))
        .cloned()
        .collect()
}

// the view's own read of the visit, so this never reads IslandService again
pub fn surface(monitor: &str, visit: u64) -> Rectangle {
    let today = clock::today();
    let calendars = Calendars::read().clone();
    let browse = Browse::read().of(visit);
    let day = browse.day(today);
    let start = grid_start(day);
    let events = events(&calendars, start);
    let agenda_events = on(&events, day);
    let browse = browse.bounded(agenda_events.len());

    let shape = geometry::EXPANDED_MAX;

    let agenda: Box<dyn Widget> = if calendars.files == 0 {
        Box::new(state(
            "No calendars",
            if config::get().calendars.is_empty() {
                "Add .ics files to ~/.local/share/calendars"
            } else {
                "No .ics files in calendar.paths"
            },
        ))
    } else {
        Box::new(agenda(day, today, &agenda_events, &browse))
    };

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .child(
            Row::new(vec![
                Box::new(month(monitor, day, today, start, &events, &browse)) as Box<dyn Widget>,
                agenda,
            ])
            .gap(SIDE_GAP),
        )
}

// the month's name with a chevron each side, its weekdays and its six weeks
fn month(
    monitor: &str,
    day: NaiveDate,
    today: NaiveDate,
    start: NaiveDate,
    events: &[Occurrence],
    browse: &Browse,
) -> Column {
    let title = Text::new(format!("{} {}", MONTHS[day.month0() as usize], day.year()))
        .size(theme::text::TITLE)
        .color(theme::ISLAND.on_surface)
        .weight(theme::text::SEMIBOLD);

    let header = Row::new(children![
        turn(monitor, Icon::Back, 'p'),
        title,
        turn(monitor, Icon::Forward, 'n'),
    ])
    .width(MONTH)
    .height(HEADER)
    .justify(SpaceBetween)
    .align(Center);

    let weekdays = Row::new(
        DAYS.iter()
            .map(|name| {
                Box::new(
                    Rectangle::new()
                        .width(CELL)
                        .height(WEEKDAYS)
                        .align_child(Center, Center)
                        .child(
                            Text::new(&name[..1])
                                .size(theme::text::LABEL_SMALL)
                                .color(theme::ISLAND.on_surface_variant)
                                .weight(theme::text::MEDIUM),
                        ),
                ) as Box<dyn Widget>
            })
            .collect(),
    );

    let weeks = (0..WEEKS)
        .map(|week| {
            Box::new(Row::new(
                (0..7)
                    .map(|weekday| {
                        let date = start + Days::new((week * 7 + weekday) as u64);
                        let busy = events.iter().any(|event| event.on(date));

                        Box::new(cell(monitor, date, day, today, busy, !browse.agenda))
                            as Box<dyn Widget>
                    })
                    .collect(),
            )) as Box<dyn Widget>
        })
        .collect();

    Column::new(vec![
        Box::new(header) as Box<dyn Widget>,
        Box::new(Column::new(children![weekdays, Column::new(weeks)])),
    ])
    .width(MONTH)
    .gap(GAP)
}

// a chevron that turns the month, as `n` and `p` do
fn turn(monitor: &str, icon: Icon, key: char) -> Rectangle {
    let monitor = monitor.to_owned();

    Rectangle::new()
        .width(TARGET)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(super::on_left(move || {
            pressed(&monitor, Key::Character(key));
        }))
        .child(icon.on(20.0, theme::ISLAND.on_surface_variant))
}

/*
 * a day of the grid: today filled, the chosen day ringed while the ring is in the month, else
 * shaded, a day of another month faded, and a dot under a day with events. Pressing it chooses it
 */
fn cell(
    monitor: &str,
    date: NaiveDate,
    day: NaiveDate,
    today: NaiveDate,
    busy: bool,
    ring: bool,
) -> Rectangle {
    let chosen = date == day;
    let is_today = date == today;
    let ink = if is_today {
        theme::ISLAND.on_primary
    } else {
        theme::ISLAND.on_surface
    };

    let dot = Rectangle::new()
        .width(DOT)
        .height(DOT)
        .radius(DOT / 2.0)
        .fill(theme::faded(ink, if busy { 1.0 } else { 0.0 }));

    let number = Text::new(date.day().to_string())
        .size(theme::text::LABEL)
        .color(ink)
        .weight(if is_today || chosen {
            theme::text::SEMIBOLD
        } else {
            theme::text::MEDIUM
        });

    let disc = Rectangle::new()
        .width(CELL - 2.0)
        .height(CELL - 2.0)
        .radius((CELL - 2.0) / 2.0)
        .align_child(Center, Center)
        .child(Column::new(children![number, dot]).gap(1.0).align(Center));

    let disc = if is_today {
        disc.fill(theme::ISLAND.primary)
    } else if chosen && !ring {
        disc.fill(theme::ISLAND.surface_container_high)
    } else {
        disc
    };
    let disc = disc.border_if(chosen && ring);
    let disc = if date.month() == day.month() {
        disc
    } else {
        disc.opacity(DISABLED)
    };

    let monitor = monitor.to_owned();

    Rectangle::new()
        .width(CELL)
        .height(CELL)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(super::on_left(move || choose(&monitor, date)))
        .child(disc)
}

// the chosen day's name, its events with the ring on one while it is in the agenda, and a count
fn agenda(day: NaiveDate, today: NaiveDate, events: &[Occurrence], browse: &Browse) -> Column {
    let title = Text::new(named(day, today))
        .size(theme::text::TITLE)
        .color(theme::ISLAND.on_surface)
        .weight(theme::text::SEMIBOLD)
        .elide();

    let header = Rectangle::new()
        .width(AGENDA)
        .height(HEADER)
        .align_child(Start, Center)
        .child(title);

    let list: Box<dyn Widget> = if events.is_empty() {
        Box::new(
            Rectangle::new()
                .width(AGENDA)
                .height(LIST)
                .align_child(Center, Center)
                .child(
                    Text::new("No events")
                        .size(theme::text::BODY)
                        .color(theme::ISLAND.on_surface_variant)
                        .weight(theme::text::MEDIUM),
                ),
        )
    } else {
        let end = (browse.first + ROWS).min(events.len());

        Box::new(
            Rectangle::new()
                .width(AGENDA)
                .height(LIST)
                .align_child(Start, Start)
                .on_scroll(|Scroll { y, .. }| wheel(y))
                .child(
                    Column::new(
                        events[browse.first..end]
                            .iter()
                            .zip(browse.first..)
                            .map(|(event, at)| {
                                let ring = browse.agenda && at == browse.selected;

                                Box::new(row(event, day, ring)) as Box<dyn Widget>
                            })
                            .collect(),
                    )
                    .gap(ROW_GAP),
                ),
        )
    };

    let count = match events.len() {
        0 => String::new(),
        all if all > ROWS => format!(
            "{}\u{2013}{} of {all} events",
            browse.first + 1,
            (browse.first + ROWS).min(all)
        ),
        1 => String::from("1 event"),
        all => format!("{all} events"),
    };

    Column::new(vec![
        Box::new(header) as Box<dyn Widget>,
        list,
        Box::new(
            Text::new(count)
                .size(theme::text::LABEL_SMALL)
                .color(theme::ISLAND.on_surface_variant)
                .weight(theme::text::MEDIUM),
        ),
    ])
    .width(AGENDA)
    .height(HEIGHT)
    .gap(GAP)
}

// what it is and when on `day`, and where
fn row(event: &Occurrence, day: NaiveDate, ring: bool) -> Rectangle {
    let title = if event.title.is_empty() {
        "Untitled event"
    } else {
        &event.title
    };

    let detail = match &event.location {
        Some(location) => format!("{} \u{b7} {location}", when(event, day)),
        None => when(event, day),
    };

    Rectangle::new()
        .width(AGENDA)
        .height(ROW)
        .radius(radius::ROW)
        .fill(theme::ISLAND.surface_container)
        .padding(Padding {
            top: 0.0,
            right: ROW_INSET,
            bottom: 0.0,
            left: ROW_INSET,
        })
        .align_child(Start, Center)
        .child(
            Column::new(children![
                Text::new(title)
                    .size(theme::text::BODY)
                    .color(theme::ISLAND.on_surface)
                    .weight(theme::text::SEMIBOLD)
                    .elide(),
                Text::new(detail)
                    .size(theme::text::LABEL_SMALL)
                    .color(theme::ISLAND.on_surface_variant)
                    .weight(theme::text::MEDIUM)
                    .elide(),
            ])
            .width(Parent)
            .gap(2.0),
        )
        .border_if(ring)
}

// "Today", "Tomorrow", "Yesterday", else the weekday and the date
fn named(day: NaiveDate, today: NaiveDate) -> String {
    match (day - today).num_days() {
        0 => String::from("Today"),
        1 => String::from("Tomorrow"),
        -1 => String::from("Yesterday"),
        _ => format!(
            "{}, {} {}",
            DAYS[day.weekday().num_days_from_monday() as usize],
            day.day(),
            MONTHS[day.month0() as usize]
        ),
    }
}

// when an event is on `day`: all day, its times, or the part of it on the day when it spans days
fn when(event: &Occurrence, day: NaiveDate) -> String {
    let hours = config::get().clock;
    let time = |at: NaiveTime| clock::time(at, hours);

    if event.all_day {
        return String::from("All day");
    }

    let starts = event.start.date() == day;
    let ends = event.end.date() == day || event.end == day.and_time(NaiveTime::MIN) + Days::new(1);

    match (starts, ends) {
        _ if event.moment() => time(event.start.time()),
        (true, true) => format!(
            "{} \u{2013} {}",
            time(event.start.time()),
            time(event.end.time())
        ),
        (true, false) => format!("From {}", time(event.start.time())),
        (false, true) if event.end.time() == NaiveTime::MIN => String::from("All day"),
        (false, true) => format!("Until {}", time(event.end.time())),
        (false, false) => String::from("All day"),
    }
}

// what it means, where the agenda goes
fn state(title: &str, detail: &str) -> Rectangle {
    Rectangle::new()
        .width(AGENDA)
        .height(HEIGHT)
        .align_child(Center, Center)
        .child(
            Column::new(children![
                Icon::Calendar.draw(28.0),
                Text::new(title)
                    .size(theme::text::BODY)
                    .color(theme::ISLAND.on_surface)
                    .weight(theme::text::SEMIBOLD),
                Text::new(detail)
                    .size(theme::text::LABEL_SMALL)
                    .color(theme::ISLAND.on_surface_variant)
                    .weight(theme::text::MEDIUM)
                    .elide(),
            ])
            .width(AGENDA)
            .gap(6.0)
            .align(Center),
        )
}

/*
 * a key while this island shows the Calendar; false for one it does not use, which the window's
 * own keys then get. A key it uses keeps the held island open for another hold
 */
pub fn key(monitor: &str, key: Key) -> bool {
    if IslandService::read().presentation(monitor) != Presentation::Expanded(Surface::Calendar) {
        return false;
    }

    pressed(monitor, key)
}

// a key or the press that stands for one, on the visit open now
fn pressed(monitor: &str, key: Key) -> bool {
    let visit = IslandService::read().visit();
    let today = clock::today();
    let browse = Browse::read().of(visit);
    let day = browse.day(today);
    let count = on(&events(&Calendars::read(), grid_start(day)), day).len();

    let Some(browse) = browse.bounded(count).step(key, today, count) else {
        return false;
    };

    set(browse);
    IslandService::write().attend(monitor, Instant::now());

    true
}

// a day pressed in the grid
fn choose(monitor: &str, date: NaiveDate) {
    let visit = IslandService::read().visit();

    set(Browse {
        day: Some(date),
        agenda: false,
        selected: 0,
        first: 0,
        ..Browse::read().of(visit)
    });
    IslandService::write().attend(monitor, Instant::now());
}

// down walks further down the agenda
fn wheel(lines: f32) {
    let visit = IslandService::read().visit();
    let today = clock::today();
    let browse = Browse::read().of(visit);
    let day = browse.day(today);
    let count = on(&events(&Calendars::read(), grid_start(day)), day).len();
    let browse = browse.bounded(count);

    let browse = if lines > 0.0 {
        let first = (browse.first + 1).min(count.saturating_sub(ROWS));
        Browse {
            first,
            selected: browse.selected.clamp(first, first + ROWS - 1),
            ..browse
        }
    } else if lines < 0.0 {
        let first = browse.first.saturating_sub(1);
        Browse {
            first,
            selected: browse.selected.clamp(first, first + ROWS - 1),
            ..browse
        }
    } else {
        browse
    };

    set(browse);
}

// a write wakes the window even when nothing changed, so only write a real change
fn set(browse: Browse) {
    if *Browse::read() != browse {
        *Browse::write() = browse;
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDateTime;

    use super::*;

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    const TODAY: (i32, u32, u32) = (2026, 10, 8);

    fn today() -> NaiveDate {
        day(TODAY.0, TODAY.1, TODAY.2)
    }

    // the keys in order from a fresh browse, the chosen day having `events` events throughout
    fn after(keys: &[Key], events: usize) -> Browse {
        keys.iter().fold(Browse::default(), |browse, &key| {
            browse.step(key, today(), events).unwrap()
        })
    }

    #[test]
    fn arrows_move_by_a_day_and_a_week() {
        assert_eq!(after(&[], 0).day(today()), today());
        assert_eq!(after(&[Key::Right], 0).day(today()), day(2026, 10, 9));
        assert_eq!(
            after(&[Key::Left, Key::Left], 0).day(today()),
            day(2026, 10, 6)
        );
        assert_eq!(after(&[Key::Down], 0).day(today()), day(2026, 10, 15));
        assert_eq!(after(&[Key::Up, Key::Up], 0).day(today()), day(2026, 9, 24));
        assert_eq!(after(&[Key::Down, Key::Home], 0).day(today()), today());
        assert_eq!(
            after(&[Key::Right, Key::Character('t')], 0).day(today()),
            today()
        );
    }

    // a month on from the 31st is the last day of a shorter month
    #[test]
    fn n_and_p_turn_the_month() {
        assert_eq!(
            after(&[Key::Character('n')], 0).day(today()),
            day(2026, 11, 8)
        );
        assert_eq!(
            after(&[Key::Character('p'), Key::Character('p')], 0).day(today()),
            day(2026, 8, 8)
        );

        let last = Browse {
            day: Some(day(2026, 10, 31)),
            ..Browse::default()
        };
        assert_eq!(
            last.step(Key::Character('n'), today(), 0).unwrap().day,
            Some(day(2026, 11, 30))
        );
    }

    #[test]
    fn tab_moves_the_ring_into_the_agenda_and_back() {
        let browse = after(&[Key::Tab, Key::Down, Key::Down], 5);
        assert!(browse.agenda);
        assert_eq!(browse.selected, 2);

        // in the agenda Up and Down walk the events, not the weeks
        assert_eq!(browse.day(today()), today());
        let browse = browse.step(Key::Up, today(), 5).unwrap();
        assert_eq!(browse.selected, 1);

        let back = browse.step(Key::Tab, today(), 5).unwrap();
        assert!(!back.agenda);

        // moving the day takes it back to the month, at the top of the new day
        let moved = browse.step(Key::Right, today(), 5).unwrap();
        assert!(!moved.agenda);
        assert_eq!(moved.selected, 0);
    }

    // a day with no events has nothing to ring, but Tab is still the Surface's key
    #[test]
    fn an_empty_agenda_keeps_the_ring_in_the_month() {
        let browse = after(&[Key::Tab], 0);

        assert!(!browse.agenda);
        assert_eq!(after(&[Key::Enter], 0), browse);
    }

    #[test]
    fn the_agenda_shows_the_ring() {
        let browse = after(&[Key::Tab, Key::Down, Key::Down, Key::Down, Key::Down], 9);
        assert_eq!((browse.selected, browse.first), (4, 1));

        let browse = after(
            &[
                Key::Tab,
                Key::Down,
                Key::Down,
                Key::Down,
                Key::Down,
                Key::Down,
            ],
            3,
        );
        assert_eq!((browse.selected, browse.first), (2, 0));

        // events gone under it pull the ring back
        let browse = after(&[Key::Tab, Key::Down, Key::Down, Key::Down, Key::Down], 9).bounded(2);
        assert_eq!((browse.selected, browse.first, browse.agenda), (1, 0, true));
        assert!(!browse.bounded(0).agenda);
    }

    // Escape leaves the agenda for the month, and only there is it the island's, to collapse
    #[test]
    fn escape_leaves_the_agenda() {
        let browse = after(&[Key::Tab, Key::Down], 3);
        let back = browse.step(Key::Escape, today(), 3).unwrap();
        assert_eq!((back.agenda, back.day), (false, browse.day));
        assert_eq!(back.step(Key::Escape, today(), 3), None);
    }

    #[test]
    fn other_keys_are_not_the_surfaces() {
        for key in [Key::Escape, Key::Other, Key::Space, Key::Character('x')] {
            assert_eq!(Browse::default().step(key, today(), 3), None, "{key:?}");
        }
    }

    // weeks start on Monday, and six of them hold any month
    #[test]
    fn the_grid_starts_on_the_monday_before_the_first() {
        assert_eq!(grid_start(today()), day(2026, 9, 28));
        assert_eq!(grid_start(day(2026, 6, 15)), day(2026, 6, 1));
        assert_eq!(grid_start(day(2026, 3, 31)), day(2026, 2, 23));

        for month in 1..=12 {
            let first = day(2026, month, 1);
            let last = first.checked_add_months(Months::new(1)).unwrap() - Days::new(1);

            assert!(
                last < grid_start(first) + Days::new(7 * WEEKS as u64),
                "{month}"
            );
        }
    }

    #[test]
    fn days_are_named_near_today() {
        assert_eq!(named(today(), today()), "Today");
        assert_eq!(named(day(2026, 10, 9), today()), "Tomorrow");
        assert_eq!(named(day(2026, 10, 7), today()), "Yesterday");
        assert_eq!(named(day(2026, 12, 25), today()), "Friday, 25 December");
    }

    fn timed(start: NaiveDateTime, end: NaiveDateTime) -> Occurrence {
        Occurrence {
            title: String::from("Event"),
            location: None,
            start,
            end,
            span: (start.and_utc().timestamp(), end.and_utc().timestamp()),
            all_day: false,
        }
    }

    #[test]
    fn an_event_says_its_part_of_the_day() {
        let at = |date: NaiveDate, hour| date.and_hms_opt(hour, 0, 0).unwrap();
        let (today, tomorrow) = (today(), day(2026, 10, 9));
        let config = config::get();
        let time = |hour| clock::time(NaiveTime::from_hms_opt(hour, 0, 0).unwrap(), config.clock);

        let meeting = timed(at(today, 9), at(today, 10));
        assert_eq!(
            when(&meeting, today),
            format!("{} \u{2013} {}", time(9), time(10))
        );

        let moment = timed(at(today, 9), at(today, 9));
        assert_eq!(when(&moment, today), time(9));

        let night = timed(at(today, 22), at(tomorrow, 6));
        assert_eq!(when(&night, today), format!("From {}", time(22)));
        assert_eq!(when(&night, tomorrow), format!("Until {}", time(6)));

        // ending at midnight ends the day it started
        let late = timed(at(today, 22), tomorrow.and_time(NaiveTime::MIN));
        assert_eq!(
            when(&late, today),
            format!("{} \u{2013} {}", time(22), time(0))
        );
    }
}
