//! Calendar subscriptions (ADR 0062): public iCalendar feeds, `calendar.subscriptions`, mirrored
//! into read-only `.ics` files under `$XDG_CACHE_HOME/kanade/calendar-subscriptions`, which the
//! `calendar` Module reads and follows like a local directory. Nothing is fetched without a feed,
//! and a feed's address may hold a secret, so no address, host or path is logged, shown in `status`
//! or put in a Problem; a feed is named by its place in the list.
//!
//! One thread fetches the feeds one at a time, each every `calendar.subscription_interval`
//! (at least 30 minutes) and on `calendar-subscriptions sync`, with `If-None-Match` and
//! `If-Modified-Since` from the last answer, so an unchanged feed costs a 304. Only https is
//! spoken, redirects included (`https_only`, at most `REDIRECTS`), an answer is read up to `BODY`
//! under `PATIENCE`, and a file is replaced only by text that starts as a calendar does. A feed that
//! fails waits twice as long after each failure in a row (`Backoff`), up to the interval, keeps the
//! last good file, and shows its problem on the Calendar Surface and in `status`.

use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::{queue::Queue, service::Service};
use ring::digest;
use ureq::Agent;

use super::{body, calendar, mirror};
use crate::backoff::Backoff;
use crate::{clock, config};

// under the cache directory, the mirrored feeds
const DIRECTORY: &str = "kanade/calendar-subscriptions";

// the most a feed's answer may be
const BODY: u64 = 5 * 1024 * 1024;

// how long a whole request may take
const PATIENCE: Duration = Duration::from_secs(30);

// the redirects followed, each of them https too
const REDIRECTS: u32 = 3;

// how many feeds may be fetched at once; a slow or silent host never blocks another feed
const IN_FLIGHT: usize = 4;

// the wait after a first failure
const RETRY: Duration = Duration::from_secs(5 * 60);

// why a feed is not up to date; none of these holds the address
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    // the host could not be reached, answered too slowly or broke off
    Offline,

    // the address, or a redirect it gave, is not https
    Insecure,

    // the server answered with this status
    Status(u16),

    // the answer was over `BODY`
    TooLarge,

    // the answer is not text that starts as an iCalendar file
    NotCalendar,

    // the mirrored files could not be written
    Disk,
}

impl Problem {
    // in a few words, for the Calendar Surface
    pub fn brief(&self) -> &'static str {
        match self {
            Problem::Offline => "offline",
            Problem::Insecure => "not https",
            Problem::Status(_) => "refused",
            Problem::TooLarge => "too large",
            Problem::NotCalendar => "not a calendar",
            Problem::Disk => "cannot save",
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::Offline => f.write_str("cannot be reached"),
            Problem::Insecure => f.write_str("refused, as it or a redirect is not https"),
            Problem::Status(status) => write!(f, "answered {status}"),
            Problem::TooLarge => write!(f, "answered over {} MiB", BODY / 1024 / 1024),
            Problem::NotCalendar => f.write_str("did not answer an iCalendar file"),
            Problem::Disk => f.write_str("cannot be written to the cache"),
        }
    }
}

// how one feed stands
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Feed {
    pub problem: Option<Problem>,

    // when it was last checked and found up to date, seconds since the epoch
    pub synced: Option<i64>,

    // whether a good copy is mirrored, which the calendar shows whatever the problem
    pub kept: bool,
}

// the feeds as the Calendar Surface and `status` read them, in the order of the config
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subscriptions {
    pub feeds: Vec<Feed>,
}

impl Service for Subscriptions {
    fn new() -> Self {
        Subscriptions::default()
    }

    fn listen() {}
}

impl Subscriptions {
    // in a few words, for the Calendar Surface; none while every feed is fine
    pub fn brief(&self) -> Option<String> {
        let mut failing = self.feeds.iter().filter_map(|feed| feed.problem.as_ref());

        match (failing.next(), failing.next()) {
            (None, _) => None,
            (Some(problem), None) => Some(format!("Subscription {}", problem.brief())),
            (Some(_), Some(_)) => Some(format!(
                "{} subscriptions failing",
                self.feeds
                    .iter()
                    .filter(|feed| feed.problem.is_some())
                    .count()
            )),
        }
    }

    // lines for `status`, a feed by its place in the list, with when it was checked read by `local`
    fn status(&self, local: impl Fn(i64) -> Option<String>) -> Vec<String> {
        if self.feeds.is_empty() {
            return vec![String::from(
                "calendar-subscriptions: no feeds; set calendar.subscriptions",
            )];
        }

        let mut lines = vec![format!(
            "calendar-subscriptions: {} feeds",
            self.feeds.len()
        )];

        for (at, feed) in self.feeds.iter().enumerate() {
            let checked = feed.synced.and_then(&local);
            let copy = if feed.kept {
                "showing the copy kept"
            } else {
                "no copy yet"
            };

            let said = match (&feed.problem, checked) {
                (Some(problem), _) => format!("{problem}, {copy}, trying again"),
                (None, Some(at)) => format!("up to date, checked at {at}"),
                (None, None) => String::from("waiting for the first fetch"),
            };

            lines.push(format!("  feed {}: {said}", at + 1));
        }

        lines
    }
}

// what `kanade calendar-subscriptions` asks
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Sync,
    Status,
}

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        match arguments {
            ["sync"] => Some(Request::Sync),
            ["status"] => Some(Request::Status),
            _ => None,
        }
    }
}

enum Message {
    // every feed is fetched now, in its turn
    Sync,

    // the config was read again
    Reread,

    // a fetch finished for the entry with `id` and `generation`; the result is applied only if
    // the entry is still present and its generation matches, so a late answer from a superseded
    // fetch is dropped
    Fetched(u64, u64, Result<Fetched, Problem>),
}

// the fetch thread's queue, which outlives a restart of it
static QUEUE: LazyLock<Queue<Message>> = LazyLock::new(Queue::new);

// what `kanade calendar-subscriptions` gets, on the draw thread, so nothing here waits on a feed
pub fn request(request: Request) -> Result<String, String> {
    match request {
        Request::Sync => {
            if Subscriptions::read().feeds.is_empty() {
                return Err(String::from("no feeds; set calendar.subscriptions"));
            }

            QUEUE.send(Message::Sync);
            Ok(String::from("fetching"))
        }
        Request::Status => Ok(status().join("\n")),
    }
}

// the config was read again
pub fn reread() {
    QUEUE.send(Message::Reread);
}

// the feeds' lines for `status`
pub fn status() -> Vec<String> {
    Subscriptions::read().status(|seconds| {
        Some(
            clock::local_time(seconds)?
                .format("%Y-%m-%d %H:%M")
                .to_string(),
        )
    })
}

// where the mirrored feeds are kept, for the calendar to read
pub fn directory() -> Option<PathBuf> {
    config::base(
        "XDG_CACHE_HOME",
        ".cache",
        std::env::var("HOME").ok().as_deref(),
    )
    .map(|dir| dir.join(DIRECTORY))
}

// the HTTP client; `secure` false only in tests, which talk to a server on this machine
fn agent(secure: bool) -> Agent {
    Agent::config_builder()
        .https_only(secure)
        .max_redirects(REDIRECTS)
        .timeout_global(Some(PATIENCE))
        .http_status_as_error(false)
        .user_agent(concat!("Kanade/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

// the name of a feed's file: a hash, which names neither the address nor the host
fn file_name(url: &str) -> String {
    let hash = digest::digest(&digest::SHA256, url.as_bytes());
    let hex: String = hash.as_ref()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();

    format!("{hex}.ics")
}

/*
 * how far past `now` a feed's first fetch waits: a feed with a copy from a run before is spread
 * over the interval by a digest of its address, so a start or an interval change does not ask every
 * feed at once and the hosts do not see a burst. A feed never fetched waits for nothing
 */
fn jitter(url: &str, kept: bool, interval: Duration) -> Duration {
    if !kept {
        return Duration::ZERO;
    }

    let hash = digest::digest(&digest::SHA256, url.as_bytes());
    let mut bytes = [0u8; 8];

    bytes.copy_from_slice(&hash.as_ref()[..8]);

    let at = (u64::from_be_bytes(bytes) >> 11) as f64 / (1_u64 << 53) as f64;
    let spread = interval.mul_f64(at);

    spread.min(interval)
}

// what a feed said last, to ask again only for a change
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Validators {
    etag: Option<String>,
    modified: Option<String>,
}

enum Fetched {
    // a 304, or the same as before
    Unchanged,
    New(String, Validators),
}

// a validator the server gave, kept only if it is plain visible text of a sensible size
fn validator(response: &ureq::http::Response<ureq::Body>, name: &str) -> Option<String> {
    let value = response.headers().get(name)?.to_str().ok()?;

    (!value.is_empty() && value.len() <= 256).then(|| value.to_owned())
}

// the text starts as an iCalendar file does, after an optional byte order mark
fn calendar_text(text: &str) -> bool {
    let start = text.trim_start_matches('\u{feff}').trim_start();

    start
        .get(.."BEGIN:VCALENDAR".len())
        .is_some_and(|begin| begin.eq_ignore_ascii_case("BEGIN:VCALENDAR"))
}

fn unreached(error: ureq::Error) -> Problem {
    match error {
        ureq::Error::RequireHttpsOnly(_) => Problem::Insecure,
        ureq::Error::BodyExceedsLimit(_) => Problem::TooLarge,
        ureq::Error::Io(error) if error.kind() == std::io::ErrorKind::InvalidData => {
            Problem::NotCalendar
        }
        _ => Problem::Offline,
    }
}

// one request; `known` is sent only for a copy that is kept
fn fetch(agent: &Agent, url: &str, known: Option<&Validators>) -> Result<Fetched, Problem> {
    let mut request = agent.get(url).header("Accept", "text/calendar, */*;q=0.5");

    if let Some(known) = known {
        if let Some(etag) = &known.etag {
            request = request.header("If-None-Match", etag);
        }
        if let Some(modified) = &known.modified {
            request = request.header("If-Modified-Since", modified);
        }
    }

    let mut response = request.call().map_err(unreached)?;

    match response.status().as_u16() {
        304 if known.is_some() => return Ok(Fetched::Unchanged),
        200 => {}
        status => return Err(Problem::Status(status)),
    }

    let validators = Validators {
        etag: validator(&response, "etag"),
        modified: validator(&response, "last-modified"),
    };
    let text = body::text(response.body_mut(), BODY).map_err(unreached)?;

    if !calendar_text(&text) {
        return Err(Problem::NotCalendar);
    }

    Ok(Fetched::New(text, validators))
}

struct Entry {
    // unique id across this worker, stable across config rereads
    id: u64,

    // never printed
    url: String,
    name: String,

    // the last good copy, also what is mirrored
    text: Option<String>,
    validators: Validators,
    backoff: Backoff,
    due: Instant,
    state: Feed,

    // incremented each time a fetch starts, so a late result from a superseded fetch is dropped
    generation: u64,
    // whether a fetch for this feed is in flight, so at most one fetch per feed at a time
    fetching: bool,
}

struct Worker {
    agent: Agent,
    dir: Option<PathBuf>,
    interval: Duration,
    entries: Vec<Entry>,
    next_id: u64,

    // how many fetches are in flight, capped at IN_FLIGHT
    in_flight: usize,

    // whether the files on disk may differ from the entries, which is mirrored
    stale: bool,
}

impl Worker {
    fn new(agent: Agent, dir: Option<PathBuf>) -> Self {
        Worker {
            agent,
            dir,
            interval: Duration::from_secs(u64::from(config::get().subscription_interval) * 60),
            entries: Vec::new(),
            next_id: 1,
            in_flight: 0,
            stale: true,
        }
    }

    // takes the feeds and the interval of the config; a feed already known keeps what it knows
    fn reread(&mut self, now: Instant) {
        let config = config::get();
        let interval = Duration::from_secs(u64::from(config.subscription_interval) * 60);
        let mut old = std::mem::take(&mut self.entries);
        let same = interval == self.interval;

        self.interval = interval;
        self.stale |= old.len() != config.subscriptions.len();

        for url in &config.subscriptions {
            let known = old.iter().position(|entry| entry.url == *url);

            let mut entry = match known {
                Some(at) => old.swap_remove(at),
                None => {
                    self.stale = true;
                    self.entry(url, now)
                }
            };

            if !same {
                entry.backoff = Backoff::new(RETRY, interval);
                entry.due = now + jitter(url, entry.text.is_some(), interval);
            }

            self.entries.push(entry);
        }

        self.stale |= !old.is_empty();
    }

    // a feed new to this thread, with the copy a run before kept
    fn entry(&mut self, url: &str, now: Instant) -> Entry {
        let name = file_name(url);
        let text = self
            .dir
            .as_ref()
            .and_then(|dir| fs::read_to_string(dir.join(&name)).ok())
            .filter(|text| calendar_text(text));

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        Entry {
            id,
            url: url.to_owned(),
            name,
            state: Feed {
                kept: text.is_some(),
                ..Feed::default()
            },
            validators: Validators::default(),
            backoff: Backoff::new(RETRY, self.interval),
            due: now + jitter(url, text.is_some(), self.interval),
            text,
            generation: 0,
            fetching: false,
        }
    }

    // submits a fetch for every due entry that is not already fetching, up to IN_FLIGHT in flight;
    // each fetch runs on its own thread and wakes this worker through the queue when done
    fn submit_due(&mut self, now: Instant) {
        for entry in &mut self.entries {
            if self.in_flight >= IN_FLIGHT {
                break;
            }
            if entry.fetching || entry.due > now {
                continue;
            }

            // a 304 is only meaningful against a copy; clone so the fetch thread is 'static
            let known = entry.text.is_some().then(|| entry.validators.clone());
            let url = entry.url.clone();
            let agent = self.agent.clone();
            let generation = entry.generation.wrapping_add(1);
            let id = entry.id;
            entry.generation = generation;
            entry.fetching = true;
            self.in_flight += 1;

            thread::spawn(move || {
                let result = fetch(&agent, &url, known.as_ref());
                QUEUE.send(Message::Fetched(id, generation, result));
            });
        }
    }

    // applies a finished fetch's result to its entry, dropping it if the generation no longer
    // matches (the entry was re-fetched or removed in the meantime)
    fn apply_fetched(&mut self, id: u64, generation: u64, result: Result<Fetched, Problem>) {
        self.in_flight = self.in_flight.saturating_sub(1);
        let now = Instant::now();
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) else {
            return;
        };
        if entry.generation != generation {
            return;
        }
        entry.fetching = false;

        match result {
            Ok(fetched) => {
                if let Fetched::New(text, validators) = fetched {
                    self.stale |= entry.text.as_ref() != Some(&text);
                    entry.text = Some(text);
                    entry.validators = validators;
                }

                entry.backoff.reset();
                entry.due = now + self.interval;
                entry.state = Feed {
                    problem: None,
                    synced: Some(clock::unix_seconds()),
                    kept: entry.text.is_some(),
                };
            }
            Err(problem) => {
                entry.backoff.failed();
                entry.due = now + entry.backoff.wait();
                entry.state.problem = Some(problem);
            }
        }

        if self.stale {
            self.mirror();
        }
        self.publish();
    }

    // writes what changed, and deletes the files of feeds gone
    fn mirror(&mut self) {
        if !self.stale {
            return;
        }
        let Some(dir) = self.dir.clone() else {
            return;
        };

        // nothing to make or delete without a feed, nor a directory
        if self.entries.is_empty() && !dir.is_dir() {
            self.stale = false;
            return;
        }

        let files: Vec<(String, String)> = self
            .entries
            .iter()
            .filter_map(|entry| Some((entry.name.clone(), entry.text.clone()?)))
            .collect();

        match mirror::sync(&dir, &files) {
            Ok(created) => {
                self.stale = false;

                for entry in &mut self.entries {
                    if entry.state.problem == Some(Problem::Disk) {
                        entry.state.problem = None;
                    }
                }

                // the calendar watches the directory only once it is there
                if created {
                    calendar::reread();
                }
            }
            // the copies stay in memory, and are written again at the next wake
            Err(error) => {
                eprintln!(
                    "kanade: calendar-subscriptions: cannot write the cache: {}",
                    error.kind()
                );

                for entry in &mut self.entries {
                    entry.state.problem = Some(Problem::Disk);
                }
            }
        }
    }

    // written only on a change, as a write wakes every window
    fn publish(&self) {
        let feeds = Subscriptions {
            feeds: self
                .entries
                .iter()
                .map(|entry| entry.state.clone())
                .collect(),
        };

        if *Subscriptions::read() != feeds {
            *Subscriptions::write() = feeds;
        }
    }

    // until the queue closes
    fn run(&mut self, queue: &Receiver<Message>) {
        self.reread(Instant::now());

        loop {
            // ask every feed that is due, at most IN_FLIGHT at a time; each fetch runs on its
            // own thread and wakes this worker through the queue when done
            self.submit_due(Instant::now());

            if self.stale {
                self.mirror();
            }
            self.publish();

            // how long until the next feed that is not already being fetched is due
            let wait = self
                .entries
                .iter()
                .filter(|entry| !entry.fetching)
                .map(|entry| entry.due)
                .min()
                .map(|due| due.saturating_duration_since(Instant::now()));

            let message = match wait {
                Some(wait) => match queue.recv_timeout(wait) {
                    Ok(message) => Some(message),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match queue.recv() {
                    Ok(message) => Some(message),
                    Err(_) => return,
                },
            };

            match message {
                Some(Message::Sync) => {
                    let now = Instant::now();

                    for entry in &mut self.entries {
                        entry.due = now;
                    }
                }
                Some(Message::Reread) => {
                    self.reread(Instant::now());
                    self.mirror();
                    self.publish();
                }
                Some(Message::Fetched(id, generation, result)) => {
                    self.apply_fetched(id, generation, result);
                }
                None => {}
            }
        }
    }
}

// fetches the feeds on their own thread for as long as Kanade runs
pub fn follow() {
    let queue = QUEUE.receiver();

    Worker::new(agent(true), directory()).run(&queue);
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    const FEED: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n";

    // answers each connection with the next of `answers`, whole, and tells what it was asked
    fn serve(answers: Vec<Vec<u8>>) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/feed.ics", listener.local_addr().unwrap());

        let asked = thread::spawn(move || {
            let mut asked = Vec::new();

            for answer in answers {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = vec![0; 4096];
                let read = stream.read(&mut request).unwrap();

                asked.push(String::from_utf8_lossy(&request[..read]).into_owned());
                let _ = stream.write_all(&answer);
            }

            asked
        });

        (url, asked)
    }

    fn answer(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut answer = format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        answer.extend_from_slice(body);

        answer
    }

    #[test]
    fn a_feed_is_asked_again_only_for_a_change() {
        let (url, asked) = serve(vec![
            answer(
                "200 OK",
                "ETag: \"v1\"\r\nLast-Modified: Tue, 06 Oct 2026 10:00:00 GMT\r\n",
                FEED.as_bytes(),
            ),
            answer("304 Not Modified", "", b""),
        ]);
        let agent = agent(false);

        let Ok(Fetched::New(text, validators)) = fetch(&agent, &url, None) else {
            panic!("the first answer is the feed");
        };
        assert_eq!(text, FEED);
        assert_eq!(validators.etag.as_deref(), Some("\"v1\""));

        assert!(matches!(
            fetch(&agent, &url, Some(&validators)),
            Ok(Fetched::Unchanged)
        ));

        let asked: Vec<String> = asked
            .join()
            .unwrap()
            .iter()
            .map(|ask| ask.to_lowercase())
            .collect();
        assert!(!asked[0].contains("if-none-match"));
        assert!(asked[1].contains("if-none-match: \"v1\""));
        assert!(asked[1].contains("if-modified-since: tue, 06 oct 2026 10:00:00 gmt"));
        assert!(asked[1].contains(concat!("user-agent: kanade/", env!("CARGO_PKG_VERSION"))));
    }

    #[test]
    fn an_answer_gzip_expands_past_the_limit_is_too_big() {
        use std::io::Write;

        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gzip.write_all(&vec![b'x'; BODY as usize * 2]).unwrap();
        let bomb = gzip.finish().unwrap();
        assert!((bomb.len() as u64) < BODY);

        let (url, _) = serve(vec![answer("200 OK", "Content-Encoding: gzip\r\n", &bomb)]);

        assert_eq!(
            fetch(&agent(false), &url, None).err(),
            Some(Problem::TooLarge)
        );
    }

    #[test]
    fn an_answer_that_is_not_a_calendar_or_too_big_or_refused_is_a_problem() {
        let big = vec![b'x'; BODY as usize + 1];
        let (url, _) = serve(vec![
            answer("200 OK", "", b"<html>sign in</html>"),
            answer("200 OK", "", &big),
            answer("404 Not Found", "", b"gone"),
            answer("200 OK", "", &[0xff, 0xfe, 0x00]),
        ]);
        let agent = agent(false);
        let problem = |result: Result<Fetched, Problem>| result.err();

        assert_eq!(
            problem(fetch(&agent, &url, None)),
            Some(Problem::NotCalendar)
        );
        assert_eq!(problem(fetch(&agent, &url, None)), Some(Problem::TooLarge));
        assert_eq!(
            problem(fetch(&agent, &url, None)),
            Some(Problem::Status(404))
        );
        assert_eq!(
            problem(fetch(&agent, &url, None)),
            Some(Problem::NotCalendar)
        );
    }

    // http is never spoken, which also covers a redirect to it, as the check runs on every request
    #[test]
    fn the_real_agent_speaks_only_https() {
        assert_eq!(
            fetch(&agent(true), "http://127.0.0.1:9/feed.ics", None).err(),
            Some(Problem::Insecure)
        );
    }

    #[test]
    fn a_problem_names_no_address() {
        for problem in [
            Problem::Offline,
            Problem::Insecure,
            Problem::Status(500),
            Problem::TooLarge,
            Problem::NotCalendar,
            Problem::Disk,
        ] {
            let said = format!("{problem} {}", problem.brief());
            assert!(!said.contains("://") && !said.contains('/'), "{said}");
        }
    }

    #[test]
    fn a_file_is_named_by_a_hash_of_the_address() {
        let name = file_name("https://example.org/secret-token/feed.ics");

        assert_eq!(name.len(), 16 + ".ics".len());
        assert!(!name.contains("secret") && !name.contains("example"));
        assert_ne!(
            name,
            file_name("https://example.org/secret-token/other.ics")
        );
    }

    // a start asks each feed with a copy at its own moment across the interval, not all at once;
    // one never fetched is asked at once, so a new subscription shows soon
    #[test]
    fn a_feed_known_from_before_is_asked_at_its_own_moment() {
        let interval = Duration::from_secs(3600);
        let urls = [
            "https://a.example.org/feed.ics",
            "https://b.example.org/feed.ics",
            "https://c.example.org/feed.ics",
        ];

        let spread: Vec<Duration> = urls.iter().map(|url| jitter(url, true, interval)).collect();

        assert!(spread.iter().all(|spread| *spread <= interval));
        assert!(spread.iter().any(|spread| *spread > Duration::ZERO));
        assert_ne!(spread[0], spread[1]);

        // the same feed waits at the same moment each start, so a restart does not reshuffle them
        assert_eq!(jitter(urls[0], true, interval), spread[0]);

        // never fetched: at once, whatever the address
        assert_eq!(jitter(urls[0], false, interval), Duration::ZERO);
    }

    // a good answer is mirrored under a hashed name; a later failure keeps that copy on disk, and
    // a restart offline still finds it
    #[test]
    fn a_failing_feed_keeps_its_last_good_copy() {
        let dir = std::env::temp_dir().join(format!("kanade-subscriptions-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (url, _) = serve(vec![
            answer("200 OK", "", FEED.as_bytes()),
            answer("503 Service Unavailable", "", b"later"),
        ]);
        let now = Instant::now();
        let mut worker = Worker::new(agent(false), Some(dir.clone()));
        let mut entry = worker.entry(&url, now);
        let file = dir.join(&entry.name);
        let known = entry.text.is_some().then(|| entry.validators.clone());
        let generation = entry.generation.wrapping_add(1);
        let id = entry.id;
        entry.generation = generation;
        entry.fetching = true;
        worker.in_flight = 1;
        let result = fetch(&agent(false), &entry.url, known.as_ref());
        worker.entries.push(entry);
        worker.apply_fetched(id, generation, result);
        worker.mirror();
        assert_eq!(fs::read_to_string(&file).ok().as_deref(), Some(FEED));
        assert_eq!(worker.entries[0].state.problem, None);

        worker.entries[0].due = now;
        let known = worker.entries[0]
            .text
            .is_some()
            .then(|| worker.entries[0].validators.clone());
        let generation = worker.entries[0].generation.wrapping_add(1);
        let id = worker.entries[0].id;
        worker.entries[0].generation = generation;
        worker.entries[0].fetching = true;
        worker.in_flight = 1;
        let result = fetch(&agent(false), &worker.entries[0].url, known.as_ref());
        worker.apply_fetched(id, generation, result);
        worker.mirror();
        assert_eq!(worker.entries[0].state.problem, Some(Problem::Status(503)));
        assert!(worker.entries[0].state.kept);
        assert!(worker.entries[0].due > now);
        assert_eq!(fs::read_to_string(&file).ok().as_deref(), Some(FEED));

        let again = worker.entry(&url, now);
        assert_eq!(again.text.as_deref(), Some(FEED));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_fetch_results_preserve_feed_order_and_survive_feed_removal() {
        let now = Instant::now();
        let mut worker = Worker::new(agent(false), None);
        let mut entry1 = worker.entry("https://a.example.org/feed.ics", now);
        let mut entry2 = worker.entry("https://b.example.org/feed.ics", now);

        let id1 = entry1.id;
        let gen1 = entry1.generation.wrapping_add(1);
        entry1.generation = gen1;
        entry1.fetching = true;

        let id2 = entry2.id;
        let gen2 = entry2.generation.wrapping_add(1);
        entry2.generation = gen2;
        entry2.fetching = true;

        worker.in_flight = 2;
        worker.entries.push(entry1);
        worker.entries.push(entry2);

        // Feed 2 finishes first (out of order) with an error
        worker.apply_fetched(id2, gen2, Err(Problem::Offline));
        assert_eq!(worker.in_flight, 1);
        assert_eq!(worker.entries[0].state.problem, None);
        assert_eq!(worker.entries[1].state.problem, Some(Problem::Offline));

        // Feed 1 finishes second with success
        worker.apply_fetched(
            id1,
            gen1,
            Ok(Fetched::New(FEED.to_owned(), Validators::default())),
        );
        assert_eq!(worker.in_flight, 0);
        assert_eq!(worker.entries[0].state.problem, None);
        assert!(worker.entries[0].state.synced.is_some());
        assert_eq!(worker.entries[1].state.problem, Some(Problem::Offline));

        // Feeds in Subscriptions service match config order, not completion order
        let published = Subscriptions::read().feeds.clone();
        assert_eq!(published.len(), 2);
        assert_eq!(published[0].problem, None);
        assert_eq!(published[1].problem, Some(Problem::Offline));

        // In-flight fetch for an entry removed from config decrements in_flight without leaking
        let mut entry3 = worker.entry("https://c.example.org/feed.ics", now);
        let id3 = entry3.id;
        let gen3 = entry3.generation.wrapping_add(1);
        entry3.generation = gen3;
        entry3.fetching = true;
        worker.in_flight = 1;
        // Entry 3 is dropped (as happens during config reread when a feed is removed)
        drop(entry3);
        worker.apply_fetched(id3, gen3, Err(Problem::Offline));
        assert_eq!(worker.in_flight, 0);
    }

    #[test]
    fn the_surface_says_one_problem_or_how_many() {
        let feed = |problem| Feed {
            problem,
            ..Feed::default()
        };
        let mut feeds = Subscriptions {
            feeds: vec![feed(None), feed(Some(Problem::Offline))],
        };
        assert_eq!(feeds.brief().as_deref(), Some("Subscription offline"));

        feeds.feeds.push(feed(Some(Problem::TooLarge)));
        assert_eq!(feeds.brief().as_deref(), Some("2 subscriptions failing"));

        assert_eq!(Subscriptions::default().brief(), None);
    }
}
