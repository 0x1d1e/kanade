//! The Calendar Surface (#150, docs/design.md Calendar): a month on the left and the agenda of the
//! chosen day on the right, from the local calendars (`sources::calendar`). It opens only from IPC
//! or a keybind, so it holds the keyboard: the arrow keys move the chosen day, by a day or a week,
//! `n` and `p` turn to the next and the previous month, Home or `t` goes back to today, and Tab
//! moves the ring into the agenda, where Up and Down walk its events, and back. Weeks start on
//! Monday (ADR 0015). It says when there are no calendars, when the day has no events, and while
//! the month's events are expanded, which is off the draw thread.

use std::mem;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
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
use crate::modules;
use crate::sources::calendar::{Calendars, Events, Occurrence};
use crate::sources::google;
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
 * the events of the six weeks a month shows from `start`, and which of them fall on each of its
 * days, in the order they happen. Found once for the month, so a frame neither copies the events
 * nor looks through them all, which a rule each second fills with a limit's worth
 */
#[derive(Debug, Default)]
struct Month {
    start: NaiveDate,
    events: Events,
    days: Vec<Vec<usize>>,
}

impl Month {
    fn new(events: Events, start: NaiveDate) -> Self {
        let mut days = vec![Vec::new(); 7 * WEEKS];

        // only the days from its start to its end can hold an event, its end before its start
        // across a change back from summer time
        for (at, event) in events.occurrences.iter().enumerate() {
            let first = event.start.min(event.end).date().max(start);
            let last = event.start.max(event.end).date();

            for (date, day) in first.iter_days().take_while(|&date| date <= last).zip(
                days.iter_mut()
                    .skip(usize::try_from((first - start).num_days()).unwrap_or(usize::MAX)),
            ) {
                if event.on(date) {
                    day.push(at);
                }
            }
        }

        Month {
            start,
            events,
            days,
        }
    }

    // a month with no events yet
    fn empty(start: NaiveDate) -> Self {
        Month {
            start,
            ..Month::default()
        }
    }

    // the events on `day`, as indices into `events`; none on a day the month does not show
    fn on(&self, day: NaiveDate) -> &[usize] {
        usize::try_from((day - self.start).num_days())
            .ok()
            .and_then(|at| self.days.get(at))
            .map_or(&[], Vec::as_slice)
    }
}

/*
 * the month last expanded for the Surface, written by the expansion's thread only, and which
 * calendars and month it is of
 */
#[derive(Default)]
struct Expansion {
    of: Option<Wanted>,
    month: Arc<Month>,
}

impl Service for Expansion {
    fn new() -> Self {
        Expansion::default()
    }

    fn listen() {}
}

// a month asked for: the calendars' generation, which run of reads, with files or without, they
// are of, and where its grid starts
#[derive(Debug, Clone, Copy, PartialEq)]
struct Wanted {
    generation: u64,
    found: u64,
    start: NaiveDate,
}

impl Wanted {
    fn of(calendars: &Calendars, start: NaiveDate) -> Self {
        Wanted {
            generation: calendars.generation,
            found: calendars.found,
            start,
        }
    }

    // whether `expanded` answers this: the same month, of these calendars or ones read since
    fn met_by(self, expanded: Wanted) -> bool {
        self.start == expanded.start && self.generation <= expanded.generation
    }
}

/*
 * the month last asked for, whether the thread expanding is running, and the months it could not
 * expand, none older than the calendars last expanded. One thread at a time, which takes the
 * latest asked once it is done, so a month turned past while it runs is never expanded
 */
struct Asked {
    wanted: Option<Wanted>,
    running: bool,
    failed: Vec<Wanted>,
}

impl Asked {
    // whether `wanted` could not be expanded, so is not tried again
    fn failed(&self, wanted: Wanted) -> bool {
        self.failed.iter().any(|&failed| wanted.met_by(failed))
    }

    // forgets the months that failed for calendars older than `read`, which may expand anew
    fn forget_failed_before(&mut self, read: u64) {
        self.failed.retain(|failed| failed.generation >= read);
    }
}

/*
 * while it is held only `Expansion` is taken, and `Expansion` is read only under it, so the
 * expansion's write never waits on a view waiting for this
 */
static ASKED: Mutex<Asked> = Mutex::new(Asked {
    wanted: None,
    running: false,
    failed: Vec::new(),
});

// why the Surface's month is not the one it asked for
#[derive(Debug, Clone, Copy, PartialEq)]
enum Pending {
    Loading,
    Failed,
}

/*
 * the month the Surface shows, without events while none of it is expanded, and why not the one
 * asked for, when it says so
 */
struct Shown {
    month: Arc<Month>,
    pending: Option<Pending>,
}

/*
 * the events of the six weeks the month shows from `start`, expanded off the draw thread, as a
 * rule each second takes a limit's worth of work, too long for a frame. Files read again, edited,
 * added or gone, keep the month shown until it is expanded anew, so a sync neither blanks it nor
 * moves the ring; one that could not be expanded stays shown, saying so. Once none are found, the
 * events of the files before never show again
 */
fn month_of(calendars: &Calendars, start: NaiveDate) -> Shown {
    let wanted = Wanted::of(calendars, start);
    let mut asked = ASKED.lock().unwrap_or_else(PoisonError::into_inner);

    let held = {
        let expansion = Expansion::read();

        // asked for, so a month expanding meanwhile never replaces it
        if expansion.of.is_some_and(|of| wanted.met_by(of)) {
            asked.wanted = Some(wanted);

            return Shown {
                month: Arc::clone(&expansion.month),
                pending: None,
            };
        }

        // none to expand, so a thread only to free the month of files gone; asked for, so one
        // expanding those never shows its month
        if calendars.files == 0 && expansion.of.is_none_or(|of| of.found == wanted.found) {
            asked.wanted = Some(wanted);

            return Shown {
                month: Arc::new(Month::empty(start)),
                pending: None,
            };
        }

        expansion
            .of
            .filter(|of| of.start == start && of.found == wanted.found)
            .map(|_| Arc::clone(&expansion.month))
    };
    let shown = |pending| match held {
        Some(month) if pending == Pending::Loading => Shown {
            month,
            pending: None,
        },
        Some(month) => Shown {
            month,
            pending: Some(pending),
        },
        None => Shown {
            month: Arc::new(Month::empty(start)),
            pending: Some(pending),
        },
    };

    // asked for even when it failed, so the month expanding for one asked before never shows
    asked.wanted = Some(wanted);

    if asked.failed(wanted) {
        return shown(Pending::Failed);
    }

    if !asked.running {
        // not marked failed, so the next draw or key tries again
        match thread::Builder::new()
            .name("calendar-month".into())
            .spawn(expand)
        {
            Ok(_) => asked.running = true,
            Err(error) => {
                eprintln!("kanade: cannot expand the calendar's month ({error})");
                return shown(Pending::Failed);
            }
        }
    }

    shown(Pending::Loading)
}

/*
 * expands the month last asked for, on its own thread, until the month shown answers what is
 * asked: the latest month asked while it ran is expanded next, and a result no longer asked for
 * never shows
 */
fn expand() {
    // the month being expanded; a panic marks it failed and leaves another month to a new thread
    struct Running(Option<Wanted>);

    impl Drop for Running {
        fn drop(&mut self) {
            let mut asked = ASKED.lock().unwrap_or_else(PoisonError::into_inner);

            asked.running = false;
            if let Some(failed) = self.0 {
                asked.forget_failed_before(failed.generation);
                asked.failed.push(failed);
            }

            // a write redraws the view, to say so
            drop(Expansion::write());
        }
    }

    let mut running = Running(None);

    loop {
        let start = {
            let mut asked = ASKED.lock().unwrap_or_else(PoisonError::into_inner);

            // one already expanded, as a view turning back to it finds, is not expanded again
            let expanded = Expansion::read().of;

            match asked.wanted {
                Some(wanted)
                    if !expanded.is_some_and(|expanded| wanted.met_by(expanded))
                        && !asked.failed(wanted) =>
                {
                    wanted.start
                }
                // done under the lock, so a month asked for from now on starts a thread
                _ => {
                    asked.running = false;
                    mem::forget(running);
                    return;
                }
            }
        };

        let calendars = Calendars::read().clone();
        let expanding = Wanted::of(&calendars, start);
        running.0 = Some(expanding);

        let end = start + Days::new(7 * WEEKS as u64);
        let month = Arc::new(Month::new(calendars.occurrences(start, end), start));

        // the month replaced is freed here, after the lock, not on the draw thread
        let _replaced = publish(expanding, month);
    }
}

// shows `month`, expanded as `expanding`, if it is still asked for, and gives back the one replaced
fn publish(expanding: Wanted, month: Arc<Month>) -> Option<Arc<Month>> {
    let mut asked = ASKED.lock().unwrap_or_else(PoisonError::into_inner);

    if !asked.wanted.is_some_and(|wanted| wanted.met_by(expanding)) {
        return None;
    }

    asked.forget_failed_before(expanding.generation);

    let mut expansion = Expansion::write();
    expansion.of = Some(expanding);

    Some(mem::replace(&mut expansion.month, month))
}

// the Monday on or before the first of `day`'s month, where its grid starts
fn grid_start(day: NaiveDate) -> NaiveDate {
    let first = day.with_day(1).unwrap_or(day);

    first - Days::new(u64::from(first.weekday().num_days_from_monday()))
}

// the view's own read of the visit, so this never reads IslandService again
pub fn surface(monitor: &str, visit: u64) -> Rectangle {
    let today = clock::today();
    let calendars = Calendars::read().clone();
    let browse = Browse::read().of(visit);
    let day = browse.day(today);
    let start = grid_start(day);
    let Shown { month, pending } = month_of(&calendars, start);
    let browse = browse.bounded(month.on(day).len());

    let shape = geometry::EXPANDED_MAX;

    // a Module that is off reads no Service
    let account = modules::on("google-calendar")
        .then(|| {
            google::Account::read()
                .problem()
                .map(google::Problem::brief)
        })
        .flatten();

    let agenda: Box<dyn Widget> = if calendars.files == 0 {
        Box::new(state(
            "No calendars",
            match account {
                Some(problem) => problem,
                None if config::get().calendars.is_empty() => {
                    "Add .ics files to ~/.local/share/calendars"
                }
                None => "No .ics files in calendar.paths",
            },
        ))
    } else {
        Box::new(agenda(day, today, &month, pending, &browse, account))
    };

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .child(
            Row::new(vec![
                Box::new(grid(monitor, day, today, &month, &browse)) as Box<dyn Widget>,
                agenda,
            ])
            .gap(SIDE_GAP),
        )
}

// the month's name with a chevron each side, its weekdays and its six weeks
fn grid(monitor: &str, day: NaiveDate, today: NaiveDate, month: &Month, browse: &Browse) -> Column {
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
                        let date = month.start + Days::new((week * 7 + weekday) as u64);
                        let busy = !month.on(date).is_empty();

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

/*
 * the chosen day's name, its events ROWS at a time with the ring on one while it is in the agenda,
 * and their count under them, which says so when a rule ran to the limit and some are missing, and
 * why Google's do not sync, when they do not. While the month is `pending`, it says so instead of
 * no events, and the count says when the events shown could not be updated
 */
fn agenda(
    day: NaiveDate,
    today: NaiveDate,
    month: &Month,
    pending: Option<Pending>,
    browse: &Browse,
    account: Option<&str>,
) -> Column {
    let events = month.on(day);
    let partial = month.events.partial;

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
                    Text::new(match pending {
                        Some(Pending::Loading) => "Loading events",
                        Some(Pending::Failed) => "Events could not be read",
                        None if partial => "Some events not shown",
                        None => "No events",
                    })
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
                            .map(|(&event, at)| {
                                let ring = browse.agenda && at == browse.selected;
                                let event = &month.events.occurrences[event];

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
    let count = if partial && !count.is_empty() {
        format!("{count}, some not shown")
    } else {
        count
    };
    // the events of calendars since read again could not be expanded
    let count = match pending {
        Some(Pending::Failed) if !count.is_empty() => format!("{count}, not updated"),
        _ => count,
    };
    let count = match account {
        Some(problem) if count.is_empty() => problem.to_owned(),
        Some(problem) => format!("{count} \u{b7} {problem}"),
        None => count,
    };

    Column::new(vec![
        Box::new(header) as Box<dyn Widget>,
        list,
        Box::new(
            Text::new(count)
                .size(theme::text::LABEL_SMALL)
                .color(theme::ISLAND.on_surface_variant)
                .weight(theme::text::MEDIUM)
                .elide(),
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
    let calendars = Calendars::read().clone();
    let count = month_of(&calendars, grid_start(day)).month.on(day).len();

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
    let calendars = Calendars::read().clone();
    let count = month_of(&calendars, grid_start(day)).month.on(day).len();
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
    use std::time::Duration;

    use chrono::NaiveDateTime;

    use super::*;
    use crate::sources::calendar;

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

    // the tests that use the calendars and the month expanded, which take turns
    static SERVICES: Mutex<()> = Mutex::new(());

    // calendar files, each a name and the title of its one event, today at four
    fn read(files: &[(&str, &str)]) -> Calendars {
        let texts: Vec<(&str, String)> = files
            .iter()
            .map(|&(name, title)| {
                let event = format!(
                    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//test//EN\r\nBEGIN:VEVENT\r\n\
                     UID:{title}@test\r\nDTSTAMP:20261001T000000Z\r\nDTSTART:20261008T160000\r\n\
                     DTEND:20261008T170000\r\nSUMMARY:{title}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
                );
                (name, event)
            })
            .collect();
        let texts: Vec<(&str, &str)> = texts
            .iter()
            .map(|(name, text)| (*name, text.as_str()))
            .collect();

        calendar::read_as(&texts)
    }

    fn titles(month: &Month) -> Vec<&str> {
        month
            .events
            .occurrences
            .iter()
            .map(|event| event.title.as_str())
            .collect()
    }

    fn pending(calendars: &Calendars, start: NaiveDate) -> Option<Pending> {
        month_of(calendars, start).pending
    }

    // the month once expanded for `calendars`
    fn expanded(calendars: &Calendars, start: NaiveDate) -> Arc<Month> {
        shown_until_expanded(calendars, start)
            .pop()
            .expect("a month")
    }

    // each month shown for `calendars` until expanded, the expanded one last
    fn shown_until_expanded(calendars: &Calendars, start: NaiveDate) -> Vec<Arc<Month>> {
        let wanted = Wanted::of(calendars, start);
        let mut months = Vec::new();

        for _ in 0..5000 {
            // first, so the month shown is the one expanded, not one held while it was
            let expanded = Expansion::read().of == Some(wanted);
            let shown = month_of(calendars, start);
            months.push(shown.month);
            match shown.pending {
                None if expanded => return months,
                None | Some(Pending::Loading) => thread::sleep(Duration::from_millis(1)),
                Some(Pending::Failed) => panic!("{start} failed"),
            }
        }
        panic!("{start} never expanded");
    }

    fn running() -> bool {
        ASKED.lock().unwrap().running
    }

    // whether the thread expanding ends
    fn ended() -> bool {
        for _ in 0..5000 {
            if !running() {
                return true;
            }
            thread::sleep(Duration::from_millis(1));
        }
        false
    }

    // the month asked for last shows once expanded, off the caller's thread, and the same files
    // read again keep the month shown until then
    #[test]
    fn a_month_is_expanded_off_the_draw_thread() {
        let _services = SERVICES.lock().unwrap_or_else(PoisonError::into_inner);
        let (october, november, december) =
            (grid_start(today()), day(2026, 10, 26), day(2026, 11, 30));
        let files = [("a.ics", "Meeting")];

        // after none, so no other test's month shows
        read(&[]);
        let calendars = read(&files);
        assert_eq!(pending(&calendars, october), Some(Pending::Loading));
        assert_eq!(titles(&expanded(&calendars, october)), ["Meeting"]);

        // another month shows none of the last one's events while it is expanded
        assert_eq!(pending(&calendars, november), Some(Pending::Loading));
        assert_eq!(expanded(&calendars, november).start, november);

        // read again, the same month shows on until it is expanded anew
        let read_again = read(&files);
        let shown = month_of(&read_again, november);
        assert_eq!((shown.month.start, shown.pending), (november, None));
        assert_eq!(expanded(&read_again, november).start, november);

        // turned on while one is expanded, the month asked last shows, and the thread ends
        assert!(pending(&read_again, december).is_some());
        assert!(pending(&read_again, october).is_some());
        assert_eq!(titles(&expanded(&read_again, october)), ["Meeting"]);
        assert!(ended());

        // a month no longer asked for never shows
        let stale = Wanted::of(&read_again, december);
        assert!(publish(stale, Arc::new(Month::empty(december))).is_none());
        assert_eq!(Expansion::read().of, Some(Wanted::of(&read_again, october)));

        // one that could not be expanded shows the last one, saying so, and is not tried again
        let calendars = read(&files);
        ASKED
            .lock()
            .unwrap()
            .failed
            .push(Wanted::of(&calendars, october));
        let shown = month_of(&calendars, october);
        assert_eq!(
            (titles(&shown.month), shown.pending),
            (vec!["Meeting"], Some(Pending::Failed))
        );
        assert!(!running());

        // turned back to it while another month is expanded, that month never replaces it; the
        // calendars held keep the thread from reading them until both were asked
        let held = Calendars::write();
        assert!(pending(&calendars, november).is_some());
        assert_eq!(pending(&calendars, october), Some(Pending::Failed));
        drop(held);
        assert!(ended());
        assert_eq!(Expansion::read().of, Some(Wanted::of(&read_again, october)));
    }

    // the events of files gone never show once none are found, not even while ones found since
    // are expanded, and are freed off the caller's thread; a file added keeps the month shown.
    // The calendars held keep the thread from expanding the new ones
    #[test]
    fn the_events_of_files_gone_never_show_after_none() {
        let _services = SERVICES.lock().unwrap_or_else(PoisonError::into_inner);
        let october = grid_start(today());
        let shows_a = |month: &Arc<Month>| titles(month).contains(&"A");

        // after none, so no other test's month shows
        read(&[]);
        let a = read(&[("a.ics", "A")]);
        let month_a = expanded(&a, october);
        assert_eq!(titles(&month_a), ["A"]);

        let none = read(&[]);
        assert!(!shows_a(&month_of(&none, october).month));

        // an expansion of the files gone, still running, never shows
        assert!(publish(Wanted::of(&a, october), month_a).is_none());

        assert!(ended());
        assert_eq!(Expansion::read().of, Some(Wanted::of(&none, october)));
        assert!(titles(&Expansion::read().month).is_empty());

        let b = read(&[("b.ics", "B")]);
        let held = Calendars::write();
        let shown = month_of(&b, october);
        assert_eq!(
            (titles(&shown.month), shown.pending),
            (vec![], Some(Pending::Loading))
        );
        drop(held);
        let months = shown_until_expanded(&b, october);
        assert!(!months.iter().any(shows_a));
        assert_eq!(titles(months.last().unwrap()), ["B"]);

        // a file added, as a sync of one file per event does, is no other calendar
        let more = read(&[("b.ics", "B"), ("c.ics", "C")]);
        let held = Calendars::write();
        let shown = month_of(&more, october);
        assert_eq!((titles(&shown.month), shown.pending), (vec!["B"], None));
        drop(held);
        assert_eq!(titles(&expanded(&more, october)), ["B", "C"]);
        assert!(ended());
    }

    // turned back to a month expanded while another is expanded, the month shows on, the other is
    // never shown once expanded, and the thread ends without expanding the month again. The
    // calendars held keep a new thread from expanding until the month was turned back to
    #[test]
    fn a_month_turned_back_to_shows_on() {
        let _services = SERVICES.lock().unwrap_or_else(PoisonError::into_inner);
        let (october, november) = (grid_start(today()), day(2026, 10, 26));

        // after none, so no other test's month shows
        read(&[]);
        let calendars = read(&[("a.ics", "Meeting")]);
        let month = expanded(&calendars, october);
        assert!(ended());

        let held = Calendars::write();
        assert_eq!(pending(&calendars, november), Some(Pending::Loading));
        let shown = month_of(&calendars, october);
        assert!(Arc::ptr_eq(&shown.month, &month) && shown.pending.is_none());

        // the other month, expanded now, never shows
        let other = Wanted::of(&calendars, november);
        assert!(publish(other, Arc::new(Month::empty(november))).is_none());
        assert!(Arc::ptr_eq(&month_of(&calendars, october).month, &month));
        drop(held);

        assert!(ended());
        assert_eq!(Expansion::read().of, Some(Wanted::of(&calendars, october)));
        assert!(Arc::ptr_eq(&month_of(&calendars, october).month, &month));
    }

    // each day of the month holds the events that fall on it, in their order, whatever their
    // length and wherever they start and end around the six weeks
    #[test]
    fn a_month_finds_each_days_events() {
        let at = |date: NaiveDate, hour, minute| date.and_hms_opt(hour, minute, 0).unwrap();
        let start = grid_start(today());
        let end = start + Days::new(7 * WEEKS as u64);
        let midnight = |date: NaiveDate| date.and_time(NaiveTime::MIN);

        let mut all_day = timed(midnight(today()), midnight(day(2026, 10, 11)));
        all_day.all_day = true;
        let occurrences = vec![
            timed(at(day(2026, 1, 1), 9, 0), at(day(2026, 1, 1), 10, 0)),
            timed(at(day(2026, 9, 1), 9, 0), at(day(2026, 10, 2), 9, 0)),
            timed(midnight(start - Days::new(1)), midnight(start)),
            timed(at(start, 0, 0), at(start, 0, 0)),
            all_day,
            timed(at(today(), 22, 0), midnight(day(2026, 10, 9))),
            // a change back from summer time ends it before it starts
            timed(at(day(2026, 10, 25), 2, 50), at(day(2026, 10, 25), 2, 10)),
            timed(at(end - Days::new(1), 23, 0), at(end, 1, 0)),
            timed(midnight(end), midnight(end)),
        ];
        let events = Events {
            occurrences: occurrences.clone(),
            partial: false,
        };

        let month = Month::new(events, start);
        for date in start.iter_days().take_while(|&date| date < end) {
            let expected: Vec<usize> = (0..occurrences.len())
                .filter(|&at| occurrences[at].on(date))
                .collect();
            assert_eq!(month.on(date), expected, "{date}");
        }
        assert!(month.on(start - Days::new(1)).is_empty());
        assert!(month.on(end).is_empty());
        assert_eq!(month.on(day(2026, 9, 28)), [1, 3]);
        assert_eq!(month.on(today()), [4, 5]);
    }
}
