//! `amane ipc call island <verb> [args]` (plan 7). Runs on the draw thread and only posts.

use std::time::Instant;

use amane::Service;

use crate::island::command::Command;
use crate::island::presentation::Surface;
use crate::island::service::IslandService;
use crate::modules;
use crate::sources::timer;

pub fn island(arguments: &[String]) -> String {
    let Some(command) = Command::parse(arguments) else {
        return Command::usage();
    };

    if let Some(module) = needs(&command).filter(|&module| !modules::on(module)) {
        return format!("module {module} is off");
    }

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

// the Module beside the core a command needs: the timer's thread, or a Surface that reads a Service
fn needs(command: &Command) -> Option<&'static str> {
    match command {
        Command::StartTimer(_) | Command::StopTimer => Some("timer"),
        Command::Open(surface) | Command::Toggle(surface) => match surface {
            Surface::Media => Some("media"),
            Surface::Notifications => Some("notifications"),
            Surface::Controls | Surface::Launcher => None,
        },
        Command::Collapse | Command::ToggleDnd | Command::Post(_) | Command::Withdraw(_) => None,
    }
}
