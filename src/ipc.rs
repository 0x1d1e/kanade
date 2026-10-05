//! `amane ipc call island <verb> [args]` (plan 7). Runs on the draw thread and only posts.

use std::time::Instant;

use amane::Service;

use crate::island::command::Command;
use crate::island::service::IslandService;

pub fn island(arguments: &[String]) -> String {
    let Some(command) = Command::parse(arguments) else {
        return Command::usage();
    };

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
