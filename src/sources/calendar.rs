//! Local calendars (#150, ADR 0015, docs/design.md Calendar): the events of the iCalendar (`.ics`)
//! files under `calendar.paths`, else `$XDG_DATA_HOME/calendars`, which is where vdirsyncer and
//! khal keep theirs, a directory of `.ics` files for each calendar. Kanade only reads them; nothing
//! here talks to a network or holds an account.
//!
//! The files are read on this source's thread, parsed whole into `Calendars`, and read again
//! after a change under a watched place settles, or a config reload. The watch is inotify on each
//! directory, each subdirectory of it and each file named on its own, or the nearest parent while
//! one does not exist yet, so watching costs no idle wakeups.
//!
//! A view asks `occurrences` for the days it shows: recurrences, their exceptions and time zones
//! come from calcard, each moment then becomes local time through `clock`. A rule is started again
//! just before those days, so one begun long ago expands near them, not from its first occurrence.
//! An occurrence keeps its absolute start and end for order and length; its local times are for
//! showing, and read backwards across a change back from summer time.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use amane::Service;
use calcard::common::PartialDateTime;
use calcard::common::timezone::Tz;
use calcard::icalendar::dates::TimeOrDelta;
use calcard::icalendar::timezone::TzResolver;
use calcard::icalendar::{
    ICalendar, ICalendarComponent, ICalendarComponentType, ICalendarDay, ICalendarFrequency,
    ICalendarMonth, ICalendarParameterName, ICalendarProperty, ICalendarRecurrenceRule,
    ICalendarStatus, ICalendarValue, ICalendarWeekday,
};
use calcard::{Entry, Parser};
use chrono::{
    DateTime, Datelike, Days, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, TimeZone, Timelike,
    Weekday,
};
use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask, Watches};

use crate::{clock, config, supervise};

// where the calendars are without `calendar.paths`, under the XDG data directory
const DIRECTORY: &str = "calendars";

// a save is several events, and a sync writes many files at once
const DEBOUNCE: Duration = Duration::from_millis(150);

// how deep a directory's subdirectories are read, past any nesting a sync tool makes
const DEPTH: usize = 8;

// a file larger is no calendar a person keeps, and is not read
const LARGEST: u64 = 32 * 1024 * 1024;

// the occurrences one file may expand to for a range, so one runaway rule costs a bounded time
const LIMIT: usize = 100_000;

// the calendars as last read
#[derive(Clone, Default)]
pub struct Calendars {
    // each read, so a view knows to ask again
    pub generation: u64,

    // the `.ics` files found, read or not
    pub files: usize,

    calendars: Arc<[ICalendar]>,
}

impl Service for Calendars {
    fn new() -> Self {
        Calendars::default()
    }

    fn listen() {}
}

impl Calendars {
    // the events that fall on the days from `from` until `to`, in local time
    pub fn occurrences(&self, from: NaiveDate, to: NaiveDate) -> Vec<Occurrence> {
        occurrences(&self.calendars, from, to, clock::local_time)
    }
}

// one time an event happens, in local time
#[derive(Debug, Clone, PartialEq)]
pub struct Occurrence {
    pub title: String,
    pub location: Option<String>,

    // what the local clock reads as it starts, for showing
    pub start: NaiveDateTime,

    // what it reads the moment after; an all-day one ends at the midnight after its last day, a
    // moment with no length at its start. Across a change back from summer time it can read
    // before the start
    pub end: NaiveDateTime,

    // its start and end in seconds since the epoch, which order it and give its length
    pub span: (i64, i64),
    pub all_day: bool,
}

impl Occurrence {
    // whether it falls on `day`, a moment on the day it is at
    pub fn on(&self, day: NaiveDate) -> bool {
        let (start, end) = bounds(day, day.succ_opt().unwrap_or(day));

        if self.moment() {
            return self.start.date() == day;
        }

        self.start.min(self.end) < end && self.start.max(self.end) > start
    }

    // whether it has no length
    pub fn moment(&self) -> bool {
        self.span.0 == self.span.1
    }
}

// the watch over the places, shared so a config reload re-arms it from its own thread
struct Watch {
    watches: Watches,
    wants: HashMap<WatchDescriptor, Vec<Want>>,
}

// held for a whole read, so the watch and a reload never interleave
static WATCH: Mutex<Option<Watch>> = Mutex::new(None);

// what makes an event under one watch a change to the calendars
#[derive(Debug, Clone, PartialEq)]
enum Want {
    // an `.ics` file or a subdirectory appearing, changing or going in a calendar directory
    Calendars,

    // this entry: a file named on its own, or the next directory on the way to a missing place
    Named(OsString),

    // anything to the watched file itself
    Any,
}

const WATCHED_DIRECTORY: WatchMask = WatchMask::CREATE
    .union(WatchMask::CLOSE_WRITE)
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::MOVED_FROM)
    .union(WatchMask::DELETE)
    .union(WatchMask::DELETE_SELF)
    .union(WatchMask::MOVE_SELF)
    .union(WatchMask::ONLYDIR);

const WATCHED_FILE: WatchMask = WatchMask::CLOSE_WRITE
    .union(WatchMask::DELETE_SELF)
    .union(WatchMask::MOVE_SELF);

/*
 * reads the calendars and follows them, on its own thread; one that cannot watch reads them once,
 * and again only on a config reload
 */
pub fn follow() {
    if let Err(error) = watch() {
        *WATCH.lock().unwrap_or_else(PoisonError::into_inner) = None;

        let why = format!("calendar changes are not followed, reload by hand: {error}");

        eprintln!("kanade: {why}");
        supervise::stopped("calendar", why);
        reread();
    }
}

/*
 * reads the calendars again from the places the config names and watches those places as they
 * are now; on the watch's thread after a change, on the reload's after a config reload
 */
pub fn reread() {
    let mut watch = WATCH.lock().unwrap_or_else(PoisonError::into_inner);
    let found = find(&places(&config::get().calendars, data_home().as_deref()));

    if let Some(watch) = watch.as_mut() {
        arm(watch, &found.targets);
    }

    let mut calendars = Vec::new();
    for file in &found.files {
        match read(file) {
            Ok(read) => calendars.extend(read),
            // only where and why: a calendar's contents are the user's own
            Err(why) => eprintln!("kanade: calendar {}: {why}", file.display()),
        }
    }

    let mut service = Calendars::write();
    service.generation += 1;
    service.files = found.files.len();
    service.calendars = calendars.into();
}

fn watch() -> io::Result<()> {
    let mut inotify = Inotify::init()?;
    let mut buffer = [0; 4096];

    *WATCH.lock().unwrap_or_else(PoisonError::into_inner) = Some(Watch {
        watches: inotify.watches(),
        wants: HashMap::new(),
    });
    reread();

    loop {
        loop {
            let events = inotify.read_events_blocking(&mut buffer)?;

            if events.into_iter().any(|event| relevant(&event)) {
                break;
            }
        }

        // until a quiet DEBOUNCE, whatever came in it
        let mut quiet = Instant::now() + DEBOUNCE;

        while let Some(wait) = quiet.checked_duration_since(Instant::now()) {
            thread::sleep(wait);

            match inotify.read_events(&mut buffer) {
                Ok(mut events) => {
                    if events.any(|event| relevant(&event)) {
                        quiet = Instant::now() + DEBOUNCE;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
        }

        reread();
    }
}

// the places to read: the config's, else the default directory
fn places(configured: &[String], data_home: Option<&Path>) -> Vec<PathBuf> {
    if configured.is_empty() {
        return data_home
            .map(|home| vec![home.join(DIRECTORY)])
            .unwrap_or_default();
    }

    configured.iter().map(PathBuf::from).collect()
}

// $XDG_DATA_HOME, else ~/.local/share, as the XDG spec has it
fn data_home() -> Option<PathBuf> {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| env::var_os("HOME").map(|home| Path::new(&home).join(".local/share")))
}

// the `.ics` files under the places, and what to watch so a change to any shows
#[derive(Debug, Default, PartialEq)]
struct Found {
    files: Vec<PathBuf>,
    targets: Vec<(PathBuf, WatchMask, Want)>,
}

fn find(places: &[PathBuf]) -> Found {
    let mut found = Found::default();
    let mut seen = HashSet::new();

    for place in places {
        match fs::metadata(place) {
            Ok(metadata) if metadata.is_dir() => walk(place, 0, &mut seen, &mut found),
            Ok(_) => {
                if let (Some(dir), Some(name)) = (place.parent(), place.file_name()) {
                    found.targets.push((
                        dir.to_owned(),
                        WATCHED_DIRECTORY,
                        Want::Named(name.to_owned()),
                    ));
                }

                found.targets.push((place.clone(), WATCHED_FILE, Want::Any));

                if !found.files.contains(place) {
                    found.files.push(place.clone());
                }
            }
            Err(_) => {
                if let Some((dir, name)) = nearest(place) {
                    found
                        .targets
                        .push((dir.to_owned(), WATCHED_DIRECTORY, Want::Named(name)));
                }
            }
        }
    }

    found
}

// a directory's `.ics` files and its subdirectories', each directory once however it is reached
fn walk(dir: &Path, depth: usize, seen: &mut HashSet<PathBuf>, found: &mut Found) {
    let Ok(real) = dir.canonicalize() else {
        return;
    };
    if !seen.insert(real) {
        return;
    }

    found
        .targets
        .push((dir.to_owned(), WATCHED_DIRECTORY, Want::Calendars));

    let Ok(entries) = dir.read_dir() else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    entries.sort();

    for path in entries {
        // followed through a symlink, as a sync tool may link a calendar in
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };

        if metadata.is_dir() {
            if depth < DEPTH {
                walk(&path, depth + 1, seen, found);
            }
        } else if metadata.is_file() && ics(path.as_os_str()) && !found.files.contains(&path) {
            found.files.push(path);
        }
    }
}

fn ics(name: &OsStr) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ics"))
}

// the nearest parent of a missing place that exists, and the entry in it on the way to the place
fn nearest(place: &Path) -> Option<(&Path, OsString)> {
    let mut path = place;

    loop {
        let name = path.file_name()?.to_owned();
        let dir = path.parent()?;
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };

        if dir.is_dir() {
            return Some((dir, name));
        }

        path = dir;
    }
}

// the watches for the places as they are now, the old ones removed
fn arm(watch: &mut Watch, targets: &[(PathBuf, WatchMask, Want)]) {
    for (old, _) in watch.wants.drain() {
        // fails for a watch the kernel already ended
        let _ = watch.watches.remove(old);
    }

    for (path, mask, want) in targets {
        match watch.watches.add(path, *mask) {
            Ok(descriptor) => watch
                .wants
                .entry(descriptor)
                .or_default()
                .push(want.clone()),
            Err(error) => eprintln!("kanade: cannot watch {}: {error}", path.display()),
        }
    }
}

/*
 * an event a current watch wants; an overflow, which comes from no watch, may have dropped a
 * change, so everything is read again
 */
fn relevant(event: &inotify::Event<&OsStr>) -> bool {
    if event.mask.contains(EventMask::Q_OVERFLOW) {
        return true;
    }

    let watch = WATCH.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(wants) = watch.as_ref().and_then(|watch| watch.wants.get(&event.wd)) else {
        return false;
    };

    wanted(wants, event.mask, event.name)
}

fn wanted(wants: &[Want], mask: EventMask, name: Option<&OsStr>) -> bool {
    let ended = EventMask::DELETE_SELF | EventMask::MOVE_SELF | EventMask::IGNORED;

    mask.intersects(ended)
        || wants.iter().any(|want| match (want, name) {
            (Want::Any, _) => true,
            (Want::Calendars, Some(name)) => mask.contains(EventMask::ISDIR) || ics(name),
            (Want::Named(wanted), Some(name)) => name == wanted,
            (_, None) => false,
        })
}

// the calendars in one file, or why it has none
fn read(file: &Path) -> Result<Vec<ICalendar>, String> {
    let size = fs::metadata(file).map_err(|error| error.to_string())?.len();
    if size > LARGEST {
        return Err(format!("over {} MiB, not read", LARGEST / 1024 / 1024));
    }

    let text = fs::read_to_string(file).map_err(|error| error.to_string())?;

    let mut calendars: Vec<ICalendar> = parse(&text)?.into_iter().flat_map(series).collect();
    for calendar in &mut calendars {
        if !counted(calendar) {
            eprintln!(
                "kanade: calendar {}: a rule with over {COUNTED} occurrences runs from its start",
                file.display()
            );
        }
        if crowded(calendar, clock::today()) {
            eprintln!(
                "kanade: calendar {}: a rule shows only its first {LIMIT} occurrences",
                file.display()
            );
        }
    }

    Ok(calendars)
}

fn parse(text: &str) -> Result<Vec<ICalendar>, String> {
    let mut parser = Parser::new(text);
    let mut calendars = Vec::new();
    let mut problem = None;

    loop {
        match parser.entry() {
            Entry::ICalendar(calendar) => calendars.push(calendar),
            Entry::Eof => break,
            // what the line said stays unsaid, as it is the user's
            Entry::InvalidLine(_) => {
                problem.get_or_insert("a line that is not iCalendar");
            }
            Entry::UnexpectedComponentEnd { .. } | Entry::UnterminatedComponent(_) => {
                problem.get_or_insert("a component left open");
            }
            Entry::TooManyComponents => {
                problem.get_or_insert("too many components");
                break;
            }
            // a vCard, or what a later calcard adds, is no event
            _ => {}
        }
    }

    match (calendars.is_empty(), problem) {
        (false, _) => Ok(calendars),
        (true, Some(problem)) => Err(format!("not read: {problem}")),
        (true, None) => Err(String::from("no VCALENDAR in it")),
    }
}

/*
 * the events of `calendars` on the days from `from` until `to`, sorted by start with all-day ones
 * first; `local` gives the local time of seconds since the epoch, so a test picks the zone
 */
fn occurrences(
    calendars: &[ICalendar],
    from: NaiveDate,
    to: NaiveDate,
    local: impl Fn(i64) -> Option<NaiveDateTime>,
) -> Vec<Occurrence> {
    let (start, end) = bounds(from, to);
    let (Some(start), Some(end)) = (instant(start, &local), instant(end, &local)) else {
        return Vec::new();
    };
    let mut found = Vec::new();

    for calendar in calendars {
        expand(
            &bounded(calendar, from, to),
            LIMIT,
            (start, end),
            &local,
            &mut found,
        );
    }

    sort(&mut found);
    found
}

// by start, all-day ones first
fn sort(found: &mut [Occurrence]) {
    found.sort_by(|a, b| {
        (a.span.0, !a.all_day, &a.title, a.span.1).cmp(&(b.span.0, !b.all_day, &b.title, b.span.1))
    });
}

// the events of one calendar that overlap `range`, in seconds since the epoch, up to `limit`
fn expand(
    calendar: &ICalendar,
    limit: usize,
    (start, end): (i64, i64),
    local: &impl Fn(i64) -> Option<NaiveDateTime>,
    found: &mut Vec<Occurrence>,
) {
    for event in calendar.expand_dates(Tz::Floating, limit).events {
        let Some(component) = calendar.component_by_id(event.comp_id) else {
            continue;
        };
        if component.component_type != ICalendarComponentType::VEvent || cancelled(component) {
            continue;
        }

        let end_at = match event.end {
            TimeOrDelta::Time(end) => end,
            TimeOrDelta::Delta(delta) => event.start + delta,
        };
        let Some(occurrence) = occurrence(component, &event.start, &end_at, local) else {
            continue;
        };

        let (starts, ends) = occurrence.span;
        let inside = if occurrence.moment() {
            starts >= start && starts < end
        } else {
            starts < end && ends > start
        };
        if inside {
            found.push(occurrence);
        }
    }
}

// the midnights that start `from` and `to`
fn bounds(from: NaiveDate, to: NaiveDate) -> (NaiveDateTime, NaiveDateTime) {
    (from.and_time(NaiveTime::MIN), to.and_time(NaiveTime::MIN))
}

fn occurrence(
    component: &ICalendarComponent,
    start: &DateTime<Tz>,
    end: &DateTime<Tz>,
    local: &impl Fn(i64) -> Option<NaiveDateTime>,
) -> Option<Occurrence> {
    let timed = start_of(component).is_none_or(PartialDateTime::has_time);

    let (start, end, span) = if timed {
        let starts = seconds(start, local)?;
        let start = moment(start, local)?;

        // no DTEND nor DURATION is a moment, RFC 5545 3.6.1, which calcard stretches to the day
        let lasting = component.property(&ICalendarProperty::Dtend).is_some()
            || component.property(&ICalendarProperty::Duration).is_some();
        let ending = lasting
            .then(|| Some((moment(end, local)?, seconds(end, local)?)))
            .flatten();

        // the length is in absolute time, as the local clock may go back within it
        match ending {
            Some((end, ends)) if ends > starts => (start, end, (starts, ends)),
            _ => (start, start, (starts, starts)),
        }
    } else {
        // a date is the same day in every zone, so it stays as written
        let first = start.naive_local().date();
        let after = end.naive_local().date().max(first.succ_opt()?);
        let (start, end) = bounds(first, after);

        (start, end, (instant(start, local)?, instant(end, local)?))
    };

    Some(Occurrence {
        title: text(component, &ICalendarProperty::Summary).unwrap_or_default(),
        location: text(component, &ICalendarProperty::Location),
        start,
        end,
        span,
        all_day: !timed,
    })
}

// the local time of a moment; one with no zone is the same wall time everywhere
fn moment(
    at: &DateTime<Tz>,
    local: &impl Fn(i64) -> Option<NaiveDateTime>,
) -> Option<NaiveDateTime> {
    if at.timezone().is_floating() {
        return Some(at.naive_local());
    }

    local(at.timestamp())
}

// a moment in seconds since the epoch; one with no zone is when the local clock reads it
fn seconds(at: &DateTime<Tz>, local: &impl Fn(i64) -> Option<NaiveDateTime>) -> Option<i64> {
    if at.timezone().is_floating() {
        return instant(at.naive_local(), local);
    }

    Some(at.timestamp())
}

/*
 * when the local clock reads `at`, in seconds since the epoch: the first time in an hour it
 * repeats, and in one it skips, when it would have read it had it not. The zone's offsets a day
 * either side are the ones in force, as it changes at most once in two days
 */
fn instant(at: NaiveDateTime, local: &impl Fn(i64) -> Option<NaiveDateTime>) -> Option<i64> {
    const DAY: i64 = 24 * 3600;

    let wall = at.and_utc().timestamp();
    let offset = |near: i64| local(near).map(|read| read.and_utc().timestamp() - near);
    let (before, after) = (offset(wall - DAY)?, offset(wall + DAY)?);

    [wall - before, wall - after]
        .into_iter()
        .filter(|&seconds| local(seconds) == Some(at))
        .min()
        .or(Some(wall - before))
}

fn start_of(component: &ICalendarComponent) -> Option<&PartialDateTime> {
    match component
        .property(&ICalendarProperty::Dtstart)?
        .values
        .first()?
    {
        ICalendarValue::PartialDateTime(start) => Some(start),
        _ => None,
    }
}

fn text(component: &ICalendarComponent, property: &ICalendarProperty) -> Option<String> {
    component
        .property(property)?
        .values
        .first()?
        .as_text()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn cancelled(component: &ICalendarComponent) -> bool {
    component
        .property(&ICalendarProperty::Status)
        .and_then(|status| status.values.first())
        .is_some_and(|status| matches!(status, ICalendarValue::Status(ICalendarStatus::Cancelled)))
}

/*
 * a copy whose recurrences run only near the range: calcard expands a rule from its first
 * occurrence until its end, so one with none, like a weekly meeting, would run to the limit and
 * crowd out the file's other events, and one long begun, like an hourly one since 2010, would
 * spend the limit on years before the range. Each ends two days past `to`, which covers any zone
 * the rule's times are in, and starts again just before `from` (`restart`); a rule that starts
 * after the range keeps only its first occurrence, which is after it too
 */
fn bounded(calendar: &ICalendar, from: NaiveDate, to: NaiveDate) -> Cow<'_, ICalendar> {
    let recurs = calendar
        .components
        .iter()
        .any(|component| rule_of(component).is_some());
    let Some(last) = to.checked_add_days(Days::new(2)).filter(|_| recurs) else {
        return Cow::Borrowed(calendar);
    };

    // a change to an occurrence and the ones after shifts those after, so the rule keeps it
    let split = calendar
        .components
        .iter()
        .filter_map(changed_from)
        .chain([from.and_time(NaiveTime::MIN)])
        .min();
    let before = split.and_then(|split| split.checked_sub_days(MARGIN));

    let zones = calendar.build_tz_resolver();
    let mut copy = calendar.clone();

    for component in &mut copy.components {
        let starts_after = start_of(component)
            .and_then(date_of)
            .is_some_and(|start| start > last);

        component.entries.retain_mut(|entry| {
            if entry.name != ICalendarProperty::Rrule {
                return true;
            }
            if starts_after {
                return false;
            }

            if let Some(ICalendarValue::RecurrenceRule(rule)) = entry.values.first_mut()
                && rule
                    .until
                    .as_ref()
                    .and_then(date_of)
                    .is_none_or(|until| until > last)
            {
                rule.until = Some(PartialDateTime {
                    year: u16::try_from(last.year()).ok(),
                    month: u8::try_from(last.month()).ok(),
                    day: u8::try_from(last.day()).ok(),
                    hour: Some(0),
                    minute: Some(0),
                    second: Some(0),
                    ..PartialDateTime::default()
                });
            }

            true
        });

        if let Some(before) = before {
            restart(component, &zones, before);
        }
    }

    Cow::Owned(copy)
}

// how far before the range a rule starts again: past any zone's offset and the days of an ISO
// week a year's rule makes in the next year
const MARGIN: Days = Days::new(9);

/*
 * starts a rule again a whole number of its periods on, the last whose start is before `before`
 * by the event's length, so its expansion begins near the range. The parts of the rule its first
 * occurrence implied are written out first, so each period from the new start on makes the same
 * occurrences, and their EXDATEs and RECURRENCE-IDs still match; the ones the old start made
 * before it all ended before the range. DTEND moves with DTSTART, keeping the length. A rule with
 * a COUNT counts from its own start: one under the limit expands whole, and `counted` turned a
 * longer one into an UNTIL
 */
fn restart(component: &mut ICalendarComponent, zones: &TzResolver<&str>, before: NaiveDateTime) {
    let Some(start) = moment_of(component, ICalendarProperty::Dtstart, None, zones) else {
        return;
    };
    let end = moment_of(component, ICalendarProperty::Dtend, Some(&start), zones);
    let Some(mut rule) = rule_of(component).cloned() else {
        return;
    };
    if rule.count.is_some() || !restartable(&rule) {
        return;
    }

    let length = match (&end, component.property(&ICalendarProperty::Duration)) {
        (Some(end), _) => end.at - start.at,
        (None, Some(duration)) => match duration.values.first() {
            Some(ICalendarValue::Duration(duration)) => {
                duration.to_time_delta().unwrap_or_default()
            }
            _ => TimeDelta::zero(),
        },
        (None, None) => TimeDelta::days(1),
    };
    let Some(target) = before.checked_sub_signed(length.max(TimeDelta::zero())) else {
        return;
    };

    // one that ends near the target has nothing in the range to reach
    let until = rule.until.as_ref().and_then(PartialDateTime::to_date_time);
    if until.is_some_and(|until| until.date_time < target + TimeDelta::days(1)) {
        return;
    }

    implied(&mut rule, start.at);

    // a time the clock skips or repeats is no DTSTART calcard reads, so a period earlier
    let Some(at) = (0..4).find_map(|back| {
        let at = moved(&rule, start.at, target, back)?;
        let shift = at - start.at;
        let fits = at > start.at
            && start.exists(at)
            && end.as_ref().is_none_or(|end| end.exists(end.at + shift));

        fits.then_some(at)
    }) else {
        return;
    };
    let shift = at - start.at;

    for entry in &mut component.entries {
        match (&entry.name, entry.values.first_mut()) {
            (ICalendarProperty::Dtstart, Some(ICalendarValue::PartialDateTime(value))) => {
                write(value, at);
            }
            (ICalendarProperty::Dtend, Some(ICalendarValue::PartialDateTime(value))) => {
                if let Some(end) = &end {
                    write(value, end.at + shift);
                }
            }
            (ICalendarProperty::Rrule, Some(ICalendarValue::RecurrenceRule(value))) => {
                **value = rule.clone();
            }
            _ => {}
        }
    }
}

// a DTSTART or DTEND as calcard reads it: the local time it names, in its zone
struct Written {
    at: NaiveDateTime,

    // the zone it is in, none for UTC or an offset, where every time exists once
    zone: Option<Tz>,
}

impl Written {
    // whether `at` is a time calcard reads in the zone, once
    fn exists(&self, at: NaiveDateTime) -> bool {
        self.zone
            .is_none_or(|zone| zone.from_local_datetime(&at).single().is_some())
    }
}

// a DTEND without a TZID is in its DTSTART's zone, as calcard reads it
fn moment_of(
    component: &ICalendarComponent,
    property: ICalendarProperty,
    start: Option<&Written>,
    zones: &TzResolver<&str>,
) -> Option<Written> {
    let entry = component.property(&property)?;
    let Some(ICalendarValue::PartialDateTime(value)) = entry.values.first() else {
        return None;
    };
    let read = value.to_date_time()?;

    let zone = match (read.offset, entry.tz_id()) {
        (Some(_), _) => None,
        (None, Some(name)) => Some(zones.resolve_or_default(Some(name))),
        (None, None) => start.map_or(Some(Tz::Floating), |start| start.zone),
    };

    Some(Written {
        at: read.date_time,
        zone,
    })
}

/*
 * whether a rule starts again exactly anywhere a whole number of periods on: each hourly to
 * yearly one does, as calcard steps it evenly, skipping hours and days its filters leave out. One
 * more often does while it steps evenly, which a filter on days or on hours, or for one each
 * second on minutes, breaks: calcard skips those a whole hour or day at a time
 */
fn restartable(rule: &ICalendarRecurrenceRule) -> bool {
    let days = !(rule.bymonth.is_empty()
        && rule.byweekno.is_empty()
        && rule.byyearday.is_empty()
        && rule.bymonthday.is_empty()
        && rule.byday.is_empty());

    match rule.freq {
        ICalendarFrequency::Minutely => !days && rule.byhour.is_empty(),
        ICalendarFrequency::Secondly => !days && rule.byhour.is_empty() && rule.byminute.is_empty(),
        _ => true,
    }
}

// the parts of a rule its first occurrence implies, written out, as calcard fills them in
fn implied(rule: &mut ICalendarRecurrenceRule, start: NaiveDateTime) {
    // each under 60, so they fit
    let (hour, minute, second) = (
        start.hour() as u8,
        start.minute() as u8,
        start.second() as u8,
    );
    let (month, day) = (start.month() as u8, start.day() as i8);

    let days = rule.byweekno.is_empty()
        && rule.byyearday.is_empty()
        && rule.bymonthday.is_empty()
        && rule.byday.is_empty();
    if days {
        match rule.freq {
            ICalendarFrequency::Yearly => {
                if rule.bymonth.is_empty() {
                    rule.bymonth = vec![ICalendarMonth::new(month, false)];
                }
                rule.bymonthday = vec![day];
            }
            ICalendarFrequency::Monthly => rule.bymonthday = vec![day],
            ICalendarFrequency::Weekly => {
                rule.byday = vec![ICalendarDay {
                    ordwk: None,
                    weekday: weekday(start.weekday()),
                }];
            }
            _ => {}
        }
    }

    if rule.byhour.is_empty() && rule.freq < ICalendarFrequency::Hourly {
        rule.byhour = vec![hour];
    }
    if rule.byminute.is_empty() && rule.freq < ICalendarFrequency::Minutely {
        rule.byminute = vec![minute];
    }
    if rule.bysecond.is_empty() && rule.freq < ICalendarFrequency::Secondly {
        rule.bysecond = vec![second];
    }
}

fn weekday(day: Weekday) -> ICalendarWeekday {
    match day {
        Weekday::Mon => ICalendarWeekday::Monday,
        Weekday::Tue => ICalendarWeekday::Tuesday,
        Weekday::Wed => ICalendarWeekday::Wednesday,
        Weekday::Thu => ICalendarWeekday::Thursday,
        Weekday::Fri => ICalendarWeekday::Friday,
        Weekday::Sat => ICalendarWeekday::Saturday,
        Weekday::Sun => ICalendarWeekday::Sunday,
    }
}

/*
 * `start` a whole number of the rule's periods on, the last at or before `target`'s period, less
 * `back` more: one period, or a day's worth of a rule more often than daily. A month's or a
 * year's period starts on its first day, as the rule's own days are written out
 */
fn moved(
    rule: &ICalendarRecurrenceRule,
    start: NaiveDateTime,
    target: NaiveDateTime,
    back: i64,
) -> Option<NaiveDateTime> {
    let interval = i64::from(rule.interval.unwrap_or(1).max(1));

    match rule.freq {
        ICalendarFrequency::Secondly
        | ICalendarFrequency::Minutely
        | ICalendarFrequency::Hourly => {
            let unit = interval
                * match rule.freq {
                    ICalendarFrequency::Secondly => 1,
                    ICalendarFrequency::Minutely => 60,
                    _ => 3600,
                };
            let periods = (target - start).num_seconds() / unit - back * (24 * 3600 / unit).max(1);

            start.checked_add_signed(TimeDelta::try_seconds(periods.checked_mul(unit)?)?)
        }
        ICalendarFrequency::Daily | ICalendarFrequency::Weekly => {
            let unit = interval
                * if rule.freq == ICalendarFrequency::Weekly {
                    7
                } else {
                    1
                };
            let periods = (target.date() - start.date()).num_days() / unit - back;

            start.checked_add_signed(TimeDelta::try_days(periods.checked_mul(unit)?)?)
        }
        ICalendarFrequency::Monthly | ICalendarFrequency::Yearly => {
            let unit = interval
                * if rule.freq == ICalendarFrequency::Yearly {
                    12
                } else {
                    1
                };
            let months = |at: NaiveDateTime| i64::from(at.year()) * 12 + i64::from(at.month0());
            let periods = (months(target) - months(start)) / unit - back;
            let month = months(start).checked_add(periods.checked_mul(unit)?)?;

            NaiveDate::from_ymd_opt(
                i32::try_from(month.div_euclid(12)).ok()?,
                u32::try_from(month.rem_euclid(12)).ok()? + 1,
                1,
            )
            .map(|first| first.and_time(start.time()))
        }
    }
}

// sets a date, and its time if it has one, keeping its zone
fn write(value: &mut PartialDateTime, at: NaiveDateTime) {
    value.year = u16::try_from(at.year()).ok();
    value.month = u8::try_from(at.month()).ok();
    value.day = u8::try_from(at.day()).ok();

    if value.has_time() {
        value.hour = u8::try_from(at.hour()).ok();
        value.minute = u8::try_from(at.minute()).ok();
        value.second = u8::try_from(at.second()).ok();
    }
}

// where a change to an occurrence and the ones after it, RANGE=THISANDFUTURE, takes over
fn changed_from(component: &ICalendarComponent) -> Option<NaiveDateTime> {
    let entry = component.property(&ICalendarProperty::RecurrenceId)?;
    if !entry
        .params
        .iter()
        .any(|param| param.name == ICalendarParameterName::Range)
    {
        return None;
    }

    match entry.values.first()? {
        ICalendarValue::PartialDateTime(value) => Some(value.to_date_time()?.date_time),
        _ => None,
    }
}

fn rule_of(component: &ICalendarComponent) -> Option<&ICalendarRecurrenceRule> {
    match component
        .property(&ICalendarProperty::Rrule)?
        .values
        .first()?
    {
        ICalendarValue::RecurrenceRule(rule) => Some(rule),
        _ => None,
    }
}

/*
 * a rule whose COUNT is over the limit made into one that ends at its last occurrence, as an
 * UNTIL, so `restart` can start it near the range: counting takes its occurrences from its own
 * start, a limit's worth at a time, each walk starting again at the last one found. Read once
 * with the file; false for one too long to walk, which keeps its COUNT
 */
fn counted(calendar: &mut ICalendar) -> bool {
    let Some(master) = calendar.components.iter_mut().find(|component| {
        component
            .property(&ICalendarProperty::RecurrenceId)
            .is_none()
            && rule_of(component).and_then(|rule| rule.count).unwrap_or(0) as usize > LIMIT
    }) else {
        return true;
    };

    let mut walk = bare(master);
    let Some(ICalendarValue::PartialDateTime(first)) = walk
        .property(&ICalendarProperty::Dtstart)
        .and_then(|entry| entry.values.first())
        .cloned()
    else {
        return true;
    };
    let (Some(start), Some(mut rule)) = (first.to_date_time(), rule_of(&walk).cloned()) else {
        return true;
    };
    let start = start.date_time;
    let mut left = rule.count.unwrap_or(0) as usize;
    let mut from = start;
    let mut last = None;
    let mut walked = 0;

    loop {
        // after the first walk, its start is the last occurrence found, which it finds again
        let again = usize::from(last.is_some());
        let ask = (left + again).min(LIMIT);
        rule.count = u32::try_from(ask).ok();

        let found = starts(&mut walk, from, &rule);

        let new = found
            .iter()
            .filter(|&&at| last.is_none_or(|last| at > last))
            .count();
        left = left.saturating_sub(new);
        walked += new;
        last = found.iter().copied().max().or(last);

        if left == 0 || found.len() < ask || new == 0 {
            break;
        }
        if walked >= COUNTED {
            return false;
        }

        if again == 0 {
            implied(&mut rule, start);
        }
        let Some(next) = last else {
            break;
        };
        from = next;
    }

    let Some(last) = last else {
        return true;
    };
    for entry in &mut master.entries {
        if let Some(ICalendarValue::RecurrenceRule(rule)) = entry.values.first_mut() {
            rule.count = None;
            // a date's rule ends on a date, any other at a time
            let mut until = PartialDateTime {
                hour: first.has_time().then_some(0),
                minute: first.has_time().then_some(0),
                ..PartialDateTime::default()
            };
            write(&mut until, last);
            rule.until = Some(until);
        }
    }

    true
}

// how many occurrences `counted` walks at most
const COUNTED: usize = 10 * LIMIT;

// a rule's start and itself alone, with no zone: the rule steps through its local times alone
fn bare(master: &ICalendarComponent) -> ICalendarComponent {
    let mut walk = master.clone();
    walk.entries.retain_mut(|entry| match entry.name {
        ICalendarProperty::Dtstart => {
            entry
                .params
                .retain(|param| param.name != ICalendarParameterName::Tzid);
            true
        }
        ICalendarProperty::Rrule => true,
        _ => false,
    });

    walk
}

// the starts of a bare rule's first occurrences, up to the limit, from `from` by `rule`
fn starts(
    walk: &mut ICalendarComponent,
    from: NaiveDateTime,
    rule: &ICalendarRecurrenceRule,
) -> Vec<NaiveDateTime> {
    for entry in &mut walk.entries {
        match (&entry.name, entry.values.first_mut()) {
            (ICalendarProperty::Dtstart, Some(ICalendarValue::PartialDateTime(value))) => {
                write(value, from);
            }
            (ICalendarProperty::Rrule, Some(ICalendarValue::RecurrenceRule(value))) => {
                **value = rule.clone();
            }
            _ => {}
        }
    }

    let head = ICalendarComponent {
        component_type: ICalendarComponentType::VCalendar,
        ..ICalendarComponent::default()
    };

    ICalendar {
        components: vec![head, walk.clone()],
    }
    .expand_dates(Tz::Floating, LIMIT)
    .events
    .iter()
    .map(|event| event.start.naive_local())
    .collect()
}

/*
 * whether a rule `restart` cannot start again, one more often than hourly that a filter steps
 * unevenly, runs to the limit before a year from `today`, so its later occurrences never show
 */
fn crowded(calendar: &ICalendar, today: NaiveDate) -> bool {
    let Some(master) = calendar.components.iter().find(|component| {
        component
            .property(&ICalendarProperty::RecurrenceId)
            .is_none()
            && rule_of(component).is_some_and(|rule| rule.count.is_none() && !restartable(rule))
    }) else {
        return false;
    };
    let (Some(mut rule), Some(horizon)) = (
        rule_of(master).cloned(),
        today.checked_add_days(Days::new(400)),
    ) else {
        return false;
    };
    let mut walk = bare(master);
    let Some(ICalendarValue::PartialDateTime(first)) = walk
        .property(&ICalendarProperty::Dtstart)
        .and_then(|entry| entry.values.first())
    else {
        return false;
    };
    let Some(start) = first.to_date_time() else {
        return false;
    };

    if rule
        .until
        .as_ref()
        .and_then(date_of)
        .is_none_or(|until| until > horizon)
    {
        let mut until = PartialDateTime::default();
        write(&mut until, horizon.and_time(NaiveTime::MIN));
        rule.until = Some(until);
    }

    starts(&mut walk, start.date_time, &rule).len() >= LIMIT
}

/*
 * a calendar as one of its events that recur each, with their exceptions, and one of the rest:
 * calcard's limit on occurrences is for a whole calendar, so a rule that runs to it, like one
 * every minute for years, would otherwise hide the events after it. Each keeps the calendar's
 * time zones, which its times may name; whatever is not an event goes
 */
fn series(calendar: ICalendar) -> Vec<ICalendar> {
    let uid = |component: &ICalendarComponent| text(component, &ICalendarProperty::Uid);

    let recurring: HashSet<String> = calendar
        .components
        .iter()
        .filter(|component| component.property(&ICalendarProperty::Rrule).is_some())
        .filter_map(uid)
        .collect();

    let mut head = Vec::new();
    let mut rest = Vec::new();
    let mut each: Vec<(Option<String>, Vec<ICalendarComponent>)> = Vec::new();

    for mut component in calendar.components {
        component.component_ids.clear();

        match component.component_type {
            ICalendarComponentType::VCalendar | ICalendarComponentType::VTimezone => {
                head.push(component);
            }
            ICalendarComponentType::VEvent => {
                let key = uid(&component).filter(|uid| recurring.contains(uid));
                let alone =
                    key.is_none() && component.property(&ICalendarProperty::Rrule).is_some();

                if key.is_none() && !alone {
                    rest.push(component);
                } else if let Some(group) = each
                    .iter_mut()
                    .find(|(uid, _)| key.is_some() && *uid == key)
                {
                    group.1.push(component);
                } else {
                    each.push((key, vec![component]));
                }
            }
            _ => {}
        }
    }

    // the VCALENDAR first, as calcard reads a calendar's root
    let with_head = |events: Vec<ICalendarComponent>| ICalendar {
        components: head.iter().cloned().chain(events).collect(),
    };

    let mut found: Vec<ICalendar> = each
        .into_iter()
        .map(|(_, events)| with_head(events))
        .collect();
    if !rest.is_empty() {
        found.push(with_head(rest));
    }

    found
}

fn date_of(at: &PartialDateTime) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(
        i32::from(at.year?),
        u32::from(at.month?),
        u32::from(at.day?),
    )
}

#[cfg(test)]
mod tests {

    use super::*;

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn at(date: NaiveDate, hour: u32, minute: u32) -> NaiveDateTime {
        date.and_hms_opt(hour, minute, 0).unwrap()
    }

    // a zone two hours east of UTC, all year
    fn plus_two(seconds: i64) -> Option<NaiveDateTime> {
        DateTime::from_timestamp(seconds + 2 * 3600, 0).map(|at| at.naive_utc())
    }

    fn calendar(events: &str) -> Vec<ICalendar> {
        parse(&format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//test//EN\r\n{events}END:VCALENDAR\r\n"
        ))
        .expect("a calendar")
        .into_iter()
        .flat_map(series)
        .map(|mut calendar| {
            assert!(counted(&mut calendar));
            calendar
        })
        .collect()
    }

    // New York's local time, daylight saving and all
    fn new_york(seconds: i64) -> Option<NaiveDateTime> {
        let zone: Tz = "America/New_York".parse().ok()?;
        DateTime::from_timestamp(seconds, 0).map(|at| at.with_timezone(&zone).naive_local())
    }

    // what calcard's whole expansion gives, with no limit and no restart
    fn whole(
        calendars: &[ICalendar],
        from: NaiveDate,
        to: NaiveDate,
        local: impl Fn(i64) -> Option<NaiveDateTime>,
    ) -> Vec<Occurrence> {
        let (start, end) = bounds(from, to);
        let range = (
            instant(start, &local).unwrap(),
            instant(end, &local).unwrap(),
        );
        let mut found = Vec::new();
        for calendar in calendars {
            expand(calendar, usize::MAX, range, &local, &mut found);
        }
        sort(&mut found);
        found
    }

    fn event(lines: &[&str]) -> String {
        let mut text = String::from("BEGIN:VEVENT\r\n");
        for line in lines {
            text.push_str(line);
            text.push_str("\r\n");
        }
        text.push_str("END:VEVENT\r\n");
        text
    }

    fn titles(found: &[Occurrence]) -> Vec<(&str, NaiveDateTime)> {
        found
            .iter()
            .map(|occurrence| (occurrence.title.as_str(), occurrence.start))
            .collect()
    }

    #[test]
    fn a_timed_event_reads_in_local_time() {
        let calendars = calendar(&event(&[
            "UID:1",
            "SUMMARY:Standup",
            "LOCATION:Room 4",
            "DTSTART:20261008T080000Z",
            "DTEND:20261008T083000Z",
        ]));

        let found = occurrences(&calendars, day(2026, 10, 1), day(2026, 11, 1), plus_two);

        assert_eq!(
            found,
            [Occurrence {
                title: String::from("Standup"),
                location: Some(String::from("Room 4")),
                start: at(day(2026, 10, 8), 10, 0),
                end: at(day(2026, 10, 8), 10, 30),
                span: (1_791_446_400, 1_791_448_200),
                all_day: false,
            }]
        );
    }

    // a zone the file names converts through UTC, daylight saving and all
    #[test]
    fn a_named_zone_converts_to_local_time() {
        let calendars = calendar(&event(&[
            "UID:1",
            "SUMMARY:Call",
            "DTSTART;TZID=America/New_York:20260707T090000",
            "DURATION:PT1H",
        ]));

        let found = occurrences(&calendars, day(2026, 7, 1), day(2026, 8, 1), plus_two);

        // 09:00 EDT is 13:00 UTC, 15:00 at UTC+2
        assert_eq!(titles(&found), [("Call", at(day(2026, 7, 7), 15, 0))]);
        assert_eq!(found[0].end, at(day(2026, 7, 7), 16, 0));
    }

    // no zone is the same wall time wherever it is read
    #[test]
    fn a_floating_time_stays_as_written() {
        let calendars = calendar(&event(&[
            "UID:1",
            "SUMMARY:Lunch",
            "DTSTART:20261008T120000",
            "DTEND:20261008T130000",
        ]));

        let found = occurrences(&calendars, day(2026, 10, 8), day(2026, 10, 9), plus_two);

        assert_eq!(titles(&found), [("Lunch", at(day(2026, 10, 8), 12, 0))]);
    }

    #[test]
    fn an_all_day_event_covers_its_days() {
        let calendars = calendar(&event(&[
            "UID:1",
            "SUMMARY:Trip",
            "DTSTART;VALUE=DATE:20261008",
            "DTEND;VALUE=DATE:20261010",
        ]));

        let found = occurrences(&calendars, day(2026, 10, 1), day(2026, 11, 1), plus_two);
        let trip = &found[0];

        assert!(trip.all_day);
        assert_eq!(
            (trip.start, trip.end),
            bounds(day(2026, 10, 8), day(2026, 10, 10))
        );
        assert!(!trip.on(day(2026, 10, 7)));
        assert!(trip.on(day(2026, 10, 8)));
        assert!(trip.on(day(2026, 10, 9)));
        assert!(!trip.on(day(2026, 10, 10)));

        // a date alone lasts the day
        let calendars = calendar(&event(&[
            "UID:2",
            "SUMMARY:Holiday",
            "DTSTART;VALUE=DATE:20261008",
        ]));
        let found = occurrences(&calendars, day(2026, 10, 1), day(2026, 11, 1), plus_two);
        assert_eq!(
            (found[0].start, found[0].end),
            bounds(day(2026, 10, 8), day(2026, 10, 9))
        );
    }

    // RFC 5545 3.6.1: a time with no end and no duration is a moment, not the rest of the day
    #[test]
    fn a_time_with_no_end_is_a_moment() {
        let calendars = calendar(&event(&[
            "UID:1",
            "SUMMARY:Call",
            "DTSTART:20261008T150000",
        ]));

        let found = occurrences(&calendars, day(2026, 10, 8), day(2026, 10, 9), plus_two);

        assert_eq!(found[0].start, found[0].end);
        assert!(found[0].on(day(2026, 10, 8)));
        assert!(!found[0].on(day(2026, 10, 9)));
    }

    #[test]
    fn a_long_event_falls_on_each_day_it_spans() {
        let calendars = calendar(&event(&[
            "UID:1",
            "SUMMARY:Night shift",
            "DTSTART:20261008T220000",
            "DTEND:20261009T060000",
        ]));

        let found = occurrences(&calendars, day(2026, 10, 9), day(2026, 10, 10), plus_two);

        assert_eq!(
            titles(&found),
            [("Night shift", at(day(2026, 10, 8), 22, 0))]
        );
        assert!(found[0].on(day(2026, 10, 8)) && found[0].on(day(2026, 10, 9)));

        // ending at midnight is not on the next day
        let calendars = calendar(&event(&[
            "UID:2",
            "SUMMARY:Late",
            "DTSTART:20261008T220000",
            "DTEND:20261009T000000",
        ]));
        let found = occurrences(&calendars, day(2026, 10, 1), day(2026, 11, 1), plus_two);
        assert!(!found[0].on(day(2026, 10, 9)));
    }

    // a rule with no end shows in any month, however far, with its exceptions
    #[test]
    fn an_endless_rule_recurs_in_any_range() {
        let calendars = calendar(&format!(
            "{}{}",
            event(&[
                "UID:weekly",
                "SUMMARY:Review",
                "DTSTART:20200106T100000",
                "DTEND:20200106T110000",
                "RRULE:FREQ=WEEKLY;BYDAY=MO",
                "EXDATE:20261012T100000",
            ]),
            event(&[
                "UID:weekly",
                "SUMMARY:Review, moved",
                "RECURRENCE-ID:20261019T100000",
                "DTSTART:20261020T140000",
                "DTEND:20261020T150000",
            ]),
        ));

        let found = occurrences(&calendars, day(2026, 10, 1), day(2026, 11, 1), plus_two);

        assert_eq!(
            titles(&found),
            [
                ("Review", at(day(2026, 10, 5), 10, 0)),
                ("Review, moved", at(day(2026, 10, 20), 14, 0)),
                ("Review", at(day(2026, 10, 26), 10, 0)),
            ]
        );

        // decades on, still there
        let found = occurrences(&calendars, day(2090, 1, 1), day(2090, 1, 8), plus_two);
        assert_eq!(titles(&found), [("Review", at(day(2090, 1, 2), 10, 0))]);
    }

    // an endless rule ends near the range, so it never crowds out the file's other events
    #[test]
    fn an_endless_rule_leaves_room_for_the_rest() {
        let calendars = calendar(&format!(
            "{}{}{}",
            event(&[
                "UID:flood",
                "SUMMARY:Flood",
                "DTSTART:20000101T000000",
                "RRULE:FREQ=MINUTELY",
            ]),
            event(&[
                "UID:hourly",
                "SUMMARY:Ping",
                "DTSTART:20200101T000000",
                "DURATION:PT1M",
                "RRULE:FREQ=HOURLY",
            ]),
            event(&[
                "UID:once",
                "SUMMARY:Dentist",
                "DTSTART:20261008T090000",
                "DTEND:20261008T100000",
            ]),
        ));

        let found = occurrences(&calendars, day(2026, 10, 8), day(2026, 10, 9), plus_two);

        assert_eq!(found.iter().filter(|o| o.title == "Ping").count(), 24);
        assert!(found.iter().any(|o| o.title == "Dentist"));
    }

    // an hourly rule begun long ago still shows its every occurrence today
    #[test]
    fn an_old_frequent_rule_recurs_now() {
        let calendars = calendar(&event(&[
            "UID:hourly",
            "SUMMARY:Ping",
            "DTSTART:20100101T001500",
            "DURATION:PT5M",
            "RRULE:FREQ=HOURLY",
        ]));

        let found = occurrences(&calendars, day(2026, 10, 1), day(2026, 11, 1), plus_two);

        assert_eq!(found.len(), 31 * 24);
        assert_eq!(found[0].start, at(day(2026, 10, 1), 0, 15));
        assert_eq!(found[1].start, at(day(2026, 10, 1), 1, 15));
        assert_eq!(found.last().unwrap().start, at(day(2026, 10, 31), 23, 15));
        assert!(found.iter().all(|o| o.span.1 - o.span.0 == 300));
    }

    // a rule started again near the range gives what its whole expansion does there
    #[test]
    fn a_restarted_rule_matches_its_whole_expansion() {
        let rules: &[&[&str]] = &[
            &[
                "DTSTART:20100315T093000",
                "DTEND:20100315T103000",
                "RRULE:FREQ=DAILY;UNTIL=20270301T000000",
                "EXDATE:20261010T093000",
            ],
            &[
                "DTSTART:20110104T180000",
                "DURATION:PT2H",
                "RRULE:FREQ=WEEKLY;INTERVAL=3;BYDAY=TU,TH;UNTIL=20270301T000000",
            ],
            &[
                "DTSTART:20100131T120000",
                "RRULE:FREQ=MONTHLY;UNTIL=20270301T000000",
            ],
            &[
                "DTSTART:20120127T160000",
                "RRULE:FREQ=MONTHLY;INTERVAL=5;BYDAY=-1FR;UNTIL=20270301T000000",
            ],
            &[
                "DTSTART;VALUE=DATE:20100104",
                "RRULE:FREQ=YEARLY;BYWEEKNO=1;BYDAY=MO,SU;UNTIL=20270301",
            ],
            &[
                "DTSTART;VALUE=DATE:20120229",
                "DTEND;VALUE=DATE:20120302",
                "RRULE:FREQ=YEARLY;UNTIL=20290301",
            ],
            &[
                "DTSTART:20100101T020000",
                "DTEND:20100104T020000",
                "RRULE:FREQ=DAILY;INTERVAL=5;UNTIL=20270301T000000",
            ],
            &[
                "DTSTART:20240101T013000",
                "DURATION:PT30M",
                "RRULE:FREQ=HOURLY;INTERVAL=5;UNTIL=20270301T000000",
            ],
            &[
                "DTSTART:20250101T010000",
                "RRULE:FREQ=HOURLY;INTERVAL=5;BYHOUR=1,6,11;UNTIL=20270301T000000",
            ],
            &[
                "DTSTART:20100106T080000",
                "RRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE,FR;BYSETPOS=-1;WKST=SU;UNTIL=20270301T000000",
            ],
            &[
                "DTSTART;TZID=America/New_York:20100315T020000",
                "RRULE:FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=1,-1;UNTIL=20270301T000000",
            ],
            &[
                "DTSTART:20100104T090000",
                "DURATION:PT15M",
                "RRULE:FREQ=HOURLY;BYDAY=MO,WE;BYHOUR=9,14;UNTIL=20270301T000000",
            ],
            &[
                "DTSTART:20150103T050000",
                "RRULE:FREQ=HOURLY;INTERVAL=7;BYDAY=SA,SU;UNTIL=20270301T000000",
            ],
            &[
                "DTSTART:20260201T090500",
                "RRULE:FREQ=MINUTELY;INTERVAL=7;BYHOUR=9,10;BYDAY=TU;UNTIL=20270301T000000",
            ],
            &[
                "DTSTART:20260101T000300",
                "RRULE:FREQ=MINUTELY;INTERVAL=7;UNTIL=20270301T000000",
            ],
            &[
                "DTSTART;TZID=America/New_York:20150104T013000",
                "DTEND;TZID=America/New_York:20150104T023000",
                "RRULE:FREQ=DAILY;INTERVAL=3;UNTIL=20270301T000000",
            ],
            &[
                "DTSTART;TZID=America/New_York:20100103T023000",
                "DURATION:PT1H",
                "RRULE:FREQ=WEEKLY;UNTIL=20270301T000000",
            ],
        ];
        let ranges = [
            (day(2026, 3, 1), day(2026, 4, 1)),
            (day(2026, 10, 1), day(2026, 11, 1)),
            (day(2026, 10, 31), day(2026, 11, 8)),
            (day(2026, 12, 25), day(2027, 1, 10)),
            (day(2028, 2, 25), day(2028, 3, 5)),
        ];

        for (index, rule) in rules.iter().enumerate() {
            let uid = format!("UID:{index}");
            let lines: Vec<&str> = [uid.as_str(), "SUMMARY:Rule"]
                .into_iter()
                .chain(rule.iter().copied())
                .collect();
            let calendars = calendar(&event(&lines));
            let mut shown = 0;

            for &(from, to) in &ranges {
                let restarted = occurrences(&calendars, from, to, new_york);
                assert_eq!(
                    restarted,
                    whole(&calendars, from, to, new_york),
                    "{rule:?} from {from} to {to}"
                );
                shown += restarted.len();
            }
            assert!(shown > 0, "{rule:?}");
        }
    }

    // a rule started again keeps its exceptions, a moved occurrence and a change to the rest
    #[test]
    fn a_restarted_rule_keeps_its_exceptions() {
        let calendars = calendar(&format!(
            "{}{}{}",
            event(&[
                "UID:daily",
                "SUMMARY:Walk",
                "DTSTART:20100101T070000",
                "DURATION:PT1H",
                "RRULE:FREQ=DAILY",
                "EXDATE:20261006T070000",
            ]),
            event(&[
                "UID:daily",
                "SUMMARY:Walk, late",
                "RECURRENCE-ID:20261007T070000",
                "DTSTART:20261007T200000",
                "DURATION:PT1H",
            ]),
            event(&[
                "UID:daily",
                "SUMMARY:Run",
                "RECURRENCE-ID;RANGE=THISANDFUTURE:20261008T070000",
                "DTSTART:20261008T060000",
                "DURATION:PT1H",
            ]),
        ));

        let found = occurrences(&calendars, day(2026, 10, 5), day(2026, 10, 9), plus_two);

        assert_eq!(
            found,
            whole(&calendars, day(2026, 10, 5), day(2026, 10, 9), plus_two)
        );
        assert_eq!(found.iter().filter(|o| o.title == "Walk, late").count(), 1);
        assert!(!found.iter().any(|o| o.start.date() == day(2026, 10, 6)));
        assert_eq!(
            titles(&found[found.len() - 1..]),
            [("Run", at(day(2026, 10, 8), 6, 0))]
        );
    }

    // a rule that cannot start again and runs to the limit before long is told of
    #[test]
    fn a_rule_that_runs_to_the_limit_is_told_of() {
        let rule = |start: &str| {
            calendar(&event(&[
                "UID:1",
                "SUMMARY:Busy",
                start,
                "RRULE:FREQ=MINUTELY;BYHOUR=9",
            ]))
        };

        assert!(crowded(
            &rule("DTSTART:20100101T090000")[0],
            day(2026, 10, 8)
        ));
        assert!(!crowded(
            &rule("DTSTART:20260901T090000")[0],
            day(2026, 10, 8)
        ));
    }

    // a COUNT over the limit still ends at its last occurrence, which it counts from its start
    #[test]
    fn a_long_count_ends_where_it_should() {
        let calendars = calendar(&event(&[
            "UID:count",
            "SUMMARY:Tick",
            "DTSTART:20100101T000000",
            "RRULE:FREQ=MINUTELY;INTERVAL=2;COUNT=250000",
        ]));

        // the 250000th starts 499998 minutes on: 14 Dec 2010, 05:18
        let found = occurrences(&calendars, day(2010, 12, 14), day(2010, 12, 15), plus_two);
        assert_eq!(found.last().unwrap().start, at(day(2010, 12, 14), 5, 18));
        assert_eq!(found.len(), 5 * 30 + 9 + 1);
    }

    // an event across the hour the clock repeats keeps its length and order
    #[test]
    fn an_event_across_the_repeated_hour_keeps_its_length() {
        let calendars = calendar(&format!(
            "{}{}",
            event(&[
                "UID:1",
                "SUMMARY:Across",
                "DTSTART:20261101T055000Z",
                "DTEND:20261101T061000Z",
            ]),
            event(&[
                "UID:2",
                "SUMMARY:After",
                "DTSTART:20261101T063000Z",
                "DTEND:20261101T064000Z",
            ]),
        ));

        let found = occurrences(&calendars, day(2026, 11, 1), day(2026, 11, 2), new_york);

        assert_eq!(
            titles(&found),
            [
                ("Across", at(day(2026, 11, 1), 1, 50)),
                ("After", at(day(2026, 11, 1), 1, 30)),
            ]
        );
        let across = &found[0];
        assert_eq!(across.end, at(day(2026, 11, 1), 1, 10));
        assert_eq!(across.span.1 - across.span.0, 20 * 60);
        assert!(!across.moment());
        assert!(across.on(day(2026, 11, 1)));
    }

    // a rule with a count or an end keeps it, the clamp only shortening one past the range
    #[test]
    fn a_rule_keeps_its_own_end() {
        let calendars = calendar(&format!(
            "{}{}",
            event(&[
                "UID:count",
                "SUMMARY:Course",
                "DTSTART;VALUE=DATE:20261001",
                "RRULE:FREQ=DAILY;COUNT=3",
            ]),
            event(&[
                "UID:until",
                "SUMMARY:Sprint",
                "DTSTART;VALUE=DATE:20261001",
                "RRULE:FREQ=WEEKLY;UNTIL=20261015",
            ]),
        ));

        let found = occurrences(&calendars, day(2026, 9, 1), day(2027, 1, 1), plus_two);

        assert_eq!(found.iter().filter(|o| o.title == "Course").count(), 3);
        assert_eq!(found.iter().filter(|o| o.title == "Sprint").count(), 3);
    }

    #[test]
    fn cancelled_events_and_todos_do_not_show() {
        let calendars = calendar(&format!(
            "{}BEGIN:VTODO\r\nUID:t\r\nSUMMARY:Task\r\nDTSTART:20261008T090000\r\nEND:VTODO\r\n",
            event(&[
                "UID:1",
                "SUMMARY:Off",
                "STATUS:CANCELLED",
                "DTSTART:20261008T090000",
            ]),
        ));

        assert_eq!(
            occurrences(&calendars, day(2026, 10, 1), day(2026, 11, 1), plus_two),
            []
        );
    }

    #[test]
    fn all_day_events_sort_first_then_by_start() {
        let calendars = calendar(&format!(
            "{}{}{}",
            event(&["UID:1", "SUMMARY:B", "DTSTART:20261008T090000"]),
            event(&["UID:2", "SUMMARY:A", "DTSTART:20261008T080000"]),
            event(&["UID:3", "SUMMARY:Day", "DTSTART;VALUE=DATE:20261008"]),
        ));

        let found = occurrences(&calendars, day(2026, 10, 8), day(2026, 10, 9), plus_two);

        assert_eq!(
            found.iter().map(|o| o.title.as_str()).collect::<Vec<_>>(),
            ["Day", "A", "B"]
        );
    }

    #[test]
    fn a_file_holds_any_number_of_calendars() {
        let one = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n";

        assert_eq!(parse(&one.repeat(3)).map(|found| found.len()), Ok(3));
        assert_eq!(parse(""), Err(String::from("no VCALENDAR in it")));
        assert!(
            parse("BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n").is_ok(),
            "read as far as it goes"
        );
    }

    // a file's contents are never said, only where it is
    #[test]
    fn a_bad_file_says_nothing_of_its_contents() {
        let problem = parse("secret meeting with someone\r\n").unwrap_err();

        assert!(!problem.contains("secret"), "{problem}");
    }

    #[test]
    fn the_default_place_is_the_data_directory() {
        assert_eq!(
            places(&[], Some(Path::new("/home/you/.local/share"))),
            [PathBuf::from("/home/you/.local/share/calendars")]
        );
        assert_eq!(
            places(&[String::from("/a.ics")], Some(Path::new("/x"))),
            [PathBuf::from("/a.ics")]
        );
    }

    // directories are read recursively, files named on their own as they are, and a missing
    // place is waited for at its nearest parent
    #[test]
    fn places_find_their_files_and_watches() {
        let root = std::env::temp_dir().join(format!("kanade-calendar-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("cal/work")).unwrap();
        fs::write(root.join("cal/a.ics"), "").unwrap();
        fs::write(root.join("cal/color"), "").unwrap();
        fs::write(root.join("cal/work/b.ICS"), "").unwrap();
        fs::write(root.join("one.ics"), "").unwrap();
        std::os::unix::fs::symlink(root.join("cal"), root.join("cal/work/loop")).unwrap();

        let found = find(&[
            root.join("cal"),
            root.join("one.ics"),
            root.join("missing/deeper"),
        ]);

        assert_eq!(
            found.files,
            [
                root.join("cal/a.ics"),
                root.join("cal/work/b.ICS"),
                root.join("one.ics"),
            ]
        );
        assert_eq!(
            found.targets,
            [
                (root.join("cal"), WATCHED_DIRECTORY, Want::Calendars),
                (root.join("cal/work"), WATCHED_DIRECTORY, Want::Calendars),
                (
                    root.clone(),
                    WATCHED_DIRECTORY,
                    Want::Named("one.ics".into())
                ),
                (root.join("one.ics"), WATCHED_FILE, Want::Any),
                (
                    root.clone(),
                    WATCHED_DIRECTORY,
                    Want::Named("missing".into())
                ),
            ]
        );

        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn only_calendar_entries_matter_in_a_calendar_directory() {
        let wants = [Want::Calendars];

        assert!(wanted(
            &wants,
            EventMask::CLOSE_WRITE,
            Some(OsStr::new("a.ics"))
        ));
        assert!(wanted(
            &wants,
            EventMask::MOVED_TO,
            Some(OsStr::new("b.ICS"))
        ));
        assert!(wanted(
            &wants,
            EventMask::CREATE | EventMask::ISDIR,
            Some(OsStr::new("work"))
        ));
        assert!(!wanted(
            &wants,
            EventMask::CLOSE_WRITE,
            Some(OsStr::new("a.ics.tmp"))
        ));
        assert!(!wanted(
            &wants,
            EventMask::CLOSE_WRITE,
            Some(OsStr::new("displayname"))
        ));
        assert!(wanted(&wants, EventMask::DELETE_SELF, None));

        let named = [Want::Named(OsString::from("one.ics"))];
        assert!(wanted(
            &named,
            EventMask::MOVED_TO,
            Some(OsStr::new("one.ics"))
        ));
        assert!(!wanted(
            &named,
            EventMask::MOVED_TO,
            Some(OsStr::new("two.ics"))
        ));
    }

    #[test]
    fn a_moment_on_a_day_boundary_is_on_its_own_day() {
        let midnight = Occurrence {
            title: String::new(),
            location: None,
            start: at(day(2026, 10, 9), 0, 0),
            end: at(day(2026, 10, 9), 0, 0),
            span: (0, 0),
            all_day: false,
        };

        assert!(midnight.on(day(2026, 10, 9)));
        assert!(!midnight.on(day(2026, 10, 8)));
        assert!(midnight.moment());
    }
}
