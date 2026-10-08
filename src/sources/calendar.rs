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
//! come from calcard, each moment then becomes local time through `clock`.

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
use calcard::icalendar::{
    ICalendar, ICalendarComponent, ICalendarComponentType, ICalendarProperty, ICalendarStatus,
    ICalendarValue,
};
use calcard::{Entry, Parser};
use chrono::{DateTime, Datelike, Days, NaiveDate, NaiveDateTime, NaiveTime};
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
    pub start: NaiveDateTime,

    // the moment after it; an all-day one ends at the midnight after its last day, a moment with
    // no length at its start
    pub end: NaiveDateTime,
    pub all_day: bool,
}

impl Occurrence {
    // whether it falls on `day`, a moment on the day it is at
    pub fn on(&self, day: NaiveDate) -> bool {
        let (start, end) = bounds(day, day.succ_opt().unwrap_or(day));

        if self.start == self.end {
            return self.start.date() == day;
        }

        self.start < end && self.end > start
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

    Ok(parse(&text)?.into_iter().flat_map(series).collect())
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
    let mut found = Vec::new();

    for calendar in calendars {
        let calendar = bounded(calendar, to);
        let calendar = calendar.as_ref();

        for event in calendar.expand_dates(Tz::Floating, LIMIT).events {
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
            let Some(occurrence) = occurrence(component, &event.start, &end_at, &local) else {
                continue;
            };

            let inside = if occurrence.start == occurrence.end {
                occurrence.start >= start && occurrence.start < end
            } else {
                occurrence.start < end && occurrence.end > start
            };
            if inside {
                found.push(occurrence);
            }
        }
    }

    found.sort_by(|a, b| {
        (a.start, !a.all_day, &a.title, a.end).cmp(&(b.start, !b.all_day, &b.title, b.end))
    });
    found
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

    let (start, end) = if timed {
        let start_at = moment(start, local)?;

        // no DTEND nor DURATION is a moment, RFC 5545 3.6.1, which calcard stretches to the day
        let lasting = component.property(&ICalendarProperty::Dtend).is_some()
            || component.property(&ICalendarProperty::Duration).is_some();
        let end_at = if lasting {
            moment(end, local)?.max(start_at)
        } else {
            start_at
        };

        (start_at, end_at)
    } else {
        // a date is the same day in every zone, so it stays as written
        let first = start.naive_local().date();
        let after = end.naive_local().date().max(first.succ_opt()?);

        bounds(first, after)
    };

    Some(Occurrence {
        title: text(component, &ICalendarProperty::Summary).unwrap_or_default(),
        location: text(component, &ICalendarProperty::Location),
        start,
        end,
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
 * a copy whose recurrences end soon after `to`: calcard expands a rule from its first occurrence
 * until its end, so one with none, like a weekly meeting, would run to the limit and crowd out the
 * file's other events. Two days past `to` covers any zone the rule's times are in; a rule that
 * starts after it keeps only its first occurrence, which is after the range too
 */
fn bounded(calendar: &ICalendar, to: NaiveDate) -> Cow<'_, ICalendar> {
    let recurs = calendar
        .components
        .iter()
        .any(|component| component.property(&ICalendarProperty::Rrule).is_some());
    let Some(last) = to.checked_add_days(Days::new(2)).filter(|_| recurs) else {
        return Cow::Borrowed(calendar);
    };
    let mut calendar = calendar.clone();

    for component in &mut calendar.components {
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
    }

    Cow::Owned(calendar)
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
    use chrono::TimeDelta;

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
        .collect()
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
            all_day: false,
        };

        assert!(midnight.on(day(2026, 10, 9)));
        assert!(!midnight.on(day(2026, 10, 8)));
        assert_eq!(midnight.end - midnight.start, TimeDelta::zero());
    }
}
