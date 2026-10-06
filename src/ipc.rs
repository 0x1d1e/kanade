//! The shell's side of `kanade <verb> [args]` (`crate::cli`), behind the one IPC handler.
//! Runs on the draw thread; `island` only posts.

use std::time::Instant;

use amane::{Notifications, Service};

use crate::cli::{self, Call, Reply};
use crate::island::command::{Command, Unparsed};
use crate::island::service::IslandService;
use crate::modules;
use crate::reload::{self, Outcome};
use crate::sources::timer;

pub fn answer(arguments: &[String]) -> String {
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();

    let reply = match cli::parse(&words) {
        // a client from another Kanade build may send what this one does not know
        Err(Unparsed::Usage) => Reply::Refused(cli::usage()),
        Err(Unparsed::Invalid(invalid)) => Reply::Refused(invalid.to_string()),
        Ok((module, _)) if !modules::on(module.name) => {
            Reply::Refused(format!("module {} is off", module.name))
        }
        Ok((_, call)) => run(call),
    };

    reply.encode()
}

fn run(call: Call) -> Reply {
    match call {
        Call::Island(command) => island(command),

        // handed to the timer's thread, which posts its Activity itself
        Call::Timer(request) => {
            timer::request(request);
            Reply::Done(String::new())
        }
        Call::ClearNotifications => {
            Notifications::clear();
            Reply::Done(String::new())
        }
        Call::Reload => config(reload::reload(), "reloaded"),
        Call::Validate => config(reload::validate(), "valid"),

        // for now the config's part (#104 adds the rest)
        Call::Status => Reply::Done(reload::status()),
    }
}

fn island(command: Command) -> Reply {
    // its own statement, so the read is released before the write; a write wakes every window
    // even when nothing changed, so only a real change writes
    let effect = IslandService::read().resolve(command);

    match effect {
        Ok(Some(effect)) => IslandService::write().apply(effect, Instant::now()),
        Ok(None) => {}
        Err(error) => return Reply::Refused(error.to_string()),
    }

    Reply::Done(String::new())
}

fn config(outcome: Outcome, valid: &str) -> Reply {
    match outcome {
        Outcome::Valid(pending) => Reply::Done(
            std::iter::once(String::from(valid))
                .chain(
                    pending
                        .iter()
                        .map(|key| format!("{key} is pending restart")),
                )
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        Outcome::Invalid(problems) => Reply::Refused(format!(
            "invalid, the config in effect stays:\n{}",
            problems.join("\n")
        )),
    }
}
