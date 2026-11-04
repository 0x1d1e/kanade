//! polkit's authentication agent: the password prompt polkit's request shows on the Island.
//!
//! polkit's own helpers run as root, so they are where the password ends up: Kanade's part is
//! only where it is typed, on the Island, in a Surface that holds the keyboard and that nothing
//! but its own answer or the session lock closes (ADR 0146). The password goes to polkit's stock
//! helper (`/usr/lib/polkit-1/polkit-agent-helper-1`, socket-activated at
//! `/run/polkit/agent-helper.socket`), which runs PAM as root and answers polkit itself. Kanade
//! never holds a privilege and registers no action (ADR 0120): `AuthenticationAgentResponse{,2,3}`
//! is refused to anything but uid 0, and no polkit-1 of ours is registered.
//!
//! The helper's words on the socket are its protocol, not Kanade's: `<user>` then `<cookie>`, a
//! line each, then a line per PAM message (`PAM_PROMPT_ECHO_OFF`, `PAM_PROMPT_ECHO_ON`,
//! `PAM_ERROR_MSG`, `PAM_TEXT_INFO`, `g_strescape`d text) until `SUCCESS` or `FAILURE`. A
//! `FAILURE` keeps the request open for another try; `SUCCESS` means the helper has answered
//! polkit already, and nothing more is said.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::MutexExt;
use kanade_runtime::service::{self, Service};
use zeroize::Zeroizing;

use super::secret::Secret;
use crate::backoff::Backoff;
use crate::bus::{Argument, Bus, Method, MethodEvent, Value};
use crate::clock::{self, WallTimer};
use crate::island::presentation::Surface;
use crate::island::service::IslandService;
use crate::lock;
use crate::supervise;

// polkit's helper, systemd socket-activated: it answers polkit itself, running PAM as root, so
// Kanade never holds one
const HELPER: &str = "/run/polkit/agent-helper.socket";

const AUTHORITY: &str = "org.freedesktop.PolicyKit1";
const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const AUTHORITY_IFACE: &str = "org.freedesktop.PolicyKit1.Authority";

// the object polkit calls to ask and to cancel, and its interface
const AGENT: &str = "/org/kanade/polkit/agent";
const AGENT_IFACE: &str = "org.freedesktop.PolicyKit1.AuthenticationAgent";

// the bus itself: the only sender of NameOwnerChanged
const DBUS: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";

// how long the prompt waits for an answer before giving up, while nothing is typed
const LONGEST: Duration = Duration::from_secs(1800);

// how long registration waits before trying polkit again, doubling after each refusal
const RETRY_FIRST: Duration = Duration::from_millis(250);
const RETRY_LONGEST: Duration = Duration::from_secs(30);

// three tries a request gets before polkit reads it as not authorized
const TRIES: usize = 3;

// at most as many requests may wait: one shown, three behind it
const MOST: usize = 4;

// how long a message or note may be before it is cut and elided: both say what outside programs
// said, and what they said is not trusted with the screen
const MOST_MESSAGE: usize = 120;

// at most as many characters a password may hold here, far more than PAM itself takes
const MOST_SECRET: usize = 256;

// the session polkit is asked for and the user whose password is typed: the caller's own, as
// logind names it, never a path or another user
struct Caller {
    session: String,
    uid: u32,
    user: String,
}

// one request polkit waits on: its cookie and words, whose password is asked, and how many tries
// are left before nothing is answered
struct Request {
    cookie: String,
    message: String,
    user: String,
    tries: usize,

    // what polkit reads as the answer, whatever this request's fate
    answer: Option<Box<dyn FnOnce() + Send>>,
}

// every request waiting: the one shown, then the ones behind it
static PENDING: Mutex<Vec<Request>> = Mutex::new(Vec::new());

// the visit the prompt was opened in, none while it is not on screen
static SHOWN: Mutex<Option<u64>> = Mutex::new(None);

// the moment the prompt gives up waiting, none while nothing waits
static GIVES_UP: Mutex<Option<i64>> = Mutex::new(None);

// Queue, prompt, visit and helper transitions must finish together across input, bus and workers.
static LIFECYCLE: Mutex<()> = Mutex::new(());

#[cfg(test)]
static INPUT_CHECKED: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

// Always acquired before lock::Requests. Lock onset, input mutations and helper password writes
// share this guard: cancellation is complete before pending lock or a compositor request exists.
// No Service is touched with the polkit Module cold and no Auth state to cancel.
pub(crate) struct LockOnset {
    _lifecycle: MutexGuard<'static, ()>,
}

pub(crate) fn lock_onset() -> LockOnset {
    let guard = LockOnset {
        _lifecycle: LIFECYCLE.locked(),
    };
    if !PENDING.locked().is_empty() || ATTEMPTING.locked().is_some() {
        reset_all();
    }
    guard
}

// Each submission has its own identity, including before its worker connects the helper.
struct Attempt {
    id: u64,
    socket: Option<UnixStream>,
}
static NEXT_ATTEMPT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static ATTEMPTING: Mutex<Option<Attempt>> = Mutex::new(None);

/*
 * the bus generation and polkitd unique name this agent is registered with: a new bus can reuse
 * an old unique name, while polkit refuses a second agent for one subject on the same bus
 */
static REGISTERING: Mutex<()> = Mutex::new(());
static REGISTERED: Mutex<Option<(u64, String)>> = Mutex::new(None);

// the timer that lets nothing wait forever, none when the machine has no timerfd
static TIMER: OnceLock<Option<Arc<WallTimer>>> = OnceLock::new();

// what polkit asks, whose password is typed, what is typed, what failed, whether the helper runs
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prompt {
    message: String,
    user: String,
    secret: Secret,
    note: Option<String>,
    checking: bool,
}

impl Prompt {
    // what polkit's words say, cut to one line
    pub fn message(&self) -> &str {
        &self.message
    }

    // whose password is typed
    pub fn user(&self) -> &str {
        &self.user
    }

    // what is typed of it, which never prints
    pub fn secret(&self) -> &Secret {
        &self.secret
    }

    // why the last try failed, if anything
    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }
}

impl Service for Prompt {
    fn new() -> Self {
        Self::default()
    }
}

// start answering polkit: on the system bus, where polkit itself is, and as the lock engages,
// nothing waits any more
pub fn start() {
    service::watch::<lock::Said>(lock_changed);

    service::watch::<IslandService>(island_changed);

    supervise::spawn("polkit agent", move || {
        let mut methods = Bus::system().methods_with_reconnect(AGENT, AGENT_IFACE);
        let mut backoff = Backoff::new(RETRY_FIRST, RETRY_LONGEST);
        let mut said: Option<String> = None;

        for event in &mut methods {
            match event {
                /*
                 * nothing can reach the agent before polkit takes it, so wait here rather than
                 * read calls it cannot send yet; a refusal may be polkitd not up, or one that
                 * still holds an agent for this subject. A retry may outlive this subscription's
                 * bus, so use the live bus generation; the cache decides whether it is registered.
                 */
                MethodEvent::Attached(_, mut generation) => loop {
                    // A retry may outlive this subscription's bus, so use the bus it will talk to.
                    generation = Bus::system().generation().unwrap_or(generation);
                    match register_attached(generation) {
                        Ok(()) => {
                            backoff.reset();
                            said = None;
                            break;
                        }
                        Err(why) => {
                            if said.as_ref() != Some(&why) {
                                eprintln!("kanade: polkit agent not registered: {why}");
                                said = Some(why);
                            }
                            thread::sleep(backoff.failed());
                        }
                    }
                },
                MethodEvent::Method(method) => answer(method),
            }
        }
    });

    // a new polkitd has none of the agents the old one had: it is answered again under it
    supervise::spawn("polkit watcher", move || {
        let mut owners = Bus::system().signals_from(DBUS_PATH, DBUS, "NameOwnerChanged");

        for signal in &mut owners {
            let [name, _, new] = signal.arguments() else {
                continue;
            };

            if name.text() == AUTHORITY
                && !new.text().is_empty()
                && let Err(why) = register(Bus::system().generation())
            {
                eprintln!("kanade: polkit agent not registered: {why}");
            }
        }
    });

    // nothing waits forever
    if let Ok(timer) = WallTimer::new() {
        let timer = Arc::new(timer);

        if TIMER.set(Some(timer.clone())).is_ok() {
            supervise::spawn("polkit timer", move || timer_loop(timer.clone()));
        }
    }
}

// a character typed into the password, and one erased
pub fn type_char(letter: char) {
    edit(Some(letter));
}

pub fn erase() {
    edit(None);
}

// Enter pressed: the typed password goes where polkit's helper reads it, one try at a time
pub fn submit() {
    let _lifecycle = LIFECYCLE.locked();
    if lock::holds() {
        reset_all();
        return;
    }
    if !showing() {
        return;
    }

    let (cookie, user) = {
        let queue = PENDING.locked();

        match queue.first() {
            Some(request) => (request.cookie.clone(), request.user.clone()),
            None => return,
        }
    };

    let password = {
        let mut prompt = Prompt::write();

        if prompt.checking || prompt.secret.is_empty() {
            return;
        }

        prompt.checking = true;
        prompt.note = None;

        Zeroizing::new(prompt.secret.plaintext().to_owned())
    };

    let id = NEXT_ATTEMPT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    *ATTEMPTING.locked() = Some(Attempt { id, socket: None });
    if thread::Builder::new()
        .name("kanade polkit".into())
        .spawn(move || attempt(id, &cookie, &user, &password))
        .is_err()
    {
        reset_all();
    }
}

// Escape pressed: polkit is told nothing is answered, and the prompt closes
pub fn cancel() {
    cancel_head();
}

// one letter typed or erased: nothing but what a key types fits, and a control character is never
// part of a password
fn edit(letter: Option<char>) {
    let _lifecycle = LIFECYCLE.locked();
    if lock::holds() {
        reset_all();
        return;
    }
    #[cfg(test)]
    if let Some(checked) = INPUT_CHECKED.locked().take() {
        checked();
    }
    if !showing() {
        return;
    }
    let mut prompt = Prompt::write();

    if prompt.checking {
        return;
    }

    let next = typed(&prompt.secret, letter);

    if next == prompt.secret {
        return;
    }

    prompt.secret = next;

    drop(prompt);
    refresh_timer(true);
}

// what polkit reads, whatever this request's fate
fn answer(method: Method) {
    if !from_polkit(&method.sender()) {
        return;
    }

    match method.name() {
        "BeginAuthentication" => begin(method),

        // one cookie cancelled: polkit itself reads the void reply as not authorized
        "CancelAuthentication" => {
            let cookie = match method.arguments().len() {
                1 | 2 => method
                    .arguments()
                    .first()
                    .map_or("", Value::text)
                    .to_owned(),
                _ => String::new(),
            };

            cancel_cookie(&cookie);
            method.reply(&[]);
        }

        _ => method.fail(
            "org.freedesktop.DBus.Error.UnknownMethod",
            "no such method on Kanade's authentication agent",
        ),
    }
}

// one request to answer: its cookie, what polkit asks, whose password, and three tries
fn begin(method: Method) {
    let arguments = method.arguments();

    let (cookie, message, identities) = layout(arguments);
    let cookie = cookie.to_owned();

    let Some(caller) = caller() else {
        method.reply(&[]);
        return;
    };

    // whose password: the first unix-user polkit offers, when it is the caller's own session user
    let user = identity(identities)
        .filter(|uid| caller.uid == *uid)
        .map(|_| caller.user.clone());

    let _lifecycle = LIFECYCLE.locked();
    let queued = PENDING.locked().len();

    // the things a request is refused outright for: nothing is shown, and polkit reads the void
    // reply as not authorized
    let refused = refuses(
        lock::holds(),
        Path::new(HELPER).exists(),
        user.as_deref(),
        queued,
    );

    // a malformed call, which polkit reads as invalid rather than waiting
    if cookie.is_empty() {
        method.fail(
            "org.freedesktop.DBus.Error.InvalidArgs",
            "no cookie to answer",
        );
        return;
    }

    if refused.is_some() {
        method.reply(&[]);
        return;
    }

    // the same cookie twice: polkit repeated itself, or two callers at once
    if PENDING
        .locked()
        .iter()
        .any(|request| request.cookie == cookie)
    {
        method.reply(&[]);
        return;
    }

    // what polkit reads as the answer, when anything ends this request
    let answer: Box<dyn FnOnce() + Send> = Box::new(move || method.reply(&[]));

    PENDING.locked().push(Request {
        cookie,
        message,
        user: user.unwrap(),
        tries: TRIES,
        answer: Some(answer),
    });

    if queued == 0 {
        advance();
    }
}

// BeginAuthentication's arguments, polkit's `(sss@a{ss}sa(sa{sv}))`: the message second, the
// cookie fifth and the identities sixth
fn layout(arguments: &[Value]) -> (&str, String, &Value) {
    (
        arguments.get(4).map_or("", Value::text),
        clamp(arguments.get(1).map_or("", Value::text), MOST_MESSAGE),
        arguments.get(5).unwrap_or(&Value::Nothing),
    )
}

// the cookie polkit cancels, any one of them
fn cancel_cookie(cookie: &str) {
    let _lifecycle = LIFECYCLE.locked();
    let (taken, head) = {
        let mut queue = PENDING.locked();
        let Some(at) = queue.iter().position(|request| request.cookie == cookie) else {
            return;
        };

        (queue.remove(at), at == 0)
    };

    if head {
        shut_socket();
    }
    taken.answer();
    if head {
        advance();
    }
}

// the request at the head cancelled: polkit reads nothing answered
fn cancel_head() {
    let _lifecycle = LIFECYCLE.locked();
    shut_socket();
    let taken = {
        let mut queue = PENDING.locked();
        (!queue.is_empty()).then(|| queue.remove(0))
    };

    if let Some(request) = taken {
        request.answer();
    }
    advance();
}

// why a request is refused outright: nothing is shown, and polkit reads the void reply as not
// authorized
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    Locked,

    // no helper socket to give the password to
    NoHelper,

    // no unix-user identity, or none that is this session's own user
    NoUser,

    // as many requests waiting as there may be
    Full,
}

fn refuses(locked: bool, helper: bool, user: Option<&str>, queued: usize) -> Option<Refusal> {
    if locked {
        return Some(Refusal::Locked);
    }

    if !helper {
        return Some(Refusal::NoHelper);
    }

    if user.is_none() {
        return Some(Refusal::NoUser);
    }

    if queued >= MOST {
        return Some(Refusal::Full);
    }

    None
}

// what polkit reads as the answer
impl Request {
    fn answer(mut self) {
        if let Some(answer) = self.answer.take() {
            answer();
        }
    }
}

// the one shown, or nothing: the Surface's own words follow
fn advance() {
    if lock::holds() {
        reset_all();
        return;
    }
    let head = {
        let queue = PENDING.locked();

        queue.first().map(|request| {
            (
                request.cookie.clone(),
                request.message.clone(),
                request.user.clone(),
            )
        })
    };

    let showing = showing();

    match head {
        Some((_cookie, message, user)) => {
            let prompt = Prompt {
                message,
                user,
                secret: Secret::default(),
                note: None,
                checking: false,
            };

            set_prompt(prompt);
            refresh_timer(true);

            if showing {
                let island = IslandService::read();

                if island.surface() == Some(Surface::Auth) {
                    *SHOWN.locked() = Some(island.visit());
                }
            } else {
                open_prompt();
            }
        }
        None => {
            set_prompt(Prompt::default());
            *SHOWN.locked() = None;
            refresh_timer(false);
            close_prompt();
        }
    }
}

fn showing() -> bool {
    let visit = *SHOWN.locked();
    visit.is_some_and(|visit| {
        let island = IslandService::read();
        island.surface() == Some(Surface::Auth) && island.visit() == visit
    })
}

// the prompt is over: collapse the Surface it shows, which nothing but its own end opens again
fn close_prompt() {
    IslandService::write().displace(Surface::Auth, Instant::now());
}

// where the prompt is shown, opened over the focused output and held until it ends
fn open_prompt() {
    if lock::holds() {
        reset_all();
        return;
    }
    // The existing Island watcher retries when focus arrives or overview ends. A hidden
    // prompt must never hold the keyboard, and no other output stands in for missing focus.
    let mut island = IslandService::write();
    if island.overview() {
        island.quiet();
        return;
    }
    if island.surface() == Some(Surface::Auth) {
        *SHOWN.locked() = Some(island.visit());
        island.quiet();
        return;
    }
    let Some(monitor) = island.confirmed_output().map(str::to_owned) else {
        island.quiet();
        return;
    };

    island.open(&monitor, Surface::Auth, Instant::now());
    if island.surface() == Some(Surface::Auth) {
        *SHOWN.locked() = Some(island.visit());
    }
}

// the island changed what it shows: a prompt shown is closed by anything but its own end, and one
// never shown is tried again
fn island_changed() {
    let _lifecycle = LIFECYCLE.locked();
    if lock::holds() {
        reset_all();
        return;
    }
    if PENDING.locked().is_empty() {
        return;
    }

    let showing = showing();

    let shown = *SHOWN.locked();
    match (showing, shown) {
        (true, _) => {}

        // the user closed the prompt, or anything else did: nothing waits any more
        (false, Some(_)) => reset_all(),

        (false, None) => open_prompt(),
    }
}

fn lock_changed() {
    if lock::holds() {
        give_up_all();
    }
}

// everything waiting is given up on: the lock engages, or the island collapses the prompt itself
fn give_up_all() {
    let _lifecycle = LIFECYCLE.locked();
    reset_all();
}

fn reset_all() {
    let taken = std::mem::take(&mut *PENDING.locked());
    shut_socket();

    for request in taken {
        request.answer();
    }

    *SHOWN.locked() = None;
    set_prompt(Prompt::default());
    refresh_timer(false);
    if IslandService::read().surface() == Some(Surface::Auth) {
        close_prompt();
    }
}

// the helper's socket of the check under way is shut, so a PAM stack waiting on an answer ends now
fn shut_socket() {
    if let Some(attempt) = ATTEMPTING.locked().take()
        && let Some(socket) = attempt.socket
    {
        let _ = socket.shutdown(Shutdown::Both);
    }
}

// the prompt's own state changed, so the windows that show it draw again
fn set_prompt(prompt: Prompt) {
    if *Prompt::read() != prompt {
        *Prompt::write() = prompt;
    }
}

// the idle window restarts while the user types, and ends when nothing waits
fn refresh_timer(arm: bool) {
    if !arm {
        *GIVES_UP.locked() = None;
        return;
    }
    let Some(timer) = TIMER.get().and_then(Option::as_ref) else {
        return;
    };

    let at = clock::unix_seconds() + LONGEST.as_secs() as i64;

    *GIVES_UP.locked() = Some(at);
    let _ = timer.set(at);
}

fn timer_loop(timer: Arc<WallTimer>) {
    loop {
        if timer.wait().is_err() {
            return;
        }

        let now = clock::unix_seconds();
        let at = *GIVES_UP.locked();

        match at {
            Some(at) if now >= at => {
                *GIVES_UP.locked() = None;
                expired();
            }
            Some(at) => {
                // the clock was set back: wait again at the moment
                let _ = timer.set(at);
            }
            None => {}
        }
    }
}

// the user stopped answering: the helper's socket is shut, polkit reads the void reply as not
// authorized, and the next waiting request shows
fn expired() {
    cancel_head();
}

// one try's fate: the helper answers polkit itself on SUCCESS, and FAILURE keeps the request open
fn settle(id: u64, cookie: &str, outcome: Result<(), String>) {
    let _lifecycle = LIFECYCLE.locked();
    if lock::holds() {
        reset_all();
        return;
    }
    if !ATTEMPTING
        .locked()
        .as_ref()
        .is_some_and(|attempt| attempt.id == id)
    {
        return;
    }
    shut_socket();
    let mut answered = None;
    let mut note = None;
    let mut done = false;

    {
        let mut queue = PENDING.locked();
        let Some(at) = queue.iter().position(|request| request.cookie == cookie) else {
            return;
        };

        match outcome {
            Ok(()) => {
                answered = Some(queue.remove(at));
                done = true;
            }
            Err(why) => {
                let request = &mut queue[at];
                request.tries = request.tries.saturating_sub(1);

                if request.tries == 0 {
                    answered = Some(queue.remove(at));
                    done = true;
                } else {
                    note = Some((why, request.tries));
                }
            }
        }
    }

    if let Some(request) = answered {
        request.answer();
    }

    if done {
        advance();
    } else if let Some((why, tries)) = note {
        retry(why, tries);
    }
}

// a try failed: the field empties, the note says so with how many tries are left, and the idle
// window restarts
fn retry(why: String, tries: usize) {
    let mut prompt = Prompt::write();
    prompt.checking = false;
    prompt.secret = Secret::default();
    prompt.note = Some(format!(
        "{why}, {tries} tr{} left",
        if tries == 1 { "y" } else { "ies" }
    ));
    drop(prompt);

    refresh_timer(true);
}

// the typed password goes to the helper, which answers polkit itself
fn attempt(id: u64, cookie: &str, user: &str, password: &Zeroizing<String>) {
    let outcome = check(id, user, cookie, password);

    settle(id, cookie, outcome);
}

// the helper's run: the user, then the cookie, then one PAM message at a time until it answers
fn check(id: u64, user: &str, cookie: &str, password: &Zeroizing<String>) -> Result<(), String> {
    let Ok(stream) = UnixStream::connect(HELPER) else {
        return Err(String::from("the polkit helper is not running"));
    };

    converse(id, user, cookie, password, stream)
}

fn converse(
    id: u64,
    user: &str,
    cookie: &str,
    password: &Zeroizing<String>,
    stream: UnixStream,
) -> Result<(), String> {
    // one stream read and one written, so shutting one does not close the other
    let Ok(writer_stream) = stream.try_clone() else {
        return Err(String::from("the polkit helper took no request"));
    };

    let Ok(attempting) = writer_stream.try_clone() else {
        return Err(String::from("the polkit helper took no request"));
    };

    {
        let _lifecycle = LIFECYCLE.locked();
        if lock::holds() {
            reset_all();
            return Err(String::from("authentication cancelled"));
        }
        let mut active = ATTEMPTING.locked();
        let Some(active) = active.as_mut().filter(|attempt| attempt.id == id) else {
            return Err(String::from("authentication cancelled"));
        };
        active.socket = Some(attempting);
    }

    let mut reader = BufReader::new(stream);
    let mut writer = writer_stream;
    // A helper that stops reading must not indefinitely delay lock onset under LIFECYCLE.
    writer
        .set_write_timeout(Some(Duration::from_millis(250)))
        .map_err(|_| String::from("the polkit helper took no request"))?;

    // who is asked, and which request is answered: the cookie polkit gave
    if writeln!(writer, "{user}")
        .and_then(|()| writeln!(writer, "{cookie}"))
        .is_err()
    {
        return Err(String::from("the polkit helper took no request"));
    }

    let mut line = String::new();

    loop {
        line.clear();

        match reader.read_line(&mut line) {
            Ok(0) => return Err(String::from("the polkit helper ended")),
            Err(error) => return Err(format!("the polkit helper said nothing ({error})")),
            Ok(_) => {}
        }

        let Some(message) = message_of(&line) else {
            continue;
        };

        match message {
            Message::Prompt { .. } => {
                let _lifecycle = LIFECYCLE.locked();
                if lock::holds() {
                    reset_all();
                    return Err(String::from("authentication cancelled"));
                }
                if !ATTEMPTING
                    .locked()
                    .as_ref()
                    .is_some_and(|attempt| attempt.id == id)
                {
                    return Err(String::from("authentication cancelled"));
                }
                // one field: every prompt takes what was typed, echo or not
                let mut typed = Zeroizing::new(password.as_str().to_owned());
                typed.push('\n');

                if writer.write_all(typed.as_bytes()).is_err() {
                    return Err(String::from("the polkit helper took no answer"));
                }
            }
            Message::Said(_) => {}
            Message::Done => return Ok(()),
            Message::Failed => return Err(String::from("Wrong password")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Message {
    Prompt { echo: bool },
    Said(String),
    Done,
    Failed,
}

// one line the helper wrote: its keyword, then what `g_strescape` made of the text
fn message_of(line: &str) -> Option<Message> {
    let line = line.trim_end_matches(['\n', '\r']);

    if line == "SUCCESS" {
        return Some(Message::Done);
    }

    if line == "FAILURE" {
        return Some(Message::Failed);
    }

    let (keyword, text) = line.split_once(' ')?;
    let text = unescape(text);

    match keyword {
        "PAM_PROMPT_ECHO_OFF" => Some(Message::Prompt { echo: false }),
        "PAM_PROMPT_ECHO_ON" => Some(Message::Prompt { echo: true }),
        "PAM_ERROR_MSG" | "PAM_TEXT_INFO" => Some(Message::Said(text)),
        _ => None,
    }
}

// `g_strescape`'s escape undone: `\n` and its kin, `\NNN` octal, and anything else given back
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(letter) = chars.next() {
        if letter != '\\' {
            out.push(letter);
            continue;
        }

        match chars.next() {
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('v') => out.push('\u{b}'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some(digit @ '0'..='7') => {
                let mut value = digit.to_digit(8).unwrap();
                let mut steps = 0;

                while steps < 2 {
                    let Some(next) = chars.peek().copied().and_then(|next| next.to_digit(8)) else {
                        break;
                    };

                    chars.next();
                    value = value * 8 + next;
                    steps += 1;
                }

                // `\0` ends the string, as in C: nothing after it is read
                if value == 0 {
                    break;
                }

                if let Some(letter) = char::from_u32(value) {
                    out.push(letter);
                }
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }

    out
}

// a letter added, or the last one erased: nothing but what a key types fits
fn typed(secret: &Secret, letter: Option<char>) -> Secret {
    let mut typed = secret.clone();

    match letter {
        Some(letter) if !letter.is_control() && typed.len() < MOST_SECRET => typed.push(letter),
        Some(_) => {}
        None => {
            typed.pop();
        }
    }

    typed
}

// `text` as one line of at most `most` characters: control characters and those that reorder text
// become spaces, runs of space one, and a longer line ends in an ellipsis
fn clamp(text: &str, most: usize) -> String {
    let spaced: String = text
        .chars()
        .map(|letter| {
            if letter.is_control()
                || matches!(letter, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                ' '
            } else {
                letter
            }
        })
        .collect();

    let line = spaced.split_whitespace().collect::<Vec<_>>().join(" ");

    if line.chars().count() <= most {
        return line;
    }

    line.chars()
        .take(most - 1)
        .chain(std::iter::once('\u{2026}'))
        .collect()
}

// whether polkit's subject runs as the caller's own session user
fn caller() -> Option<Caller> {
    let session = Bus::system().property(LOGIND, AUTO, "org.freedesktop.login1.Session", "Id");

    let session = session.text().to_owned();

    if session.is_empty() {
        return None;
    }

    let user_value = Bus::system().property(LOGIND, AUTO, "org.freedesktop.login1.Session", "User");

    let user = user_value.list();

    let uid = user.first().map_or(0.0, Value::number) as u32;
    let user_path = user.get(1).map_or("", Value::text);

    if user_path.is_empty() {
        return None;
    }

    let name = Bus::system()
        .property(LOGIND, user_path, "org.freedesktop.login1.User", "Name")
        .text()
        .to_owned();

    (!name.is_empty()).then_some(Caller {
        session,
        uid,
        user: name,
    })
}

// whether a call on the agent's path came from polkit itself: anyone on the bus can name it,
fn from_polkit(sender: &str) -> bool {
    let owner = crate::sources::bus::name_owner(Bus::system(), AUTHORITY);

    !owner.is_empty() && owner == sender
}

// the uid of the first unix-user identity polkit offers: a session or a group is not a user;
// polkit's `(sa{sv})` is one struct: the kind word, then the dict whose `uid` names the user
fn identity(identities: &Value) -> Option<u32> {
    // `a(sa{sv})`: the argument is an array, each element one identity's `(sa{sv})`
    for identity in identities.list() {
        let fields = identity.list();

        if fields.first().map_or("", Value::text) != "unix-user" {
            continue;
        }

        if let Some(Value::Map(map)) = fields.get(1) {
            return Some(map.get("uid").map_or(0.0, Value::number) as u32);
        }
    }

    None
}

// Kanade registers where polkit asks: the agent's object, and the caller's session as the subject.
// `Ok` once the polkitd running now has it, including when a try already under way took it
fn register(generation: Option<u64>) -> Result<(), String> {
    let _turns = REGISTERING.locked();
    register_locked(generation)
}

fn register_attached(generation: u64) -> Result<(), String> {
    let _turns = REGISTERING.locked();
    register_locked(Some(generation))
}

fn register_locked(generation: Option<u64>) -> Result<(), String> {
    let generation = generation.ok_or_else(|| String::from("no live system bus connection"))?;
    let Some(caller) = caller() else {
        return Err(String::from("logind names no session to register"));
    };

    let subject = Argument::Struct(vec![
        Argument::from("unix-session"),
        Argument::Map(BTreeMap::from([(
            String::from("session-id"),
            Argument::from(caller.session.as_str()),
        )])),
    ]);

    // what the agent understands: nothing but the locale it runs in
    let capabilities = std::env::var_os("LANG")
        .map_or(String::from("C"), |lang| lang.to_string_lossy().to_string());

    let owner = crate::sources::bus::name_owner(Bus::system(), AUTHORITY);

    if owner.is_empty() {
        return Err(String::from("polkitd is not running"));
    }

    if REGISTERED
        .locked()
        .as_ref()
        .is_some_and(|(registered_generation, registered_owner)| {
            *registered_generation == generation && registered_owner == &owner
        })
    {
        return Ok(());
    }

    Bus::system()
        .try_call(
            AUTHORITY,
            AUTHORITY_PATH,
            AUTHORITY_IFACE,
            "RegisterAuthenticationAgent",
            &[
                subject,
                Argument::from(capabilities.as_str()),
                Argument::from(AGENT),
            ],
        )
        .map(|_| {
            *REGISTERED.locked() = Some((generation, owner));
            eprintln!("kanade: polkit agent registered for this session");
        })
}

const LOGIND: &str = "org.freedesktop.login1";
const AUTO: &str = "/org/freedesktop/login1/session/auto";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_placement_and_cancellation_regressions() {
        let mut failed = Vec::new();
        for case in [
            "umbriel",
            "niri",
            "deferred",
            "overview",
            "cancel-last",
            "cancel-head",
            "cancel-tail",
            "escape",
            "outcomes",
            "lock",
            "lock-deferred",
            "lock-input",
            "lock-submit",
            "lock-result",
            "same-cookie",
            "overview-direct",
            "displaced",
            "helper-lock",
            "helper-cancel",
            "helper-answer",
            "lock-edit-race",
            "producer-missing",
            "producer-stale",
            "producer-overview",
            "producer-cached",
            "producer-window",
            "producer-workspace",
        ] {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "sources::polkit::tests::auth_fixture",
                    "--nocapture",
                ])
                .env("KANADE_AUTH_FIXTURE", case)
                .status()
                .unwrap();
            if !status.success() {
                failed.push(case);
            }
        }
        assert!(failed.is_empty(), "failed Auth fixtures: {failed:?}");
    }

    // Private subprocess: no agent registration, stock helper, PAM or live shell.
    #[test]
    fn auth_fixture() {
        use kanade_runtime::Keyboard;
        use std::io::Read;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let Ok(case) = std::env::var("KANADE_AUTH_FIXTURE") else {
            return;
        };
        let answers = Arc::new(AtomicUsize::new(0));
        let enqueue = |cookie: &str| {
            let answers = answers.clone();
            PENDING.locked().push(Request {
                cookie: cookie.into(),
                message: cookie.into(),
                user: "fixture".into(),
                tries: TRIES,
                answer: Some(Box::new(move || {
                    answers.fetch_add(1, Ordering::SeqCst);
                })),
            });
        };
        let focus = |output: Option<&str>, overview| {
            IslandService::write().set_niri(output.map(str::to_owned), overview, Instant::now());
        };
        let visible = || {
            let island = IslandService::read();
            assert_eq!(island.surface(), Some(Surface::Auth));
            assert_eq!(island.expanded_on(), Some("selected"));
            assert_eq!(island.keyboard("selected"), Keyboard::Exclusive);
            assert_eq!(*SHOWN.locked(), Some(island.visit()));
        };
        let cleared = || {
            assert!(PENDING.locked().is_empty());
            assert_eq!(*Prompt::read(), Prompt::default());
            assert!(SHOWN.locked().is_none());
            assert!(ATTEMPTING.locked().is_none());
            assert!(GIVES_UP.locked().is_none());
            let island = IslandService::read();
            assert_ne!(island.surface(), Some(Surface::Auth));
            assert_eq!(island.keyboard("selected"), Keyboard::None);
        };
        let active = |id, socket| {
            *ATTEMPTING.locked() = Some(Attempt { id, socket });
        };
        service::watch::<IslandService>(island_changed);
        service::watch::<lock::Said>(lock_changed);
        if case.starts_with("producer-") {
            use super::super::umbriel::{self, Umbriel};
            service::watch::<Umbriel>(umbriel::publish_island);
            let event = |event| {
                umbriel::handle_event(&event);
                service::derive();
            };
            if case == "producer-stale" || case == "producer-cached" {
                let output = if case == "producer-stale" {
                    "removed"
                } else {
                    "selected"
                };
                event(serde_json::json!({"event":"outputs", "data":[
                    {"name":output, "focused":true}
                ]}));
                event(serde_json::json!({"event":"windows", "data":[
                    {"id":"fixture", "output":output, "active":true}
                ]}));
            }
            event(serde_json::json!({"event":"outputs", "data":[
                {"name":"selected", "focused":false}
            ]}));
            if case == "producer-overview" {
                event(serde_json::json!({"event":"overview", "open":true}));
            }
            enqueue("first");
            advance();
            assert_ne!(IslandService::read().surface(), Some(Surface::Auth));
            for output in ["selected", "removed"] {
                assert_eq!(IslandService::read().keyboard(output), Keyboard::None);
            }
            type_char('x');
            assert!(Prompt::read().secret.is_empty());
            match case.as_str() {
                "producer-window" => event(serde_json::json!({"event":"windows", "data":[
                    {"id":"fixture", "output":"selected", "active":true}
                ]})),
                "producer-workspace" => event(serde_json::json!({"event":"workspaces", "data":[
                    {"id":"fixture", "output":"selected", "focused":true}
                ]})),
                _ => event(serde_json::json!({"event":"outputs", "data":[
                    {"name":"selected", "focused":true}
                ]})),
            }
            if case == "producer-overview" {
                assert_ne!(IslandService::read().surface(), Some(Surface::Auth));
                assert_eq!(IslandService::read().keyboard("selected"), Keyboard::None);
                event(serde_json::json!({"event":"overview", "open":false}));
            }
            visible();
            cancel();
            cleared();
            return;
        }
        if case == "overview-direct" {
            focus(Some("selected"), true);
            IslandService::write().open("selected", Surface::Auth, Instant::now());
            assert_eq!(IslandService::read().keyboard("selected"), Keyboard::None);
            return;
        }
        assert!(
            super::super::niri::Niri::read()
                .seen
                .focused_output
                .is_none()
        );
        if case == "niri" {
            super::super::niri::Niri::write().seen.focused_output = Some("selected".into());
        }
        focus(
            (case != "deferred" && case != "lock-deferred").then_some("selected"),
            case == "overview",
        );
        enqueue("first");
        advance();
        if case == "lock-deferred" {
            lock::pending_fixture();
            assert!(lock::holds());
            focus(Some("selected"), false);
            service::derive();
            cleared();
            assert_eq!(answers.load(Ordering::SeqCst), 1);
            return;
        }
        if case == "deferred" || case == "overview" {
            assert_ne!(IslandService::read().surface(), Some(Surface::Auth));
            assert_eq!(IslandService::read().keyboard("selected"), Keyboard::None);
            assert!(SHOWN.locked().is_none());
            type_char('x');
            assert!(
                Prompt::read().secret.is_empty(),
                "a hidden prompt accepts no secret"
            );
            focus(Some("selected"), false);
            service::derive();
        }
        visible();
        type_char('x');
        assert_eq!(Prompt::read().secret.len(), 1);
        if case == "lock-edit-race" {
            use std::sync::mpsc;
            let (checked, after_check) = mpsc::channel();
            let (resume, resumed) = mpsc::channel();
            *INPUT_CHECKED.locked() = Some(Box::new(move || {
                checked.send(()).unwrap();
                resumed.recv_timeout(Duration::from_secs(2)).unwrap();
            }));
            let editor = thread::spawn(|| type_char('y'));
            after_check.recv_timeout(Duration::from_secs(2)).unwrap();
            let (started, starting) = mpsc::channel();
            let (finished, locked) = mpsc::channel();
            let locker = thread::spawn(move || {
                started.send(()).unwrap();
                lock::pending_fixture();
                finished.send(()).unwrap();
            });
            starting.recv_timeout(Duration::from_secs(2)).unwrap();
            // Lock onset must wait for the edit that already passed its unlocked check.
            let early = locked.recv_timeout(Duration::from_millis(100));
            let held_early = lock::holds();
            resume.send(()).unwrap();
            editor.join().unwrap();
            locker.join().unwrap();
            assert!(matches!(early, Err(mpsc::RecvTimeoutError::Timeout)));
            assert!(
                !held_early,
                "pending lock overtook an in-flight input mutation"
            );
            assert!(lock::holds());
            cleared();
            assert_eq!(answers.load(Ordering::SeqCst), 1);
            return;
        }
        if case.starts_with("helper-") {
            // Pause the real helper conversation after publication of its shutdown socket and
            // receipt of both header lines. Lock/Cancel then precedes PAM's password request,
            // without service derivation or a late settle() standing in for the external write.
            let (socket, peer) = UnixStream::pair().unwrap();
            peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut peer = BufReader::new(peer);
            active(55, None);
            Prompt::write().checking = true;
            let worker = thread::spawn(move || {
                converse(55, "fixture", "first", &Zeroizing::new("x".into()), socket)
            });
            let mut line = String::new();
            peer.read_line(&mut line).unwrap();
            assert_eq!(line, "fixture\n");
            line.clear();
            peer.read_line(&mut line).unwrap();
            assert_eq!(line, "first\n");
            match case.as_str() {
                "helper-lock" => lock::pending_fixture(),
                "helper-cancel" => cancel(),
                _ => {}
            }
            let _ = peer
                .get_mut()
                .write_all(b"PAM_PROMPT_ECHO_OFF Password:\nFAILURE\n");
            line.clear();
            peer.read_line(&mut line).unwrap();
            assert!(worker.join().unwrap().is_err());
            if case == "helper-answer" {
                assert_eq!(line, "x\n", "the unlocked conversation sends the answer");
                cancel();
            } else {
                assert!(line.is_empty(), "a cancelled helper received a password");
            }
            cleared();
            assert_eq!(answers.load(Ordering::SeqCst), 1);
            return;
        }
        if case == "outcomes" {
            active(1, None);
            settle(1, "first", Err("Wrong password".into()));
            assert!(Prompt::read().secret.is_empty());
            assert_eq!(PENDING.locked()[0].tries, TRIES - 1);
            assert_eq!(answers.load(Ordering::SeqCst), 0);
            active(2, None);
            settle(2, "first", Ok(()));
            cleared();
            settle(2, "first", Err("late".into()));
            assert_eq!(answers.load(Ordering::SeqCst), 1);
            return;
        }
        if case == "cancel-head" || case == "cancel-tail" {
            enqueue("second");
        }
        let (socket, mut peer) = UnixStream::pair().unwrap();
        peer.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        active(3, Some(socket));
        Prompt::write().checking = true;
        *GIVES_UP.locked() = Some(123);
        if case.starts_with("lock") || case == "displaced" {
            enqueue("second");
            if case == "displaced" {
                IslandService::write().displace(Surface::Auth, Instant::now());
                service::derive();
            } else {
                lock::pending_fixture();
                assert!(lock::holds());
                match case.as_str() {
                    "lock-input" => type_char('y'),
                    "lock-submit" => submit(),
                    "lock-result" => settle(3, "first", Ok(())),
                    _ => service::derive(),
                }
            }
            cleared();
            assert_eq!(peer.read(&mut [0]).unwrap(), 0);
            settle(3, "first", Ok(()));
            focus(Some("selected"), false);
            service::derive();
            cleared();
            assert_eq!(answers.load(Ordering::SeqCst), 2);
            return;
        }
        match case.as_str() {
            "cancel-tail" => {
                let before = Prompt::read().clone();
                cancel_cookie("second");
                assert_eq!(*Prompt::read(), before);
                assert!(ATTEMPTING.locked().is_some());
                assert_eq!(answers.load(Ordering::SeqCst), 1);
                cancel_cookie("first");
            }
            "escape" => cancel(),
            _ => cancel_cookie("first"),
        }
        assert_eq!(
            peer.read(&mut [0])
                .expect("cancellation must shut down the helper"),
            0
        );
        if case == "cancel-head" {
            visible();
            assert_eq!(Prompt::read().message(), "second");
            assert!(Prompt::read().secret.is_empty());
            active(4, None);
            settle(3, "first", Err("late".into()));
            assert_eq!(PENDING.locked()[0].tries, TRIES);
            cancel();
        }
        cleared();
        if case == "same-cookie" {
            enqueue("first");
            advance();
            active(4, None);
            let before = Prompt::read().clone();
            settle(3, "first", Ok(()));
            settle(3, "first", Err("late".into()));
            assert_eq!(*Prompt::read(), before);
            assert_eq!(PENDING.locked()[0].tries, TRIES);
            assert_eq!(ATTEMPTING.locked().as_ref().unwrap().id, 4);
            cancel();
            cleared();
            assert_eq!(answers.load(Ordering::SeqCst), 2);
            return;
        }
        cancel_cookie("first");
        cancel();
        service::derive();
        assert_eq!(
            answers.load(Ordering::SeqCst),
            if case.starts_with("cancel-") && case != "cancel-last" {
                2
            } else {
                1
            }
        );
    }

    // nobody answers polkit while the session locks, nor without the helper socket, nor for a
    // user this session does not run as, nor with four requests waiting
    #[test]
    fn a_refusal_is_immediate_and_quiet() {
        assert_eq!(
            refuses(true, true, Some("kanade"), 0),
            Some(Refusal::Locked)
        );
        assert_eq!(
            refuses(false, false, Some("kanade"), 0),
            Some(Refusal::NoHelper)
        );
        assert_eq!(refuses(false, true, None, 0), Some(Refusal::NoUser));
        assert_eq!(
            refuses(false, true, Some("kanade"), MOST),
            Some(Refusal::Full)
        );
        assert_eq!(refuses(false, true, Some("kanade"), MOST - 1), None);
    }

    // the helper's `g_strescape`d lines read back as PAM wrote them
    #[test]
    fn an_escape_reads_back() {
        assert_eq!(unescape("hello"), "hello");
        assert_eq!(unescape("\\n"), "\n");
        assert_eq!(unescape("\\\\t"), "\\t");
        assert_eq!(unescape("\\012"), "\n");
        assert_eq!(unescape("\\07"), "\u{7}");
        assert_eq!(unescape("\\\\"), "\\");
        assert_eq!(unescape("\\099"), ""); // 0o99 is not octal: the `9` is not escaped
    }

    // what is typed is bounded and controlled: what no key types never is
    #[test]
    fn only_what_a_key_types_fits() {
        let secret = |text: &str| {
            text.chars().fold(Secret::default(), |mut secret, letter| {
                secret.push(letter);
                secret
            })
        };

        let full = secret(&"q".repeat(MOST_SECRET));
        assert_eq!(typed(&full, Some('r')).len(), MOST_SECRET);
        assert_eq!(typed(&secret("a"), Some(' ')), secret("a "));
        assert_eq!(typed(&secret("a"), Some('\u{7f}')), secret("a"));
        assert_eq!(typed(&secret("ab"), None), secret("a"));
        assert_eq!(typed(&Secret::default(), None), Secret::default());
    }

    // the identities polkit offers are an array of `(sa{sv})`, read from the first unix-user
    #[test]
    fn a_unix_user_identity_is_found() {
        let one = |kind: &str, uid: u32| {
            Value::List(vec![
                Value::Text(kind.into()),
                Value::Map(BTreeMap::from([(
                    String::from("uid"),
                    Value::Number(uid as f64),
                )])),
            ])
        };
        let identities = |kind: &str, uid: u32| Value::List(vec![one(kind, uid)]);

        assert_eq!(identity(&identities("unix-user", 1000)), Some(1000));
        assert_eq!(identity(&identities("unix-user", 0)), Some(0));

        // the first unix-user wins, whatever other identities come before it
        assert_eq!(
            identity(&Value::List(vec![
                one("unix-session", 1000),
                one("unix-user", 1000),
            ])),
            Some(1000)
        );

        // no unix-user at all, and no identities at all
        assert_eq!(
            identity(&Value::List(vec![
                one("unix-session", 1000),
                one("unix-group", 1000),
            ])),
            None
        );
        assert_eq!(identity(&Value::List(Vec::new())), None);
    }

    // BeginAuthentication is `(sss a{ss} s a(sa{sv}))`: the message second, the cookie fifth and
    // the identities sixth, as polkitd's own `(sss@a{ss}sa(sa{sv}))` says
    #[test]
    fn begin_authentication_reads_its_six_arguments() {
        let identities = |kind: &str, uid: u32| {
            Value::List(vec![Value::List(vec![
                Value::Text(kind.into()),
                Value::Map(BTreeMap::from([(
                    String::from("uid"),
                    Value::Number(uid as f64),
                )])),
            ])])
        };

        let call = |offered: Value| {
            vec![
                Value::Text(String::from("org.freedesktop.timedate1.set-timezone")),
                Value::Text(String::from(
                    "Authentication is required to set the timezone",
                )),
                Value::Text(String::from("preferences-system-time")),
                Value::Map(BTreeMap::new()),
                Value::Text(String::from("cookie-1")),
                offered,
            ]
        };

        let arguments = call(identities("unix-user", 1000));
        let (cookie, message, offered) = layout(&arguments);

        assert_eq!(cookie, "cookie-1");
        assert_eq!(message, "Authentication is required to set the timezone");
        assert_eq!(identity(offered), Some(1000));

        // a call shorter than the six has no cookie to answer
        assert_eq!(layout(&call(identities("unix-user", 1000))[..4]).0, "");
    }

    // a try that failed empties the field and says what is left
    #[test]
    fn a_retry_empties_the_field_and_says_what_is_left() {
        *Prompt::write() = Prompt {
            message: String::from("Authentication is required"),
            user: String::from("kanade"),
            secret: "hunter2"
                .chars()
                .fold(Secret::default(), |mut secret, letter| {
                    secret.push(letter);
                    secret
                }),
            note: None,
            checking: true,
        };

        retry(String::from("Wrong password"), 2);

        let prompt = Prompt::read();
        assert_eq!(prompt.note(), Some("Wrong password, 2 tries left"));
        assert!(prompt.secret().is_empty());
    }
}
