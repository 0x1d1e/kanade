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
//!
//! Only the sync thread (`Worker`) keeps, uses and forgets the credentials, one message at a time,
//! so a sign-out never interleaves with a sign-in being kept, even one waiting on a keyring prompt.
//! A sign-in's thread only brings the code back and trades it; one cancelled by then is not kept.
//! A sign-in kept is a new account to Kanade: the events synced before go once it is kept, and
//! one not kept leaves the account it would have replaced as it was. The synced events name the
//! sign-in they are for (`OWNER`). The calendar reads them only once this thread has checked them
//! at start (`vouched`), so events a crash left, synced for another sign-in or for none after an
//! interrupted sign-out, are deleted before they can show.

mod ics;
mod oauth;
mod secret;

use std::fmt;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

// in the synced events' directory, the sign-in they were synced for (`Credentials::account`); not
// `.ics`, so the calendar never reads it
const OWNER: &str = ".account";

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

    /*
     * a sign-in's credentials, by the sign-in's number, to keep unless a newer sign-in or a
     * sign-out came since; whether they were kept is told back, for the browser's tab
     */
    Exchanged {
        sign_in: u64,
        credentials: Credentials,
        kept: Sender<Result<(), String>>,
    },

    // a sign-in did not finish, and why
    Unsigned(u64, String),
    SignOut,
}

// the sync thread's queue, which outlives a restart of it
static QUEUE: LazyLock<(Sender<Message>, Mutex<Receiver<Message>>)> = LazyLock::new(|| {
    let (send, receive) = mpsc::channel();

    (send, Mutex::new(receive))
});

// the sign-in going on; a newer one, or a sign-out, ends the one before
static SIGN_IN: AtomicU64 = AtomicU64::new(0);

// whether the sync thread found the events on disk the kept sign-in's since start (`Worker::vet`)
static VOUCHED: AtomicBool = AtomicBool::new(false);

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

/*
 * where the synced events are kept, for the calendar to read; none until the sync thread found
 * them the kept sign-in's, as a crash may have left another's, or a signed-out one's
 */
pub fn vouched() -> Option<PathBuf> {
    VOUCHED.load(Ordering::Relaxed).then(directory).flatten()
}

// where the synced events are kept
pub fn directory() -> Option<PathBuf> {
    config::base("XDG_CACHE_HOME", ".cache", home().as_deref()).map(|dir| dir.join(DIRECTORY))
}

// the file that says a sign-in was kept
fn marker() -> Option<PathBuf> {
    config::base("XDG_STATE_HOME", ".local/state", home().as_deref()).map(|dir| dir.join(MARKER))
}

fn home() -> Option<String> {
    std::env::var("HOME").ok()
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

        send(Message::Unsigned(sign_in, why.clone()));
        return Err(why);
    }

    Ok(format!(
        "sign in to Google in the browser; if none opened, open\n{address}"
    ))
}

/*
 * the sign-in's own thread: the browser and the code. The sync thread keeps the credentials, as it
 * runs a sign-out too, so a sign-out never comes in the middle of keeping them
 */
fn signing_in(pending: &Pending, sign_in: u64) {
    let cancelled = || SIGN_IN.load(Ordering::Relaxed) != sign_in;

    launch(OPEN, &pending.address);

    let signed = pending
        .wait(Instant::now() + oauth::PATIENCE, cancelled)
        .and_then(|returned| {
            let kept = pending
                .exchange(&agent(), &returned.code)
                .and_then(|credentials| {
                    let (told, heard) = mpsc::channel();

                    send(Message::Exchanged {
                        sign_in,
                        credentials,
                        kept: told,
                    });
                    heard
                        .recv()
                        .unwrap_or_else(|_| Err(String::from("the sync thread stopped")))
                });

            returned.tell(kept.as_ref().map_err(String::as_str).copied());
            kept
        });

    if let Err(why) = signed
        && !cancelled()
    {
        eprintln!("kanade: google-calendar: not signed in: {why}");
        send(Message::Unsigned(sign_in, why));
    }
}

/*
 * `program` on `address`, never waited on here: an opener may run as long as the browser it
 * started, and the browser's answer must not wait for that. A thread of its own reaps it
 */
fn launch(program: &str, address: &str) {
    let child = Command::new(program)
        .arg(address)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            eprintln!("kanade: google-calendar: {program}: {error}");
            return;
        }
    };

    let program = program.to_owned();
    let reaping = thread::Builder::new()
        .name(String::from("google-open"))
        .spawn(move || match child.wait() {
            Ok(status) if !status.success() => {
                eprintln!("kanade: google-calendar: {program} failed ({status})");
            }
            Ok(_) => {}
            Err(error) => eprintln!("kanade: google-calendar: {program}: {error}"),
        });

    if let Err(error) = reaping {
        eprintln!("kanade: google-calendar: the browser's opener is not reaped: {error}");
    }
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

    Worker::new(
        Google { agent: agent() },
        Places {
            events: directory(),
            marker: marker(),
        },
    )
    .run(&queue);
}

// what the sync thread works with outside itself, so a test stands in for Google and the keyring
trait Outside {
    fn load(&self) -> Result<Option<Credentials>, String>;
    fn store(&self, credentials: &Credentials) -> Result<(), String>;
    fn delete(&self) -> Result<(), String>;

    // ends the grant at Google; one that cannot be ended is only said
    fn revoke(&self, credentials: &Credentials);

    // each calendar shown in Google Calendar, as the name of its file and the file
    fn fetch(
        &self,
        credentials: &Credentials,
        access: &mut Option<Access>,
    ) -> Result<Vec<(String, String)>, Problem>;

    // the sign-in going on, which a sign-in's credentials must still be to be kept
    fn sign_in(&self) -> u64;

    // the calendar reads its places again
    fn reread(&self);

    // the events on disk are the kept sign-in's, or none, so the calendar may read them
    fn vouch(&self);

    // the account as the Calendar Surface and `status` read it
    fn publish(&self, account: &Account);
}

// the real ones
struct Google {
    agent: Agent,
}

impl Outside for Google {
    fn load(&self) -> Result<Option<Credentials>, String> {
        secret::load()
    }

    fn store(&self, credentials: &Credentials) -> Result<(), String> {
        secret::store(credentials)
    }

    fn delete(&self) -> Result<(), String> {
        secret::delete()
    }

    fn revoke(&self, credentials: &Credentials) {
        if let Err(why) = oauth::revoke(&self.agent, credentials) {
            eprintln!("kanade: google-calendar: the grant was not revoked at Google: {why}");
        }
    }

    fn fetch(
        &self,
        credentials: &Credentials,
        access: &mut Option<Access>,
    ) -> Result<Vec<(String, String)>, Problem> {
        fetch(&self.agent, credentials, access)
    }

    fn sign_in(&self) -> u64 {
        SIGN_IN.load(Ordering::Relaxed)
    }

    fn reread(&self) {
        calendar::reread();
    }

    fn vouch(&self) {
        VOUCHED.store(true, Ordering::Relaxed);
        calendar::reread();
    }

    // written only on a change, as a write wakes every window
    fn publish(&self, account: &Account) {
        if *Account::read() != *account {
            *Account::write() = account.clone();
        }
    }
}

// where the sync thread keeps things on disk
struct Places {
    // the synced events
    events: Option<PathBuf>,

    // the file that says a sign-in was kept, so the keyring is asked only then
    marker: Option<PathBuf>,
}

/*
 * the sync thread: the one place the credentials are kept, synced with and forgotten, one
 * message at a time
 */
struct Worker<O> {
    outside: O,
    places: Places,
    kept: Option<Credentials>,
    access: Option<Access>,
    account: Account,

    // whether the events on disk were found the kept sign-in's since start (`vet`)
    vetted: bool,
}

impl<O: Outside> Worker<O> {
    fn new(outside: O, places: Places) -> Self {
        Worker {
            outside,
            places,
            kept: None,
            access: None,
            account: Account::default(),
            vetted: false,
        }
    }

    // until the queue closes
    fn run(&mut self, queue: &Receiver<Message>) {
        if self.signed_in() {
            self.load();
        }
        let mut due = self.kept.is_some();

        loop {
            self.vet();

            if due {
                self.sync();
            }

            // a refused grant waits for a sign-in, as asking again is refused again
            let waits = self.kept.is_some() && self.account.problem() != Some(&Problem::Revoked);
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

            due = self.handle(message);
        }
    }

    // whether to sync now
    fn handle(&mut self, message: Message) -> bool {
        match message {
            Message::Sync => {
                // the keyring may have been locked at start
                if self.kept.is_none() && self.signed_in() {
                    self.load();
                }
                self.kept.is_some()
            }
            // an account already kept stays as it stands until another is kept in its place
            Message::Signing => {
                if self.kept.is_none() {
                    self.set(State::SigningIn, self.account.synced);
                }
                false
            }
            Message::Exchanged {
                sign_in,
                credentials,
                kept,
            } => {
                let keeping = self.keep(sign_in, credentials);
                let due = keeping.is_ok();

                // the sign-in's thread may be gone
                let _ = kept.send(keeping);
                due
            }
            // the browser's tab already said why
            Message::Unsigned(sign_in, why) => {
                if sign_in == self.outside.sign_in() && self.kept.is_none() {
                    self.set(State::Failed(Problem::SignIn(why)), self.account.synced);
                }
                false
            }
            Message::SignOut => {
                self.sign_out();
                false
            }
        }
    }

    /*
     * keeps a sign-in's credentials, unless a newer sign-in or a sign-out came since, which then
     * runs after this, including one that comes while the keyring prompts. They may be another
     * account's: the events synced before stay until these are kept, then go. A sign-in not kept leaves the account it replaces as it was; a crash
     * between keeping and deleting leaves the events to `run`, which drops a cache synced for
     * another account
     */
    fn keep(&mut self, sign_in: u64, credentials: Credentials) -> Result<(), String> {
        let superseded = || sign_in != self.outside.sign_in();
        if superseded() {
            return Err(String::from("cancelled"));
        }

        let marked = self.signed_in();
        self.outside.store(&credentials)?;

        // a newer sign-in or a sign-out may have come while the keyring prompted
        if superseded() {
            self.restore(marked);
            return Err(String::from("cancelled"));
        }

        if let Err(error) = self.mark() {
            self.restore(marked);
            return Err(format!("cannot keep the sign-in: {error}"));
        }

        // they would show under these credentials, so the last account comes back instead
        if let Err(why) = self.forget_events() {
            self.restore(marked);
            return Err(format!("cannot delete the events synced before: {why}"));
        }

        self.kept = Some(credentials);
        self.access = None;
        self.set(State::SignedIn, None);

        Ok(())
    }

    /*
     * the keyring and the marker back as they were before a sign-in that was not kept: the
     * credentials kept, else none, and the marker only if it was there (`marked`)
     */
    fn restore(&self, marked: bool) {
        if !marked {
            self.unmark();
        }

        let restored = match &self.kept {
            Some(kept) => self.outside.store(kept),
            None => self.outside.delete(),
        };

        if let Err(why) = restored {
            eprintln!("kanade: google-calendar: the keyring was not put back: {why}");
        }
    }

    /*
     * lets the calendar show the events on disk once they are known to be the kept sign-in's. A
     * crash can leave another's: between keeping a sign-in and deleting the last one's events,
     * or in a sign-out, after the marker went. Those are deleted first. While the keyring has not
     * said which sign-in is kept, whose they are is not known, so they stay hidden
     */
    fn vet(&mut self) {
        if self.vetted {
            return;
        }

        let foreign = match &self.kept {
            Some(kept) => !self.owns(&kept.account),
            None if self.signed_in() => return,
            // signed out, so any are left over
            None => true,
        };
        let there = self.places.events.as_ref().is_some_and(|dir| dir.exists());

        if foreign && there {
            if self.forget_events().is_err() {
                return;
            }
            eprintln!("kanade: google-calendar: deleted events synced for no current sign-in");
        }

        self.vetted = true;
        self.outside.vouch();
    }

    // whether the events on disk were synced for `account`; none on disk is no other's
    fn owns(&self, account: &str) -> bool {
        let Some(dir) = &self.places.events else {
            return true;
        };

        match fs::read_to_string(dir.join(OWNER)) {
            Ok(owner) => owner == account,
            Err(error) => error.kind() == io::ErrorKind::NotFound && !dir.exists(),
        }
    }

    fn signed_in(&self) -> bool {
        self.places
            .marker
            .as_ref()
            .is_some_and(|marker| marker.exists())
    }

    fn mark(&self) -> io::Result<()> {
        let marker = self
            .places
            .marker
            .as_ref()
            .ok_or_else(|| io::Error::other("no home directory"))?;

        if let Some(dir) = marker.parent() {
            fs::create_dir_all(dir)?;
        }

        fs::write(
            marker,
            "signed in to Google Calendar; the credentials are in the keyring\n",
        )
    }

    fn unmark(&self) {
        if let Some(marker) = &self.places.marker {
            let _ = fs::remove_file(marker);
        }
    }

    // deletes the synced events; why not when they could not be
    fn forget_events(&self) -> Result<(), String> {
        let gone = match &self.places.events {
            Some(dir) => match fs::remove_dir_all(dir) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => {
                    eprintln!(
                        "kanade: google-calendar: cannot delete {}: {error}",
                        dir.display()
                    );
                    Err(error.to_string())
                }
                _ => Ok(()),
            },
            None => Ok(()),
        };

        self.outside.reread();

        gone
    }

    // the account's state, and when it last synced
    fn set(&mut self, state: State, synced: Option<i64>) {
        self.account = Account { state, synced };
        self.outside.publish(&self.account);
    }

    // the credentials the keyring keeps, which may prompt to unlock it
    fn load(&mut self) {
        match self.outside.load() {
            // signed in before the first sync ends, so `sync` and `status` know it meanwhile
            Ok(Some(credentials)) => {
                self.kept = Some(credentials);
                self.set(State::SignedIn, self.account.synced);
            }
            // gone from the keyring, as by a keyring app: signed out
            Ok(None) => {
                self.unmark();
                self.set(State::SignedOut, self.account.synced);
            }
            Err(why) => {
                eprintln!("kanade: google-calendar: {why}");
                self.set(State::Failed(Problem::Keyring(why)), self.account.synced);
            }
        }
    }

    fn sync(&mut self) {
        let Some(credentials) = &self.kept else {
            return;
        };

        let synced = self
            .outside
            .fetch(credentials, &mut self.access)
            .and_then(|files| {
                let dir = self
                    .places
                    .events
                    .as_ref()
                    .ok_or_else(|| Problem::Disk(String::from("no home directory")))?;

                write(dir, &credentials.account, &files)
                    .map_err(|error| Problem::Disk(error.to_string()))
            });

        match synced {
            Ok(created) => {
                // the calendar watches the directory only once it is there
                if created {
                    self.outside.reread();
                }

                self.set(State::SignedIn, Some(now()));
            }
            Err(problem) => {
                if self.account.problem() != Some(&problem) {
                    eprintln!("kanade: google-calendar: {problem}");
                }

                // the events synced before are this account's, so they stay
                self.set(State::Failed(problem), self.account.synced);
            }
        }
    }

    /*
     * ends the grant at Google, forgets the credentials and deletes the synced events; what could
     * not be done is said, and the account fails with it
     */
    fn sign_out(&mut self) {
        self.access = None;

        let credentials = match self.kept.take() {
            Some(credentials) => Some(credentials),
            None if self.signed_in() => self.outside.load().ok().flatten(),
            None => None,
        };
        if let Some(credentials) = &credentials {
            self.outside.revoke(credentials);
        }

        let mut problem = None;
        if let Err(why) = self.outside.delete() {
            eprintln!("kanade: google-calendar: the credentials stay in the keyring: {why}");
            problem = Some(Problem::Keyring(why));
        }

        self.unmark();
        if let Err(why) = self.forget_events() {
            problem.get_or_insert(Problem::Disk(why));
        }

        self.set(problem.map_or(State::SignedOut, State::Failed), None);
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
 * the synced files in `dir`, for the sign-in `account`, each written only when it changed, and
 * the files of calendars no longer shown deleted; true when it made `dir`. Only Kanade reads them,
 * as they hold the user's events
 */
fn write(dir: &Path, account: &str, files: &[(String, String)]) -> io::Result<bool> {
    let created = !dir.is_dir();

    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;

    // first, so the events are never another sign-in's than it names
    let owner = dir.join(OWNER);
    if fs::read_to_string(&owner).ok().as_deref() != Some(account) {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(owner)?
            .write_all(account.as_bytes())?;
    }

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
            write(&dir, "a", &files(&[("a.ics", "A"), ("b.ics", "B")])),
            Ok(true)
        ));
        let mode = fs::metadata(&dir).map(|metadata| {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o777
        });
        assert_eq!(mode.ok(), Some(0o700));

        let written = fs::metadata(dir.join("a.ics")).and_then(|metadata| metadata.modified());
        fs::write(dir.join("notes.txt"), "mine").ok();

        assert!(matches!(
            write(&dir, "a", &files(&[("a.ics", "A")])),
            Ok(false)
        ));
        assert_eq!(
            fs::metadata(dir.join("a.ics"))
                .and_then(|metadata| metadata.modified())
                .ok(),
            written.ok()
        );
        assert!(!dir.join("b.ics").exists());
        assert!(dir.join("notes.txt").exists());
        assert_eq!(
            fs::read_to_string(dir.join(OWNER)).ok().as_deref(),
            Some("a")
        );

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

    // a scratch directory of the test's own
    fn scratch(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("kanade-google-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);

        dir
    }

    // a sign-in named by its token, which is its account too
    fn credentials(token: &str) -> Credentials {
        Credentials {
            account: String::from(token),
            client_id: String::from("id"),
            client_secret: String::from("secret"),
            refresh_token: String::from(token),
        }
    }

    // Google and the keyring as a test wants them
    #[derive(Default)]
    struct Fake {
        keyring: Option<Credentials>,

        // what `store` waits on, and says it is waiting through
        gate: Option<Receiver<()>>,
        storing: Option<Sender<()>>,

        // the tokens whose syncs fail, as offline
        failing: Vec<String>,

        // the keyring refuses to store, as when its prompt is dismissed
        refusing: bool,

        // the keyring does not unlock, so it gives nothing
        locked: bool,

        // where the events are, and whether a's showed each time they were vouched for
        events: Option<PathBuf>,
        vouched: Vec<bool>,
        revoked: Vec<String>,
        published: Vec<Account>,
    }

    #[derive(Clone, Default)]
    struct Shared {
        fake: std::sync::Arc<Mutex<Fake>>,
        sign_in: std::sync::Arc<AtomicU64>,
    }

    impl Shared {
        fn fake(&self) -> std::sync::MutexGuard<'_, Fake> {
            self.fake.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    impl Outside for Shared {
        fn load(&self) -> Result<Option<Credentials>, String> {
            let fake = self.fake();
            if fake.locked {
                return Err(String::from("the keyring was not unlocked"));
            }

            Ok(fake.keyring.clone())
        }

        fn store(&self, credentials: &Credentials) -> Result<(), String> {
            let (gate, storing) = {
                let mut fake = self.fake();
                (fake.gate.take(), fake.storing.take())
            };

            // a keyring prompt the user has not answered yet
            if let (Some(gate), Some(storing)) = (gate, storing) {
                let _ = storing.send(());
                let _ = gate.recv();
            }

            let mut fake = self.fake();
            if fake.refusing {
                return Err(String::from("the keyring was not unlocked"));
            }

            fake.keyring = Some(credentials.clone());
            Ok(())
        }

        fn delete(&self) -> Result<(), String> {
            self.fake().keyring = None;
            Ok(())
        }

        fn revoke(&self, credentials: &Credentials) {
            self.fake().revoked.push(credentials.refresh_token.clone());
        }

        fn fetch(
            &self,
            credentials: &Credentials,
            _: &mut Option<Access>,
        ) -> Result<Vec<(String, String)>, Problem> {
            if self.fake().failing.contains(&credentials.refresh_token) {
                return Err(Problem::Offline(String::from("timeout")));
            }

            Ok(vec![(
                format!("{}.ics", credentials.refresh_token),
                String::from("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n"),
            )])
        }

        fn sign_in(&self) -> u64 {
            self.sign_in.load(Ordering::Relaxed)
        }

        fn reread(&self) {}

        fn vouch(&self) {
            let mut fake = self.fake();
            let shown = fake
                .events
                .as_ref()
                .is_some_and(|dir| dir.join("a.ics").exists());
            fake.vouched.push(shown);
        }

        fn publish(&self, account: &Account) {
            self.fake().published.push(account.clone());
        }
    }

    // a worker on its own thread, over places in `dir`
    fn worker(shared: &Shared, dir: &Path) -> (Sender<Message>, thread::JoinHandle<Account>) {
        let (send, queue) = mpsc::channel();
        shared.fake().events = Some(dir.join("events"));
        let places = Places {
            events: Some(dir.join("events")),
            marker: Some(dir.join("state/marker")),
        };
        let shared = shared.clone();

        let running = thread::spawn(move || {
            let mut worker = Worker::new(shared, places);
            worker.run(&queue);
            worker.account
        });

        (send, running)
    }

    fn exchanged(
        send: &Sender<Message>,
        sign_in: u64,
        token: &str,
    ) -> Receiver<Result<(), String>> {
        let (told, heard) = mpsc::channel();
        let _ = send.send(Message::Exchanged {
            sign_in,
            credentials: credentials(token),
            kept: told,
        });

        heard
    }

    fn ended(send: Sender<Message>, running: thread::JoinHandle<Account>) -> Account {
        drop(send);
        running.join().unwrap_or_default()
    }

    #[test]
    fn the_callback_is_heard_while_the_opener_still_runs() {
        use std::io::Read;
        use std::net::TcpStream;

        let dir = scratch("opener");
        // an opener that stays, as xdg-open may for the browser's whole life
        let opener = dir.join("opener");
        let _ = fs::write(&opener, "sleep 3\n");

        let Ok(pending) = Pending::start(Client {
            id: String::from("id"),
            secret: String::from("secret"),
        }) else {
            panic!("cannot listen");
        };

        let started = Instant::now();
        launch("sh", &opener.to_string_lossy());
        assert!(started.elapsed() < Duration::from_secs(1));

        let field = |name: &str| {
            pending
                .address
                .split(['?', '&'])
                .find_map(|field| field.strip_prefix(name))
                .map(str::to_owned)
                .unwrap_or_default()
        };
        let state = field("state=");
        let port: u16 = field("redirect_uri=http%3A%2F%2F127%2E0%2E0%2E1%3A")
            .parse()
            .unwrap_or_default();

        let browser = thread::spawn(move || {
            let mut page = String::new();
            if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
                let _ = write!(stream, "GET /?state={state}&code=c HTTP/1.1\r\n\r\n");
                let _ = stream.read_to_string(&mut page);
            }
            page
        });

        let Ok(returned) = pending.wait(Instant::now() + Duration::from_secs(2), || false) else {
            panic!("the callback was not heard while the opener ran");
        };
        assert_eq!(returned.code, "c");
        assert!(started.elapsed() < Duration::from_secs(3));

        returned.tell(Ok(()));
        assert!(browser.join().unwrap_or_default().contains("is signed in"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sign_out_during_a_keyring_prompt_still_signs_out() {
        let dir = scratch("prompt");
        let shared = Shared::default();
        let (open, gate) = mpsc::channel();
        let (storing, store_started) = mpsc::channel();
        {
            let mut fake = shared.fake();
            fake.gate = Some(gate);
            fake.storing = Some(storing);
        }
        shared.sign_in.store(1, Ordering::Relaxed);

        let (send, running) = worker(&shared, &dir);
        let kept = exchanged(&send, 1, "a");

        // the keyring prompts; meanwhile the user signs out, as `request` does
        assert!(store_started.recv_timeout(Duration::from_secs(2)).is_ok());
        shared.sign_in.store(2, Ordering::Relaxed);
        let _ = send.send(Message::SignOut);
        let _ = open.send(());

        // signed out while the keyring prompted, so it is put back, not kept
        assert_eq!(
            kept.recv_timeout(Duration::from_secs(2)),
            Ok(Err(String::from("cancelled")))
        );
        let account = ended(send, running);

        assert_eq!(account.state, State::SignedOut);
        assert_eq!(shared.fake().keyring, None);
        assert!(!dir.join("state/marker").exists());
        assert!(!dir.join("events").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sign_in_cancelled_before_it_is_kept_is_not_kept() {
        let dir = scratch("cancelled");
        let shared = Shared::default();
        shared.sign_in.store(2, Ordering::Relaxed);

        let (send, running) = worker(&shared, &dir);
        let kept = exchanged(&send, 1, "a");

        assert_eq!(
            kept.recv_timeout(Duration::from_secs(2)),
            Ok(Err(String::from("cancelled")))
        );
        let account = ended(send, running);

        assert_eq!(account.state, State::SignedOut);
        assert_eq!(shared.fake().keyring, None);
        assert!(!dir.join("state/marker").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn another_account_never_shows_the_last_ones_events() {
        let dir = scratch("switch");
        let shared = Shared::default();
        {
            let mut fake = shared.fake();
            fake.keyring = Some(credentials("a"));
            fake.failing.push(String::from("b"));
        }
        let _ = fs::create_dir_all(dir.join("state"));
        let _ = fs::write(dir.join("state/marker"), "");
        shared.sign_in.store(1, Ordering::Relaxed);

        let (send, running) = worker(&shared, &dir);

        // a's first sync, at start, then b signs in and cannot sync
        let kept = exchanged(&send, 1, "b");
        assert_eq!(kept.recv_timeout(Duration::from_secs(2)), Ok(Ok(())));
        let account = ended(send, running);

        let fake = shared.fake();
        assert!(
            fake.published
                .iter()
                .any(|account| account.synced.is_some())
        );
        assert_eq!(fake.keyring, Some(credentials("b")));
        assert_eq!(
            account,
            Account {
                state: State::Failed(Problem::Offline(String::from("timeout"))),
                synced: None,
            }
        );
        assert!(!dir.join("events/a.ics").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sign_in_the_keyring_refuses_leaves_the_last_account_as_it_was() {
        let dir = scratch("refused");
        let shared = Shared::default();
        shared.fake().keyring = Some(credentials("a"));
        let _ = fs::create_dir_all(dir.join("state"));
        let _ = fs::write(dir.join("state/marker"), "");
        shared.sign_in.store(1, Ordering::Relaxed);

        let (send, running) = worker(&shared, &dir);
        let synced = loop {
            let synced = shared
                .fake()
                .published
                .iter()
                .find_map(|account| account.synced);
            if let Some(synced) = synced {
                break synced;
            }
            thread::sleep(Duration::from_millis(10));
        };

        // the whole sign-in to b, as `sign_in` and `signing_in` send it
        shared.fake().refusing = true;
        let _ = send.send(Message::Signing);
        let kept = exchanged(&send, 1, "b");
        let refused = kept.recv_timeout(Duration::from_secs(2));
        assert_eq!(
            refused,
            Ok(Err(String::from("the keyring was not unlocked")))
        );
        let _ = send.send(Message::Unsigned(
            1,
            String::from("the keyring was not unlocked"),
        ));
        let account = ended(send, running);

        assert_eq!(shared.fake().keyring, Some(credentials("a")));
        assert!(dir.join("events/a.ics").exists());
        assert_eq!(
            account,
            Account {
                state: State::SignedIn,
                synced: Some(synced),
            }
        );
        // a's state throughout, never b's sign-in
        assert!(
            shared
                .fake()
                .published
                .iter()
                .all(|account| account.state == State::SignedIn)
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_sync_keeps_the_same_accounts_events() {
        let dir = scratch("failed");
        let shared = Shared::default();
        shared.fake().keyring = Some(credentials("a"));
        let _ = fs::create_dir_all(dir.join("state"));
        let _ = fs::write(dir.join("state/marker"), "");

        let (send, running) = worker(&shared, &dir);
        while shared.fake().published.is_empty() {
            thread::sleep(Duration::from_millis(10));
        }
        // signed in from the load, before the first sync ends
        assert_eq!(
            shared.fake().published[0],
            Account {
                state: State::SignedIn,
                synced: None
            }
        );

        // after the first sync, Google goes away
        while !shared
            .fake()
            .published
            .iter()
            .any(|account| account.synced.is_some())
        {
            thread::sleep(Duration::from_millis(10));
        }
        shared.fake().failing.push(String::from("a"));
        let _ = send.send(Message::Sync);
        let account = ended(send, running);

        assert!(matches!(account.state, State::Failed(Problem::Offline(_))));
        assert!(account.synced.is_some());
        assert!(dir.join("events/a.ics").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    // events synced for `owner` in `dir`, and a sign-in kept for `kept`, as a crash left them
    fn crashed(dir: &Path, owner: &str, kept: &str) -> Shared {
        let shared = Shared::default();
        {
            let mut fake = shared.fake();
            fake.keyring = Some(credentials(kept));
            // Google is not there at the restart
            fake.failing = vec![String::from("a"), String::from("b")];
        }
        let _ = fs::create_dir_all(dir.join("state"));
        let _ = fs::write(dir.join("state/marker"), "");
        let _ = fs::create_dir_all(dir.join("events"));
        let _ = fs::write(dir.join("events/a.ics"), "A");
        let _ = fs::write(dir.join("events").join(OWNER), owner);

        shared
    }

    #[test]
    fn a_crash_before_a_sign_in_is_kept_leaves_the_accounts_events() {
        let dir = scratch("crash-before");
        let shared = crashed(&dir, "a", "a");

        let (send, running) = worker(&shared, &dir);
        let account = ended(send, running);

        assert!(dir.join("events/a.ics").exists());
        assert!(matches!(account.state, State::Failed(Problem::Offline(_))));
        // its own, so shown
        assert_eq!(shared.fake().vouched, [true]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crash_after_a_sign_in_is_kept_drops_the_last_accounts_events() {
        let dir = scratch("crash-after");
        let shared = crashed(&dir, "a", "b");

        let (send, running) = worker(&shared, &dir);
        let account = ended(send, running);

        // never shown: the calendar may read them only once they are gone
        assert_eq!(shared.fake().vouched, [false]);
        assert!(!dir.join("events/a.ics").exists());
        assert_eq!(
            account,
            Account {
                state: State::Failed(Problem::Offline(String::from("timeout"))),
                synced: None,
            }
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn events_that_name_no_sign_in_are_dropped() {
        let dir = scratch("unowned");
        let shared = crashed(&dir, "a", "a");
        let _ = fs::remove_file(dir.join("events").join(OWNER));

        let (send, running) = worker(&shared, &dir);
        ended(send, running);

        assert!(!dir.join("events/a.ics").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sign_in_superseded_during_a_keyring_prompt_leaves_the_account_as_it_was() {
        let dir = scratch("superseded");
        let shared = Shared::default();
        shared.fake().keyring = Some(credentials("a"));
        let _ = fs::create_dir_all(dir.join("state"));
        let _ = fs::write(dir.join("state/marker"), "");
        shared.sign_in.store(1, Ordering::Relaxed);

        let (send, running) = worker(&shared, &dir);
        let synced = loop {
            let synced = shared
                .fake()
                .published
                .iter()
                .find_map(|account| account.synced);
            if let Some(synced) = synced {
                break synced;
            }
            thread::sleep(Duration::from_millis(10));
        };

        // b's store waits on the keyring's prompt
        let (open, gate) = mpsc::channel();
        let (storing, store_started) = mpsc::channel();
        {
            let mut fake = shared.fake();
            fake.gate = Some(gate);
            fake.storing = Some(storing);
        }
        let _ = send.send(Message::Signing);
        let b = exchanged(&send, 1, "b");
        assert!(store_started.recv_timeout(Duration::from_secs(2)).is_ok());

        // c starts, as `sign_in` does, then b's prompt is answered, then c fails
        shared.sign_in.store(2, Ordering::Relaxed);
        let _ = send.send(Message::Signing);
        let _ = open.send(());
        assert_eq!(
            b.recv_timeout(Duration::from_secs(2)),
            Ok(Err(String::from("cancelled")))
        );
        let _ = send.send(Message::Unsigned(2, String::from("access was not granted")));
        let account = ended(send, running);

        assert_eq!(shared.fake().keyring, Some(credentials("a")));
        assert!(dir.join("events/a.ics").exists());
        assert!(dir.join("state/marker").exists());
        assert_eq!(
            account,
            Account {
                state: State::SignedIn,
                synced: Some(synced),
            }
        );
        assert!(
            shared
                .fake()
                .published
                .iter()
                .all(|account| account.state == State::SignedIn)
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crash_in_a_sign_out_drops_the_events_before_they_show() {
        let dir = scratch("crash-sign-out");
        // the keyring and the marker went, the events did not
        let shared = crashed(&dir, "a", "a");
        shared.fake().keyring = None;
        let _ = fs::remove_file(dir.join("state/marker"));

        let (send, running) = worker(&shared, &dir);
        let account = ended(send, running);

        assert_eq!(shared.fake().vouched, [false]);
        assert!(!dir.join("events").exists());
        assert_eq!(account.state, State::SignedOut);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn events_stay_hidden_while_the_keyring_cannot_say_whose_they_are() {
        let dir = scratch("locked");
        let shared = crashed(&dir, "a", "b");
        shared.fake().locked = true;

        let (send, running) = worker(&shared, &dir);
        while shared.fake().published.is_empty() {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(shared.fake().vouched.is_empty());
        assert!(dir.join("events/a.ics").exists());

        // unlocked on `sync`: b's, so a's go before they show
        shared.fake().locked = false;
        let _ = send.send(Message::Sync);
        ended(send, running);

        assert_eq!(shared.fake().vouched, [false]);
        assert!(!dir.join("events/a.ics").exists());

        let _ = fs::remove_dir_all(&dir);
    }
}
