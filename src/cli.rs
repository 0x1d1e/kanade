//! The public CLI (#103, docs/design.md CLI): `kanade` runs the shell, `kanade <verb> [args]` asks
//! the running one over Amane IPC. Each Module names the verbs it owns (`Module::verbs`), so the
//! usage and the parse follow from the Modules, and the shell refuses the verbs of a Module that is
//! off. Both sides parse the same words: this process to answer a typo without a shell, the shell to
//! act. `amane ipc call kanade <verb> [args]` reaches the same handler; it is internal.

use std::env;
use std::io::{self, Read};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::time::Duration;

use amane::{IpcCall, ipc_socket};

use crate::config;
use crate::island::command::{Command, Unparsed};
use crate::modules::{self, Module};
use crate::sources::timer;

// the one IPC handler the shell registers, which every verb goes through
pub const HANDLER: &str = "kanade";

// the words a call and its reply are made of; another number means a client and shell that may
// misread each other
pub const PROTOCOL: u32 = 1;

// the first word after `kanade`, owned by a Module
pub struct Verb {
    pub name: &'static str,

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
    Reload,
    Validate,
    Status,
}

// what the shell answers; a refusal exits with failure
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Done(String),
    Refused(String),
}

impl Reply {
    // the outcome on the first line, so a reply from a shell that is not Kanade is told apart
    pub fn encode(&self) -> String {
        match self {
            Reply::Done(text) => format!("ok\n{text}"),
            Reply::Refused(text) => format!("refused\n{text}"),
        }
    }

    fn decode(text: &str) -> Option<Reply> {
        let (outcome, text) = text.split_once('\n').unwrap_or((text, ""));
        let text = text.trim_end().to_owned();

        match outcome {
            "ok" => Some(Reply::Done(text)),
            "refused" => Some(Reply::Refused(text)),
            _ => None,
        }
    }
}

// the Module owning the verb, with what it asks
pub fn parse(words: &[&str]) -> Result<(&'static Module, Call), Unparsed> {
    let [name, arguments @ ..] = words else {
        return Err(Unparsed::Usage);
    };

    let (module, verb) = modules::ALL
        .iter()
        .find_map(|module| {
            let verb = module.verbs.iter().find(|verb| verb.name == *name)?;
            Some((module, verb))
        })
        .ok_or(Unparsed::Usage)?;

    (verb.parse)(arguments).map(|call| (module, call))
}

// every verb, in the Modules' order, then those this process answers itself
pub fn usage() -> String {
    let verbs = modules::ALL
        .iter()
        .flat_map(|module| module.verbs)
        .map(|verb| (verb.usage)());

    std::iter::once(String::from(
        "usage: kanade [<verb> [args]]\nwith no verb, runs the shell; a verb asks the running one:",
    ))
    .chain(verbs)
    .chain([String::from("doctor\nhelp")])
    .collect::<Vec<_>>()
    .join("\n")
}

// how long the shell may take to answer before it counts as stuck
const PATIENCE: Duration = Duration::from_secs(5);

// `kanade <verb> [args]`: 2 for words that are no verb, 1 for a refusal or no shell to ask
pub fn run(arguments: &[String]) -> ExitCode {
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();

    match words[..] {
        ["help" | "-h" | "--help"] => {
            println!("{}", usage());
            return ExitCode::SUCCESS;
        }
        ["doctor"] => return doctor(),
        _ => {}
    }

    match parse(&words) {
        Ok(_) => {}
        Err(Unparsed::Usage) => {
            eprintln!("{}", usage());
            return ExitCode::from(2);
        }
        Err(Unparsed::Invalid(invalid)) => {
            eprintln!("kanade: {invalid}");
            return ExitCode::from(2);
        }
    }

    match call(arguments) {
        Ok(Reply::Done(text)) => {
            if !text.is_empty() {
                println!("{text}");
            }
            ExitCode::SUCCESS
        }
        Ok(Reply::Refused(text)) | Err(text) => {
            eprintln!("kanade: {text}");
            ExitCode::FAILURE
        }
    }
}

fn call(arguments: &[String]) -> Result<Reply, String> {
    // a newline would split one argument into two on the shell's side
    if arguments.iter().any(|argument| argument.contains('\n')) {
        return Err(String::from("an argument cannot hold a newline"));
    }

    // Amane's socket lives there, and it panics without one
    if env::var_os("XDG_RUNTIME_DIR").is_none() {
        return Err(String::from(
            "XDG_RUNTIME_DIR is not set, so there is no shell to find",
        ));
    }

    let stream = UnixStream::connect(ipc_socket())
        .map_err(|_| String::from("no shell is running; start one with `kanade`"))?;

    let reply =
        send(stream, &IpcCall::new(HANDLER, arguments)).map_err(|error| match error.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
                String::from("the shell did not answer")
            }
            _ => format!("the shell closed the connection: {error}"),
        })?;

    Reply::decode(&reply).ok_or_else(|| String::from("the running Amane shell is not Kanade"))
}

fn send(mut stream: UnixStream, call: &IpcCall) -> io::Result<String> {
    stream.set_read_timeout(Some(PATIENCE))?;
    call.write(&mut stream)?;

    // closing our side is how the shell knows the call is complete
    stream.shutdown(Shutdown::Write)?;

    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;

    Ok(reply)
}

// read-only, and works without a shell, which may be what is wrong; #105 adds the rest
fn doctor() -> ExitCode {
    let mut healthy = true;

    match call(&[String::from("status")]) {
        Ok(_) => println!("shell: running"),
        Err(problem) => {
            println!("shell: {problem}");
            healthy = false;
        }
    }

    let (_, problems) = config::read();

    if problems.is_empty() {
        println!("config: valid");
    } else {
        println!("config: invalid");
        problems.iter().for_each(|problem| println!("  {problem}"));
        healthy = false;
    }

    match healthy {
        true => ExitCode::SUCCESS,
        false => ExitCode::FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::presentation::Surface;

    fn parsed(words: &[&str]) -> Result<(&'static str, Call), Unparsed> {
        parse(words).map(|(module, call)| (module.name, call))
    }

    #[test]
    fn verbs_go_with_their_module() {
        let island = |command| Ok(("island", Call::Island(command)));

        assert_eq!(
            parsed(&["launcher", "toggle"]),
            island(Command::Toggle(Surface::Launcher))
        );
        assert_eq!(
            parsed(&["controls", "close"]),
            island(Command::Close(Surface::Controls))
        );
        assert_eq!(parsed(&["island", "collapse"]), island(Command::Collapse));
        assert_eq!(parsed(&["config", "reload"]), Ok(("island", Call::Reload)));
        assert_eq!(
            parsed(&["config", "validate"]),
            Ok(("island", Call::Validate))
        );
        assert_eq!(parsed(&["status"]), Ok(("island", Call::Status)));
        assert_eq!(
            parsed(&["debug", "withdraw", "timer", "timer"]).map(|(module, _)| module),
            Ok("island")
        );
        assert_eq!(
            parsed(&["media", "open"]),
            Ok(("media", Call::Island(Command::Open(Surface::Media))))
        );
        assert_eq!(
            parsed(&["timer", "start", "25m"]),
            Ok((
                "timer",
                Call::Timer(timer::Request::Start(Duration::from_secs(1500)))
            ))
        );
        assert_eq!(
            parsed(&["timer", "pause"]),
            Ok(("timer", Call::Timer(timer::Request::Pause)))
        );
        assert_eq!(
            parsed(&["timer", "resume"]),
            Ok(("timer", Call::Timer(timer::Request::Resume)))
        );
        assert_eq!(
            parsed(&["timer", "cancel"]),
            Ok(("timer", Call::Timer(timer::Request::Cancel)))
        );
        assert_eq!(
            parsed(&["notifications", "open"]),
            Ok((
                "notifications",
                Call::Island(Command::Open(Surface::Notifications))
            ))
        );
        assert_eq!(
            parsed(&["notifications", "clear"]),
            Ok(("notifications", Call::ClearNotifications))
        );

        // Do Not Disturb only quiets notifications, so it goes with them
        assert_eq!(
            parsed(&["notifications", "dnd", "on"]),
            Ok(("notifications", Call::Island(Command::SetDnd(true))))
        );
        assert_eq!(
            parsed(&["notifications", "dnd", "toggle"]),
            Ok(("notifications", Call::Island(Command::ToggleDnd)))
        );
    }

    #[test]
    fn anything_else_gets_the_usage() {
        for words in [
            &[][..],
            &["open", "launcher"],
            &["launcher"],
            &["launcher", "open", "eDP-1"],
            &["Launcher", "open"],
            &["island"],
            &["island", "open"],
            &["config"],
            &["config", "reload", "now"],
            &["status", "now"],
            &["media", "clear"],
            &["timer"],
            &["timer", "start"],
            &["timer", "start", "0s"],
            &["timer", "stop"],
            &["timer", "pause", "5m"],
            &["notifications", "dnd"],
            &["notifications", "dnd", "maybe"],
            &["notifications", "clear", "all"],
            &["debug"],
            &["doctor"],
            &["help"],
        ] {
            assert_eq!(parsed(words), Err(Unparsed::Usage), "{words:?}");
        }
    }

    #[test]
    fn the_usage_names_every_verb() {
        assert_eq!(
            usage(),
            format!(
                "usage: kanade [<verb> [args]]
with no verb, runs the shell; a verb asks the running one:
launcher open|close|toggle
controls open|close|toggle
island collapse
config reload|validate
status
{}
media open|close|toggle
timer start <duration>|pause|resume|cancel
  <duration>: like 90s, 25m or 1h30m, up to 24h
notifications open|close|toggle|clear
notifications dnd on|off|toggle
doctor
help",
                Command::debug_usage()
            )
        );
    }

    // so the usage lists a verb under its own name, and each name means one verb
    #[test]
    fn each_verb_has_one_owner_and_its_usage_starts_with_its_name() {
        let verbs: Vec<&Verb> = modules::ALL
            .iter()
            .flat_map(|module| module.verbs)
            .collect();

        for (index, verb) in verbs.iter().enumerate() {
            assert!(
                verbs[..index].iter().all(|other| other.name != verb.name),
                "{} is owned twice",
                verb.name
            );
            assert!(!["doctor", "help"].contains(&verb.name));

            for line in (verb.usage)()
                .lines()
                .filter(|line| !line.starts_with("  "))
            {
                assert!(line.starts_with(verb.name), "{line}");
            }
        }
    }

    #[test]
    fn replies_round_trip_and_anything_else_is_no_kanade() {
        for reply in [
            Reply::Done(String::new()),
            Reply::Done(String::from(
                "config schema_version 1, generation 2\nconfig modules.media is pending restart",
            )),
            Reply::Refused(String::from("module media is off")),
        ] {
            assert_eq!(Reply::decode(&reply.encode()), Some(reply));
        }

        assert_eq!(Reply::decode("no handler named kanade"), None);
        assert_eq!(Reply::decode(""), None);
    }
}
