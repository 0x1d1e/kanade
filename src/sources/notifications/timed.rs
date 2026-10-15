//! Timed Do Not Disturb (ADR 0075): `kanade notifications dnd on 15m` or `on until 08:00` turns it
//! on and off again by itself.
//!
//! What ends it is a moment on the wall clock, kept here and waited for on a `CLOCK_REALTIME` timer
//! (`clock::WallTimer`, as the calendar reminders do, ADR 0043): nothing polls, a suspend that
//! passes the moment ends it on resume, and a clock set makes the thread look again.
//!
//! The moment is cleared by every other setting of DND (`notifications::set_dnd`), under the same
//! lock that the end takes, so it turns off only the DND it turned on: one the user turned off, or
//! off and on again, or that Focus set, meanwhile is never turned off by an old timer.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use chrono::{Days, NaiveDateTime, NaiveTime};
use kanade_runtime::MutexExt;
use kanade_runtime::service::Service;

use crate::clock::{self, WallTimer};
use crate::island::service::IslandService;
use crate::sources::timer;
use crate::{config, supervise};

// when the timed DND ends, in seconds since the epoch, none while DND is not timed
static ENDS: Mutex<Option<i64>> = Mutex::new(None);

// the timer and its thread, started by the first timed DND, none if there is no timer to be had
static TIMER: OnceLock<Option<Arc<WallTimer>>> = OnceLock::new();

// how long a timed DND lasts
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    For(Duration),

    // until the next time the clock reads this
    Until(NaiveTime),
}

// what `kanade notifications dnd` asks beyond `on`, `off` and `toggle`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    On(Span),
    Status,
}

impl Request {
    // the words after `dnd`: `on <duration>`, `on until <HH:MM>` or `status`
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        match arguments {
            ["on", "until", time] => time_of_day(time).map(|at| Request::On(Span::Until(at))),
            ["on", length] => timer::duration(length).map(|length| Request::On(Span::For(length))),
            ["status"] => Some(Request::Status),
            _ => None,
        }
    }
}

// like 8:00 or 23:30, a time of day on the 24 hour clock
fn time_of_day(text: &str) -> Option<NaiveTime> {
    let (hours, minutes) = text.split_once(':')?;
    let digits = |part: &str, most: usize| {
        (1..=most).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_digit())
    };

    if !digits(hours, 2) || !digits(minutes, 2) || minutes.len() != 2 {
        return None;
    }

    NaiveTime::from_hms_opt(hours.parse().ok()?, minutes.parse().ok()?, 0)
}

/*
 * the seconds from `local`, the local time now, to the end of `span`. A time of day that has
 * already passed today is tomorrow's; across a change of daylight saving time it is an hour off
 */
fn seconds(span: Span, local: NaiveDateTime) -> i64 {
    match span {
        Span::For(length) => i64::try_from(length.as_secs()).unwrap_or(i64::MAX),
        Span::Until(at) => {
            let mut end = local.date().and_time(at);

            if end <= local {
                end = end + Days::new(1);
            }

            (end - local).num_seconds()
        }
    }
}

fn held() -> MutexGuard<'static, Option<i64>> {
    ENDS.locked()
}

/*
 * clears the end of a timed DND, for a setting of DND that is not the timer's own; the guard is
 * held through that setting, so an end that is due waits and then finds nothing to end
 */
pub fn cancel() -> MutexGuard<'static, Option<i64>> {
    let mut ends = held();
    *ends = None;
    ends
}

// the moment a timed DND ends, none when DND is not timed
pub fn ends() -> Option<i64> {
    *held()
}

// the time of day `ends` reads on the clock, as the user set it to show
pub fn label(ends: i64) -> Option<String> {
    clock::local_time(ends).map(|at| clock::time(at.time(), config::get().clock))
}

// "off", "on" or "on until 08:00"
pub fn status() -> String {
    match ends().and_then(label) {
        Some(at) => format!("on until {at}"),
        None if IslandService::read().dnd() => String::from("on"),
        None => String::from("off"),
    }
}

pub fn request(request: Request, now: Instant) -> Result<String, String> {
    match request {
        Request::Status => Ok(status()),
        Request::On(span) => {
            let wall = clock::unix_seconds();
            let local = clock::local_time(wall).ok_or("cannot read the local time")?;
            let end = wall.saturating_add(seconds(span, local));
            let timer = timer()?;

            let mut ends = cancel();

            timer
                .set(end)
                .map_err(|error| format!("cannot arm the timer: {error}"))?;
            super::apply(true, now);
            *ends = Some(end);
            drop(ends);

            Ok(status())
        }
    }
}

fn timer() -> Result<Arc<WallTimer>, String> {
    TIMER
        .get_or_init(|| match WallTimer::new() {
            Ok(timer) => {
                let timer = Arc::new(timer);
                let waiting = timer.clone();

                supervise::spawn("dnd-timer", move || end(&waiting));
                Some(timer)
            }
            Err(error) => {
                eprintln!("kanade: notifications: no timer for a timed DND ({error})");
                None
            }
        })
        .clone()
        .ok_or_else(|| String::from("no timer to end it with"))
}

// asleep until the end, then turns DND off if this timer's is still the one on
fn end(timer: &WallTimer) {
    loop {
        if let Err(error) = timer.wait() {
            let why = format!("lost the timed DND's timer ({error})");

            eprintln!("kanade: notifications: {why}");
            supervise::stopped("dnd-timer", why);
            return;
        }

        let mut ends = held();

        match *ends {
            Some(end) if clock::unix_seconds() >= end => {
                *ends = None;
                super::apply(false, Instant::now());
            }
            // woken early by the clock being set
            Some(end) => {
                if let Err(error) = timer.set(end) {
                    eprintln!("kanade: notifications: cannot arm the timer ({error})");
                }
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    fn at(hours: u32, minutes: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(hours, minutes, 0).unwrap()
    }

    fn local(hours: u32, minutes: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 3, 9)
            .unwrap()
            .and_time(at(hours, minutes))
    }

    // the words are the user's, the end is a moment: a time of day is the next time it reads so
    #[test]
    fn a_time_of_day_is_the_next_one_and_a_bad_word_is_refused() {
        assert_eq!(
            Request::parse(&["on", "15m"]),
            Some(Request::On(Span::For(Duration::from_secs(900))))
        );
        assert_eq!(
            Request::parse(&["on", "until", "8:05"]),
            Some(Request::On(Span::Until(at(8, 5))))
        );
        assert_eq!(Request::parse(&["status"]), Some(Request::Status));

        for words in [
            &["on", "until", "24:00"][..],
            &["on", "until", "08:60"],
            &["on", "until", "0800"],
            &["on", "until", "8:5"],
            &["on", "until", "-1:00"],
            &["on", "0m"],
            &["on", "25h"],
            &["on", "until"],
            &["on", "15m", "extra"],
        ] {
            assert_eq!(Request::parse(words), None, "{words:?}");
        }

        // later today, and tomorrow when it has passed or is now
        assert_eq!(seconds(Span::Until(at(8, 0)), local(7, 30)), 30 * 60);
        assert_eq!(seconds(Span::Until(at(8, 0)), local(8, 0)), 24 * 3600);
        assert_eq!(seconds(Span::Until(at(8, 0)), local(9, 0)), 23 * 3600);
    }
}
