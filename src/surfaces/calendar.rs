//! The Calendar Surface (docs/archived/design.md Calendar): a month on the left and the agenda of the
//! chosen day on the right, from the local calendars (`sources::calendar`). It opens only from IPC
//! or a keybind, so it holds the keyboard: the arrow keys move the chosen day, by a day or a week,
//! `n` and `p` turn to the next and the previous month, Home or `t` goes back to today, and Tab
//! moves the ring into the agenda, where Up and Down walk its events, and back, and `a` types a new
//! event for the chosen day into a file of its own (ADR 0045); `e` or an event's Edit changes one
//! added here, in place and keeping what its file holds, and holding `d` or its Remove removes it
//! (ADR 0113). Weeks start on Monday (ADR 0015). It says when there are no calendars, when the
//! day has no events, and while the month's events are expanded, which is off the draw thread.

use std::mem;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use chrono::{Datelike, Days, Months, NaiveDate, NaiveTime};
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Column, Cursor, Key, Padding, Parent, Rectangle, Row, Scroll, SpaceBetween, Start,
    Text, Widget, children,
};
use kanade_runtime::{
    MutexExt,
    service::{self, Service},
};

use crate::clock;
use crate::config;
use crate::icon::Icon;
use crate::island::geometry;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::modules;
use crate::sources::calendar::{Calendars, Events, Occurrence, add};
use crate::sources::google;
use crate::sources::subscriptions::Subscriptions;
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, DISABLED, radius};
use crate::view::rise;

use super::search;
use super::visit::Visited;
use super::{Outline, confirm, store};

// the content's height, which both sides fill
const HEIGHT: f32 = geometry::CALENDAR.height - 2.0 * INSET;

// a day of the month, and the six weeks any month fits in
const CELL: f32 = 32.0;
const WEEKS: usize = 6;
const MONTH: f32 = 7.0 * CELL;

const HEADER: f32 = 28.0;
const WEEKDAYS: f32 = 18.0;
const GAP: f32 = 8.0;

// a week's height, the weeks filling what the header and weekdays leave of the content
const WEEK: f32 = (HEIGHT - HEADER - GAP - WEEKDAYS) / WEEKS as f32;
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

impl Visited for Browse {
    fn visit(&self) -> u64 {
        self.visit
    }

    fn begin(visit: u64) -> Self {
        Browse {
            visit,
            ..Browse::default()
        }
    }
}

// tells one save from another
static SAVES: AtomicU64 = AtomicU64::new(0);

/*
 * the event being typed, for one visit: none until `a` or the Add pill starts it, or `e` starts
 * changing an event added here. Written by input and by the save it started, never by the view
 */
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Adding {
    visit: u64,
    text: Option<String>,

    // the event `text` replaces, none for a new one
    replacing: Option<Replacing>,

    // what the last Enter could not save
    problem: Option<String>,

    // the save that is writing the file, which keeps the text as it is until it says how it went
    saving: Option<u64>,
}

/*
 * the event being changed: its file, and the event as the Surface showed it, which the file must
 * still hold when the change is written (ADR 0113)
 */
#[derive(Debug, Clone, PartialEq)]
struct Replacing {
    stem: String,
    was: Occurrence,
}

impl Service for Adding {
    fn new() -> Self {
        Adding::default()
    }

    fn listen() {}
}

impl Visited for Adding {
    fn visit(&self) -> u64 {
        self.visit
    }

    fn begin(visit: u64) -> Self {
        Adding {
            visit,
            ..Adding::default()
        }
    }
}

impl Adding {
    // the text after `key`, none for a key that edits nothing
    fn typed(&self, key: Key) -> Option<String> {
        let mut text = self.text.clone()?;

        match key {
            Key::Character(letter) => text.push(letter),
            Key::Space => text.push(' '),
            Key::Backspace => {
                text.pop();
            }
            _ => return None,
        }

        Some(text)
    }
}

impl Browse {
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
 * expand the shown six weeks off the draw thread; recurrence rules can exceed a frame's work limit.
 * Keep the old month through sync until re-expanded, including on failure; removed files' events
 * disappear once expansion completes
 */
fn month_of(calendars: &Calendars, start: NaiveDate) -> Shown {
    let wanted = Wanted::of(calendars, start);
    let mut asked = ASKED.locked();

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
            let mut asked = ASKED.locked();

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
            let mut asked = ASKED.locked();

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
    let mut asked = ASKED.locked();

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
    let adding = Adding::read().of(visit);
    let day = browse.day(today);
    let start = grid_start(day);
    let Shown { month, pending } = month_of(&calendars, start);
    let browse = browse.bounded(month.on(day).len());

    let shape = geometry::CALENDAR;

    // a Module that is off reads no Service
    let google = modules::on("google-calendar")
        .then(|| google::Account::read().problem().cloned())
        .flatten();
    // a grant that is gone is mended in Settings: the agenda offers the way there as a button
    let reauth = matches!(
        google,
        Some(google::Problem::Revoked | google::Problem::SignIn(_))
    );
    let account = google
        .as_ref()
        .map(google::Problem::brief)
        .map(str::to_owned);
    let feeds = modules::on("calendar-subscriptions")
        .then(|| Subscriptions::read().brief())
        .flatten();
    let feeds_only = feeds.clone();
    let account = match (account, feeds) {
        (Some(account), Some(feeds)) => Some(format!("{account} \u{b7} {feeds}")),
        (account, feeds) => account.or(feeds),
    };
    let account = account.as_deref();
    // the button says it, so the footer does not repeat it
    let footer = if reauth && calendars.files != 0 {
        feeds_only.as_deref()
    } else {
        account
    };

    let agenda: Box<dyn Widget> = if let Some(text) = &adding.text {
        Box::new(entry(
            day,
            today,
            text,
            adding.problem.as_deref(),
            adding.saving.is_some(),
        ))
    } else if calendars.files == 0 {
        Box::new(state(
            monitor,
            "No calendars",
            account.unwrap_or("Add an event, or connect an account in Settings"),
        ))
    } else {
        Box::new(agenda(
            monitor,
            day,
            today,
            &month,
            pending,
            &browse,
            (footer, adding.problem.as_deref(), reauth),
        ))
    };

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::Region, "calendar"))
        .child(
            Row::new(vec![
                Box::new(rise(0, grid(monitor, day, today, &month, &browse))) as Box<dyn Widget>,
                // a hairline in the gap, so the month and the agenda read as two panes
                Box::new(
                    Rectangle::new()
                        .width(1.0)
                        .height(shape.height - 2.0 * INSET)
                        .fill(theme::island().surface_container_high),
                ),
                Box::new(rise(1, agenda)),
            ])
            .gap((SIDE_GAP - 1.0) / 2.0),
        )
}

// the month's name with a chevron each side, its weekdays and its six weeks
fn grid(monitor: &str, day: NaiveDate, today: NaiveDate, month: &Month, browse: &Browse) -> Column {
    let title = Text::new(format!("{} {}", MONTHS[day.month0() as usize], day.year()))
        .size(theme::text::TITLE)
        .color(theme::island().on_surface)
        .weight(theme::text::SEMIBOLD);

    let header = Row::new(children![
        turn(monitor, "previous", Icon::Back, 'p'),
        title,
        turn(monitor, "next", Icon::Forward, 'n'),
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
fn turn(monitor: &str, node: &'static str, icon: Icon, key: char) -> Rectangle {
    let monitor = monitor.to_owned();

    Rectangle::new()
        .width(TARGET)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .semantics(|| Semantics::new(Role::Button, node))
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
        .height(WEEK)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .semantics(|| {
            // the date is the shell's own: `2026-10-15`
            Semantics::new(Role::Button, "day")
                .label(date.to_string())
                .selected(chosen)
        })
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
    monitor: &str,
    day: NaiveDate,
    today: NaiveDate,
    month: &Month,
    pending: Option<Pending>,
    browse: &Browse,
    (account, problem, reauth): (Option<&str>, Option<&str>, bool),
) -> Column {
    let events = month.on(day);
    let partial = month.events.partial;

    let title = Text::new(named(day, today))
        .size(theme::text::TITLE)
        .color(theme::island().on_surface)
        .weight(theme::text::SEMIBOLD)
        .elide();

    let mut head: Vec<Box<dyn Widget>> = Vec::new();
    let room = AGENDA - ADD_WIDTH - GAP;
    let room = if reauth {
        room - SIGN_IN_WIDTH - GAP
    } else {
        room
    };

    head.push(Box::new(
        Rectangle::new()
            .width(room)
            .height(HEADER)
            .align_child(Start, Center)
            .child(title),
    ));
    if reauth {
        head.push(Box::new(sign_in_pill(monitor)));
    }
    head.push(Box::new(add_pill(monitor)));

    let header = Row::new(head)
        .width(AGENDA)
        .height(HEADER)
        .gap(GAP)
        .align(Center);

    let list: Box<dyn Widget> = if events.is_empty() {
        let quiet = theme::island().on_surface_variant;
        let message = Text::new(match pending {
            Some(Pending::Loading) => format!("Loading {} events", day.format("%B")),
            Some(Pending::Failed) => String::from("Events could not be read"),
            None if partial => String::from("Some events not shown"),
            None => String::from("No events"),
        })
        .size(theme::text::BODY)
        .color(quiet)
        .weight(theme::text::MEDIUM);

        // an empty day, not a failure or a wait, is marked with the calendar's own glyph
        let mut parts: Vec<Box<dyn Widget>> = Vec::new();
        if pending.is_none() && !partial {
            parts.push(Box::new(Icon::Calendar.on(24.0, quiet)));
        }
        parts.push(Box::new(message));

        Box::new(
            Rectangle::new()
                .width(AGENDA)
                .height(LIST)
                .align_child(Center, Center)
                .child(Column::new(parts).gap(8.0).align(Center)),
        )
    } else {
        let end = (browse.first + ROWS).min(events.len());

        Box::new(
            Rectangle::new()
                .width(AGENDA)
                .height(LIST)
                .align_child(Start, Start)
                .semantics(|| Semantics::new(Role::List, "events"))
                .on_scroll(|Scroll { y, .. }| wheel(y))
                .child(
                    Column::new(
                        events[browse.first..end]
                            .iter()
                            .zip(browse.first..)
                            .map(|(&event, at)| {
                                let ring = browse.agenda && at == browse.selected;
                                let event = &month.events.occurrences[event];

                                Box::new(row(monitor, event, day, ring)) as Box<dyn Widget>
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
    // what the ring may do with the event it is on
    let count = match events
        .get(browse.selected)
        .filter(|_| browse.agenda)
        .map(|&at| &month.events.occurrences[at])
    {
        Some(event) if event.added.is_none() => String::from("Read-only here"),
        Some(event) if confirm::fill(removal(event)) > 0.0 => {
            if confirm::pointer(removal(event)) {
                String::from("Keep holding to remove")
            } else {
                String::from("Keep holding d to remove")
            }
        }
        Some(_) => String::from("e changes, hold d to remove"),
        None => count,
    };
    let count = match account {
        Some(problem) if count.is_empty() => problem.to_owned(),
        Some(problem) => format!("{count} \u{b7} {problem}"),
        None => count,
    };
    // what the last removal could not do
    let (count, ink) = match problem {
        Some(problem) => (problem.to_owned(), theme::signals().warning),
        None => (count, theme::island().on_surface_variant),
    };

    Column::new(vec![
        Box::new(header) as Box<dyn Widget>,
        list,
        Box::new(
            Text::new(count)
                .size(theme::text::LABEL_SMALL)
                .color(ink)
                .weight(theme::text::MEDIUM)
                .elide(),
        ),
    ])
    .width(AGENDA)
    .height(HEIGHT)
    .gap(GAP)
}

// icon only, so the day's title keeps its room; it is named "sign-in" for `kanade ui`
const SIGN_IN_WIDTH: f32 = 28.0;
const SIGN_IN_ICON: f32 = 14.0;

// the way to mend the Google grant: a button to the Settings page that signs in again
fn sign_in_pill(monitor: &str) -> Rectangle {
    let monitor = monitor.to_owned();
    let ink = theme::signals().warning;

    super::pill_face(
        Rectangle::new()
            .width(SIGN_IN_WIDTH)
            .height(ADD_HEIGHT)
            .radius(ADD_HEIGHT / 2.0),
    )
    .border(1.0, ink)
    .align_child(Center, Center)
    .cursor(Cursor::Pointer)
    .semantics(|| Semantics::new(Role::Button, "sign-in").label("Sign in to Google Calendar"))
    .on_click(super::on_left(move || {
        search::open_settings(&monitor, search::DATA)
    }))
    .child(Icon::Lock.on(SIGN_IN_ICON, ink))
}

const ADD_WIDTH: f32 = 60.0;
const ADD_HEIGHT: f32 = 28.0;

// starts an event, as `a` does
fn add_pill(monitor: &str) -> Rectangle {
    let monitor = monitor.to_owned();

    super::pill_face(
        Rectangle::new()
            .width(ADD_WIDTH)
            .height(ADD_HEIGHT)
            .radius(ADD_HEIGHT / 2.0),
    )
    .align_child(Center, Center)
    .cursor(Cursor::Pointer)
    .semantics(|| Semantics::new(Role::Button, "new"))
    .on_click(super::on_left(move || add_event(&monitor)))
    .child(
        Text::new("+ Add")
            .size(theme::text::LABEL_SMALL)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD),
    )
}

/*
 * the agenda while an event is typed for `day`: the day's name, the field with what is typed, and
 * under it what the text can say, or what the last Enter could not save
 */
fn entry(
    day: NaiveDate,
    today: NaiveDate,
    text: &str,
    problem: Option<&str>,
    saving: bool,
) -> Column {
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

    let hint = |line: &str, ink| {
        Box::new(
            Text::new(line)
                .size(theme::text::LABEL_SMALL)
                .color(ink)
                .weight(theme::text::MEDIUM)
                .elide(),
        ) as Box<dyn Widget>
    };
    let quiet = theme::island().on_surface_variant;

    let mut lines = vec![
        Box::new(search::field(Icon::Calendar, text, "14:30 Dentist", AGENDA)) as Box<dyn Widget>,
    ];
    lines.push(match problem {
        _ if saving => hint("Saving", quiet),
        Some(problem) => hint(problem, theme::signals().warning),
        None => hint("Enter saves, Esc cancels", quiet),
    });
    lines.push(hint("2026-10-15 moves it; else today", quiet));
    lines.push(hint("14:30 or 9:00-10:30; else all day", quiet));

    Column::new(vec![
        Box::new(header) as Box<dyn Widget>,
        Box::new(Column::new(lines).width(AGENDA).gap(GAP)),
    ])
    .width(AGENDA)
    .height(HEIGHT)
    .gap(GAP)
}

// the room an owned event's Edit and Remove pills take at the end of its row
const CHIP_WIDTH: f32 = 52.0;
const CHIP_HEIGHT: f32 = 21.0;
const CHIP_GAP: f32 = 4.0;

// a pill of an owned event's row, named for `kanade ui`
fn chip(label: &str, node: &'static str, ink: kanade_runtime::Color) -> Rectangle {
    Rectangle::new()
        .width(CHIP_WIDTH)
        .height(CHIP_HEIGHT)
        .radius(CHIP_HEIGHT / 2.0)
        .border(1.0, ink)
        .clip()
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .semantics(|| Semantics::new(Role::Button, node))
        .child(
            Text::new(label)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface)
                .weight(theme::text::SEMIBOLD),
        )
}

/*
 * what it is and when on `day`, and where. An event added here also has Edit, as `e`, and Remove,
 * which is held like `d` (ADR 0085): pressed by name or tapped it removes nothing
 */
fn row(monitor: &str, event: &Occurrence, day: NaiveDate, ring: bool) -> Rectangle {
    let title = if event.title.is_empty() {
        "Untitled event"
    } else {
        &event.title
    };

    let detail = match &event.location {
        Some(location) => format!("{} \u{b7} {location}", when(event, day)),
        None => when(event, day),
    };

    let owned = event.added.is_some();
    let reach = if owned {
        AGENDA - CHIP_WIDTH - ROW_INSET
    } else {
        AGENDA
    };

    let text = Rectangle::new()
        .width(reach)
        .height(ROW)
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
        );

    let content: Box<dyn Widget> = match &event.added {
        Some(stem) => {
            let (monitor, was, stem) = (monitor.to_owned(), event.clone(), stem.clone());
            let quiet = theme::island().outline;

            let edit = chip("Edit", "edit", quiet).on_click(super::on_left({
                let (monitor, was) = (monitor.clone(), was.clone());

                move || edit(&monitor, &was)
            }));
            let remove = confirm::target(
                chip("Remove", "remove", theme::signals().critical),
                ("calendar remove", stem.clone()),
                {
                    let visit = IslandService::read().visit();

                    Arc::new(move || remove(visit, &stem, &was))
                },
            );

            Box::new(
                Row::new(children![
                    text,
                    Column::new(children![edit, remove]).gap(CHIP_GAP),
                ])
                .width(AGENDA)
                .height(ROW)
                .align(Center),
            )
        }
        None => Box::new(text),
    };

    // the fill of a removal being held grows behind the text
    Rectangle::new()
        .width(AGENDA)
        .height(ROW)
        .radius(radius::ROW)
        .fill(theme::island().surface_container)
        .clip()
        .child(confirm::behind(
            removal(event),
            AGENDA,
            ROW,
            theme::signals().critical,
            content,
        ))
        .border_if(ring)
        .semantics(|| {
            Semantics::new(Role::Item, "event")
                .content(title)
                .selected(ring)
        })
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
fn state(monitor: &str, title: &str, detail: &str) -> Rectangle {
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
                add_pill(monitor),
                search::settings_pill(monitor, search::DATA),
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

    let adding = Adding::read().of(IslandService::read().visit());

    if adding.text.is_some() {
        return typing(monitor, adding, key);
    }

    // what a removal could not do is said until the next key
    if adding.problem.is_some() {
        store(Adding {
            problem: None,
            ..adding
        });
    }
    if key == Key::Character('a') {
        add_event(monitor);
        return true;
    }
    if added(monitor, key) {
        return true;
    }

    pressed(monitor, key)
}

// the event typed so far takes the key, Enter saving it on the chosen day and Escape dropping it
fn typing(monitor: &str, adding: Adding, key: Key) -> bool {
    let mut write = None;
    let next = match key {
        _ if adding.saving.is_some() => adding,
        Key::Escape => Adding {
            text: None,
            replacing: None,
            problem: None,
            ..adding
        },
        Key::Enter => {
            let (next, saving) = save(adding);

            write = saving;
            next
        }
        key => match adding.typed(key) {
            Some(text) => Adding {
                text: Some(text),
                problem: None,
                ..adding
            },
            None => adding,
        },
    };

    store(next);
    if let Some(write) = write {
        write.start();
    }
    IslandService::write().attend(monitor, Instant::now());

    true
}

// the typed event as saving, with the write to start once that is stored, else what to correct in it
fn save(adding: Adding) -> (Adding, Option<Write>) {
    let day = Browse::read().of(adding.visit).day(clock::today());
    let text = adding.text.as_deref().unwrap_or_default();

    match add::parse(text, day) {
        Ok(event) => {
            let id = SAVES.fetch_add(1, Ordering::Relaxed);
            let write = Write {
                visit: adding.visit,
                id,
                replacing: adding.replacing.clone(),
                event,
            };

            (
                Adding {
                    problem: None,
                    saving: Some(id),
                    ..adding
                },
                Some(write),
            )
        }
        Err(problem) => (
            Adding {
                problem: Some(problem),
                ..adding
            },
            None,
        ),
    }
}

// one save: which it is, so only its own end settles the field that waits on it
struct Write {
    visit: u64,
    id: u64,
    replacing: Option<Replacing>,
    event: add::Event,
}

impl Write {
    /*
     * writes the file on a thread of its own, as a slow disk must not stall the frames, nor the
     * volume keys on the control worker; `settled` says how it went. After `Adding` says it is
     * saving, so an end never comes before it
     */
    fn start(self) {
        let (visit, id) = (self.visit, self.id);

        let write = move || {
            let written = match &self.replacing {
                Some(Replacing { stem, was }) => add::replace(stem, &self.event, was),
                None => add::save(&self.event),
            };

            settled(visit, id, written.map_err(|error| add::say(&error, "save")));
        };

        if let Err(error) = thread::Builder::new()
            .name(String::from("calendar-write"))
            .spawn(write)
        {
            settled(visit, id, Err(format!("Could not save: {error}")));
        }
    }
}

// the text is dropped once it is saved, and kept with what went wrong otherwise
fn settled(visit: u64, id: u64, written: Result<(), String>) {
    let mut adding = Adding::write();

    if adding.visit != visit || adding.saving != Some(id) {
        return;
    }

    match written {
        Ok(()) => *adding = Adding::begin(visit),
        Err(problem) => {
            adding.saving = None;

            // the file changed under us: the field takes the event as it now reads, or drops
            // what it cannot say, so the next Enter does not refuse on the old baseline again
            if problem == add::CHANGED {
                match adding.replacing.as_ref().and_then(|r| add::reread(&r.stem)) {
                    Some(fresh) => {
                        adding.text = Some(add::text(&fresh));
                        adding.replacing = Some(Replacing {
                            stem: fresh.added.clone().unwrap_or_default(),
                            was: fresh,
                        });
                    }
                    None => {
                        adding.text = None;
                        adding.replacing = None;
                    }
                }
            }

            adding.problem = Some(problem);
        }
    }
}

// an empty event to type, on the visit open now
fn add_event(monitor: &str) {
    change(monitor, String::new(), None);
}

// the Calendar Surface is open: its watches go up once, when the Module starts
pub fn start() {
    service::watch::<IslandService>(follow);
}

/*
 * the edit field belongs to a visit of the Surface, and to the Surface alone: once it is not
 * showing, any open field, its text and its baseline are over, so reopening starts fresh
 */
fn follow() {
    let open = IslandService::read().surface() == Some(Surface::Calendar);
    let adding = Adding::read();

    if !open && (adding.text.is_some() || adding.replacing.is_some() || adding.problem.is_some()) {
        drop(adding);
        store(Adding::default());
    }
}

// `text` to type on the visit open now, in place of the event `replacing`, if any
fn change(monitor: &str, text: String, replacing: Option<Replacing>) {
    store(Adding {
        visit: IslandService::read().visit(),
        text: Some(text),
        replacing,
        problem: None,
        saving: None,
    });
    IslandService::write().attend(monitor, Instant::now());
}

/*
 * `e` changes and `d` held removes the event the ring is on, if it was added here (ADR 0045,
 * ADR 0085); false for any other key or event
 */
fn added(monitor: &str, key: Key) -> bool {
    let visit = IslandService::read().visit();
    let browse = Browse::read().of(visit);

    if !browse.agenda {
        return false;
    }

    let Some(event) = ringed(&browse) else {
        return false;
    };
    if event.added.is_none() {
        return false;
    }

    match key {
        Key::Character('e') => edit(monitor, &event),
        Key::Character('d') => hold(monitor, event),
        _ => return false,
    }

    true
}

// opens the field with `event` as typed, to change it; as `e` does, for an event added here
fn edit(monitor: &str, event: &Occurrence) {
    let Some(stem) = event.added.clone() else {
        return;
    };

    change(
        monitor,
        add::text(event),
        Some(Replacing {
            stem,
            was: event.clone(),
        }),
    );
}

// starts holding the removal of `event`, which a release before `HOLD` drops (ADR 0085)
fn hold(monitor: &str, event: Occurrence) {
    let Some(stem) = event.added.clone() else {
        return;
    };

    // once kept, the file is removed if it is still the event shown, else the agenda says so
    let visit = IslandService::read().visit();
    let what = ("calendar remove", stem.clone());

    confirm::press(what, Arc::new(move || remove(visit, &stem, &event)));
    IslandService::write().attend(monitor, Instant::now());
}

// what holding `d` on an event is held as, so the fill shows on its own row
fn removal(event: &Occurrence) -> (&'static str, &str) {
    (
        "calendar remove",
        event.added.as_deref().unwrap_or_default(),
    )
}

// the file of an event removed, on the thread that held `d` or the pointer
fn remove(visit: u64, stem: &str, event: &Occurrence) {
    let Err(error) = add::remove(stem, event) else {
        return;
    };

    // what failed, never the event
    eprintln!(
        "kanade: calendar: could not remove an event ({})",
        error.kind()
    );

    let mut adding = Adding::write();

    if adding.visit != visit {
        *adding = Adding::begin(visit);
    }
    if adding.text.is_none() {
        adding.problem = Some(add::say(&error, "remove"));
    }
}

// the event the ring is on, none outside the agenda
fn ringed(browse: &Browse) -> Option<Occurrence> {
    let day = browse.day(clock::today());
    let shown = month_of(&Calendars::read().clone(), grid_start(day));
    let at = *shown.month.on(day).get(browse.selected)?;

    shown.month.events.occurrences.get(at).cloned()
}

// a key or the press that stands for one, on the visit open now
fn pressed(monitor: &str, key: Key) -> bool {
    // Enter on the empty state presses its way out
    if key == Key::Enter && Calendars::read().files == 0 {
        search::open_settings(monitor, search::DATA);
        return true;
    }

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

    // read apart, since `store` writes
    let browse = Browse::read().of(visit);

    store(Browse {
        day: Some(date),
        agenda: false,
        selected: 0,
        first: 0,
        ..browse
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

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use chrono::{NaiveTime, TimeDelta};

    use super::*;

    const TEST: &str = "test";

    // `Adding` and the island are one global Service each, so tests that write them take this in turn
    fn field_tests() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    // pressing a day wrote the Browse it still held a read of, which hung the UI thread for good
    #[test]
    fn pressing_a_day_chooses_it_without_deadlocking() {
        let date = NaiveDate::from_ymd_opt(2026, 10, 9).unwrap();
        let (done, finished) = mpsc::channel();

        std::thread::spawn(move || {
            choose("test", date);
            let _ = done.send(());
        });

        finished
            .recv_timeout(Duration::from_secs(5))
            .expect("choose hung");
        assert_eq!(Browse::read().day, Some(date));
    }

    // the field of a visit is over once the Surface is not showing, however it stopped showing
    #[test]
    fn an_open_field_is_dropped_once_the_surface_is_not_showing() {
        let _lock = field_tests();

        // a visit with a field open, as typing leaves it
        IslandService::write().open(TEST, Surface::Calendar, Instant::now());
        let visit = IslandService::read().visit();
        store(Adding {
            visit,
            text: Some(String::from("Dentist")),
            problem: Some(String::from("Changed elsewhere")),
            ..Adding::default()
        });

        // the Surface still shows: the field stays
        follow();
        assert!(Adding::read().text.is_some());

        // another Surface in its place, as any collapse leaves it
        IslandService::write().open(TEST, Surface::Controls, Instant::now());
        follow();

        let text = Adding::read().text.clone();

        assert_eq!(text, None);
    }

    // after Changed elsewhere the field takes the event as the file now reads, or drops what it
    // cannot say, so the next Enter does not refuse on the old baseline again
    #[test]
    fn a_change_elsewhere_retakes_the_event_or_drops_the_field() {
        let _lock = field_tests();
        let start = NaiveDate::from_ymd_opt(2026, 10, 24)
            .unwrap()
            .and_time(NaiveTime::from_hms_opt(14, 30, 0).unwrap());
        let visit = IslandService::read().visit();

        *Adding::write() = Adding {
            visit,
            text: Some(String::from("Dentist")),
            replacing: Some(Replacing {
                stem: String::from("elsewhere-1"),
                was: Occurrence {
                    title: String::from("Dentist"),
                    location: None,
                    start,
                    end: start + TimeDelta::hours(1),
                    span: (0, 3600),
                    all_day: false,
                    added: Some(String::from("elsewhere-1")),
                },
            }),
            problem: None,
            saving: Some(7),
        };

        // nothing to re-read, as when the event was deleted: the stale field is dropped
        settled(visit, 7, Err(String::from(add::CHANGED)));

        // read apart, since the next case writes
        let (text, replacing, problem, saving) = {
            let adding = Adding::read();

            (
                adding.text.clone(),
                adding.replacing.clone(),
                adding.problem.clone(),
                adding.saving,
            )
        };

        assert_eq!(saving, None);
        assert_eq!(problem.as_deref(), Some(add::CHANGED));
        assert_eq!(text, None);
        assert_eq!(replacing, None);

        // what went wrong in the writing itself keeps the text to correct
        *Adding::write() = Adding {
            visit,
            text: Some(String::from("Dentist")),
            saving: Some(8),
            ..Adding::default()
        };
        settled(visit, 8, Err(String::from("Could not save: disk full")));

        let (text, problem) = {
            let adding = Adding::read();

            (adding.text.clone(), adding.problem.clone())
        };

        assert_eq!(text.as_deref(), Some("Dentist"));
        assert_eq!(problem.as_deref(), Some("Could not save: disk full"));
    }

    // a save of an earlier field, or one already settled, changes nothing
    #[test]
    fn a_stale_save_settles_nothing() {
        let _lock = field_tests();
        let visit = IslandService::read().visit();

        *Adding::write() = Adding {
            visit,
            text: Some(String::from("Dentist")),
            saving: Some(9),
            ..Adding::default()
        };

        settled(visit + 1, 9, Ok(()));
        assert_eq!(Adding::read().text.as_deref(), Some("Dentist"));

        settled(visit, 10, Ok(()));
        assert_eq!(Adding::read().text.as_deref(), Some("Dentist"));
    }
}
