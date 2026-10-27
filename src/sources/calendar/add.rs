//! An event typed into the Calendar Surface (ADR 0045), kept as an iCalendar file of its own in
//! `$XDG_DATA_HOME/kanade/events`, one file per event, which the `calendar` Module reads like any
//! other. The text is `[date ][start[-end] ]title`, a date as `2026-10-15` and a time as `14:30`:
//! with no date it is the chosen day, with no time the event lasts the day, with a start alone it
//! lasts an hour. Times are floating, so they keep their wall time. Changing one rewrites only its
//! SUMMARY, DTSTART and DTEND and keeps what else the file holds (ADR 0113).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Days, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta};

use calcard::icalendar::{ICalendarComponentType, ICalendarProperty};

use super::{Occurrence, data_home};
use crate::atomic::{self, Mode};
use crate::clock;
use crate::sources::ical::{escape, line};

const PRODID: &str = "-//Kanade//Calendar//EN";

// ends the UID of every event typed here, ahead of which is the name of its file
const DOMAIN: &str = "@kanade";

// under the XDG data directory
const DIRECTORY: &str = "kanade/events";

// a title longer is no title, and would fold into many lines
const TITLE: usize = 200;

// how long an event with a start and no end lasts
const HOUR: TimeDelta = TimeDelta::hours(1);

static ADDED: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, PartialEq, Eq)]
pub struct Event {
    title: String,
    when: When,
}

#[derive(Debug, PartialEq, Eq)]
enum When {
    Day(NaiveDate),
    Span(NaiveDateTime, NaiveDateTime),
}

// where the events added here are, for the calendar to read
pub fn directory() -> Option<PathBuf> {
    data_home().map(|home| home.join(DIRECTORY))
}

// `text` as an event on `day`, or what to correct in it
pub fn parse(text: &str, day: NaiveDate) -> Result<Event, String> {
    let text = text.trim();
    let (first, rest) = split(text);

    // a first word that starts like a date is meant as one, so a typo in it is said, not made a title
    let (day, text) = if date_like(first) {
        (date(first)?, rest.trim())
    } else {
        (day, text)
    };

    let (first, rest) = split(text);

    // the same for a time
    let (title, when) =
        if first.starts_with(|char: char| char.is_ascii_digit()) && first.contains(':') {
            (rest.trim(), span(first, day)?)
        } else {
            (text, When::Day(day))
        };

    if title.is_empty() {
        return Err(String::from("Give it a title"));
    }
    if title.chars().count() > TITLE {
        return Err(format!("Keep the title under {TITLE} letters"));
    }

    Ok(Event {
        title: title.to_owned(),
        when,
    })
}

fn split(text: &str) -> (&str, &str) {
    text.split_once(char::is_whitespace).unwrap_or((text, ""))
}

/*
 * whether `word` is meant as a date: four digits and a dash start it (so `2026-10`, `2026-1-5`),
 * or it is only digits and two or more dashes or slashes (`10/15/2026`). A word with a colon is
 * a time's, said as one
 */
fn date_like(word: &str) -> bool {
    let bytes = word.as_bytes();
    let year = bytes.len() > 4 && bytes[..4].iter().all(u8::is_ascii_digit) && bytes[4] == b'-';
    let marks = word
        .chars()
        .filter(|char| matches!(char, '-' | '/'))
        .count();
    let plain = word
        .chars()
        .all(|char| char.is_ascii_digit() || matches!(char, '-' | '/'));

    !word.contains(':') && (year || (plain && marks >= 2))
}

// `2026-10-15`, only as written with four, two and two digits
fn date(text: &str) -> Result<NaiveDate, String> {
    let bytes = text.as_bytes();
    let shaped = bytes.len() == 10
        && bytes.iter().enumerate().all(|(at, byte)| {
            matches!(at, 4 | 7) == (*byte == b'-') && (matches!(at, 4 | 7) || byte.is_ascii_digit())
        });

    shaped
        .then(|| NaiveDate::parse_from_str(text, "%Y-%m-%d").ok())
        .flatten()
        .ok_or_else(|| format!("{text} is not a date, like 2026-10-15"))
}

// `14:30` or `14:30-15:45` on `day`
fn span(text: &str, day: NaiveDate) -> Result<When, String> {
    let time = |text: &str| {
        NaiveTime::parse_from_str(text, "%H:%M")
            .map_err(|_| format!("{text} is not a time, like 14:30"))
    };
    let (from, to) = match text.split_once('-') {
        Some((from, to)) => (time(from)?, Some(time(to)?)),
        None => (time(text)?, None),
    };

    let start = day.and_time(from);
    let end = match to {
        Some(to) if to <= from => return Err(String::from("It ends before it starts")),
        Some(to) => day.and_time(to),
        None => start + HOUR,
    };

    Ok(When::Span(start, end))
}

// the name of the file an event's UID points to, none for a UID not made here; a file elsewhere
// may carry any UID, so only what this makes names a path
pub fn stem(uid: &str) -> Option<&str> {
    uid.strip_suffix(DOMAIN).filter(|stem| {
        !stem.is_empty()
            && stem.len() <= 64
            && stem
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    })
}

// the name of the file in `events`, the events directory, that `file` is, none for any other file
pub fn named<'a>(file: &'a Path, events: Option<&Path>) -> Option<&'a str> {
    let stem = file.file_stem()?.to_str()?;

    (events.is_some()
        && file.parent() == events
        && file.extension().is_some_and(|extension| extension == "ics")
        && self::stem(&uid(stem)) == Some(stem))
    .then_some(stem)
}

// the UID of the event kept in file `stem`
pub fn uid(stem: &str) -> String {
    format!("{stem}{DOMAIN}")
}

/*
 * the event as typed again, to change it, its date first so the change can move it. An end an hour
 * after the start is the start alone, as typed: the end may be past midnight, which `span` takes
 * for an end before the start
 */
pub fn text(event: &Occurrence) -> String {
    let hour = event.end == event.start + HOUR;
    let day = event.start.format("%Y-%m-%d");

    match (event.all_day, event.moment() || hour) {
        (true, _) => format!("{day} {}", event.title),
        (false, true) => format!("{day} {} {}", event.start.format("%H:%M"), event.title),
        (false, false) => format!(
            "{day} {}-{} {}",
            event.start.format("%H:%M"),
            event.end.format("%H:%M"),
            event.title
        ),
    }
}

// the event as a file of its own; the calendar sees it once the watch reads the directory again
pub fn save(event: &Event) -> io::Result<()> {
    let uid = format!(
        "{:x}-{:x}-{:x}",
        clock::unix_seconds(),
        process::id(),
        ADDED.fetch_add(1, Ordering::Relaxed)
    );

    write(&uid, event)
}

/*
 * why a change or a removal was refused, in words for the field: the file is left as it was
 */
#[derive(Debug)]
struct Refused(&'static str);

impl std::fmt::Display for Refused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for Refused {}

pub const CHANGED: &str = "Changed elsewhere";
const UNSAFE: &str = "Its file holds more than Kanade can change safely";
const UNSAID: &str = "The field cannot say this event; change its file";

fn refuse(why: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, Refused(why))
}

// what went wrong in `doing`, for the Surface: a refusal as it is said, any other error named
pub fn say(error: &io::Error, doing: &str) -> String {
    match error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<Refused>())
    {
        Some(refused) => refused.0.to_owned(),
        None => format!("Could not {doing}: {error}"),
    }
}

// a file larger is no event of ours
const LARGEST: u64 = 1 << 20;

/*
 * `event` in place of the one in file `stem`, which must still be as `was` showed it: the file is
 * read, checked and rewritten keeping all it holds besides SUMMARY, DTSTART and DTEND
 */
pub fn replace(stem: &str, event: &Event, was: &Occurrence) -> io::Result<()> {
    replace_in(&events()?, stem, event, was, clock::local_time)
}

// the file of event `stem` removed, if it is still as `was` showed it
pub fn remove(stem: &str, was: &Occurrence) -> io::Result<()> {
    remove_in(&events()?, stem, was, clock::local_time)
}

// the event in file `stem` re-read from disk, if it is still owned, typable, and valid
pub fn reread(stem: &str) -> Option<Occurrence> {
    reread_in(&events().ok()?, stem, clock::local_time)
}

fn reread_in(
    dir: &Path,
    stem: &str,
    local: impl Fn(i64) -> Option<NaiveDateTime>,
) -> Option<Occurrence> {
    let path = file_in(dir, stem).ok()?;
    let text = current(&path).ok()?;
    let read = super::parse(&text).ok()?;

    if !super::kept_here(&path, &read, Some(dir)) {
        return None;
    }

    let calendars: Vec<_> = read.into_iter().flat_map(super::series).collect();
    let event = calendars
        .iter()
        .flat_map(|calendar| &calendar.components)
        .find(|component| {
            component.component_type == ICalendarComponentType::VEvent
                && super::text(component, &ICalendarProperty::Uid)
                    .as_deref()
                    .and_then(self::stem)
                    == Some(stem)
        })?;

    let date = super::start_of(event)?.to_date_time()?.date_time.date();
    let last = date.succ_opt().unwrap_or(date);
    let found = super::occurrences(&calendars, date, last, local);
    let fresh = found
        .occurrences
        .into_iter()
        .find(|now| now.added.as_deref() == Some(stem))?;

    if !typable(&fresh) {
        return None;
    }

    Some(fresh)
}

fn events() -> io::Result<PathBuf> {
    directory().ok_or_else(|| io::Error::other("no data directory"))
}

fn replace_in(
    dir: &Path,
    stem: &str,
    event: &Event,
    was: &Occurrence,
    local: impl Fn(i64) -> Option<NaiveDateTime>,
) -> io::Result<()> {
    let path = file_in(dir, stem)?;
    let text = current(&path)?;

    check(&text, (dir, &path), stem, was, local)?;

    // the field holds one date and HH:MM times: an event it cannot say would be cut to fit
    if !typable(was) {
        return Err(refuse(UNSAID));
    }

    let text = rewrite(&text, &uid(stem), event)?;

    atomic::write(&path, text.as_bytes(), Mode::Private)
}

fn remove_in(
    dir: &Path,
    stem: &str,
    was: &Occurrence,
    local: impl Fn(i64) -> Option<NaiveDateTime>,
) -> io::Result<()> {
    let path = file_in(dir, stem)?;

    check(&current(&path)?, (dir, &path), stem, was, local)?;

    fs::remove_file(path)
}

fn file(stem: &str) -> io::Result<PathBuf> {
    file_in(&events()?, stem)
}

// the file of event `stem` in `dir`; only a name this makes is one
fn file_in(dir: &Path, stem: &str) -> io::Result<PathBuf> {
    if self::stem(&uid(stem)) != Some(stem) {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }

    Ok(dir.join(format!("{stem}.ics")))
}

// what the file holds now; one gone is one changed elsewhere
fn current(path: &Path) -> io::Result<String> {
    let read = || {
        if fs::metadata(path)?.len() > LARGEST {
            return Err(refuse(UNSAFE));
        }

        fs::read_to_string(path)
    };

    read().map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => refuse(CHANGED),
        io::ErrorKind::InvalidData if error.get_ref().is_none() => refuse(UNSAFE),
        _ => error,
    })
}

/*
 * `text` must parse back to the same event: the field cannot preserve multi-day events, missing or
 * later-day timed ends, seconds, or all-day titles that look like times. Missing or overlong titles
 * are left for the field to supply
 */
fn typable(was: &Occurrence) -> bool {
    let title = was.title.trim();
    let given = !title.is_empty() && title.chars().count() <= TITLE;
    let shape = Occurrence {
        title: if given {
            title.to_owned()
        } else {
            String::from("x")
        },
        ..was.clone()
    };
    let Ok(typed) = parse(&text(&shape), was.start.date()) else {
        return false;
    };

    (!given || typed.title == title)
        && match typed.when {
            When::Day(day) => {
                was.all_day
                    && was.start == day.and_time(NaiveTime::MIN)
                    && day
                        .succ_opt()
                        .is_some_and(|next| was.end == next.and_time(NaiveTime::MIN))
            }
            When::Span(start, end) => !was.all_day && (was.start, was.end) == (start, end),
        }
}

/*
 * whether `text`, the file of event `stem` in `dir`, still holds the event as `was` showed it: its
 * title, start and end, read the way the calendar reads them, so a time written with a zone
 * compares as it showed. Then whether it is Kanade's by the rule that read it as such, so a file
 * merged by hand with another UID's event is not
 */
fn check(
    text: &str,
    (dir, path): (&Path, &Path),
    stem: &str,
    was: &Occurrence,
    local: impl Fn(i64) -> Option<NaiveDateTime>,
) -> io::Result<()> {
    let Ok(read) = super::parse(text) else {
        return Err(refuse(CHANGED));
    };
    let owned = super::kept_here(path, &read, Some(dir));

    let calendars: Vec<_> = read.into_iter().flat_map(super::series).collect();
    let last = was.end.date().max(was.start.date());
    let found = super::occurrences(
        &calendars,
        was.start.date(),
        last.succ_opt().unwrap_or(last),
        local,
    );
    let same = found.occurrences.iter().any(|now| {
        now.added.as_deref() == Some(stem)
            && (&now.title, now.start, now.end, now.span, now.all_day)
                == (&was.title, was.start, was.end, was.span, was.all_day)
    });

    match (same, owned) {
        (false, _) => Err(refuse(CHANGED)),
        (true, false) => Err(refuse(UNSAFE)),
        (true, true) => Ok(()),
    }
}

// the properties a change would break or lose the meaning of
const NOT_CHANGED: [&str; 6] = [
    "RRULE",
    "RDATE",
    "EXRULE",
    "EXDATE",
    "RECURRENCE-ID",
    "DURATION",
];

/*
 * update only SUMMARY, DTSTART and DTEND for `uid`, preserving every other line and its place.
 * Refuse other events, mismatched UIDs, repeats, durations, or zoned times rather than lose or
 * guess at data
 */
fn rewrite(text: &str, uid: &str, event: &Event) -> io::Result<String> {
    let lines = unfold(text);

    let mut open: Vec<String> = Vec::new();
    let (mut events, mut uids) = (0, 0);
    let (mut summary, mut start, mut end, mut last) = (None, None, None, None);

    for (at, content) in lines.iter().enumerate() {
        let split = content.find([':', ';']).ok_or(refuse(UNSAFE))?;
        let (name, after) = content.split_at(split);
        let name = name.to_ascii_uppercase();
        let value = after
            .strip_prefix(':')
            .map(|value| value.trim().to_ascii_uppercase());

        match (name.as_str(), value) {
            ("BEGIN", Some(kind)) => {
                let allowed = match kind.as_str() {
                    "VCALENDAR" => open.is_empty(),
                    "VEVENT" => open == ["VCALENDAR"],
                    _ => !open.is_empty(),
                };

                if !allowed {
                    return Err(refuse(UNSAFE));
                }
                events += usize::from(kind == "VEVENT");
                open.push(kind);
            }
            ("END", Some(kind)) => {
                if open.pop() != Some(kind.clone()) {
                    return Err(refuse(UNSAFE));
                }
                if kind == "VEVENT" {
                    last = Some(at);
                }
            }
            ("BEGIN" | "END", None) => return Err(refuse(UNSAFE)),
            (name, _) if open == ["VCALENDAR", "VEVENT"] => {
                let slot = match name {
                    "SUMMARY" => Some(&mut summary),
                    "DTSTART" => Some(&mut start),
                    "DTEND" => Some(&mut end),
                    "UID" => {
                        uids += 1;
                        if after.strip_prefix(':').map(str::trim) != Some(uid) {
                            return Err(refuse(UNSAFE));
                        }
                        None
                    }
                    other if NOT_CHANGED.contains(&other) => return Err(refuse(UNSAFE)),
                    _ => None,
                };

                if let Some(slot) = slot
                    && slot.replace(at).is_some()
                {
                    return Err(refuse(UNSAFE));
                }
            }
            _ => {}
        }
    }

    // one event of this UID, in a calendar, with a start whose times Kanade writes
    let (Some(start), Some(last), true) =
        (start, last, open.is_empty() && events == 1 && uids == 1)
    else {
        return Err(refuse(UNSAFE));
    };
    for at in [Some(start), end].into_iter().flatten() {
        if !floating(&lines[at]) {
            return Err(refuse(UNSAFE));
        }
    }

    let mut out = String::with_capacity(text.len() + 64);
    let when = when_lines(&event.when);

    for (at, content) in lines.iter().enumerate() {
        match at {
            // both times where DTSTART was, so a missing DTEND has a place and none stays twice
            _ if at == start => when.iter().for_each(|content| line(&mut out, content)),
            _ if Some(at) == end => {}
            _ if Some(at) == summary => line(&mut out, &title_line(&event.title)),
            _ => {
                // a title that was missing goes at the end of the event, after its reminders
                if at == last && summary.is_none() {
                    line(&mut out, &title_line(&event.title));
                }
                line(&mut out, content);
            }
        }
    }

    Ok(out)
}

fn title_line(title: &str) -> String {
    format!("SUMMARY:{}", escape(title))
}

// a DTSTART or DTEND of a date or of a floating moment, as Kanade writes them and no other
fn floating(content: &str) -> bool {
    let content = content.to_ascii_uppercase();
    let digits = |text: &str| text.bytes().all(|byte| byte.is_ascii_digit());

    let Some((name, value)) = content.split_once(':') else {
        return false;
    };

    match name {
        "DTSTART" | "DTEND" => {
            value.len() == 15
                && value.as_bytes()[8] == b'T'
                && digits(&value[..8])
                && digits(&value[9..])
        }
        "DTSTART;VALUE=DATE" | "DTEND;VALUE=DATE" => value.len() == 8 && digits(value),
        _ => false,
    }
}

// the content lines of `text`, unfolded (RFC 5545 3.1), blank ones left out
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    for physical in text.split('\n') {
        let physical = physical.strip_suffix('\r').unwrap_or(physical);

        match (physical.strip_prefix([' ', '\t']), lines.last_mut()) {
            (Some(more), Some(last)) => last.push_str(more),
            _ if physical.trim().is_empty() => {}
            _ => lines.push(physical.to_owned()),
        }
    }

    lines
}

fn write(stem: &str, event: &Event) -> io::Result<()> {
    let stamp = DateTime::from_timestamp(clock::unix_seconds(), 0).unwrap_or_default();
    let text = vcalendar(event, &uid(stem), stamp.naive_utc());

    atomic::write(&file(stem)?, text.as_bytes(), Mode::Private)
}

// the DTSTART and DTEND of `when`
fn when_lines(when: &When) -> Vec<String> {
    let date = |date: NaiveDate| date.format("%Y%m%d");
    let moment = |at: NaiveDateTime| at.format("%Y%m%dT%H%M%S");

    match when {
        When::Day(day) => {
            let next = day.checked_add_days(Days::new(1)).unwrap_or(*day);

            vec![
                format!("DTSTART;VALUE=DATE:{}", date(*day)),
                format!("DTEND;VALUE=DATE:{}", date(next)),
            ]
        }
        When::Span(start, end) => vec![
            format!("DTSTART:{}", moment(*start)),
            format!("DTEND:{}", moment(*end)),
        ],
    }
}

fn vcalendar(event: &Event, uid: &str, stamp: NaiveDateTime) -> String {
    let mut text = String::new();
    let mut put = |content: String| line(&mut text, &content);

    put(String::from("BEGIN:VCALENDAR"));
    put(String::from("VERSION:2.0"));
    put(format!("PRODID:{PRODID}"));
    put(String::from("BEGIN:VEVENT"));
    put(format!("UID:{uid}"));
    put(format!("DTSTAMP:{}Z", stamp.format("%Y%m%dT%H%M%S")));
    when_lines(&event.when).into_iter().for_each(&mut put);
    put(title_line(&event.title));
    put(String::from("END:VEVENT"));
    put(String::from("END:VCALENDAR"));

    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 13).unwrap()
    }

    fn at(hour: u32, minute: u32) -> NaiveDateTime {
        day().and_hms_opt(hour, minute, 0).unwrap()
    }

    #[test]
    fn a_title_alone_lasts_the_day_and_a_start_lasts_an_hour() {
        assert_eq!(
            parse("  Dentist, 2nd visit ", day()),
            Ok(Event {
                title: String::from("Dentist, 2nd visit"),
                when: When::Day(day())
            })
        );
        assert_eq!(
            parse("9:05 Standup", day()).map(|event| event.when),
            Ok(When::Span(at(9, 5), at(10, 5)))
        );
        assert_eq!(
            parse("14:30-15:45 Review", day()).map(|event| event.when),
            Ok(When::Span(at(14, 30), at(15, 45)))
        );
    }

    #[test]
    fn what_cannot_be_an_event_says_what_to_correct() {
        assert!(parse("25:00 Late", day()).is_err());
        assert!(parse("15:00-14:00 Backwards", day()).is_err());
        assert!(parse("14:30", day()).is_err());
        assert!(parse("   ", day()).is_err());
    }

    // a first word that starts like a time never becomes the title of an all-day event
    #[test]
    fn a_malformed_time_is_not_a_title() {
        for typed in [
            "14:xx Dentist",
            "3:30pm Dentist",
            "14:30- Dentist",
            "9:00-10:xx Standup",
        ] {
            assert!(parse(typed, day()).is_err(), "{typed}");
        }
        assert_eq!(
            parse("Re: 14:30 plan", day()).map(|event| event.when),
            Ok(When::Day(day()))
        );
        assert!(parse(&"x".repeat(TITLE + 1), day()).is_err());
    }

    // the one event a file holds, as the calendar's own reader finds it
    fn read_back(text: &str) -> Occurrence {
        let calendars: Vec<_> = super::super::parse(text)
            .unwrap()
            .into_iter()
            .flat_map(super::super::series)
            .collect();
        let mut found =
            super::super::occurrences(&calendars, day(), day().succ_opt().unwrap(), |seconds| {
                DateTime::from_timestamp(seconds, 0).map(|at| at.naive_utc())
            });

        assert_eq!(found.occurrences.len(), 1);
        found.occurrences.remove(0)
    }

    // the file reads back through the calendar's own reader as the event typed
    #[test]
    fn the_file_is_an_event_the_calendar_reads() {
        let event = parse("14:30-15:00 Plan; ship, repeat", day()).unwrap();
        let text = vcalendar(&event, "a@kanade", at(8, 0));

        assert!(text.contains("DTSTART:20261013T143000\r\n"));
        assert!(text.contains("SUMMARY:Plan\\; ship\\, repeat\r\n"));

        let found = read_back(&text);

        assert_eq!(found.title, "Plan; ship, repeat");
        assert_eq!(found.start, at(14, 30));
        assert_eq!(found.added.as_deref(), Some("a"));
    }

    // what the field shows to change an event types back as the same event
    #[test]
    fn an_event_typed_again_is_the_event() {
        for typed in [
            "Dentist",
            "9:05-10:05 Standup",
            "14:30-15:45 Review",
            "23:30 Deployment",
            "2026-10-13 23:30 Deployment",
        ] {
            let event = parse(typed, day()).unwrap();
            let found = read_back(&vcalendar(&event, "a@kanade", at(8, 0)));

            assert_eq!(parse(&text(&found), day()), Ok(event));
        }
    }

    // a UID in a file from elsewhere never names a path
    #[test]
    fn only_a_uid_made_here_names_a_file() {
        assert_eq!(stem("69e5ea0f-1f4a-0@kanade"), Some("69e5ea0f-1f4a-0"));
        assert_eq!(stem("../../x@kanade"), None);
        assert_eq!(stem("a/b@kanade"), None);
        assert_eq!(stem("@kanade"), None);
        assert_eq!(stem("abc@example.com"), None);
        assert!(file("../x").is_err());
    }

    fn utc(seconds: i64) -> Option<NaiveDateTime> {
        DateTime::from_timestamp(seconds, 0).map(|at| at.naive_utc())
    }

    // a scratch events directory, removed with it
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("kanade-add-{name}-{}", process::id()));

            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn put(&self, stem: &str, text: &str) -> PathBuf {
            let path = self.0.join(format!("{stem}.ics"));

            fs::write(&path, text).unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    // an event with what Kanade never writes: a place, a folded note, a program's own property and
    // a reminder with a SUMMARY and a DESCRIPTION of its own
    fn rich(summary: &str, uid: &str) -> String {
        [
            "BEGIN:VCALENDAR",
            "VERSION:2.0",
            "PRODID:-//Other//EN",
            "BEGIN:VEVENT",
            &format!("UID:{uid}"),
            "DTSTAMP:20261001T080000Z",
            "DTSTART:20261013T143000",
            "DTEND:20261013T150000",
            &format!("SUMMARY:{summary}"),
            "LOCATION:Clinic\\, 2nd floor",
            "DESCRIPTION:A note that folds",
            "  onto a second line",
            "X-KANADE-NOTE:keep me",
            "BEGIN:VALARM",
            "ACTION:DISPLAY",
            "DESCRIPTION:Dentist soon",
            "SUMMARY:alarm title",
            "TRIGGER:-PT15M",
            "END:VALARM",
            "END:VEVENT",
            "END:VCALENDAR",
            "",
        ]
        .join("\r\n")
    }

    // the event as the Surface would have loaded it
    fn loaded(text: &str) -> Occurrence {
        read_back(text)
    }

    #[test]
    fn a_date_first_moves_the_event_to_that_day() {
        let other = NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();

        assert_eq!(
            parse("2026-11-02 Dentist", day()).map(|event| event.when),
            Ok(When::Day(other))
        );
        assert_eq!(
            parse("2026-11-02 14:30-15:00 Dentist", day()).map(|event| event.when),
            Ok(When::Span(
                other.and_hms_opt(14, 30, 0).unwrap(),
                other.and_hms_opt(15, 0, 0).unwrap()
            ))
        );
        // a word that is not the first stays in the title, and so does a short dash
        assert_eq!(
            parse("2026-11-02 2026-27 budget", day()).map(|event| event.title),
            Ok(String::from("2026-27 budget"))
        );
        assert_eq!(
            parse("3-2 win", day()).map(|event| event.title),
            Ok(String::from("3-2 win"))
        );
    }

    // a first word that starts like a date never becomes the title, as with a time
    #[test]
    fn a_malformed_or_impossible_date_is_said_not_made_a_title() {
        for typed in [
            "2026-1-5 Dentist",
            "2026-13-01 Dentist",
            "2026-02-30 Dentist",
            "2026-10 Dentist",
            "2026-10-15x Dentist",
            "10/15/2026 Dentist",
            "10-15-2026 Dentist",
            "2026-10-15T14:30 Dentist",
            "2026-10-15",
        ] {
            assert!(parse(typed, day()).is_err(), "{typed}");
        }
    }

    // parse, write, read and parse again gives the event, on a day of its own
    #[test]
    fn a_dated_event_survives_being_written_and_read() {
        for typed in [
            "2026-12-24 Dinner",
            "2026-12-24 18:00 Dinner",
            "2026-12-24 18:00-23:30 Dinner",
        ] {
            let event = parse(typed, day()).unwrap();
            let file = vcalendar(&event, "a@kanade", at(8, 0));
            let found = {
                let calendars: Vec<_> = super::super::parse(&file)
                    .unwrap()
                    .into_iter()
                    .flat_map(super::super::series)
                    .collect();
                let from = NaiveDate::from_ymd_opt(2026, 12, 24).unwrap();
                let mut found =
                    super::super::occurrences(&calendars, from, from.succ_opt().unwrap(), utc);

                found.occurrences.remove(0)
            };

            assert_eq!(parse(&text(&found), day()), Ok(event), "{typed}");
        }
    }

    // a change keeps the UID, the place, the notes, a program's property and the reminder
    #[test]
    fn a_change_rewrites_only_the_title_and_the_times() {
        let scratch = Scratch::new("keep");
        let original = rich("Dentist", "ab-1@kanade");
        let path = scratch.put("ab-1", &original);
        let was = loaded(&original);

        let event = parse("2026-10-14 9:00-9:45 Dentist, moved", day()).unwrap();
        replace_in(&scratch.0, "ab-1", &event, &was, utc).unwrap();

        let changed = fs::read_to_string(&path).unwrap();
        let (before, after) = (unfold(&original), unfold(&changed));
        let kept: Vec<_> = before
            .iter()
            .filter(|line| {
                !["SUMMARY:Dentist", "DTSTART:", "DTEND:"]
                    .iter()
                    .any(|own| line.starts_with(own))
            })
            .cloned()
            .collect();
        let mut rest = after.iter();

        // every other line, in its order: the alarm's own SUMMARY too
        for line in &kept {
            assert!(rest.any(|now| now == line), "{line} lost");
        }
        assert!(after.contains(&String::from("SUMMARY:Dentist\\, moved")));
        assert!(after.contains(&String::from("DTSTART:20261014T090000")));
        assert!(after.contains(&String::from("DTEND:20261014T094500")));
        assert!(after.contains(&String::from("SUMMARY:alarm title")));
        assert_eq!(after.len(), before.len());

        let now = loaded_on(&changed, NaiveDate::from_ymd_opt(2026, 10, 14).unwrap());

        assert_eq!(now.title, "Dentist, moved");
        assert_eq!(now.location.as_deref(), Some("Clinic, 2nd floor"));
        assert_eq!(now.added.as_deref(), Some("ab-1"));
        assert!(after.contains(&String::from(
            "DESCRIPTION:A note that folds onto a second line"
        )));
    }

    fn loaded_on(text: &str, on: NaiveDate) -> Occurrence {
        let calendars: Vec<_> = super::super::parse(text)
            .unwrap()
            .into_iter()
            .flat_map(super::super::series)
            .collect();
        let mut found = super::super::occurrences(&calendars, on, on.succ_opt().unwrap(), utc);

        assert_eq!(found.occurrences.len(), 1);
        found.occurrences.remove(0)
    }

    // an event with no title gets one where the file can hold it
    #[test]
    fn a_missing_title_is_added_to_the_event() {
        let scratch = Scratch::new("missing");
        let original = rich("Dentist", "ab-1@kanade").replace("SUMMARY:Dentist\r\n", "");
        let path = scratch.put("ab-1", &original);
        let was = loaded(&original);

        let event = parse("14:30-15:00 Dentist", day()).unwrap();
        replace_in(&scratch.0, "ab-1", &event, &was, utc).unwrap();

        let lines = unfold(&fs::read_to_string(&path).unwrap());

        assert!(lines.contains(&String::from("SUMMARY:Dentist")));
        // the title joined the event, not the reminder
        let alarm = lines
            .iter()
            .position(|line| line == "BEGIN:VALARM")
            .unwrap();
        assert!(
            lines
                .iter()
                .position(|line| line == "SUMMARY:Dentist")
                .unwrap()
                > alarm
        );
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.starts_with("SUMMARY:"))
                .count(),
            2
        );
    }

    // the file changed under the field: nothing is written, and the Surface says why
    #[test]
    fn a_change_or_removal_of_an_event_changed_elsewhere_is_refused() {
        let scratch = Scratch::new("conflict");
        let original = rich("Dentist", "ab-1@kanade");
        let path = scratch.put("ab-1", &original);
        let was = loaded(&original);
        let event = parse("Mine", day()).unwrap();

        for elsewhere in [
            original.replace("SUMMARY:Dentist", "SUMMARY:Doctor"),
            original.replace("T143000", "T153000"),
            original.replace("DTEND:20261013T150000", "DTEND:20261013T160000"),
        ] {
            fs::write(&path, &elsewhere).unwrap();

            let replaced = replace_in(&scratch.0, "ab-1", &event, &was, utc).unwrap_err();
            let removed = remove_in(&scratch.0, "ab-1", &was, utc).unwrap_err();

            assert_eq!(say(&replaced, "save"), "Changed elsewhere");
            assert_eq!(say(&removed, "remove"), "Changed elsewhere");
            assert_eq!(fs::read_to_string(&path).unwrap(), elsewhere);
        }

        // a place or a note changed elsewhere is not a change to what was shown
        fs::write(&path, original.replace("Clinic", "Annex")).unwrap();
        replace_in(&scratch.0, "ab-1", &event, &was, utc).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("Annex"));

        // one gone, or put back as it was and removed
        let again = loaded(&original);
        fs::write(&path, &original).unwrap();
        remove_in(&scratch.0, "ab-1", &again, utc).unwrap();
        let gone = remove_in(&scratch.0, "ab-1", &again, utc).unwrap_err();

        assert_eq!(say(&gone, "remove"), "Changed elsewhere");
    }

    // only Kanade's own files change: a name that is no stem, a UID that is another's, a repeat,
    // a time with a zone, several events
    #[test]
    fn a_file_that_is_not_an_owned_plain_event_is_never_rewritten_or_removed() {
        let scratch = Scratch::new("owned");
        let original = rich("Dentist", "ab-1@kanade");
        let was = loaded(&original);
        let event = parse("Mine", day()).unwrap();

        for stem in ["../x", "a/b", "", "zz", "ab-1/../ab-1"] {
            let error = replace_in(&scratch.0, stem, &event, &was, utc).unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{stem}");
            assert!(remove_in(&scratch.0, stem, &was, utc).is_err());
        }

        let odd = [
            original.replace("UID:ab-1@kanade", "UID:ab-1@example.com"),
            original.replace("UID:ab-1@kanade", "UID:cd-2@kanade"),
            original.replace("DTSTAMP", "RRULE:FREQ=DAILY\r\nDTSTAMP"),
            original.replace("DTSTART:20261013T143000", "DTSTART:20261013T143000Z"),
            original.replace("DTSTART:", "DTSTART;TZID=Europe/Paris:"),
            original.replace("DTEND:20261013T150000", "DURATION:PT30M"),
            original.replace(
                "END:VCALENDAR",
                "BEGIN:VEVENT\r\nUID:ab-1@kanade\r\nEND:VEVENT\r\nEND:VCALENDAR",
            ),
            original.replace("END:VEVENT", "END:VALARM"),
            original.replace("UID:ab-1@kanade\r\n", ""),
            original.replace("LOCATION", "garbage with no colon\r\nLOCATION"),
        ];

        for text in odd {
            let path = scratch.put("ab-1", &text);
            // the rewrite alone refuses it too, whatever the check before it says
            assert!(rewrite(&text, "ab-1@kanade", &event).is_err(), "{text}");

            let error = replace_in(&scratch.0, "ab-1", &event, &was, utc).unwrap_err();

            assert_eq!(fs::read_to_string(&path).unwrap(), text);
            assert_ne!(error.kind(), io::ErrorKind::NotFound);
        }

        // a copy under another name, or another file's UID, is not removed either
        let foreign = original.replace("UID:ab-1@kanade", "UID:ab-1@example.com");
        let path = scratch.put("ab-1", &foreign);

        assert!(remove_in(&scratch.0, "ab-1", &was, utc).is_err());
        assert!(path.exists());
    }

    // what the field cannot say is not cut to fit: the change is refused, the removal is not
    #[test]
    fn an_event_the_field_cannot_say_is_not_changed_but_can_be_removed() {
        let scratch = Scratch::new("shrink");
        let event = parse("Mine", day()).unwrap();
        let original = rich("Dentist", "ab-1@kanade");
        let longer = [
            original
                .replace("DTSTART:20261013T143000", "DTSTART;VALUE=DATE:20261013")
                .replace("DTEND:20261013T150000", "DTEND;VALUE=DATE:20261015"),
            original.replace("DTEND:20261013T150000", "DTEND:20261014T010000"),
            original.replace("DTEND:20261013T150000\r\n", ""),
            original.replace("T143000", "T143005"),
            // a title that starts like a time would be read as one
            original
                .replace("DTSTART:20261013T143000", "DTSTART;VALUE=DATE:20261013")
                .replace("DTEND:20261013T150000", "DTEND;VALUE=DATE:20261014")
                .replace("SUMMARY:Dentist", "SUMMARY:10:30 Standup"),
        ];

        for text in longer {
            let path = scratch.put("ab-1", &text);
            let was = loaded(&text);
            let error = replace_in(&scratch.0, "ab-1", &event, &was, utc).unwrap_err();

            assert_eq!(say(&error, "save"), UNSAID, "{text}");
            assert_eq!(fs::read_to_string(&path).unwrap(), text);

            remove_in(&scratch.0, "ab-1", &was, utc).unwrap();
            assert!(!path.exists());
        }

        // what Kanade writes is always one it can say
        for typed in [
            "Dentist",
            "9:05 Standup",
            "14:30-15:45 Review",
            "23:30 Deployment",
        ] {
            let first = vcalendar(&parse(typed, day()).unwrap(), "ab-1@kanade", at(8, 0));
            let path = scratch.put("ab-1", &first);

            replace_in(&scratch.0, "ab-1", &event, &loaded(&first), utc).unwrap();
            assert_ne!(fs::read_to_string(&path).unwrap(), first, "{typed}");
        }
    }

    // a file merged by hand with another UID's event loses neither
    #[test]
    fn a_file_holding_another_event_is_not_removed() {
        let scratch = Scratch::new("merged");
        let original = rich("Dentist", "ab-1@kanade");
        let was = loaded(&original);
        let merged = original.replace(
            "END:VCALENDAR",
            "BEGIN:VEVENT\r\nUID:cd-2@kanade\r\nDTSTART:20261020T100000\r\nSUMMARY:Other\r\nEND:VEVENT\r\nEND:VCALENDAR",
        );
        let path = scratch.put("ab-1", &merged);

        let removed = remove_in(&scratch.0, "ab-1", &was, utc).unwrap_err();
        let replaced = replace_in(
            &scratch.0,
            "ab-1",
            &parse("Mine", day()).unwrap(),
            &was,
            utc,
        )
        .unwrap_err();

        assert_eq!(say(&removed, "remove"), UNSAFE);
        assert_eq!(say(&replaced, "save"), UNSAFE);
        assert_eq!(fs::read_to_string(&path).unwrap(), merged);
    }

    // the file as another program writes it: LF line ends, lower case names, folded lines and a
    // SUMMARY with a parameter all change in place and lose nothing
    #[test]
    fn the_rewrite_reads_a_file_as_other_programs_write_it() {
        let scratch = Scratch::new("shapes");
        let original = rich("Dentist", "ab-1@kanade");
        let event = parse("2026-10-14 9:00-9:45 Moved", day()).unwrap();
        let on = NaiveDate::from_ymd_opt(2026, 10, 14).unwrap();

        let shapes = [
            original.replace("\r\n", "\n"),
            original
                .replace("SUMMARY:Dentist", "summary:Dentist")
                .replace("DTSTART:", "dtstart:")
                .replace("DTEND:", "dtend:")
                .replace("LOCATION", "location"),
            original
                .replace("DTSTART:20261013T143000", "DTSTART:20261013\r\n T143000")
                .replace("SUMMARY:Dentist", "SUMMARY:Den\r\n tist"),
            original.replace("SUMMARY:Dentist", "SUMMARY;LANGUAGE=en:Dentist"),
        ];

        for text in shapes {
            let path = scratch.put("ab-1", &text);

            replace_in(&scratch.0, "ab-1", &event, &loaded(&text), utc).unwrap();

            let changed = fs::read_to_string(&path).unwrap();
            let lines = unfold(&changed);
            let now = loaded_on(&changed, on);

            assert_eq!(now.title, "Moved", "{text}");
            assert_eq!(
                (now.start, now.end),
                (
                    on.and_hms_opt(9, 0, 0).unwrap(),
                    on.and_hms_opt(9, 45, 0).unwrap()
                )
            );
            assert_eq!(now.location.as_deref(), Some("Clinic, 2nd floor"));
            // the event's own, once each, and the reminder's SUMMARY as it was
            for name in [
                "SUMMARY:Moved",
                "DTSTART:20261014T090000",
                "DTEND:20261014T094500",
                "SUMMARY:alarm title",
                "X-KANADE-NOTE:keep me",
            ] {
                assert_eq!(
                    lines.iter().filter(|line| *line == name).count(),
                    1,
                    "{name} in {changed}"
                );
            }
            assert_eq!(
                lines
                    .iter()
                    .filter(|line| line.to_ascii_uppercase().starts_with("SUMMARY"))
                    .count(),
                2
            );
        }
    }

    // an all-day event with no DTEND is one day, and gets its end where DTSTART was
    #[test]
    fn an_all_day_event_with_no_end_is_given_one() {
        let scratch = Scratch::new("no-end");
        let original = rich("Dentist", "ab-1@kanade")
            .replace("DTSTART:20261013T143000", "DTSTART;VALUE=DATE:20261013")
            .replace("DTEND:20261013T150000\r\n", "");
        let path = scratch.put("ab-1", &original);
        let was = loaded(&original);

        replace_in(
            &scratch.0,
            "ab-1",
            &parse("Mine", day()).unwrap(),
            &was,
            utc,
        )
        .unwrap();

        let lines = unfold(&fs::read_to_string(&path).unwrap());
        let at = |name: &str| lines.iter().position(|line| line == name).unwrap();

        assert_eq!(
            at("DTEND;VALUE=DATE:20261014"),
            at("DTSTART;VALUE=DATE:20261013") + 1
        );
    }

    // after a change elsewhere, the event is re-read from disk with its new values
    #[test]
    fn reread_returns_updated_occurrence_and_none_when_gone_or_unsafe() {
        let scratch = Scratch::new("reread");
        let original = rich("Dentist", "cd-2@kanade");
        scratch.put("cd-2", &original);

        let read = reread_in(&scratch.0, "cd-2", utc).unwrap();
        assert_eq!(read.title, "Dentist");
        assert_eq!(read.added.as_deref(), Some("cd-2"));

        // modified event with new title
        let modified = original.replace("SUMMARY:Dentist", "SUMMARY:Doctor");
        scratch.put("cd-2", &modified);

        let read2 = reread_in(&scratch.0, "cd-2", utc).unwrap();
        assert_eq!(read2.title, "Doctor");

        // deleted file returns None
        let _ = fs::remove_file(scratch.0.join("cd-2.ics"));
        assert_eq!(reread_in(&scratch.0, "cd-2", utc), None);
    }
}
