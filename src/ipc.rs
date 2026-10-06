//! `amane ipc call island <verb> [args]` (plan 7). Runs on the draw thread and only posts.

use std::time::Instant;

use amane::Service;

use crate::island::command::{Command, Unparsed};
use crate::island::presentation::Surface;
use crate::island::service::IslandService;
use crate::modules;
use crate::sources::timer;

pub fn island(arguments: &[String]) -> String {
    let command = match Command::parse(arguments) {
        Ok(command) => command,
        Err(Unparsed::Invalid(invalid)) => return invalid.to_string(),
        Err(Unparsed::Usage) => return Command::usage(),
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

// the Module beside the core a command needs: the timer's thread, a Surface that reads a Service,
// or Do Not Disturb, which only notifications heed
fn needs(command: &Command) -> Option<&'static str> {
    match command {
        Command::StartTimer(_) | Command::StopTimer => Some("timer"),
        Command::Open(surface) | Command::Toggle(surface) => match surface {
            Surface::Media => Some("media"),
            Surface::Notifications => Some("notifications"),
            Surface::Controls | Surface::Launcher => None,
        },
        Command::ToggleDnd => Some("notifications"),
        Command::Collapse | Command::Post(_) | Command::Withdraw(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Do Not Disturb only quiets notifications, so it goes with them
    #[test]
    fn verbs_go_with_their_module() {
        assert_eq!(needs(&Command::ToggleDnd), Some("notifications"));
        assert_eq!(needs(&Command::StopTimer), Some("timer"));
        assert_eq!(needs(&Command::Open(Surface::Media)), Some("media"));
        assert_eq!(needs(&Command::Toggle(Surface::Controls)), None);
        assert_eq!(needs(&Command::Collapse), None);
    }
}
