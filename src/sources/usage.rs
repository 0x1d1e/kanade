//! Plan usage on the Island (ADR 0145): Claude's plan usage as two rings and their
//! percentages, off unless turned on.
//!
//! Off, this file is never read: no thread, no key read, no fetch, nothing posted. On, the
//! key comes from `$XDG_STATE_HOME/kanade/usage-key`, written by `kanade usage key` from
//! stdin without echo and kept at 0600. A worker fetches Claude's usage endpoint with a
//! bearer token at most every five minutes, and at once on `kanade usage`. It parses the
//! five-hour and seven-day windows leniently, posts their rings to the focused output's
//! island, and tells once per window when one is at or over the limit. Failures back off;
//! one notice after repeated ones, then silence till `kanade usage` is asked again, never
//! a tight loop. The body is never kept: only the two numbers and when they were read, in
//! memory.

use std::fs::File;
use std::io::{self, BufRead, Read};
use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use kanade_runtime::{queue::Queue, service::Service};
use serde_json::Value;
use ureq::Agent;

use crate::atomic::{self, Mode};
use crate::backoff::Backoff;
use crate::clock;
use crate::config;
use crate::island::activity::{Activity, Detail, Id, Interrupt, Kind, Lifetime, Priority, Scope};
use crate::island::service::IslandService;
use crate::supervise;

// Claude's usage endpoint, GET with `Authorization: Bearer <key>`
const API: &str = "https://api.anthropic.com/v1/usage";

// at most one fetch in this span
const INTERVAL: Duration = Duration::from_secs(5 * 60);

// how long after a failure it tries again, doubling up to `LONGEST`, then quiet till `usage`
const FIRST_RETRY: Duration = Duration::from_secs(60);
const LONGEST: Duration = Duration::from_secs(30 * 60);

// repeated failures before one notice, and how long a notice shows
const NOTICE_AFTER: u32 = 3;
const SHOWN: Duration = Duration::from_secs(5);

// at or over this share of a window, one Transient for the window, until it drops below it
const LIMIT: f32 = 0.90;

const FILE: &str = "usage-key";

// how many bytes of an answer are read at most, before its shape is judged
const BODY: u64 = 64 * 1024;

// the two windows the plan is read in, and their utilization share of each
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Usage {
    // five hours, 0 to 1
    pub five_hour: Option<f32>,
    // seven days, 0 to 1
    pub seven_day: Option<f32>,
}

impl Service for Usage {
    fn new() -> Self {
        Usage::default()
    }

    fn listen() {}
}

impl From<Usage> for crate::island::activity::Usage {
    fn from(usage: Usage) -> Self {
        crate::island::activity::Usage {
            five_hour: usage.five_hour,
            seven_day: usage.seven_day,
        }
    }
}

// what `kanade usage` asks: fetch at once, or the setup the shell never sees
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Refresh,
    Key,
}

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        match arguments {
            [] => Some(Request::Refresh),
            ["key"] => Some(Request::Key),
            _ => None,
        }
    }
}

// what the fetch thread is told
enum Message {
    // `kanade usage`: fetch at once, whatever the clock says
    Refresh,

    // the wait ran out
    Due,
}

// the fetch thread's queue, which outlives a restart of it
static QUEUE: LazyLock<Queue<Message>> = LazyLock::new(Queue::new);

// `kanade usage` answers at once: the worker fetches at once when a key exists, and does
// nothing otherwise
pub fn request(request: Request) -> Result<String, String> {
    match request {
        Request::Refresh => {
            QUEUE.send(Message::Refresh);
            Ok(String::from("refreshing"))
        }
        // never through the socket: the key is read at a terminal and written to a file
        Request::Key => Err(String::from(
            "`usage key` reads at a terminal; run `kanade usage key`",
        )),
    }
}

// the key from the user's terminal to a file only they can read, nothing of it in argv,
// the socket, a log or `kanade status`
pub fn key() -> std::process::ExitCode {
    use std::io::{IsTerminal, Write};

    let stdin = io::stdin();

    let said = if stdin.is_terminal() {
        eprint!("Claude plan key (hidden): ");
        drop(io::stderr().flush());

        match hidden(|| read_secret(io::stdin().lock())) {
            Ok(key) => store_key(key.trim()),
            Err(why) => Err(why),
        }
    } else {
        match read_line(io::stdin().lock()) {
            Ok(line) => store_key(line.trim()),
            Err(why) => Err(why),
        }
    };

    match said {
        Ok(path) => match writeln!(io::stdout().lock(), "key stored in {}", path.display()) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(_) => std::process::ExitCode::FAILURE,
        },
        Err(why) => {
            eprintln!("kanade: {why}");
            std::process::ExitCode::FAILURE
        }
    }
}

// the key on `input`, a line, and the state file it is written to at 0600
fn store_key(key: &str) -> Result<PathBuf, String> {
    let path = key_path().ok_or("no state directory; set XDG_STATE_HOME or HOME")?;

    (!key.is_empty()).then_some(key).ok_or("the key is empty")?;

    atomic::write(&path, key.as_bytes(), Mode::Private)
        .map(|()| path)
        .map_err(|error| format!("cannot write the key: {error}"))
}

fn key_path() -> Option<PathBuf> {
    config::base(
        "XDG_STATE_HOME",
        ".local/state",
        std::env::var("HOME").ok().as_deref(),
    )
    .map(|dir| dir.join("kanade").join(FILE))
}

// the fetch thread runs for as long as the Module is on
pub fn start() {
    supervise::spawn("usage", follow);
}

fn follow() {
    let agent: Agent = Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();

    let mut worker = Worker::new(Real { agent });
    let queue = QUEUE.receiver();

    loop {
        let now = Instant::now();
        let due = worker.next.is_some_and(|next| next <= now);

        if due {
            worker.handle(Message::Due, now);
            continue;
        }

        let message = match worker.next {
            Some(next) => match queue.recv_timeout(next.saturating_duration_since(now)) {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout) => Message::Due,
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match queue.recv() {
                Ok(message) => message,
                Err(_) => return,
            },
        };

        worker.handle(message, Instant::now());
    }
}

// what the worker stands on, so a test stands in for the clock, the key, the endpoint and
// what the island shows
trait Outside {
    // the key from the state file, none while there is none
    fn key(&self) -> Option<String>;

    // the plan for `key`, or why not
    fn fetch(&self, key: &str) -> Result<Usage, Error>;

    // what the island shows now
    fn post(&self, activity: Activity);
}

struct Real {
    agent: Agent,
}

impl Outside for Real {
    fn key(&self) -> Option<String> {
        key_path().and_then(|path| {
            File::open(path).ok().and_then(|mut file| {
                let mut text = String::new();
                file.read_to_string(&mut text).ok()?;
                let key = text.trim();
                (!key.is_empty()).then_some(key.to_owned())
            })
        })
    }

    fn fetch(&self, key: &str) -> Result<Usage, Error> {
        let mut answer = self
            .agent
            .get(API)
            .header("Authorization", &format!("Bearer {key}"))
            .call()
            .map_err(|error| Error::Fetch(error.to_string()))?;

        if answer.status() != 200 {
            return Err(Error::Refused(answer.status().to_string()));
        }

        let text = crate::sources::body::text(answer.body_mut(), BODY)
            .map_err(|error| Error::Read(error.to_string()))?;
        read_usage(&text)
    }

    fn post(&self, activity: Activity) {
        IslandService::write().post(activity, Instant::now());
    }
}

struct Worker<O: Outside> {
    outside: O,

    // how long the next failed try waits, doubling up to `LONGEST`
    backoff: Backoff,

    // when the next fetch is due; None without a key, so nothing polls
    next: Option<Instant>,

    // the last good plan and when it was read, the only thing kept of an answer
    usage: Option<(Usage, Instant)>,

    // the windows at or over the limit that already had their one notice this reset
    posted: std::collections::BTreeSet<Window>,

    // one notice for repeated failures, not one per failure
    noticed: bool,
}

// one window the plan is read in
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Window {
    FiveHour,
    SevenDay,
}

impl Window {
    fn label(self) -> &'static str {
        match self {
            Window::FiveHour => "5 hours",
            Window::SevenDay => "7 days",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Window::FiveHour => "five_hour",
            Window::SevenDay => "seven_day",
        }
    }
}

impl<O: Outside> Worker<O> {
    fn new(outside: O) -> Self {
        Worker {
            outside,
            backoff: Backoff::new(FIRST_RETRY, LONGEST),
            next: None,
            usage: None,
            posted: std::collections::BTreeSet::new(),
            noticed: false,
        }
    }

    // until the queue closes; testable directly: a message and a time, the posts it makes
    fn handle(&mut self, message: Message, now: Instant) {
        let due = matches!(message, Message::Refresh) || self.next.is_some_and(|next| next <= now);

        if !due {
            return;
        }

        self.next = None;

        let Some(key) = self.outside.key() else {
            // nothing posts without a key; wait for a nudge or for the key to appear
            return;
        };

        match self.outside.fetch(&key) {
            Ok(usage) => {
                self.backoff.reset();
                self.noticed = false;

                let read_at = clock::unix_seconds();
                self.usage = Some((usage, Instant::now()));
                self.outside.post(Self::rings_activity(usage));
                self.notices(usage, read_at);

                self.next = Some(now + INTERVAL);
            }
            Err(_) => {
                self.backoff.failed();

                if self.backoff.failures() >= NOTICE_AFTER && !self.noticed {
                    // one notice, then quiet till the next `kanade usage`, never a loop
                    self.noticed = true;
                    self.outside.post(failure_activity());
                    self.next = None;
                    self.backoff.reset();
                } else {
                    self.next = Some(now + self.backoff.wait());
                }
            }
        }
    }

    // the plan's two rings as an Ongoing Activity on the focused output, replaced as the plan
    // is read again; nothing of the body is kept
    fn rings_activity(usage: Usage) -> Activity {
        Activity::new(
            Id::new(Kind::Usage, "plan"),
            Priority::Ongoing,
            Lifetime::Persistent,
            Scope::FocusedOutput,
            Interrupt::None,
        )
        .expect("a Persistent Activity does not auto-expand")
        .with_detail(Detail::Usage(usage.into()))
    }

    // one Transient per window at or over the limit, until that window drops below it again
    fn notices(&mut self, usage: Usage, _read_at: i64) {
        for window in [Window::FiveHour, Window::SevenDay] {
            let utilization = match window {
                Window::FiveHour => usage.five_hour,
                Window::SevenDay => usage.seven_day,
            };

            let over = utilization.is_some_and(|utilization| utilization >= LIMIT);

            if over {
                if self.posted.insert(window) {
                    // newly over: one notice for this window, not one per refresh
                    let share = (utilization.unwrap() * 100.0).round() as u32;

                    self.outside.post(
                        Activity::new(
                            Id::new(Kind::Usage, format!("limit-{}", window.key())),
                            Priority::Feedback,
                            Lifetime::Transient(SHOWN),
                            Scope::FocusedOutput,
                            Interrupt::None,
                        )
                        .expect("a Transient lasts longer than zero")
                        .with_detail(Detail::UsageNotice(format!(
                            "Claude plan at {share}% of {}",
                            window.label()
                        ))),
                    );
                }
            } else {
                // below it again: a later window may have its own one notice
                self.posted.remove(&window);
            }
        }
    }
}

// repeated failures have worn out the backoff: one notice, nothing more till `kanade usage`
fn failure_activity() -> Activity {
    Activity::new(
        Id::new(Kind::Usage, "offline"),
        Priority::Feedback,
        Lifetime::Transient(SHOWN),
        Scope::FocusedOutput,
        Interrupt::None,
    )
    .expect("a Transient lasts longer than zero")
    .with_detail(Detail::UsageNotice(String::from(
        "the plan could not be reached",
    )))
}

#[derive(Debug)]
pub(crate) enum Error {
    #[expect(dead_code)]
    Fetch(String),
    #[expect(dead_code)]
    Refused(String),
    #[expect(dead_code)]
    Read(String),
    Shape,
}

// the answer's shape: the window key, then `utilization` or `used_percent`, a number or a
// string of one; missing a window leaves it unknown, and nothing at all is unrecognised
pub(crate) fn read_usage(text: &str) -> Result<Usage, Error> {
    let trimmed = text.trim().trim_start();
    let limit = (BODY as usize).min(trimmed.len());

    if limit != trimmed.len() {
        return Err(Error::Read(String::from("the answer is too large")));
    }

    let Value::Object(object) =
        serde_json::from_str(&trimmed[0..limit]).map_err(|_| Error::Shape)?
    else {
        return Err(Error::Shape);
    };

    let five_hour = window_share(&object, "five_hour");
    let seven_day = window_share(&object, "seven_day");

    (five_hour.is_some() || seven_day.is_some())
        .then_some(Usage {
            five_hour,
            seven_day,
        })
        .ok_or(Error::Shape)
}

fn window_share(object: &serde_json::Map<String, Value>, window: &str) -> Option<f32> {
    let Value::Object(window) = object.get(window)? else {
        return None;
    };

    let raw = window
        .get("utilization")
        .or_else(|| window.get("used_percent"))?;

    match raw {
        Value::Number(number) => number.as_f64().map(|share| share_of(share as f32)),
        Value::String(text) => share(text).map(share_of),
        _ => None,
    }
}

// a fraction already (0 to 1), or a percent (over 1), clamped to the window
fn share_of(share: f32) -> f32 {
    let share = if share > 1.0 { share / 100.0 } else { share };

    share.clamp(0.0, 1.0)
}

fn share(text: &str) -> Option<f32> {
    let text = text.trim();
    let text = text.strip_suffix('%').unwrap_or(text);
    let number: f32 = text.parse().ok()?;

    Some(number)
}

fn read_secret(input: impl Read) -> Result<String, String> {
    let mut line = Vec::new();
    let mut input = input;
    let mut byte = [0u8; 1];

    loop {
        match input.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("cannot read stdin: {error}")),
        }

        match byte[0] {
            b'\n' | b'\r' => break,
            0x03 => return Err(String::from("cancelled")),
            0x04 if line.is_empty() => return Err(String::from("cancelled")),
            0x7f | 0x08 => drop(line.pop()),
            other if line.len() < 512 => line.push(other),
            _ => {}
        }
    }

    String::from_utf8(line).map_err(|_| String::from("the key is not plain ASCII"))
}

fn read_line(input: impl io::BufRead) -> Result<String, String> {
    let mut line = String::new();

    input
        .take(512)
        .read_line(&mut line)
        .map_err(|error| format!("cannot read stdin: {error}"))?;

    Ok(line.trim_end().to_owned())
}

// hidden terminal reading and a plain line, as `dictate key` does
fn hidden<T>(read: impl FnOnce() -> T) -> T {
    use std::process::{Command, Stdio};

    let saved = Command::new("stty")
        .arg("-g")
        .stdin(Stdio::inherit())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|saved| saved.trim().to_owned())
        .filter(|saved| !saved.is_empty());

    let hidden = saved.is_some()
        && Command::new("stty")
            .args(["-echo", "-isig", "-icanon", "min", "1", "time", "0"])
            .stdin(Stdio::inherit())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .is_ok_and(|output| output.status.success());

    if !hidden {
        eprintln!("(cannot hide the typing: stty failed)");
    }

    let read = read();

    if let Some(saved) = saved {
        drop(
            Command::new("stty")
                .arg(saved.as_str())
                .stdin(Stdio::inherit())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .output(),
        );
    }

    read
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn as_str(text: &str) -> &str {
        text
    }

    #[test]
    fn usage_reads_the_two_windows_leniently() {
        assert_eq!(
            read_usage(as_str(
                r#"{"five_hour":{"utilization":0.5},"seven_day":{"used_percent":0.3}}"#
            ))
            .unwrap(),
            Usage {
                five_hour: Some(0.5),
                seven_day: Some(0.3),
            }
        );

        // a string of a percent, a string over one, and one window missing
        assert_eq!(
            read_usage(as_str(
                r#"{"five_hour":{"used_percent":"50%"},"seven_day":{"utilization":"90"}}"#
            ))
            .unwrap(),
            Usage {
                five_hour: Some(0.5),
                seven_day: Some(0.9),
            }
        );

        assert_eq!(
            read_usage(as_str(r#"{"five_hour":{"utilization":0.2}}"#)).unwrap(),
            Usage {
                five_hour: Some(0.2),
                seven_day: None,
            }
        );

        // a bogus cell is missing, not fatal
        assert_eq!(
            read_usage(as_str(
                r#"{"five_hour":{"utilization":"nope"},"seven_day":{"used_percent":0.4}}"#
            ))
            .unwrap(),
            Usage {
                five_hour: None,
                seven_day: Some(0.4),
            }
        );
    }

    #[test]
    fn usage_refuses_garbage() {
        assert!(read_usage(as_str("not json")).is_err());
        assert!(read_usage(as_str("[]")).is_err());
        assert!(read_usage(as_str("{}")).is_err());
        assert!(read_usage(as_str(r#"{"five_hour":{}}"#)).is_err());
        assert!(
            read_usage(as_str(
                r#"{"five_hour":{"utilization":0.5},"seven_day":{}}"#
            ))
            .is_ok()
        );
    }

    // one notice for a window at or over the limit, not one per refresh; it drops below and
    // may have its own one notice again
    #[test]
    fn the_threshold_posts_once_till_it_drops() {
        let outside = TestOutside::new(vec![
            Fetch::Ok(Usage {
                five_hour: Some(0.95),
                seven_day: None,
            }),
            Fetch::Ok(Usage {
                five_hour: Some(0.95),
                seven_day: None,
            }),
            Fetch::Ok(Usage {
                five_hour: Some(0.5),
                seven_day: None,
            }),
            Fetch::Ok(Usage {
                five_hour: Some(0.95),
                seven_day: None,
            }),
        ]);
        let mut worker = Worker::new(outside);
        let now = Instant::now();

        worker.handle(Message::Refresh, now);
        assert_eq!(worker.outside.posts(), 2); // the rings, then the one notice

        worker.handle(Message::Due, now + INTERVAL);
        assert_eq!(worker.outside.posts(), 3); // the rings again, no new notice

        worker.handle(Message::Due, now + 2 * INTERVAL);
        assert_eq!(worker.outside.posts(), 4); // below the limit: notice clears for next time

        worker.handle(Message::Due, now + 3 * INTERVAL);
        assert_eq!(worker.outside.posts(), 6); // back over: the rings plus a new notice
    }

    // nothing posts without a key, and nothing polls for one
    #[test]
    fn nothing_posts_without_a_key() {
        let outside = TestOutside::with_key(None, vec![]);
        let mut worker = Worker::new(outside);

        worker.handle(Message::Refresh, Instant::now());
        assert_eq!(worker.outside.posts(), 0);
        assert_eq!(worker.next, None);
    }

    // failures back off, tell once, then wait for `kanade usage`
    #[test]
    fn failures_tell_once_and_then_wait() {
        let outside = TestOutside::new(vec![Fetch::Err; 5]);
        let mut worker = Worker::new(outside);
        let now = Instant::now();

        worker.handle(Message::Refresh, now);
        assert_eq!(worker.outside.posts(), 0);
        assert_eq!(worker.next, Some(now + FIRST_RETRY));

        worker.handle(Message::Due, now + FIRST_RETRY);
        assert_eq!(worker.outside.posts(), 0);
        assert_eq!(worker.next, Some(now + 3 * FIRST_RETRY));

        worker.handle(Message::Due, now + 3 * FIRST_RETRY);
        assert_eq!(worker.outside.posts(), 1);
        assert_eq!(worker.next, None);

        // no polling while it waits to be asked again
        worker.handle(Message::Due, now + Duration::from_secs(3600));
        assert_eq!(worker.outside.posts(), 1);

        // asked again, it tries once more and still fails
        worker.handle(Message::Refresh, now + Duration::from_secs(3600));
        assert_eq!(worker.outside.posts(), 1);
        assert_eq!(
            worker.next,
            Some(now + Duration::from_secs(3600) + FIRST_RETRY)
        );
    }

    #[test]
    fn the_key_file_is_private_and_holds_only_the_key() {
        let dir = std::env::temp_dir().join(format!("kanade-usage-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        unsafe { std::env::set_var("XDG_STATE_HOME", &dir) };
        let said = store_key("sk-ant-...");
        unsafe { std::env::remove_var("XDG_STATE_HOME") };

        assert!(said.is_ok());

        let path = dir.join("kanade").join(FILE);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read(&path).unwrap(), b"sk-ant-...");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // a test's fetch, key and posts
    struct TestOutside {
        key: Option<String>,
        fetch: std::cell::RefCell<Vec<Fetch>>,
        posts: std::cell::RefCell<Vec<String>>,
    }

    #[derive(Debug, Clone)]
    enum Fetch {
        Ok(Usage),
        Err,
    }

    impl TestOutside {
        fn new(fetch: Vec<Fetch>) -> Self {
            Self::with_key(Some(String::from("key")), fetch)
        }

        fn with_key(key: Option<String>, fetch: Vec<Fetch>) -> Self {
            Self {
                key,
                fetch: std::cell::RefCell::new(fetch),
                posts: std::cell::RefCell::new(Vec::new()),
            }
        }

        fn posts(&self) -> usize {
            self.posts.borrow().len()
        }
    }

    impl Outside for TestOutside {
        fn key(&self) -> Option<String> {
            self.key.clone()
        }

        fn fetch(&self, _key: &str) -> Result<Usage, Error> {
            match self.fetch.borrow_mut().remove(0) {
                Fetch::Ok(usage) => Ok(usage),
                Fetch::Err => Err(Error::Shape),
            }
        }

        fn post(&self, activity: Activity) {
            let detail = match activity.detail() {
                Detail::Usage(usage) => {
                    format!("usage {:?} {:?}", usage.five_hour, usage.seven_day)
                }
                Detail::UsageNotice(text) => format!("notice {text}"),
                _ => String::from("other"),
            };

            self.posts.borrow_mut().push(detail);
        }
    }
}
