//! `amane ipc call island <verb> [args]` (plan 7). Runs on the draw thread and only posts.

use std::time::Instant;

use amane::Service;

use crate::island::command::Command;
use crate::island::service::IslandService;
use crate::sources::timer;

pub fn island(arguments: &[String]) -> String {
    let Some(command) = Command::parse(arguments) else {
        return Command::usage();
    };

    // handed to the timer's thread, which posts its Activity itself
    match command {
        Command::StartTimer(length) => {
            timer::start(length);
            return String::new();
        }
        Command::StopTimer => {
            timer::stop();
            return String::new();
        }
        _ => {}
    }

    // its own statement, so the read is released before the write; a write wakes every window
    // even when nothing changed, so only a real change writes
    let effect = IslandService::read().resolve(command);

    match effect {
        Ok(Some(effect)) => IslandService::write().apply(effect, Instant::now()),
        Ok(None) => {}
        Err(error) => return error.to_string(),
    }

    String::new()
}
