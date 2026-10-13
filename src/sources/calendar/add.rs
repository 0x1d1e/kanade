//! An event typed into the Calendar Surface (ADR 0045), kept as an iCalendar file of its own in
//! `$XDG_DATA_HOME/kanade/events`, one file per event, which the `calendar` Module reads like any
//! other. The text is `[start[-end] ]title`, a time as `14:30`: with none the event lasts the day,
//! with a start alone it lasts an hour. Times are floating, so they keep their wall time.

use std::io;
use std::path::PathBuf;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Days, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta};

use super::data_home;
use crate::atomic::{self, Mode};
use crate::clock;
use crate::sources::ical::{escape, line};

const PRODID: &str = "-//Kanade//Calendar//EN";

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
    let (first, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));

    let (title, when) = if first.contains(':')
        && first
            .chars()
            .all(|char| char.is_ascii_digit() || ":-".contains(char))
    {
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

// the event as a file of its own; the calendar sees it once the watch reads the directory again
pub fn save(event: &Event) -> io::Result<()> {
    let dir = directory().ok_or_else(|| io::Error::other("no data directory"))?;
    let uid = format!(
        "{:x}-{:x}-{:x}",
        clock::unix_seconds(),
        process::id(),
        ADDED.fetch_add(1, Ordering::Relaxed)
    );
    let stamp = DateTime::from_timestamp(clock::unix_seconds(), 0).unwrap_or_default();
    let text = vcalendar(event, &format!("{uid}@kanade"), stamp.naive_utc());

    atomic::write(
        &dir.join(format!("{uid}.ics")),
        text.as_bytes(),
        Mode::Private,
    )
}

fn vcalendar(event: &Event, uid: &str, stamp: NaiveDateTime) -> String {
    let date = |date: NaiveDate| date.format("%Y%m%d");
    let moment = |at: NaiveDateTime| at.format("%Y%m%dT%H%M%S");

    let mut text = String::new();
    let mut put = |content: String| line(&mut text, &content);

    put(String::from("BEGIN:VCALENDAR"));
    put(String::from("VERSION:2.0"));
    put(format!("PRODID:{PRODID}"));
    put(String::from("BEGIN:VEVENT"));
    put(format!("UID:{uid}"));
    put(format!("DTSTAMP:{}Z", moment(stamp)));
    match &event.when {
        When::Day(day) => {
            put(format!("DTSTART;VALUE=DATE:{}", date(*day)));
            let next = day.checked_add_days(Days::new(1)).unwrap_or(*day);
            put(format!("DTEND;VALUE=DATE:{}", date(next)));
        }
        When::Span(start, end) => {
            put(format!("DTSTART:{}", moment(*start)));
            put(format!("DTEND:{}", moment(*end)));
        }
    }
    put(format!("SUMMARY:{}", escape(&event.title)));
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
        assert!(parse(&"x".repeat(TITLE + 1), day()).is_err());
    }

    // the file reads back through the calendar's own reader as the event typed
    #[test]
    fn the_file_is_an_event_the_calendar_reads() {
        let event = parse("14:30-15:00 Plan; ship, repeat", day()).unwrap();
        let text = vcalendar(&event, "a@kanade", at(8, 0));

        assert!(text.contains("DTSTART:20261013T143000\r\n"));
        assert!(text.contains("SUMMARY:Plan\\; ship\\, repeat\r\n"));

        let calendars: Vec<_> = super::super::parse(&text)
            .unwrap()
            .into_iter()
            .flat_map(super::super::series)
            .collect();
        let found =
            super::super::occurrences(&calendars, day(), day().succ_opt().unwrap(), |seconds| {
                DateTime::from_timestamp(seconds, 0).map(|at| at.naive_utc())
            });

        assert_eq!(found.occurrences.len(), 1);
        assert_eq!(found.occurrences[0].title, "Plan; ship, repeat");
        assert_eq!(found.occurrences[0].start, at(14, 30));
    }
}
