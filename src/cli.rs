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

use crate::doctor;
use crate::island::command::{Command, Unparsed};
use crate::modules::{self, Module};
use crate::sources::{capture, timer};

// the one IPC handler the shell registers, which every verb goes through
pub const HANDLER: &str = "kanade";

// the words a call and its reply are made of; another number means a client and shell that may
// misread each other
pub const PROTOCOL: u32 = 2;

/*
 * the first word after `kanade`, with the arguments a Module owns; Modules may share a name, each
 * owning its own arguments, like `notifications clear` and `notifications open`
 */
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
    ClearClipboard,
    Screenshot(capture::Mode),
    Reload,
    Validate,
    Status,
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

// every verb, in the Modules' order with the lines of one name together, then those this process answers itself
pub fn usage() -> String {
    let mut names: Vec<&str> = Vec::new();

    for (_, verb) in verbs() {
        if !names.contains(&verb.name) {
            names.push(verb.name);
        }
    }

    let verbs = names.into_iter().flat_map(|name| {
        verbs()
            .filter(move |(_, verb)| verb.name == name)
            .map(|(_, verb)| (verb.usage)())
    });

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

/*
 * `kanade <verb> [args]`: 2 for words that are no verb, 1 for a refusal or no shell to ask, 3 when
 * it is not known whether it was done, as the shell or niri took it but did not answer
 */
pub fn run(arguments: &[String]) -> ExitCode {
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();

    match words[..] {
        ["help" | "-h" | "--help"] => {
            println!("{}", usage());
            return ExitCode::SUCCESS;
        }
        ["doctor"] => return doctor::run(),
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
        Ok(Reply::Unknown(text)) => {
            eprintln!("kanade: {text}");
            ExitCode::from(3)
        }
    }
}

// what the running shell answers, or why there is none to ask
pub fn call(arguments: &[String]) -> Result<Reply, String> {
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

    send(stream, &IpcCall::new(HANDLER, arguments), PATIENCE)
}

/*
 * the shell's reply to `call`; one that is late or missing once the call is sent leaves unknown
 * whether the shell did it, like a screenshot niri may already be saving
 */
fn send(mut stream: UnixStream, call: &IpcCall, patience: Duration) -> Result<Reply, String> {
    let sent = stream
        .set_read_timeout(Some(patience))
        .and_then(|()| call.write(&mut stream))
        // closing our side is how the shell knows the call is complete
        .and_then(|()| stream.shutdown(Shutdown::Write));

    sent.map_err(|error| format!("the shell closed the connection: {error}"))?;

    let mut reply = String::new();
    let unanswered = match stream.read_to_string(&mut reply) {
        Ok(_) if reply.is_empty() => Some(String::from("closed the connection without answering")),
        Ok(_) => None,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            Some(format!("did not answer within {patience:?}"))
        }
        Err(error) => Some(format!("closed the connection: {error}")),
    };

    if let Some(why) = unanswered {
        return Ok(Reply::Unknown(format!(
            "the shell got the call, but {why}; it may still have done it"
        )));
    }

    Reply::decode(&reply).ok_or_else(|| String::from("the running Amane shell is not Kanade"))
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
            Ok(("launcher", Call::Island(Command::Toggle(Surface::Launcher))))
        );
        assert_eq!(
            parsed(&["controls", "close"]),
            Ok(("controls", Call::Island(Command::Close(Surface::Controls))))
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
        // their Surface is its own Module, which shares the verb
        assert_eq!(
            parsed(&["notifications", "open"]),
            Ok((
                "notification-surface",
                Call::Island(Command::Open(Surface::Notifications))
            ))
        );
        assert_eq!(
            parsed(&["notifications", "clear"]),
            Ok(("notifications", Call::ClearNotifications))
        );

        assert_eq!(
            parsed(&["capture", "screenshot", "window"]),
            Ok(("capture", Call::Screenshot(capture::Mode::Window)))
        );

        // likewise the clipboard history and its Surface
        assert_eq!(
            parsed(&["clipboard", "clear"]),
            Ok(("clipboard", Call::ClearClipboard))
        );
        assert_eq!(
            parsed(&["clipboard", "open"]),
            Ok((
                "clipboard-surface",
                Call::Island(Command::Open(Surface::Clipboard))
            ))
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
            &["clipboard"],
            &["clipboard", "clear", "all"],
            &["clipboard", "delete"],
            &["capture"],
            &["capture", "screenshot"],
            &["capture", "screenshot", "all"],
            &["capture", "record", "area"],
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
island collapse
config reload|validate
status
{}
media open|close|toggle
timer start <duration>|pause|resume|cancel
  <duration>: like 90s, 25m or 1h30m, up to 24h
notifications clear
notifications dnd on|off|toggle
notifications open|close|toggle
tray open|close|toggle
clipboard clear
clipboard open|close|toggle
controls open|close|toggle
launcher open|close|toggle
capture screenshot area|window|output
doctor
help",
                Command::debug_usage()
            )
        );
    }

    // so the usage lists a verb under its own name; Modules sharing one each own their arguments
    #[test]
    fn a_module_owns_a_verb_once_and_its_usage_starts_with_its_name() {
        for module in modules::ALL {
            for (index, verb) in module.verbs.iter().enumerate() {
                assert!(
                    module.verbs[..index]
                        .iter()
                        .all(|other| other.name != verb.name),
                    "{} owns {} twice",
                    module.name,
                    verb.name
                );
            }
        }

        for (_, verb) in verbs() {
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
            Err(String::from("the running Amane shell is not Kanade"))
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
