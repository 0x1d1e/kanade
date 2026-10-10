//! The public CLI (#103, docs/design.md CLI): `kanade` runs the shell, `kanade <verb> [args]` asks
//! the running one over the runtime's IPC. Each Module names the verbs it owns (`Module::verbs`), so the
//! usage and the parse follow from the Modules, and the shell refuses the verbs of a Module that is
//! off. Both sides parse the same words: this process to answer a typo without a shell, the shell to
//! act. The socket is `$XDG_RUNTIME_DIR/kanade.sock`; it is internal.

use std::env;
use std::io::{self, IsTerminal, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::{IpcCall, ipc_socket};

use crate::island::activity::Leave;
use crate::island::command::{Command, Unparsed};
use crate::modules::{self, Module};
use crate::sources::recording::Settled;
use crate::sources::{caffeine, capture, google, osd, recording, timer, wallpaper, weather};
use crate::{config, doctor, lock};

// the one IPC handler the shell registers, which every verb goes through
pub const HANDLER: &str = "kanade";

// the words a call and its reply are made of; another number means a client and shell that may
// misread each other
pub const PROTOCOL: u32 = 4;

/*
 * the first word after `kanade`, with the arguments a Module owns; Modules may share a name, each
 * owning its own arguments, like `notifications clear` and `notifications open`
 */
pub struct Verb {
    pub name: &'static str,

    // what it does, in a line of the command list; Modules sharing the name word it alike
    pub about: &'static str,

    // its lines of the usage, each starting with the name; an indented line says what an argument takes
    pub usage: fn() -> String,

    // the words after the name
    pub parse: fn(&[&str]) -> Result<Call, Unparsed>,
}

// what a verb asks of the shell
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Island(Command),
    Timer(timer::Request),
    ClearNotifications,
    ClearClipboard,
    Screenshot(capture::Mode),
    Record(recording::Request),
    Caffeine(caffeine::Request),
    Wallpaper(wallpaper::Request),
    Google(google::Request),
    Weather(weather::Request),
    Osd(osd::Asked),
    Lock,
    LockStatus,
    Session(Leave),
    Settings(Option<&'static str>),
    CloseSettings,
    Reload,
    Validate,
    Status,
    Modules,

    // a Module, turned on or off
    Turn(&'static str, bool),
}

// what the shell answers; a refusal exits with failure
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Done(String),
    Refused(String),

    // asked of another program that did not say whether it did it, so not to be asked again blindly
    Unknown(String),
}

impl Reply {
    // the outcome on the first line, so a reply from a shell that is not Kanade is told apart
    pub fn encode(&self) -> String {
        match self {
            Reply::Done(text) => format!("ok\n{text}"),
            Reply::Refused(text) => format!("refused\n{text}"),
            Reply::Unknown(text) => format!("unknown\n{text}"),
        }
    }

    fn decode(text: &str) -> Option<Reply> {
        let (outcome, text) = text.split_once('\n').unwrap_or((text, ""));
        let text = text.trim_end().to_owned();

        match outcome {
            "ok" => Some(Reply::Done(text)),
            "refused" => Some(Reply::Refused(text)),
            "unknown" => Some(Reply::Unknown(text)),
            _ => None,
        }
    }
}

// the Module owning the verb, with what it asks
pub fn parse(words: &[&str]) -> Result<(&'static Module, Call), Unparsed> {
    let [name, arguments @ ..] = words else {
        return Err(Unparsed::Usage);
    };

    let mut unparsed = Unparsed::Usage;

    for (module, verb) in verbs().filter(|(_, verb)| verb.name == *name) {
        match (verb.parse)(arguments) {
            Ok(call) => return Ok((module, call)),
            Err(Unparsed::Invalid(invalid)) => unparsed = Unparsed::Invalid(invalid),
            Err(Unparsed::Usage) => {}
        }
    }

    Err(unparsed)
}

// every verb with its Module, in the Modules' order
fn verbs() -> impl Iterator<Item = (&'static Module, &'static Verb)> {
    modules::ALL
        .iter()
        .flat_map(|module| module.verbs.iter().map(move |verb| (module, verb)))
}

// a name the CLI answers to, with the forms it takes
struct Entry {
    name: &'static str,
    about: &'static str,

    // the lines of `Verb::usage` of every Module owning the name, then what this process adds
    usage: Vec<String>,
}

// the name of a verb this process answers itself, with what it does and the form it takes
const OWN: &[(&str, &str, &str)] = &[
    ("config", "", "config defaults"),
    (
        "doctor",
        "Check what Kanade runs on, without a shell",
        "doctor",
    ),
    (
        "help",
        "Show this list, or a command's arguments",
        "help [<command>]",
    ),
];

// every name, in the Modules' order with the lines of one name together, then those this process answers itself
fn commands() -> Vec<Entry> {
    let mut commands: Vec<Entry> = Vec::new();

    for (_, verb) in verbs() {
        match commands
            .iter_mut()
            .find(|command| command.name == verb.name)
        {
            Some(command) => command.usage.push((verb.usage)()),
            None => commands.push(Entry {
                name: verb.name,
                about: verb.about,
                usage: vec![(verb.usage)()],
            }),
        }
    }

    for &(name, about, usage) in OWN {
        match commands.iter_mut().find(|command| command.name == name) {
            Some(command) => command.usage.push(usage.to_owned()),
            None => commands.push(Entry {
                name,
                about,
                usage: vec![usage.to_owned()],
            }),
        }
    }

    commands
}

// the width the help is wrapped to
const WIDTH: usize = 80;

// bold on a terminal, else as it is
#[derive(Clone, Copy)]
struct Style(bool);

impl Style {
    // for what is written to `stream`: no color off a terminal, with NO_COLOR, or on a dumb one
    fn of(stream: &impl IsTerminal) -> Style {
        Style(
            stream.is_terminal()
                && env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
                && env::var_os("TERM").is_none_or(|term| term != "dumb"),
        )
    }

    fn bold(self, text: &str) -> String {
        if self.0 {
            format!("\x1b[1m{text}\x1b[0m")
        } else {
            text.to_owned()
        }
    }

    // `text` in bold, then spaces up to `width` columns
    fn column(self, text: &str, width: usize) -> String {
        format!(
            "{}{}",
            self.bold(text),
            " ".repeat(width.saturating_sub(text.chars().count()))
        )
    }
}

// `text` broken into lines of at most `width` columns, after a space or a `|`
fn wrapped(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = vec![String::new()];

    for word in text.split_inclusive([' ', '|']) {
        let line = lines.last_mut().expect("a line");

        if !line.is_empty()
            && line.trim_end().chars().count() + word.trim_end().chars().count() > width
        {
            lines.push(String::new());
        }

        lines.last_mut().expect("a line").push_str(word);
    }

    lines
        .iter()
        .map(|line| line.trim_end().to_owned())
        .collect()
}

// `rows` as an indented table of a name and a description, the description wrapped under itself
fn table(rows: &[(String, String)], style: Style) -> String {
    let name = rows
        .iter()
        .map(|(name, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    let indent = 2 + name + 2;
    let mut table = String::new();

    for (key, description) in rows {
        for (index, line) in wrapped(description, WIDTH.saturating_sub(indent))
            .iter()
            .enumerate()
        {
            let left = if index == 0 {
                style.column(key, name)
            } else {
                " ".repeat(name)
            };

            table.push_str(&format!("  {left}  {line}\n"));
        }
    }

    table
}

// every command with what it does
fn help(style: Style) -> String {
    let commands = commands();
    let rows: Vec<(String, String)> = commands
        .iter()
        .map(|command| (command.name.to_owned(), command.about.to_owned()))
        .collect();

    format!(
        "Kanade - a niri shell around an adaptive activity island

{usage} kanade [COMMAND]

With no command, runs the shell. A command asks the running one.

{commands}
{rows}
Run 'kanade help <command>' for what a command takes.
",
        usage = style.bold("Usage:"),
        commands = style.bold("Commands:"),
        rows = table(&rows, style),
    )
}

// what one command does, the forms it takes and what their arguments are
fn help_for(command: &Entry, style: Style) -> String {
    let mut forms = String::new();
    let mut arguments: Vec<(String, String)> = Vec::new();

    for line in command.usage.iter().flat_map(|usage| usage.lines()) {
        match line
            .strip_prefix("  ")
            .and_then(|line| line.split_once(": "))
        {
            Some((argument, meaning)) => arguments.push((argument.to_owned(), meaning.to_owned())),
            None => forms.push_str(&format!("  kanade {line}\n")),
        }
    }

    let mut help = format!("{}\n\n{}\n{forms}", command.about, style.bold("Usage:"));

    if !arguments.is_empty() {
        help.push_str(&format!(
            "\n{}\n{}",
            style.bold("Arguments:"),
            table(&arguments, style)
        ));
    }

    help
}

// what to say of words that make no call: the command's help when it is one, else that it is none
fn mistake(words: &[&str], style: Style) -> String {
    match words
        .first()
        .and_then(|name| commands().into_iter().find(|command| command.name == *name))
    {
        Some(command) => format!(
            "kanade: bad arguments for '{}'\n\n{}",
            command.name,
            help_for(&command, style)
        ),
        None => format!(
            "kanade: unknown command{}\n\nRun 'kanade help' for the commands.",
            words
                .first()
                .map_or_else(String::new, |name| format!(" '{name}'"))
        ),
    }
}

// how long the shell may take to answer before it counts as stuck
const PATIENCE: Duration = Duration::from_secs(5);

/*
 * `kanade <verb> [args]`: 2 for words that are no verb, 1 for a refusal, no shell to ask or a failed
 * stdout, 3 when it is not known whether it was done, as the shell or niri took it but did not answer
 */
pub fn run(arguments: &[String]) -> ExitCode {
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();

    match words[..] {
        ["help" | "-h" | "--help"] => {
            return print(&help(Style::of(&io::stdout())), ExitCode::SUCCESS);
        }
        ["-V" | "--version"] => {
            return print(
                concat!("kanade ", env!("CARGO_PKG_VERSION"), "\n"),
                ExitCode::SUCCESS,
            );
        }
        ["help", name] | [name, "-h" | "--help"] => {
            return match commands().into_iter().find(|command| command.name == name) {
                Some(command) => print(
                    &help_for(&command, Style::of(&io::stdout())),
                    ExitCode::SUCCESS,
                ),
                None => {
                    complain(&mistake(&[name], Style::of(&io::stderr())));
                    ExitCode::from(2)
                }
            };
        }
        ["doctor"] => return doctor::run(),
        ["config", "defaults"] => return print(&config::defaults(), ExitCode::SUCCESS),
        _ => {}
    }

    let asked = match parse(&words) {
        Ok((_, call)) => call,
        Err(Unparsed::Usage) => {
            complain(&mistake(&words, Style::of(&io::stderr())));
            return ExitCode::from(2);
        }
        Err(Unparsed::Invalid(invalid)) => {
            complain(&format!("kanade: {invalid}"));
            return ExitCode::from(2);
        }
    };

    // the shell runs elsewhere, so a path goes to it whole
    let asked = match asked {
        Call::Wallpaper(wallpaper::Request::Set(path)) => match std::path::absolute(&path) {
            Ok(path) => Call::Wallpaper(wallpaper::Request::Set(path)),
            Err(error) => {
                complain(&format!("kanade: {}: {error}", path.display()));
                return ExitCode::FAILURE;
            }
        },
        Call::Google(google::Request::SignIn(path)) => match std::path::absolute(&path) {
            Ok(path) => Call::Google(google::Request::SignIn(path)),
            Err(error) => {
                complain(&format!("kanade: {}: {error}", path.display()));
                return ExitCode::FAILURE;
            }
        },
        asked => asked,
    };
    let arguments = match &asked {
        Call::Wallpaper(wallpaper::Request::Set(path)) => vec![
            String::from("wallpaper"),
            String::from("set"),
            path.to_string_lossy().into_owned(),
        ],
        Call::Google(google::Request::SignIn(path)) => vec![
            String::from("google-calendar"),
            String::from("sign-in"),
            path.to_string_lossy().into_owned(),
        ],
        _ => arguments.to_vec(),
    };

    let reply = match (asked, call(&arguments)) {
        (
            Call::Record(request @ (recording::Request::Start | recording::Request::Stop)),
            Ok(Reply::Done(path)),
        ) => Ok(settle_recording(request, path, PATIENCE, |deadline| {
            call_until(&status_call(), deadline)
        })),
        (
            Call::Caffeine(caffeine::Request::On(_) | caffeine::Request::Toggle(_)),
            Ok(Reply::Done(text)),
        ) if let Some(serial) = caffeine::starting(&text) => {
            Ok(settle_caffeine(serial, PATIENCE, |deadline| {
                call_until(&caffeine_status_call(), deadline)
            }))
        }
        (Call::Wallpaper(wallpaper::Request::Set(path)), Ok(Reply::Done(text)))
            if let Some(serial) = wallpaper::setting(&text) =>
        {
            Ok(settle_wallpaper(serial, &path, PATIENCE, |deadline| {
                call_until(&wallpaper_status_call(), deadline)
            }))
        }
        (Call::Lock, Ok(Reply::Done(text))) if let Some(request) = lock::requested(&text) => {
            Ok(settle_lock(&request, PATIENCE, |deadline| {
                call_until(&lock_status_call(), deadline)
            }))
        }
        // a shell from before #196, or any answer naming no request to wait on
        (Call::Lock, Ok(Reply::Done(_))) => Ok(Reply::Unknown(String::from(
            "the shell names no lock request to wait on, as it is older than this kanade; restart \
             it. The session may still lock",
        ))),
        (_, reply) => reply,
    };

    match reply {
        Ok(Reply::Done(text)) if text.is_empty() => ExitCode::SUCCESS,
        Ok(Reply::Done(text)) => print(&format!("{text}\n"), ExitCode::SUCCESS),
        Ok(Reply::Refused(text)) | Err(text) => {
            complain(&format!("kanade: {text}"));
            ExitCode::FAILURE
        }
        Ok(Reply::Unknown(text)) => {
            complain(&format!("kanade: {text}"));
            ExitCode::from(3)
        }
    }
}

/*
 * writes `text` to stdout and gives back `code` as the exit code. A closed pipe is no failure, as its
 * reader, like `head`, wanted no more (#197); any other error is said on stderr and fails
 */
pub fn print(text: &str, code: ExitCode) -> ExitCode {
    match written(io::stdout().lock(), text) {
        Ok(()) => code,
        Err(error) => {
            complain(&format!("kanade: stdout: {error}"));
            ExitCode::FAILURE
        }
    }
}

// writes all of `text` to `output`, a pipe its reader closed taking it as written
fn written(mut output: impl Write, text: &str) -> io::Result<()> {
    match output
        .write_all(text.as_bytes())
        .and_then(|()| output.flush())
    {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        written => written,
    }
}

// writes `text` and a newline to stderr; when that fails too, nothing is left to tell
fn complain(text: &str) {
    drop(writeln!(io::stderr().lock(), "{text}"));
}

// where a call the shell answered at once stands, by the status it says
enum Settling {
    Waiting,
    Settled(Reply),
}

/*
 * waits on a call the shell answers at once and does after: asks its status through `ask` by the
 * deadline it is given, until `settling` says it settled. All of it within `patience`, a slow
 * answer included; then it is `unsettled`. `aside` says what is left of it when that is not known
 */
fn settle(
    patience: Duration,
    unsettled: impl FnOnce() -> String,
    aside: &str,
    mut ask: impl FnMut(Instant) -> Result<Reply, String>,
    mut settling: impl FnMut(&str) -> Option<Settling>,
) -> Reply {
    let deadline = Instant::now() + patience;

    loop {
        if Instant::now() >= deadline {
            return Reply::Unknown(unsettled());
        }

        let status = match ask(deadline) {
            Ok(Reply::Done(status)) => status,
            _ if Instant::now() >= deadline => return Reply::Unknown(unsettled()),
            Ok(Reply::Refused(why) | Reply::Unknown(why)) | Err(why) => {
                return Reply::Unknown(format!("{why}; {aside}"));
            }
        };

        match settling(&status) {
            Some(Settling::Settled(reply)) => return reply,
            Some(Settling::Waiting) => {
                thread::sleep(LOOK.min(deadline.saturating_duration_since(Instant::now())));
            }
            None => return Reply::Unknown(format!("the shell says {status:?}; {aside}")),
        }
    }
}

/*
 * the shell answers a recording's start or stop at once (`ipc::record`), so this waits until the
 * first frame is written or the file saved, done at its path
 */
fn settle_recording(
    request: recording::Request,
    path: String,
    patience: Duration,
    ask: impl FnMut(Instant) -> Result<Reply, String>,
) -> Reply {
    settle(
        patience,
        || request.unsettled(&path, patience),
        &format!("the recording is at {path}"),
        ask,
        |status| {
            Some(
                match request.settled(&path, &recording::Status::parse(status)?) {
                    Settled::Done => Settling::Settled(Reply::Done(path.clone())),
                    Settled::Failed(why) => Settling::Settled(Reply::Refused(why)),
                    Settled::Lost(why) => Settling::Settled(Reply::Unknown(why)),
                    Settled::Waiting => Settling::Waiting,
                },
            )
        },
    )
}

/*
 * the shell answers a lock at once, as it only asks niri for it (`lock::start`), so this waits
 * until niri says it holds the lock for `request`, which no lock before it can, and the lock still
 * holds when asked (#196)
 */
fn settle_lock(
    request: &lock::Request,
    patience: Duration,
    ask: impl FnMut(Instant) -> Result<Reply, String>,
) -> Reply {
    settle(
        patience,
        || {
            format!(
                "niri did not hold the lock within {}s, and a password typed meanwhile may \
                 have unlocked it; the session may still lock",
                patience.as_secs()
            )
        },
        "the session may still lock",
        ask,
        |status| {
            Some(match lock::settled(status, request)? {
                lock::Settled::Locked => Settling::Settled(Reply::Done(String::new())),
                lock::Settled::Waiting => Settling::Waiting,
                lock::Settled::Unlocked => Settling::Settled(Reply::Unknown(String::from(
                    "a password typed on the lock screen unlocks the session",
                ))),
                lock::Settled::Denied => Settling::Settled(Reply::Refused(String::from(
                    "niri refused the lock: another locker holds the session, or its VT is not \
                     shown",
                ))),
                lock::Settled::Lost => Settling::Settled(Reply::Unknown(String::from(
                    "the shell restarted, which lost the lock it was asked for; the session may \
                     still lock",
                ))),
            })
        },
    )
}

/*
 * the shell answers caffeine's `on` at once, as starting (`caffeine::request`), so this waits
 * until its inhibitor is held
 */
fn settle_caffeine(
    serial: u64,
    patience: Duration,
    ask: impl FnMut(Instant) -> Result<Reply, String>,
) -> Reply {
    settle(
        patience,
        || {
            format!(
                "{} did not hold the inhibitor within {}s; caffeine may still turn on",
                caffeine::INHIBIT,
                patience.as_secs()
            )
        },
        "caffeine may still turn on",
        ask,
        |status| {
            Some(match caffeine::Status::parse(status)?.settled(serial) {
                caffeine::Settled::On(text) => Settling::Settled(Reply::Done(text)),
                caffeine::Settled::Failed(why) => Settling::Settled(Reply::Refused(why)),
                caffeine::Settled::Lost(why) => Settling::Settled(Reply::Unknown(why)),
                caffeine::Settled::Waiting => Settling::Waiting,
            })
        },
    )
}

/*
 * the shell answers a wallpaper's set at once, as setting (`wallpaper::request`), so this waits
 * until awww showed it, done at the path it shows
 */
fn settle_wallpaper(
    serial: u64,
    path: &Path,
    patience: Duration,
    ask: impl FnMut(Instant) -> Result<Reply, String>,
) -> Reply {
    settle(
        patience,
        || {
            format!(
                "{} did not show the wallpaper within {}s; it may still",
                wallpaper::AWWW,
                patience.as_secs()
            )
        },
        "the wallpaper may still change",
        ask,
        |status| {
            let status = wallpaper::Status::parse(status)?;

            Some(match status.settled(serial) {
                // its own path: the current one may be from a set done after it
                wallpaper::Settled::Set => {
                    Settling::Settled(Reply::Done(path.display().to_string()))
                }
                wallpaper::Settled::Failed(why) => Settling::Settled(Reply::Refused(why)),
                wallpaper::Settled::Lost(why) => Settling::Settled(Reply::Unknown(why)),
                wallpaper::Settled::Waiting => Settling::Waiting,
            })
        },
    )
}

fn status_call() -> Vec<String> {
    ["capture", "record", "status"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn wallpaper_status_call() -> Vec<String> {
    ["wallpaper", "status"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn caffeine_status_call() -> Vec<String> {
    ["caffeine", "status"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn lock_status_call() -> Vec<String> {
    ["lock", "status"].into_iter().map(String::from).collect()
}

// how often `settle` asks
const LOOK: Duration = Duration::from_millis(50);

// what the running shell answers, or why there is none to ask
pub fn call(arguments: &[String]) -> Result<Reply, String> {
    call_until(arguments, Instant::now() + PATIENCE)
}

// as `call`, answered by `deadline`
fn call_until(arguments: &[String], deadline: Instant) -> Result<Reply, String> {
    // a newline would split one argument into two on the shell's side
    if arguments.iter().any(|argument| argument.contains('\n')) {
        return Err(String::from("an argument cannot hold a newline"));
    }

    // the runtime's socket lives there, and it panics without one
    if env::var_os("XDG_RUNTIME_DIR").is_none() {
        return Err(String::from(
            "XDG_RUNTIME_DIR is not set, so there is no shell to find",
        ));
    }

    let stream = UnixStream::connect(ipc_socket())
        .map_err(|_| String::from("no shell is running; start one with `kanade`"))?;

    let patience = deadline.saturating_duration_since(Instant::now());

    send(stream, &IpcCall::new(HANDLER, arguments), patience)
}

/*
 * the shell's reply to `call`, whole within `patience`; one that is late or missing once the call
 * is sent leaves unknown whether the shell did it, like a screenshot niri may already be saving
 */
fn send(mut stream: UnixStream, call: &IpcCall, patience: Duration) -> Result<Reply, String> {
    let deadline = Instant::now() + patience;

    let sent = call
        .write(&mut stream)
        // closing our side is how the shell knows the call is complete
        .and_then(|()| stream.shutdown(Shutdown::Write));

    sent.map_err(|error| format!("the shell closed the connection: {error}"))?;

    let mut reply = Vec::new();
    let mut read = [0; 4096];

    // each read waits only what is left, so a reply in pieces is still bounded by `patience`
    let unanswered = loop {
        let left = deadline.saturating_duration_since(Instant::now());

        if left.is_zero() {
            break Some(format!("did not answer within {patience:?}"));
        }

        let got = stream
            .set_read_timeout(Some(left))
            .and_then(|()| stream.read(&mut read));

        match got {
            Ok(0) if reply.is_empty() => {
                break Some(String::from("closed the connection without answering"));
            }
            Ok(0) => break None,
            Ok(count) => reply.extend_from_slice(&read[..count]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                break Some(format!("did not answer within {patience:?}"));
            }
            Err(error) => break Some(format!("closed the connection: {error}")),
        }
    };

    if let Some(why) = unanswered {
        return Ok(Reply::Unknown(format!(
            "the shell got the call, but {why}; it may still have done it"
        )));
    }

    String::from_utf8(reply)
        .ok()
        .and_then(|reply| Reply::decode(&reply))
        .ok_or_else(|| String::from("the running shell is not Kanade"))
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn replies_round_trip_and_anything_else_is_no_kanade() {
        for reply in [
            Reply::Done(String::new()),
            Reply::Done(String::from(
                "config schema_version 1, generation 2\nconfig modules.media is pending restart",
            )),
            Reply::Refused(String::from("module media is off")),
            Reply::Unknown(String::from(
                "niri got the request, but did not answer within 2s",
            )),
        ] {
            assert_eq!(Reply::decode(&reply.encode()), Some(reply));
        }

        assert_eq!(Reply::decode("no handler named kanade"), None);
        assert_eq!(Reply::decode(""), None);
    }

    // the shell at the other end of a socket, answering `reply` after `delay`, or closing unanswered
    fn sent(reply: Option<&str>, delay: Duration) -> Result<Reply, String> {
        let (ours, mut theirs) = UnixStream::pair().expect("a socket pair");
        let reply = reply.map(str::to_owned);

        let shell = std::thread::spawn(move || {
            let mut call = Vec::new();
            theirs.read_to_end(&mut call).expect("the call");
            std::thread::sleep(delay);

            if let Some(reply) = reply {
                // the caller may have stopped listening
                let _ = io::Write::write_all(&mut theirs, reply.as_bytes());
            }
            call
        });

        let call = IpcCall::new(HANDLER, &[String::from("status")]);
        let reply = send(ours, &call, Duration::from_millis(50));

        assert!(!shell.join().expect("the shell").is_empty());
        reply
    }

    #[test]
    fn a_reply_is_the_shells_answer() {
        assert_eq!(
            sent(Some("ok\nkanade 0.1.0"), Duration::ZERO),
            Ok(Reply::Done(String::from("kanade 0.1.0")))
        );
        assert_eq!(
            sent(Some("no handler named kanade"), Duration::ZERO),
            Err(String::from("the running shell is not Kanade"))
        );
    }

    // a shell that takes the call and answers `answer` a byte every `drip`, holding on for a while
    fn dripping(answer: &'static str, drip: Duration) -> UnixStream {
        let (ours, mut theirs) = UnixStream::pair().expect("a socket pair");

        std::thread::spawn(move || {
            let mut call = Vec::new();
            theirs.read_to_end(&mut call).expect("the call");

            for byte in answer.as_bytes() {
                std::thread::sleep(drip);

                if io::Write::write_all(&mut theirs, &[*byte]).is_err() {
                    return;
                }
            }

            std::thread::sleep(Duration::from_secs(2));
        });

        ours
    }

    #[test]
    fn a_reply_in_pieces_is_still_due_by_the_deadline() {
        let call = IpcCall::new(HANDLER, &[String::from("status")]);
        let begun = Instant::now();

        // every piece comes within the time left, all of them well after it
        let reply = send(
            dripping("ok\nkanade 0.1.0", Duration::from_millis(30)),
            &call,
            Duration::from_millis(100),
        );

        assert!(matches!(reply, Ok(Reply::Unknown(_))), "{reply:?}");
        assert!(
            begun.elapsed() < Duration::from_millis(250),
            "{:?}",
            begun.elapsed()
        );
    }

    // the shell may have done a call it answers late or never, so that is no failure
    #[test]
    fn a_late_or_missing_reply_is_unknown() {
        assert_eq!(
            sent(Some("ok\n"), Duration::from_millis(300)),
            Ok(Reply::Unknown(String::from(
                "the shell got the call, but did not answer within 50ms; it may still have done it"
            )))
        );
        assert_eq!(
            sent(None, Duration::ZERO),
            Ok(Reply::Unknown(String::from(
                "the shell got the call, but closed the connection without answering; it may \
                 still have done it"
            )))
        );
    }
}
