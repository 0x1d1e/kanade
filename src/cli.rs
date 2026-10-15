//! The public CLI (docs/design.md CLI): `kanade` runs the shell, `kanade <verb> [args]` asks
//! the running one over the runtime's IPC. Each Module names the verbs it owns (`Module::verbs`), so the
//! usage and the parse follow from the Modules, and the shell refuses the verbs of a Module that is
//! off. Both sides parse the same words: this process to answer a typo without a shell, the shell to
//! act. The socket is `$XDG_RUNTIME_DIR/kanade.sock`; it is internal.

use std::env;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::{IpcCall, ipc_socket};

mod completions;
mod keys;

use crate::island::activity::Leave;
use crate::island::command::{Command, Unparsed};
use crate::modules::{self, Module};
use crate::sources::notifications::timed;
use crate::sources::system::Switch;
use crate::sources::{
    brightness, caffeine, capture, charge_limit, focus, google, live, media, night_light, niri,
    osd, power, recording, reminders, subscriptions, timer, wallpaper, weather,
};
use crate::surfaces::launcher::choose;
use crate::{config, doctor};

// the one IPC handler the shell registers, which every verb goes through
pub const HANDLER: &str = "kanade";

// the words a call and its reply are made of; another number means a client and shell that may
// misread each other
pub const PROTOCOL: u32 = 5;

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
    Live(live::Request),
    Dmenu(choose::Asked),
    ClearNotifications,
    ClearClipboard,
    Screenshot(capture::Mode),
    PickColor,
    Record(recording::Request),
    Cast(niri::Retarget),
    Caffeine(caffeine::Request),
    NightLight(night_light::Request),
    Focus(focus::Request),
    Dnd(timed::Request),
    Battery(charge_limit::Request),
    Media(media::Transport),
    Wifi(Switch),
    Bluetooth(Switch),
    PowerProfile(power::Request),
    Brightness(brightness::Request),
    Wallpaper(wallpaper::Request),
    Google(google::Request),
    Reminders(reminders::Request),
    Weather(weather::Request),
    Subscriptions(subscriptions::Request),
    Osd(osd::Asked),
    Lock,
    LockStatus,
    IdleStatus,
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
    (
        "completions",
        "Print shell completions, from the commands in this list",
        "completions bash|fish|zsh",
    ),
    (
        "config",
        "",
        "config defaults|schema
config get <key>
config set <key> <value>
config reset <key>
config validate <file>
  <key>: a dotted key of `kanade config defaults`, like timings.hover
  <value>: text as it is for a key that takes text, else a TOML value, like 300, true or [1, 2]
  set: writes the settings file, as the Settings window does, never your own files
  reset: removes the key from the settings file, so your files and the defaults give it again
  validate: says what a file alone would not apply, without a shell",
    ),
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

// how long the shell may take to answer before it counts as stuck; over its own `LATER`, so what it says of a late answer arrives
const PATIENCE: Duration = Duration::from_secs(6);

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
        ["completions", shell] if completions::script(shell).is_some() => {
            return print(
                &completions::script(shell).unwrap_or_default(),
                ExitCode::SUCCESS,
            );
        }
        ["doctor"] => return doctor::run(),
        ["config", "defaults"] => return print(&config::defaults(), ExitCode::SUCCESS),
        ["config", "schema"] => return print(&keys::schema(), ExitCode::SUCCESS),
        ["config", "get", key] => return keyed(keys::get(key)),
        ["config", "set", key, value] => return keyed(keys::set(key, value)),
        ["config", "reset", key] => return keyed(keys::reset(key)),
        ["config", "validate", file] => return validated(file),
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

    // runs here, telling the shell as it goes
    if let Call::Live(live::Request::Run { id, title, command }) = &asked {
        return live::run(id, title, command);
    }

    // the agent's event comes on stdin, never the arguments
    if let Call::Live(live::Request::Hook(agent)) = asked {
        return live::hook(agent);
    }

    // the lines come from stdin, never the arguments
    if let Call::Dmenu(asked) = asked {
        if !asked.choices.is_empty() {
            complain(&mistake(&words, Style::of(&io::stderr())));
            return ExitCode::from(2);
        }

        return dmenu(asked.prompt);
    }

    // the shell runs elsewhere, so a path goes to it whole
    let asked = match asked {
        Call::Wallpaper(wallpaper::Request::Set(path, output)) => {
            match std::path::absolute(&path) {
                Ok(path) => Call::Wallpaper(wallpaper::Request::Set(path, output)),
                Err(error) => {
                    complain(&format!("kanade: {}: {error}", path.display()));
                    return ExitCode::FAILURE;
                }
            }
        }
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
        Call::Wallpaper(request @ wallpaper::Request::Set(..)) => request.arguments(),
        Call::Google(google::Request::SignIn(path)) => vec![
            String::from("google-calendar"),
            String::from("sign-in"),
            path.to_string_lossy().into_owned(),
        ],
        _ => arguments.to_vec(),
    };

    // the user picks at their own pace
    let reply = match asked {
        Call::PickColor => call_waiting(&arguments),
        _ => call(&arguments),
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
 * reader, like `head`, wanted no more; any other error is said on stderr and fails
 */
// what a key's verb says, or 1 for why it refused
fn keyed(said: Result<String, String>) -> ExitCode {
    match said {
        Ok(said) => print(&said, ExitCode::SUCCESS),
        Err(why) => {
            complain(&format!("kanade: {why}"));
            ExitCode::FAILURE
        }
    }
}

// `config validate <file>`: 1 for a problem in it, or for a file that does not read
fn validated(file: &str) -> ExitCode {
    match keys::validate(file.as_ref()) {
        Ok(problems) if problems.is_empty() => print("valid\n", ExitCode::SUCCESS),
        Ok(problems) => {
            complain(&problems.join("\n"));
            ExitCode::FAILURE
        }
        Err(why) => keyed(Err(why)),
    }
}

pub fn print(text: &str, code: ExitCode) -> ExitCode {
    match written(io::stdout().lock(), text.as_bytes()) {
        Ok(()) => code,
        Err(error) => {
            complain(&format!("kanade: stdout: {error}"));
            ExitCode::FAILURE
        }
    }
}

// writes all of `text` to `output`, a pipe its reader closed taking it as written
fn written(mut output: impl Write, text: &[u8]) -> io::Result<()> {
    match output.write_all(text).and_then(|()| output.flush()) {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        written => written,
    }
}

// writes `text` and a newline to stderr; when that fails too, nothing is left to tell
fn complain(text: &str) {
    drop(writeln!(io::stderr().lock(), "{text}"));
}

// what the running shell answers, or why there is none to ask
pub fn call(arguments: &[String]) -> Result<Reply, String> {
    call_within(arguments, PATIENCE)
}

// as `call`, for a caller that cannot wait as long, like an agent's hook: connecting, writing and
// the reply all fall within `patience`
pub fn call_within(arguments: &[String], patience: Duration) -> Result<Reply, String> {
    let deadline = Instant::now() + patience;
    let stream = connect(arguments, Some(patience))?;
    let left = deadline.saturating_duration_since(Instant::now());

    send(stream, &IpcCall::new(HANDLER, arguments), Some(left))
}

// what `call` says when no shell listens
pub const NO_SHELL: &str = "no shell is running; start one with `kanade`";

// what the shell refuses the verbs of a Module that is off with
pub fn module_off(name: &str) -> String {
    format!("module {name} is off")
}

// as `call`, waiting as long as the shell takes, as on the user
fn call_waiting(arguments: &[String]) -> Result<Reply, String> {
    let stream = connect(arguments, None)?;

    send(stream, &IpcCall::new(HANDLER, arguments), None)
}

/*
 * the running shell to send `arguments` to. A connect to a shell whose queue is full waits, so
 * with a `patience` it is made on a thread that is left behind when the time is up: the caller
 * exits soon after, and the thread ends with it
 */
fn connect(arguments: &[String], patience: Option<Duration>) -> Result<UnixStream, String> {
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

    let socket = ipc_socket();

    let Some(patience) = patience else {
        return UnixStream::connect(socket).map_err(|_| String::from(NO_SHELL));
    };

    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || drop(sender.send(UnixStream::connect(socket))));

    // a shell with room answers at once, so a spent deadline still tells "no shell" from "full"
    match receiver.recv_timeout(patience.max(Duration::from_millis(50))) {
        Ok(connected) => connected.map_err(|_| String::from(NO_SHELL)),
        Err(_) => Err(format!(
            "the shell did not take the call within {patience:?}"
        )),
    }
}

/*
 * `kanade dmenu`: the lines on stdin, chosen from in the Launcher; prints the one chosen exactly as
 * read and exits 0, or exits 1 with nothing printed when the user dismisses it. The shell answers
 * with the line's place, so a line the Launcher shows cleaned is still printed whole
 */
fn dmenu(prompt: Option<String>) -> ExitCode {
    let lines = match choices(io::stdin().lock()) {
        Ok(lines) => lines,
        Err(why) => {
            complain(&format!("kanade: {why}"));
            return ExitCode::FAILURE;
        }
    };

    let sent: Vec<String> = lines
        .iter()
        .map(|line| String::from_utf8_lossy(line).into_owned())
        .collect();

    let asked = choose::Asked {
        prompt,
        choices: sent,
    };

    match call_waiting(&asked.words()) {
        Ok(Reply::Done(index)) => match index
            .parse::<usize>()
            .ok()
            .and_then(|index| lines.get(index))
        {
            Some(line) => {
                let mut chosen = line.to_vec();
                chosen.push(b'\n');

                match written(io::stdout().lock(), &chosen) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(error) => {
                        complain(&format!("kanade: stdout: {error}"));
                        ExitCode::FAILURE
                    }
                }
            }
            None => {
                complain(&format!(
                    "kanade: the shell chose {index:?}, which is no line"
                ));
                ExitCode::FAILURE
            }
        },
        Ok(Reply::Refused(why)) if why == choose::DISMISSED => ExitCode::FAILURE,
        Ok(Reply::Refused(why) | Reply::Unknown(why)) | Err(why) => {
            complain(&format!("kanade: {why}"));
            ExitCode::FAILURE
        }
    }
}

/*
 * the lines on `input` as read, empty ones no choice; refused as soon as there are more than a
 * dmenu takes or one is longer, so a command's endless output is never held whole
 */
fn choices(mut input: impl BufRead) -> Result<Vec<Vec<u8>>, String> {
    // a character is at most 4 bytes, and a byte not UTF-8 shows as one, so more is too long
    let most_bytes = choose::LONGEST * 4;
    let too_long = |number: usize| {
        format!(
            "line {number} on stdin is longer than the {} characters a dmenu takes",
            choose::LONGEST
        )
    };

    let mut lines = Vec::new();

    for number in 1.. {
        let mut line = Vec::new();
        (&mut input)
            .take(most_bytes as u64 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("stdin: {error}"))?;

        let ended = line.last() == Some(&b'\n');
        if ended {
            line.pop();
        }

        if line.len() > most_bytes
            || String::from_utf8_lossy(&line).chars().count() > choose::LONGEST
        {
            return Err(too_long(number));
        }

        if !line.is_empty() {
            if lines.len() == choose::MOST {
                return Err(format!(
                    "more than the {} lines a dmenu takes on stdin",
                    choose::MOST
                ));
            }

            lines.push(line);
        }

        if !ended {
            break;
        }
    }

    if lines.is_empty() {
        return Err(String::from("no lines to choose from on stdin"));
    }

    Ok(lines)
}

/*
 * the shell's reply to `call`, whole within `patience`; one that is late or missing once the call
 * is sent leaves unknown whether the shell did it, like a screenshot niri may already be saving
 */
fn send(
    mut stream: UnixStream,
    call: &IpcCall,
    patience: Option<Duration>,
) -> Result<Reply, String> {
    let deadline = patience.map(|patience| Instant::now() + patience);
    let late = || format!("did not answer within {:?}", patience.unwrap_or_default());

    // a shell that reads no more must not hold the writer either
    let sent = stream
        .set_write_timeout(patience.map(|patience| patience.max(Duration::from_millis(1))))
        .and_then(|()| call.write(&mut stream))
        // closing our side is how the shell knows the call is complete
        .and_then(|()| stream.shutdown(Shutdown::Write));

    sent.map_err(|error| match error.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
            String::from("the shell did not take the call in time")
        }
        _ => format!("the shell closed the connection: {error}"),
    })?;

    let mut reply = Vec::new();
    let mut read = [0; 4096];

    // each read waits only what is left, so a reply in pieces is still bounded by `patience`
    let unanswered = loop {
        let left = deadline.map(|deadline| deadline.saturating_duration_since(Instant::now()));

        if left.is_some_and(|left| left.is_zero()) {
            break Some(late());
        }

        let got = stream
            .set_read_timeout(left)
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
                break Some(late());
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

    // endless "a\n", as a command that never stops writing
    struct Endless;

    impl Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            for (at, byte) in buf.iter_mut().enumerate() {
                *byte = if at % 2 == 0 { b'a' } else { b'\n' };
            }
            Ok(buf.len() / 2 * 2)
        }
    }

    // a dmenu's stdin is refused at its limits without being read whole, and kept byte for byte
    #[test]
    fn dmenu_input_is_bounded_as_it_is_read() {
        let lines = choices(io::BufReader::new(Endless));
        assert!(lines.unwrap_err().starts_with("more than the"));

        let line = choices(io::BufReader::new(io::repeat(b'a')));
        assert!(line.unwrap_err().starts_with("line 1 "));

        let longest = "\u{1F600}".repeat(choose::LONGEST);
        assert_eq!(choices(longest.as_bytes()).unwrap().len(), 1);
        let longer = format!("a\n\n{longest}x\n");
        assert!(
            choices(longer.as_bytes())
                .unwrap_err()
                .starts_with("line 3 ")
        );

        assert_eq!(
            choices(&b"one\n\n\xff two\nthree"[..]).unwrap(),
            [b"one".to_vec(), b"\xff two".to_vec(), b"three".to_vec()]
        );
        assert!(choices(&b"\n\n"[..]).is_err());
    }

    // a verb that acts on a daemon or player is its Module's, which the shell refuses it for while off
    #[test]
    fn verbs_that_act_on_the_system_belong_to_their_module() {
        for (words, owner) in [
            (&["media", "play-pause"][..], "media"),
            (&["media", "next"], "media"),
            (&["wifi", "toggle"], "network"),
            (&["bluetooth", "off"], "bluetooth"),
            (&["power-profile", "set", "balanced"], "power"),
            (&["power-profile", "cycle"], "power"),
        ] {
            let (module, _) = parse(words).expect("parses");
            assert_eq!(module.name, owner, "{words:?}");
        }

        assert!(parse(&["wifi", "maybe"]).is_err());
        assert!(parse(&["power-profile", "set", "turbo"]).is_err());
        assert!(parse(&["media", "stop"]).is_err());
    }

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
        let reply = send(ours, &call, Some(Duration::from_millis(50)));

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
            Some(Duration::from_millis(100)),
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
