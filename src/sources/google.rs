//! Google Calendar sync (#151, ADR 0016, docs/design.md Calendar): an optional account whose
//! calendars the `calendar` Module shows beside the local ones. `kanade google-calendar sign-in
//! <client.json>` signs in through the browser with the user's own OAuth client (`oauth`); the
//! client and the refresh token go to the Secret Service (`secret`), never to the config or a log.
//!
//! The sync runs on this source's thread: each calendar the user shows in Google Calendar, its
//! events from `PAST` before now until `AHEAD` after, expanded by Google, written as one `.ics`
//! file each (`ics`) under `$XDG_CACHE_HOME/kanade/google-calendar`, which the `calendar` Module
//! reads and follows like a local directory. Again every `INTERVAL`, and on `sync`; a file whose
//! events did not change is not written, so the calendars are not read again for nothing.
//!
//! Signed out, the thread waits on its queue and costs no wakeups; the keyring is not asked until
//! a sign-in has been kept (`marker`), so a user without an account never meets its prompt. When
//! Google refuses the grant, as after a revoke, the account fails with `Problem::Revoked`, which
//! the Calendar Surface and `status` show, and waits for a sign-in; the events synced before stay
//! until a sign-out, and the local calendars are untouched either way. Sign-out revokes the grant
//! at Google, forgets the credentials and deletes the synced files.

mod ics;
mod oauth;
mod secret;

use std::fmt;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use amane::Service;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use ring::digest;
use serde_json::Value;
use ureq::Agent;

use self::oauth::{Access, Client, Pending};
use self::secret::Credentials;
use super::calendar;
use crate::{clock, config};

// the Secret Service, which keeps the credentials
pub const SECRETS: &str = "org.freedesktop.secrets";

// what opens the consent screen in the browser
pub const OPEN: &str = "xdg-open";

// how often the events are fetched again: a change made elsewhere shows within it
const INTERVAL: Duration = Duration::from_secs(15 * 60);

// the days synced each side of now, which the Calendar Surface shows Google's events within
const PAST: i64 = 366 * 24 * 60 * 60;
const AHEAD: i64 = 2 * 366 * 24 * 60 * 60;

const API: &str = "https://www.googleapis.com/calendar/v3";

// a page of events, Google's most, and how many pages one calendar may take
const PAGE: u32 = 2500;
const PAGES: usize = 20;

// the calendars a page lists, and how many pages
const CALENDARS: u32 = 250;
const CALENDAR_PAGES: usize = 4;

// one answer's largest, far past a page of events
const BODY: u64 = 32 * 1024 * 1024;

// how long one request may take, an answer's reading included
const PATIENCE: Duration = Duration::from_secs(60);

// under the cache directory, the synced events, and under the state directory, the marker
const DIRECTORY: &str = "kanade/google-calendar";
const MARKER: &str = "kanade/google-calendar";

// why the account does not sync
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    // Google refuses the grant: revoked, expired or ended by a password change
    Revoked,

    // Google could not be reached, in ureq's words
    Offline(String),

    // Google answered no, in its words, which name no secret
    Refused(String),

    // the Secret Service has no credentials to give, in its words
    Keyring(String),

    // a sign-in did not finish
    SignIn(String),

    // the synced events could not be written
    Disk(String),
}

impl Problem {
    // in a few words, for the Calendar Surface
    pub fn brief(&self) -> &'static str {
        match self {
            Problem::Revoked => "Google Calendar: sign in again",
            Problem::Offline(_) => "Google Calendar: offline",
            Problem::Refused(_) | Problem::Disk(_) => "Google Calendar: sync failed",
            Problem::Keyring(_) => "Google Calendar: keyring unavailable",
            Problem::SignIn(_) => "Google Calendar: not signed in",
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::Revoked => f.write_str(
                "access was revoked or has expired; sign in again with \
                 `kanade google-calendar sign-in <client.json>`",
            ),
            Problem::Offline(why) => write!(f, "Google cannot be reached: {why}"),
            Problem::Refused(why) => f.write_str(why),
            Problem::Keyring(why) => write!(f, "no credentials: {why}"),
            Problem::SignIn(why) => write!(f, "not signed in: {why}"),
            Problem::Disk(why) => write!(f, "cannot write the synced events: {why}"),
        }
    }
}

// how the account stands, for the Calendar Surface and `status`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Account {
    pub state: State,

    // when the events on disk were synced, in seconds since the epoch; none before a first sync
    pub synced: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    SignedOut,

    // waiting for the browser
    SigningIn,
    SignedIn,
    Failed(Problem),
}

impl Service for Account {
    fn new() -> Self {
        Account::default()
    }

    fn listen() {}
}

impl Account {
    pub fn problem(&self) -> Option<&Problem> {
        match &self.state {
            State::Failed(problem) => Some(problem),
            _ => None,
        }
    }

    // a line for `status`, with when it last synced read by `local`
    fn status(&self, local: impl Fn(i64) -> Option<String>) -> String {
        let synced = self.synced.and_then(local);

        let said = match (&self.state, synced) {
            (State::SignedOut, _) => String::from("signed out"),
            (State::SigningIn, _) => String::from("signing in, waiting for the browser"),
            (State::SignedIn, None) => String::from("signed in, syncing"),
            (State::SignedIn, Some(at)) => format!("signed in, synced at {at}"),
            (State::Failed(problem), None) => problem.to_string(),
            (State::Failed(problem), Some(at)) => {
                format!("{problem}; showing the events synced at {at}")
            }
        };

        format!("google-calendar: {said}")
    }
}

// what `kanade google-calendar` asks
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    // the OAuth client's JSON file, as Google Cloud gives it
    SignIn(PathBuf),
    SignOut,
    Sync,
    Status,
}

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        match arguments {
            ["sign-in", path] => Some(Request::SignIn(PathBuf::from(path))),
            ["sign-out"] => Some(Request::SignOut),
            ["sync"] => Some(Request::Sync),
            ["status"] => Some(Request::Status),
            _ => None,
        }
    }
}

// what the sync thread is told
enum Message {
    Sync,
    Signing,
    SignedIn(Credentials),

    // a sign-in did not finish, and why
    Unsigned(String),
    SignOut,
}

// the sync thread's queue, which outlives a restart of it
static QUEUE: LazyLock<(Sender<Message>, Mutex<Receiver<Message>>)> = LazyLock::new(|| {
    let (send, receive) = mpsc::channel();

    (send, Mutex::new(receive))
});

// the sign-in going on; a newer one, or a sign-out, ends the one before
static SIGN_IN: AtomicU64 = AtomicU64::new(0);

fn send(message: Message) {
    // the receiver lives in a static, so it is never gone
    let _ = QUEUE.0.send(message);
}

// what `kanade google-calendar` gets, asked on the draw thread, so nothing here waits on Google
pub fn request(request: Request) -> Result<String, String> {
    match request {
        Request::SignIn(path) => sign_in(&path),
        Request::SignOut => {
            SIGN_IN.fetch_add(1, Ordering::Relaxed);
            send(Message::SignOut);

            Ok(String::from("signing out"))
        }
        Request::Sync => match Account::read().state {
            State::SignedOut => Err(String::from(
                "not signed in; sign in with `kanade google-calendar sign-in <client.json>`",
            )),
            _ => {
                send(Message::Sync);
                Ok(String::from("syncing"))
            }
        },
        Request::Status => Ok(status()),
    }
}

// the account's line for `status`
pub fn status() -> String {
    Account::read().status(|seconds| {
        Some(
            clock::local_time(seconds)?
                .format("%Y-%m-%d %H:%M")
                .to_string(),
        )
    })
}

// where the synced events are kept
pub fn directory() -> Option<PathBuf> {
    config::base("XDG_CACHE_HOME", ".cache", home().as_deref()).map(|dir| dir.join(DIRECTORY))
}

// a file that says a sign-in was kept, so the keyring is asked only then
fn marker() -> Option<PathBuf> {
    config::base("XDG_STATE_HOME", ".local/state", home().as_deref()).map(|dir| dir.join(MARKER))
}

fn home() -> Option<String> {
    std::env::var("HOME").ok()
}

fn signed_in() -> bool {
    marker().is_some_and(|marker| marker.exists())
}

// starts a sign-in, answering the address the browser opens
fn sign_in(path: &Path) -> Result<String, String> {
    let pending = Pending::start(Client::read(path)?)?;
    let address = pending.address.clone();
    let sign_in = SIGN_IN.fetch_add(1, Ordering::Relaxed) + 1;

    // before the thread, so how the sign-in went always comes after
    send(Message::Signing);
    if let Err(error) = thread::Builder::new()
        .name(String::from("google-sign-in"))
        .spawn(move || signing_in(&pending, sign_in))
    {
        let why = format!("cannot start a thread: {error}");

        send(Message::Unsigned(why.clone()));
        return Err(why);
    }

    Ok(format!(
        "sign in to Google in the browser; if none opened, open\n{address}"
    ))
}

// the sign-in's own thread: the browser, the code, then the credentials kept
fn signing_in(pending: &Pending, sign_in: u64) {
    let cancelled = || SIGN_IN.load(Ordering::Relaxed) != sign_in;

    open(&pending.address);

    let signed = pending
        .wait(Instant::now() + oauth::PATIENCE, cancelled)
        .and_then(|returned| {
            let kept = pending
                .exchange(&agent(), &returned.code)
                .and_then(|credentials| {
                    if cancelled() {
                        return Err(String::from("cancelled"));
                    }

                    secret::store(&credentials)?;
                    mark().map_err(|error| format!("cannot keep the sign-in: {error}"))?;

                    Ok(credentials)
                });

            returned.tell(kept.as_ref().map(|_| ()).map_err(String::as_str));
            kept
        });

    if cancelled() {
        return;
    }

    match signed {
        Ok(credentials) => send(Message::SignedIn(credentials)),
        Err(why) => {
            eprintln!("kanade: google-calendar: not signed in: {why}");
            send(Message::Unsigned(why));
        }
    }
}

// the consent screen in the browser, which outlives Kanade, so only reaped
fn open(address: &str) {
    let opened = Command::new(OPEN)
        .arg(address)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .and_then(|mut child| child.wait());

    match opened {
        Ok(status) if !status.success() => {
            eprintln!("kanade: google-calendar: {OPEN} failed ({status})");
        }
        Ok(_) => {}
        Err(error) => eprintln!("kanade: google-calendar: {OPEN}: {error}"),
    }
}

fn mark() -> io::Result<()> {
    let marker = marker().ok_or_else(|| io::Error::other("no home directory"))?;

    if let Some(dir) = marker.parent() {
        fs::create_dir_all(dir)?;
    }

    fs::write(
        marker,
        "signed in to Google Calendar; the credentials are in the keyring\n",
    )
}

fn agent() -> Agent {
    Agent::config_builder()
        .timeout_global(Some(PATIENCE))
        .http_status_as_error(false)
        .build()
        .into()
}

// why a request did not get an answer
fn unreached(error: ureq::Error) -> Problem {
    match error {
        ureq::Error::BodyExceedsLimit(limit) => Problem::Refused(format!(
            "Google's answer was over {} MiB",
            limit / 1024 / 1024
        )),
        error => Problem::Offline(error.to_string()),
    }
}

/*
 * syncs the account, on its own thread: at start when signed in, again every `INTERVAL` and when
 * asked, until signed out
 */
pub fn follow() {
    let queue = QUEUE.1.lock().unwrap_or_else(PoisonError::into_inner);
    let agent = agent();
    let mut kept = if signed_in() { load() } else { None };
    let mut access = None;
    let mut due = kept.is_some();

    loop {
        if due && let Some(credentials) = &kept {
            sync(&agent, credentials, &mut access);
        }

        // a refused grant waits for a sign-in, as asking again is refused again
        let waits = kept.is_some() && Account::read().problem() != Some(&Problem::Revoked);
        let message = if waits {
            match queue.recv_timeout(INTERVAL) {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout) => Message::Sync,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match queue.recv() {
                Ok(message) => message,
                Err(_) => return,
            }
        };

        due = false;
        match message {
            Message::Sync => {
                // the keyring may have been locked at start
                if kept.is_none() && signed_in() {
                    kept = load();
                }
                due = kept.is_some();
            }
            Message::Signing => set(State::SigningIn, None),
            Message::SignedIn(credentials) => {
                kept = Some(credentials);
                access = None;
                due = true;
                set(State::SignedIn, None);
            }
            Message::Unsigned(why) => set(State::Failed(Problem::SignIn(why)), None),
            Message::SignOut => {
                access = None;
                sign_out(&agent, kept.take());
            }
        }
    }
}

// the account's state, and when it synced if that changed; written only on a change
fn set(state: State, synced: Option<i64>) {
    let account = Account::read().clone();
    let synced = synced.or(account.synced);

    if account.state != state || account.synced != synced {
        let mut account = Account::write();
        account.state = state;
        account.synced = synced;
    }
}

// the credentials the keyring keeps, which may prompt to unlock it
fn load() -> Option<Credentials> {
    match secret::load() {
        Ok(Some(credentials)) => Some(credentials),
        // gone from the keyring, as by a keyring app: signed out
        Ok(None) => {
            if let Some(marker) = marker() {
                let _ = fs::remove_file(marker);
            }
            set(State::SignedOut, None);
            None
        }
        Err(why) => {
            eprintln!("kanade: google-calendar: {why}");
            set(State::Failed(Problem::Keyring(why)), None);
            None
        }
    }
}

fn sync(agent: &Agent, credentials: &Credentials, access: &mut Option<Access>) {
    let synced = fetch(agent, credentials, access).and_then(|files| {
        let dir = directory().ok_or_else(|| Problem::Disk(String::from("no home directory")))?;

        write(&dir, &files).map_err(|error| Problem::Disk(error.to_string()))
    });

    match synced {
        Ok(created) => {
            // the calendar watches the directory only once it is there
            if created {
                calendar::reread();
            }

            set(State::SignedIn, Some(now()));
        }
        Err(problem) => {
            if Account::read().problem() != Some(&problem) {
                eprintln!("kanade: google-calendar: {problem}");
            }

            set(State::Failed(problem), None);
        }
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

// each calendar shown in Google Calendar, as the name of its file and the file
fn fetch(
    agent: &Agent,
    credentials: &Credentials,
    access: &mut Option<Access>,
) -> Result<Vec<(String, String)>, Problem> {
    let mut api = Api {
        agent,
        credentials,
        access,
    };

    let calendars = api.all(
        &format!("{API}/users/me/calendarList"),
        &[
            ("maxResults", &CALENDARS.to_string()),
            ("fields", "items(id,selected,hidden),nextPageToken"),
        ],
        CALENDAR_PAGES,
    )?;

    let now = now();
    let (Some(from), Some(to)) = (ics::rfc3339(now - PAST), ics::rfc3339(now + AHEAD)) else {
        return Err(Problem::Refused(String::from("the clock is out of range")));
    };

    let mut files = Vec::new();
    for calendar in calendars.iter().filter(|calendar| shown(calendar)) {
        let Some(id) = calendar.get("id").and_then(Value::as_str) else {
            continue;
        };

        let events = api.all(
            &format!(
                "{API}/calendars/{}/events",
                utf8_percent_encode(id, NON_ALPHANUMERIC)
            ),
            &[
                ("singleEvents", "true"),
                ("timeMin", &from),
                ("timeMax", &to),
                ("maxResults", &PAGE.to_string()),
                (
                    "fields",
                    "items(id,status,summary,location,start,end,attendees(self,responseStatus)),\
                     nextPageToken",
                ),
            ],
            PAGES,
        )?;

        files.push((file_name(id), ics::calendar(&events)));
    }

    Ok(files)
}

// whether the user shows it in Google Calendar, which is where they choose what they see
fn shown(calendar: &Value) -> bool {
    calendar.get("selected").and_then(Value::as_bool) == Some(true)
        && calendar.get("hidden").and_then(Value::as_bool) != Some(true)
}

// a calendar's file, named by a hash of its id, which is often an address
fn file_name(id: &str) -> String {
    let hash = digest::digest(&digest::SHA256, id.as_bytes());
    let hex: String = hash.as_ref()[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();

    format!("{hex}.ics")
}

// the Calendar API, with the access token kept fresh
struct Api<'a> {
    agent: &'a Agent,
    credentials: &'a Credentials,
    access: &'a mut Option<Access>,
}

impl Api<'_> {
    // the items of every page, up to `pages`
    fn all(
        &mut self,
        url: &str,
        query: &[(&str, &str)],
        pages: usize,
    ) -> Result<Vec<Value>, Problem> {
        let mut items = Vec::new();
        let mut next: Option<String> = None;

        for _ in 0..pages {
            let mut asked = query.to_vec();
            if let Some(next) = &next {
                asked.push(("pageToken", next));
            }

            let mut page = self.get(url, &asked)?;
            if let Some(Value::Array(found)) = page.get_mut("items").map(Value::take) {
                items.extend(found);
            }

            next = page
                .get("nextPageToken")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if next.is_none() {
                return Ok(items);
            }
        }

        eprintln!("kanade: google-calendar: a calendar has more events than are synced");
        Ok(items)
    }

    // one answer; an access token Google no longer takes is refreshed once
    fn get(&mut self, url: &str, query: &[(&str, &str)]) -> Result<Value, Problem> {
        for retry in [false, true] {
            let token = self.token()?;
            let mut response = self
                .agent
                .get(url)
                .header("Authorization", &format!("Bearer {token}"))
                .query_pairs(query.iter().copied())
                .call()
                .map_err(unreached)?;

            let status = response.status().as_u16();
            let body: Value = response
                .body_mut()
                .with_config()
                .limit(BODY)
                .read_to_string()
                .map_err(unreached)
                .map(|text| serde_json::from_str(&text).unwrap_or(Value::Null))?;

            match status {
                200 => return Ok(body),
                401 if !retry => *self.access = None,
                status => return Err(refusal(status, &body)),
            }
        }

        Err(Problem::Refused(String::from(
            "Google refused a fresh access token",
        )))
    }

    fn token(&mut self) -> Result<String, Problem> {
        if let Some(access) = self
            .access
            .as_ref()
            .filter(|access| access.until > Instant::now())
        {
            return Ok(access.token.clone());
        }

        let access = oauth::refresh(self.agent, self.credentials)?;
        let token = access.token.clone();
        *self.access = Some(access);

        Ok(token)
    }
}

// why the Calendar API said no, in its message, like the API not being on for the client's project
fn refusal(status: u16, body: &Value) -> Problem {
    match body.pointer("/error/message").and_then(Value::as_str) {
        Some(message) => Problem::Refused(format!("Google refused it ({status}): {message}")),
        None => Problem::Refused(format!("Google answered {status}")),
    }
}

/*
 * the synced files in `dir`, each written only when it changed, and the files of calendars no
 * longer shown deleted; true when it made `dir`. Only Kanade reads them, as they hold the user's
 * events
 */
fn write(dir: &Path, files: &[(String, String)]) -> io::Result<bool> {
    let created = !dir.is_dir();

    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;

    for (name, text) in files {
        let path = dir.join(name);
        if fs::read_to_string(&path).is_ok_and(|kept| kept == *text) {
            continue;
        }

        // not `.ics`, so the calendar never reads one half written
        let part = dir.join(format!(".{name}.part"));
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&part)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;

        fs::rename(&part, &path)?;
    }

    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let ours = path.extension().is_some_and(|extension| extension == "ics");
        let kept = path
            .file_name()
            .is_some_and(|name| files.iter().any(|(kept, _)| name == kept.as_str()));

        if ours && !kept {
            fs::remove_file(path)?;
        }
    }

    Ok(created)
}

/*
 * ends the grant at Google, forgets the credentials and deletes the synced events; what could not
 * be done is said, and the account fails with it
 */
fn sign_out(agent: &Agent, kept: Option<Credentials>) {
    let mut problem = None;

    let credentials = match kept {
        Some(credentials) => Some(credentials),
        None if signed_in() => secret::load().ok().flatten(),
        None => None,
    };
    if let Some(credentials) = &credentials
        && let Err(why) = oauth::revoke(agent, credentials)
    {
        eprintln!("kanade: google-calendar: the grant was not revoked at Google: {why}");
    }

    if let Err(why) = secret::delete() {
        eprintln!("kanade: google-calendar: the credentials stay in the keyring: {why}");
        problem = Some(Problem::Keyring(why));
    }

    if let Some(marker) = marker() {
        let _ = fs::remove_file(marker);
    }
    if let Some(dir) = directory()
        && let Err(error) = fs::remove_dir_all(&dir)
        && error.kind() != io::ErrorKind::NotFound
    {
        eprintln!(
            "kanade: google-calendar: cannot delete {}: {error}",
            dir.display()
        );
        problem.get_or_insert(Problem::Disk(error.to_string()));
    }
    calendar::reread();

    let mut account = Account::write();
    account.state = problem.map_or(State::SignedOut, State::Failed);
    account.synced = None;
}

#[cfg(test)]
mod tests {
    use std::env;

    use super::*;

    #[test]
    fn requests_are_named() {
        assert_eq!(
            Request::parse(&["sign-in", "client.json"]),
            Some(Request::SignIn(PathBuf::from("client.json")))
        );
        assert_eq!(Request::parse(&["sign-out"]), Some(Request::SignOut));
        assert_eq!(Request::parse(&["sync"]), Some(Request::Sync));
        assert_eq!(Request::parse(&["status"]), Some(Request::Status));
        assert_eq!(Request::parse(&["sign-in"]), None);
        assert_eq!(Request::parse(&["login", "a"]), None);
    }

    #[test]
    fn status_says_how_the_account_stands() {
        let at = |seconds: i64| Some(format!("t{seconds}"));
        let account = |state, synced| Account { state, synced };

        assert_eq!(
            account(State::SignedOut, None).status(at),
            "google-calendar: signed out"
        );
        assert_eq!(
            account(State::SignedIn, None).status(at),
            "google-calendar: signed in, syncing"
        );
        assert_eq!(
            account(State::SignedIn, Some(5)).status(at),
            "google-calendar: signed in, synced at t5"
        );
        assert_eq!(
            account(
                State::Failed(Problem::Offline(String::from("timeout"))),
                Some(5)
            )
            .status(at),
            "google-calendar: Google cannot be reached: timeout; showing the events synced at t5"
        );
        assert!(
            account(State::Failed(Problem::Revoked), None)
                .status(at)
                .contains("sign in again")
        );
    }

    #[test]
    fn only_calendars_shown_in_google_calendar_sync() {
        let calendar = |json: &str| serde_json::from_str::<Value>(json).unwrap_or_default();

        assert!(shown(&calendar(r#"{"id": "a", "selected": true}"#)));
        assert!(!shown(&calendar(r#"{"id": "a"}"#)));
        assert!(!shown(&calendar(
            r#"{"id": "a", "selected": true, "hidden": true}"#
        )));
    }

    #[test]
    fn files_are_named_by_a_hash_of_the_id() {
        let name = file_name("you@gmail.com");

        assert_eq!(name.len(), 16 + 4);
        assert!(name.ends_with(".ics"));
        assert!(!name.contains("gmail"));
        assert_ne!(
            name,
            file_name("en.usa#holiday@group.v.calendar.google.com")
        );
    }

    #[test]
    fn a_sync_writes_only_what_changed_and_deletes_what_went() {
        let dir = env::temp_dir().join(format!("kanade-google-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let files = |list: &[(&str, &str)]| -> Vec<(String, String)> {
            list.iter()
                .map(|(name, text)| (String::from(*name), String::from(*text)))
                .collect()
        };

        assert!(matches!(
            write(&dir, &files(&[("a.ics", "A"), ("b.ics", "B")])),
            Ok(true)
        ));
        let mode = fs::metadata(&dir).map(|metadata| {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o777
        });
        assert_eq!(mode.ok(), Some(0o700));

        let written = fs::metadata(dir.join("a.ics")).and_then(|metadata| metadata.modified());
        fs::write(dir.join("notes.txt"), "mine").ok();

        assert!(matches!(write(&dir, &files(&[("a.ics", "A")])), Ok(false)));
        assert_eq!(
            fs::metadata(dir.join("a.ics"))
                .and_then(|metadata| metadata.modified())
                .ok(),
            written.ok()
        );
        assert!(!dir.join("b.ics").exists());
        assert!(dir.join("notes.txt").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_api_names_why_it_refused() {
        let body: Value = serde_json::from_str(
            r#"{"error": {"code": 403, "message": "Google Calendar API has not been used in project 1 before or it is disabled."}}"#,
        )
        .unwrap_or_default();

        assert_eq!(
            refusal(403, &body),
            Problem::Refused(String::from(
                "Google refused it (403): Google Calendar API has not been used in project 1 \
                 before or it is disabled."
            ))
        );
    }
}
