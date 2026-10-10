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

use chrono::{Datelike, Days, Months, NaiveDate, NaiveTime};
use kanade_runtime::service::Service;
use kanade_runtime::{
    Center, Column, Cursor, Key, Padding, Parent, Rectangle, Row, Scroll, SpaceBetween, Start,
    Text, Widget, children,
};

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

use super::{Outline, store};

// the content's height, which both sides fill
const HEIGHT: f32 = geometry::CALENDAR.height - 2.0 * INSET;

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
const AGENDA: f32 = geometry::CALENDAR.width - 2.0 * INSET - MONTH - SIDE_GAP;
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

    let shape = geometry::CALENDAR;

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
        .color(theme::island().on_surface)
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
                                .color(theme::island().on_surface_variant)
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
        .child(icon.on(20.0, theme::island().on_surface_variant))
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
        theme::island().on_primary
    } else {
        theme::island().on_surface
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
        disc.fill(theme::island().primary)
    } else if chosen && !ring {
        disc.fill(theme::island().surface_container_high)
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
        .color(theme::island().on_surface)
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
                    .color(theme::island().on_surface_variant)
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
                .color(theme::island().on_surface_variant)
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
        .fill(theme::island().surface_container)
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
                    .color(theme::island().on_surface)
                    .weight(theme::text::SEMIBOLD)
                    .elide(),
                Text::new(detail)
                    .size(theme::text::LABEL_SMALL)
                    .color(theme::island().on_surface_variant)
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
                    .color(theme::island().on_surface)
                    .weight(theme::text::SEMIBOLD),
                Text::new(detail)
                    .size(theme::text::LABEL_SMALL)
                    .color(theme::island().on_surface_variant)
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

    store(browse);
    IslandService::write().attend(monitor, Instant::now());

    true
}

// a day pressed in the grid
fn choose(monitor: &str, date: NaiveDate) {
    let visit = IslandService::read().visit();

    store(Browse {
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

    store(browse);
}
