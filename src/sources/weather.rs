//! Weather (#152, ADR 0017, docs/design.md Weather): the current conditions and a short forecast
//! for `weather.location`, from Open-Meteo, which needs no account or key. Only the location's
//! coordinates leave the machine, and only while the Module is on and a location is set: with
//! none, the thread waits on its queue and nothing is asked or timed.
//!
//! The fetch runs on this source's thread: at start, again every `INTERVAL`, on `weather refresh`,
//! and when a config reload changes the location. A failed one tries again after `FIRST_RETRY`,
//! doubling up to `INTERVAL`, so an offline machine costs a few wakeups an hour. The last good
//! forecast stays shown after a failure, marked stale with the time it is from; it is dropped
//! only when the location changes, as it is then for somewhere else.
//!
//! The thread waits on a monotonic clock, which stands still while the machine sleeps, so after
//! a resume the forecast may be older than `INTERVAL`; opening the Weather Surface then fetches
//! at once (`opened`). Forecasts are kept in memory only: a restart fetches again.

use std::fmt;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, NaiveDate};
use kanade_runtime::service::Service;
use serde_json::Value;
use ureq::Agent;

use crate::clock;
use crate::config::{self, Location};

// how often the forecast is fetched again; Open-Meteo's models run hourly at most
const INTERVAL: Duration = Duration::from_secs(30 * 60);

// how long after a failure it tries again, doubling up to `INTERVAL`
const FIRST_RETRY: Duration = Duration::from_secs(60);

// a forecast older than this is stale even without a failure, as after a suspend
const STALE: i64 = 2 * INTERVAL.as_secs() as i64;

// the days of the forecast, today first
const DAYS: u8 = 5;

const API: &str = "https://api.open-meteo.com/v1/forecast";

// how long one request may take, the answer's reading included, and the answer's largest; one is
// about 1 KiB
const PATIENCE: Duration = Duration::from_secs(30);
const BODY: u64 = 64 * 1024;

// how the Weather Surface reads temperatures and speeds, `weather.units`
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Units {
    // °C and km/h, as Open-Meteo answers
    #[default]
    Metric,

    // °F and mph
    Imperial,
}

impl Units {
    // a temperature in °C, as these units read it, rounded
    pub fn temperature(self, celsius: f64) -> String {
        let degrees = match self {
            Units::Metric => celsius,
            Units::Imperial => celsius * 9.0 / 5.0 + 32.0,
        };

        // never "-0°"
        format!("{}°", degrees.round() + 0.0)
    }

    // a speed in km/h, as these units read it, rounded
    pub fn speed(self, kmh: f64) -> String {
        match self {
            Units::Metric => format!("{} km/h", kmh.round() + 0.0),
            Units::Imperial => format!("{} mph", (kmh / 1.609_344).round() + 0.0),
        }
    }
}

// the sky, from a WMO weather code as Open-Meteo gives it
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Condition {
    Clear,
    MainlyClear,
    PartlyCloudy,
    Overcast,
    Fog,
    Drizzle,
    FreezingDrizzle,
    Rain,
    FreezingRain,
    Snow,
    RainShowers,
    SnowShowers,
    Thunderstorm,

    // a code WMO 4677 as Open-Meteo uses it has no name for
    Unknown,
}

impl Condition {
    fn of(code: i64) -> Condition {
        match code {
            0 => Condition::Clear,
            1 => Condition::MainlyClear,
            2 => Condition::PartlyCloudy,
            3 => Condition::Overcast,
            45 | 48 => Condition::Fog,
            51 | 53 | 55 => Condition::Drizzle,
            56 | 57 => Condition::FreezingDrizzle,
            61 | 63 | 65 => Condition::Rain,
            66 | 67 => Condition::FreezingRain,
            71 | 73 | 75 | 77 => Condition::Snow,
            80..=82 => Condition::RainShowers,
            85 | 86 => Condition::SnowShowers,
            95 | 96 | 99 => Condition::Thunderstorm,
            _ => Condition::Unknown,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Condition::Clear => "Clear",
            Condition::MainlyClear => "Mainly clear",
            Condition::PartlyCloudy => "Partly cloudy",
            Condition::Overcast => "Overcast",
            Condition::Fog => "Fog",
            Condition::Drizzle => "Drizzle",
            Condition::FreezingDrizzle => "Freezing drizzle",
            Condition::Rain => "Rain",
            Condition::FreezingRain => "Freezing rain",
            Condition::Snow => "Snow",
            Condition::RainShowers => "Rain showers",
            Condition::SnowShowers => "Snow showers",
            Condition::Thunderstorm => "Thunderstorm",
            Condition::Unknown => "Unknown",
        }
    }
}

// the weather now; temperatures in °C, speeds in km/h
#[derive(Debug, Clone, PartialEq)]
pub struct Current {
    pub temperature: f64,
    pub feels: Option<f64>,

    // relative, in percent
    pub humidity: Option<f64>,
    pub wind: Option<f64>,
    pub condition: Condition,

    // whether the sun is up there, for a clear night's moon
    pub day: bool,
}

// one day of the forecast, the date the location's own
#[derive(Debug, Clone, PartialEq)]
pub struct Day {
    pub date: NaiveDate,
    pub condition: Condition,
    pub high: f64,
    pub low: f64,

    // the chance of rain or snow, in percent, where the model gives one
    pub precipitation: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Forecast {
    // where it is for, so one for an old location is never shown for a new one
    pub location: Location,

    // when it was fetched, in seconds since the epoch
    pub at: i64,

    // the location's offset from UTC then, in seconds, which its days' dates are in
    pub offset: i64,
    pub current: Current,
    pub days: Vec<Day>,
}

impl Forecast {
    // the location's own date at `now`, which may not be the machine's
    pub fn today(&self, now: i64) -> Option<NaiveDate> {
        DateTime::from_timestamp(now.checked_add(self.offset)?, 0).map(|at| at.date_naive())
    }
}

// why the forecast could not be fetched
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    // Open-Meteo could not be reached, in ureq's words
    Offline(String),

    // Open-Meteo answered, but not with a forecast, in its words or ours
    Refused(String),
}

impl Problem {
    // in a few words, for the Weather Surface
    pub fn brief(&self) -> &'static str {
        match self {
            Problem::Offline(_) => "Offline",
            Problem::Refused(_) => "Weather unavailable",
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::Offline(why) => write!(f, "Open-Meteo cannot be reached: {why}"),
            Problem::Refused(why) => write!(f, "Open-Meteo gave no forecast: {why}"),
        }
    }
}

// the weather as the Weather Surface and `status` read it
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Weather {
    pub state: State,

    // the last good forecast for the location set, kept through failures
    pub forecast: Option<Forecast>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum State {
    // no `weather.location`, so nothing is fetched
    #[default]
    Unset,

    // the first fetch for the location, not answered yet
    Fetching,

    // the last fetch gave the forecast
    Fetched,

    // the last fetch failed; the forecast, if any, is from before
    Failed(Problem),
}

impl Service for Weather {
    fn new() -> Self {
        Weather::default()
    }

    fn listen() {}
}

impl Weather {
    pub fn problem(&self) -> Option<&Problem> {
        match &self.state {
            State::Failed(problem) => Some(problem),
            _ => None,
        }
    }

    // whether the forecast shown is not the latest: a fetch failed since, or it is old
    pub fn stale(&self, now: i64) -> bool {
        self.problem().is_some()
            || self
                .forecast
                .as_ref()
                .is_some_and(|forecast| now - forecast.at > STALE)
    }

    // a line for `status`, with when it was fetched read by `local`
    fn status(&self, local: impl Fn(i64) -> Option<String>) -> String {
        let at = self
            .forecast
            .as_ref()
            .and_then(|forecast| local(forecast.at));

        let said = match (&self.state, at) {
            (State::Unset, _) => String::from("no location; set weather.location"),
            (State::Fetching, _) => String::from("fetching"),
            (State::Fetched, Some(at)) => format!("fetched at {at}"),
            (State::Fetched, None) => String::from("fetched"),
            (State::Failed(problem), None) => problem.to_string(),
            (State::Failed(problem), Some(at)) => {
                format!("{problem}; showing the forecast fetched at {at}")
            }
        };

        format!("weather: {said}")
    }
}

// what `kanade weather` asks of the `weather` Module
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Refresh,
    Status,
}

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        match arguments {
            ["refresh"] => Some(Request::Refresh),
            ["status"] => Some(Request::Status),
            _ => None,
        }
    }
}

// what the fetch thread is told
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Message {
    // the config was read again, maybe with another location
    Reread,

    // asked to fetch now
    Refresh,

    // the Weather Surface opened, which wants a forecast that is not old
    Opened,

    // the wait ran out
    Due,
}

// the fetch thread's queue, which outlives a restart of it
static QUEUE: LazyLock<(Sender<Message>, Mutex<Receiver<Message>>)> = LazyLock::new(|| {
    let (send, receive) = mpsc::channel();

    (send, Mutex::new(receive))
});

fn send(message: Message) {
    // the receiver lives in a static, so it is never gone
    let _ = QUEUE.0.send(message);
}

// what `kanade weather` gets, on the draw thread, so nothing here waits on Open-Meteo
pub fn request(request: Request) -> Result<String, String> {
    match request {
        Request::Refresh => match Weather::read().state {
            State::Unset => Err(String::from(
                "no location; set weather.location to [latitude, longitude]",
            )),
            _ => {
                send(Message::Refresh);
                Ok(String::from("refreshing"))
            }
        },
        Request::Status => Ok(status()),
    }
}

// the config was read again; a new location is fetched at once
pub fn reread() {
    send(Message::Reread);
}

// the Weather Surface was asked to open; an old forecast is fetched again
pub fn opened() {
    send(Message::Opened);
}

// the weather's line for `status`
pub fn status() -> String {
    Weather::read().status(|seconds| {
        Some(
            clock::local_time(seconds)?
                .format("%Y-%m-%d %H:%M")
                .to_string(),
        )
    })
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

// fetches the forecast on its own thread for as long as Kanade runs
pub fn follow() {
    let queue = QUEUE.1.lock().unwrap_or_else(PoisonError::into_inner);
    let agent: Agent = Agent::config_builder()
        .timeout_global(Some(PATIENCE))
        .http_status_as_error(false)
        .build()
        .into();

    Worker::new(OpenMeteo { agent }).run(&queue);
}

// what the fetch thread works with outside itself, so a test stands in for Open-Meteo and time
trait Outside {
    fn location(&self) -> Option<Location>;
    fn fetch(&self, location: Location) -> Result<Forecast, Problem>;
    fn now(&self) -> i64;

    // the weather as the Weather Surface and `status` read it
    fn publish(&self, weather: &Weather);
}

struct OpenMeteo {
    agent: Agent,
}

impl Outside for OpenMeteo {
    fn location(&self) -> Option<Location> {
        config::get().location
    }

    fn fetch(&self, location: Location) -> Result<Forecast, Problem> {
        fetch(&self.agent, location)
    }

    fn now(&self) -> i64 {
        now()
    }

    // written only on a change, as a write wakes every window
    fn publish(&self, weather: &Weather) {
        if *Weather::read() != *weather {
            *Weather::write() = weather.clone();
        }
    }
}

struct Worker<O> {
    outside: O,
    location: Option<Location>,
    weather: Weather,

    // failures in a row, which set how long until the next try
    failures: u32,

    // when the last fetch was tried, in seconds since the epoch
    tried: Option<i64>,

    /*
     * when the next fetch is due; a message that fetches nothing leaves it, so frequent config
     * reloads or openings never put it off. None without a location
     */
    next: Option<Instant>,
}

impl<O: Outside> Worker<O> {
    fn new(outside: O) -> Self {
        let location = outside.location();
        let weather = Weather {
            state: if location.is_some() {
                State::Fetching
            } else {
                State::Unset
            },
            forecast: None,
        };

        Worker {
            outside,
            location,
            weather,
            failures: 0,
            tried: None,
            next: None,
        }
    }

    // until the queue closes
    fn run(&mut self, queue: &Receiver<Message>) {
        self.outside.publish(&self.weather);
        let mut due = self.location.is_some();

        loop {
            while due {
                self.fetch();

                // what was asked while it fetched is answered by it, but for a new location
                due = false;
                while let Ok(message) = queue.try_recv() {
                    if message == Message::Reread {
                        due |= self.handle(message);
                    }
                }
            }

            let message = match self.next {
                Some(next) => {
                    match queue.recv_timeout(next.saturating_duration_since(Instant::now())) {
                        Ok(message) => message,
                        Err(RecvTimeoutError::Timeout) => Message::Due,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                None => match queue.recv() {
                    Ok(message) => message,
                    Err(_) => return,
                },
            };

            due = self.handle(message);
        }
    }

    // how long until the next fetch; none without a location, which waits for a reread
    fn wait(&self) -> Option<Duration> {
        self.location?;

        Some(match self.failures {
            0 => INTERVAL,
            failures => FIRST_RETRY
                .saturating_mul(1 << (failures - 1).min(16))
                .min(INTERVAL),
        })
    }

    // whether to fetch now
    fn handle(&mut self, message: Message) -> bool {
        match message {
            Message::Reread => {
                let location = self.outside.location();
                if location == self.location {
                    return false;
                }

                // a forecast for somewhere else is not this one's, nor are its failures
                self.location = location;
                self.failures = 0;
                self.tried = None;
                self.next = None;
                self.weather = Weather {
                    state: if location.is_some() {
                        State::Fetching
                    } else {
                        State::Unset
                    },
                    forecast: None,
                };
                self.outside.publish(&self.weather);

                location.is_some()
            }
            Message::Refresh | Message::Due => self.location.is_some(),

            // only an old forecast, and not again right after a try, so opening it often asks little
            Message::Opened => {
                let now = self.outside.now();
                let old = self
                    .weather
                    .forecast
                    .as_ref()
                    .is_none_or(|forecast| now - forecast.at >= INTERVAL.as_secs() as i64);
                let recent = self
                    .tried
                    .is_some_and(|tried| now - tried < FIRST_RETRY.as_secs() as i64);

                self.location.is_some() && old && !recent
            }
        }
    }

    fn fetch(&mut self) {
        let Some(location) = self.location else {
            return;
        };

        self.tried = Some(self.outside.now());

        match self.outside.fetch(location) {
            Ok(forecast) => {
                self.failures = 0;
                self.weather = Weather {
                    state: State::Fetched,
                    forecast: Some(forecast),
                };
            }
            Err(problem) => {
                // said once, not at every retry while offline
                if self.weather.problem() != Some(&problem) {
                    eprintln!("kanade: weather: {problem}");
                }

                self.failures = self.failures.saturating_add(1);
                self.weather.state = State::Failed(problem);
            }
        }

        self.next = self.wait().map(|wait| Instant::now() + wait);
        self.outside.publish(&self.weather);
    }
}

// the forecast for `location` from Open-Meteo
fn fetch(agent: &Agent, location: Location) -> Result<Forecast, Problem> {
    let mut answer = agent
        .get(API)
        .query("latitude", format!("{:.4}", location.latitude))
        .query("longitude", format!("{:.4}", location.longitude))
        .query(
            "current",
            "temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,is_day,\
             wind_speed_10m",
        )
        .query(
            "daily",
            "weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max",
        )
        .query("timezone", "auto")
        .query("forecast_days", DAYS.to_string())
        .call()
        .map_err(unreached)?;

    let status = answer.status().as_u16();
    let body: Value = answer
        .body_mut()
        .with_config()
        .limit(BODY)
        .read_to_string()
        .map_err(unreached)
        .map(|text| serde_json::from_str(&text).unwrap_or(Value::Null))?;

    if status != 200 {
        return Err(Problem::Refused(
            body.get("reason")
                .and_then(Value::as_str)
                .map_or_else(|| format!("status {status}"), str::to_owned),
        ));
    }

    forecast(&body, location, now())
        .ok_or_else(|| Problem::Refused(String::from("an answer that is not a forecast")))
}

// why a request did not get an answer
fn unreached(error: ureq::Error) -> Problem {
    match error {
        ureq::Error::BodyExceedsLimit(limit) => {
            Problem::Refused(format!("an answer over {} KiB", limit / 1024))
        }
        error => Problem::Offline(error.to_string()),
    }
}

// Open-Meteo's answer as a forecast fetched at `at`; none when it lacks the weather now
fn forecast(body: &Value, location: Location, at: i64) -> Option<Forecast> {
    let current = body.get("current")?;
    let number = |value: &Value, key: &str| value.get(key).and_then(Value::as_f64);

    let current = Current {
        temperature: number(current, "temperature_2m")?,
        feels: number(current, "apparent_temperature"),
        humidity: number(current, "relative_humidity_2m"),
        wind: number(current, "wind_speed_10m"),
        condition: Condition::of(current.get("weather_code").and_then(Value::as_i64)?),
        day: current.get("is_day").and_then(Value::as_i64) != Some(0),
    };

    // a day the model has no temperatures for is left out
    let daily = body.get("daily");
    let column = |key: &str| {
        daily
            .and_then(|daily| daily.get(key))
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
    };
    let at_day = |key: &str, day: usize| column(key).get(day).and_then(Value::as_f64);

    let days = column("time")
        .iter()
        .enumerate()
        .filter_map(|(day, date)| {
            Some(Day {
                date: NaiveDate::parse_from_str(date.as_str()?, "%Y-%m-%d").ok()?,
                condition: Condition::of(column("weather_code").get(day)?.as_i64()?),
                high: at_day("temperature_2m_max", day)?,
                low: at_day("temperature_2m_min", day)?,
                precipitation: at_day("precipitation_probability_max", day),
            })
        })
        .collect();

    Some(Forecast {
        location,
        at,
        offset: body
            .get("utc_offset_seconds")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        current,
        days,
    })
}
