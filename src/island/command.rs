//! What `kanade <verb>` asks of the island (docs/design.md CLI). Pure: words in, Command out;
//! `crate::cli` says which words reach which of these.

use std::time::Duration;

use super::activity::{Activity, Id, Interrupt, InvalidActivity, Kind, Lifetime, Priority, Scope};
use super::presentation::Surface;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Open(Surface),

    // collapses the focused island when it shows this Surface
    Close(Surface),

    // open, or collapse when the focused island already shows it
    Toggle(Surface),

    Collapse,

    SetDnd(bool),
    ToggleDnd,

    // a fake Activity, for testing what the island does with one
    Post(Activity),
    Withdraw(Id),
}

impl Command {
    // `open|close|toggle`, after the Surface's name
    pub fn surface(surface: Surface, arguments: &[&str]) -> Option<Command> {
        match *arguments {
            ["open"] => Some(Command::Open(surface)),
            ["close"] => Some(Command::Close(surface)),
            ["toggle"] => Some(Command::Toggle(surface)),
            _ => None,
        }
    }

    // `on|off|toggle`, after `dnd`
    pub fn dnd(arguments: &[&str]) -> Option<Command> {
        match *arguments {
            ["on"] => Some(Command::SetDnd(true)),
            ["off"] => Some(Command::SetDnd(false)),
            ["toggle"] => Some(Command::ToggleDnd),
            _ => None,
        }
    }

    /*
     * `post ...|withdraw ...`, after `debug`; a fake Activity whose policy cannot be carried out
     * says why, anything else unknown gets the usage text
     */
    pub fn debug(arguments: &[&str]) -> Result<Command, Unparsed> {
        match *arguments {
            ["post", kind, key, priority, lifetime, scope, interrupt] => {
                let id = Id::new(Kind::parse(kind).ok_or(Unparsed::Usage)?, key);
                let policy = (|| {
                    Some((
                        Priority::parse(priority)?,
                        parse_lifetime(lifetime)?,
                        parse_scope(scope)?,
                        parse_interrupt(interrupt)?,
                    ))
                })();
                let (priority, lifetime, scope, interrupt) = policy.ok_or(Unparsed::Usage)?;

                Activity::new(id, priority, lifetime, scope, interrupt)
                    .map(Command::Post)
                    .map_err(Unparsed::Invalid)
            }
            ["withdraw", kind, key] => Kind::parse(kind)
                .map(|kind| Command::Withdraw(Id::new(kind, key)))
                .ok_or(Unparsed::Usage),
            _ => Err(Unparsed::Usage),
        }
    }

    // the `debug` verbs and what their arguments take
    pub fn debug_usage() -> String {
        let kinds: Vec<&str> = Kind::ALL.into_iter().map(Kind::name).collect();
        let priorities: Vec<&str> = Priority::ALL.into_iter().map(Priority::name).collect();

        format!(
            "debug post <kind> <key> <priority> <lifetime> <scope> <interrupt>
debug withdraw <kind> <key>
  <kind>: {}
  <priority>: {}
  <lifetime>: persistent|<ms>
  <scope>: global|focused-output
  <interrupt>: none|preempt|auto-expand:<ms>",
            kinds.join("|"),
            priorities.join("|")
        )
    }
}

// why arguments make no Command
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unparsed {
    // not a verb, or not its arguments
    Usage,

    // a fake Activity with a policy that cannot be carried out
    Invalid(InvalidActivity),
}

// a Transient's milliseconds, zero included so `Activity::new` is the one to refuse it
fn milliseconds(text: &str) -> Option<Duration> {
    text.parse().ok().map(Duration::from_millis)
}

fn parse_lifetime(text: &str) -> Option<Lifetime> {
    match text {
        "persistent" => Some(Lifetime::Persistent),
        ms => milliseconds(ms).map(Lifetime::Transient),
    }
}

fn parse_scope(text: &str) -> Option<Scope> {
    match text {
        "global" => Some(Scope::Global),
        "focused-output" => Some(Scope::FocusedOutput),
        _ => None,
    }
}

fn parse_interrupt(text: &str) -> Option<Interrupt> {
    match text {
        "none" => Some(Interrupt::None),
        "preempt" => Some(Interrupt::Preempt),
        _ => milliseconds(text.strip_prefix("auto-expand:")?).map(Interrupt::AutoExpand),
    }
}
