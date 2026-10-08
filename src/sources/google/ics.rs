//! A Google calendar's events as one iCalendar file, which the `calendar` Module reads like any
//! other (ADR 0016). Google expands the recurrences itself (`singleEvents`), so each occurrence is
//! one VEVENT of its own, with no rule: a timed one in UTC, an all-day one by its dates. Only what
//! the Calendar Surface shows is written: title, place and times.

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde_json::Value;

const PRODID: &str = "-//Kanade//Google Calendar//EN";

// a content line's longest, in octets, before it folds (RFC 5545 3.1)
const LINE: usize = 75;

// the file for `events`, each an item of Google's `events.list`
pub fn calendar(events: &[Value]) -> String {
    let mut text = String::new();

    line(&mut text, "BEGIN:VCALENDAR");
    line(&mut text, "VERSION:2.0");
    line(&mut text, &format!("PRODID:{PRODID}"));
    for event in events {
        if let Some(vevent) = vevent(event) {
            text.push_str(&vevent);
        }
    }
    line(&mut text, "END:VCALENDAR");

    text
}

/*
 * one event as a VEVENT, none for one cancelled, declined by the user, or with no id or start.
 * Each occurrence's id is its own, while its iCalUID is the series', so the id is the UID
 */
fn vevent(event: &Value) -> Option<String> {
    let field = |name: &str| event.get(name).and_then(Value::as_str);

    if field("status") == Some("cancelled") || declined(event) {
        return None;
    }

    let id = field("id")?;
    let start = moment(event.get("start")?)?;
    let end = event.get("end").and_then(moment);

    let mut text = String::new();
    line(&mut text, "BEGIN:VEVENT");
    line(&mut text, &format!("UID:{}@google.com", escape(id)));
    line(&mut text, &format!("DTSTART{start}"));
    if let Some(end) = end {
        line(&mut text, &format!("DTEND{end}"));
    }
    if let Some(summary) = field("summary") {
        line(&mut text, &format!("SUMMARY:{}", escape(summary)));
    }
    if let Some(location) = field("location").filter(|location| !location.is_empty()) {
        line(&mut text, &format!("LOCATION:{}", escape(location)));
    }
    line(&mut text, "END:VEVENT");

    Some(text)
}

// whether the user said no to it, which Google Calendar hides too
fn declined(event: &Value) -> bool {
    event
        .get("attendees")
        .and_then(Value::as_array)
        .is_some_and(|attendees| {
            attendees.iter().any(|attendee| {
                attendee.get("self").and_then(Value::as_bool) == Some(true)
                    && attendee.get("responseStatus").and_then(Value::as_str) == Some("declined")
            })
        })
}

/*
 * a start or end as what follows its property's name: a date for an all-day one, else its
 * RFC 3339 time in UTC, which keeps it whatever zone it was made in
 */
fn moment(value: &Value) -> Option<String> {
    if let Some(date) = value.get("date").and_then(Value::as_str) {
        let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;

        return Some(format!(";VALUE=DATE:{}", date.format("%Y%m%d")));
    }

    let at = DateTime::parse_from_rfc3339(value.get("dateTime")?.as_str()?).ok()?;

    Some(format!(":{}", utc(at.naive_utc())))
}

fn utc(at: NaiveDateTime) -> String {
    at.format("%Y%m%dT%H%M%SZ").to_string()
}

// RFC 3339 in UTC, as Google's `timeMin` and `timeMax` take it
pub fn rfc3339(seconds: i64) -> Option<String> {
    Some(
        DateTime::<Utc>::from_timestamp(seconds, 0)?
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string(),
    )
}

// a TEXT value, its backslashes, separators and line breaks escaped
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());

    for char in text.chars() {
        match char {
            '\\' => escaped.push_str("\\\\"),
            ';' => escaped.push_str("\\;"),
            ',' => escaped.push_str("\\,"),
            '\n' => escaped.push_str("\\n"),
            '\r' => {}
            char if char.is_control() => {}
            char => escaped.push(char),
        }
    }

    escaped
}

// one content line, folded into lines of at most `LINE` octets, never inside a character
fn line(text: &mut String, content: &str) {
    let mut room = LINE;
    let mut used = 0;

    for char in content.chars() {
        if used + char.len_utf8() > room {
            text.push_str("\r\n ");
            used = 0;
            // the space that starts a folded line is one of its octets
            room = LINE - 1;
        }

        text.push(char);
        used += char.len_utf8();
    }

    text.push_str("\r\n");
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;
    use crate::sources::calendar;

    fn events(json: &str) -> Vec<Value> {
        serde_json::from_str(json).unwrap_or_default()
    }

    #[test]
    fn timed_events_are_written_in_utc_and_all_day_ones_by_date() {
        let text = calendar(&events(
            r#"[
                {"id": "a_20261008T080000Z", "summary": "Standup, daily; 15'",
                 "location": "Room 1\nfloor 2",
                 "start": {"dateTime": "2026-10-08T10:00:00+02:00", "timeZone": "Europe/Berlin"},
                 "end": {"dateTime": "2026-10-08T10:15:00+02:00"}},
                {"id": "b", "summary": "Trip",
                 "start": {"date": "2026-10-09"}, "end": {"date": "2026-10-12"}}
            ]"#,
        ));

        assert_eq!(
            text,
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Kanade//Google Calendar//EN\r\n\
             BEGIN:VEVENT\r\nUID:a_20261008T080000Z@google.com\r\n\
             DTSTART:20261008T080000Z\r\nDTEND:20261008T081500Z\r\n\
             SUMMARY:Standup\\, daily\\; 15'\r\nLOCATION:Room 1\\nfloor 2\r\nEND:VEVENT\r\n\
             BEGIN:VEVENT\r\nUID:b@google.com\r\n\
             DTSTART;VALUE=DATE:20261009\r\nDTEND;VALUE=DATE:20261012\r\n\
             SUMMARY:Trip\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
        );
    }

    #[test]
    fn cancelled_declined_and_broken_events_are_left_out() {
        let text = calendar(&events(
            r#"[
                {"id": "gone", "status": "cancelled", "start": {"date": "2026-10-09"}},
                {"id": "no", "start": {"date": "2026-10-09"},
                 "attendees": [{"email": "a@b", "self": true, "responseStatus": "declined"},
                               {"email": "c@d", "responseStatus": "accepted"}]},
                {"id": "nostart"},
                {"id": "badtime", "start": {"dateTime": "tomorrow"}},
                {"id": "yes", "start": {"date": "2026-10-09"},
                 "attendees": [{"email": "a@b", "self": true, "responseStatus": "accepted"},
                               {"email": "c@d", "responseStatus": "declined"}]}
            ]"#,
        ));

        assert_eq!(text.matches("BEGIN:VEVENT").count(), 1);
        assert!(text.contains("UID:yes@google.com"));
    }

    #[test]
    fn long_lines_fold_between_characters() {
        let mut text = String::new();
        let content = format!("SUMMARY:{}", "é".repeat(80));

        line(&mut text, &content);

        assert!(text.split("\r\n").all(|line| line.len() <= LINE));
        assert_eq!(text.replace("\r\n ", ""), format!("{content}\r\n"));
    }

    #[test]
    fn the_calendar_module_reads_what_is_written() {
        let text = calendar(&events(
            r#"[
                {"id": "a", "summary": "Standup",
                 "start": {"dateTime": "2026-10-08T10:00:00+02:00"},
                 "end": {"dateTime": "2026-10-08T10:15:00+02:00"}},
                {"id": "b", "summary": "Trip",
                 "start": {"date": "2026-10-09"}, "end": {"date": "2026-10-11"}}
            ]"#,
        ));
        let day = |day| NaiveDate::from_ymd_opt(2026, 10, day).unwrap_or_default();

        let Ok(calendars) = calendar::parse(&text) else {
            panic!("not read: {text}");
        };
        // in UTC, so the times read as written
        let found = calendar::occurrences(&calendars, day(8), day(12), |seconds| {
            DateTime::<Utc>::from_timestamp(seconds, 0).map(|at| at.naive_utc())
        });

        let read: Vec<(&str, String, bool)> = found
            .occurrences
            .iter()
            .map(|event| (event.title.as_str(), event.start.to_string(), event.all_day))
            .collect();
        assert_eq!(
            read,
            [
                ("Standup", String::from("2026-10-08 08:00:00"), false),
                ("Trip", String::from("2026-10-09 00:00:00"), true),
            ]
        );
        assert!(found.occurrences[1].on(day(10)));
        assert!(!found.occurrences[1].on(day(11)));
    }

    #[test]
    fn the_window_is_written_as_google_takes_it() {
        assert_eq!(
            rfc3339(1_791_446_400).as_deref(),
            Some("2026-10-08T08:00:00Z")
        );
    }
}
