//! Focus (ADR 0060): one switch for the pieces that quiet the session, Do Not Disturb, caffeine and
//! the night light, as `focus.*` chooses. It owns no thread and no state of its own beyond what it
//! turned on: each piece that was already on is left alone, so `off` restores the session to how
//! `on` found it. What it turned on is kept in memory, so a restart forgets it and leaves those
//! pieces as they are; the night light, kept in the settings file, then stays on until turned off.
//!
//! A piece whose Module is off is skipped, so Focus works with any of them.

use std::sync::Mutex;
use std::time::Instant;

use kanade_runtime::MutexExt;
use kanade_runtime::service::Service;

use super::{caffeine, night_light, notifications};
use crate::config::{self, Focus, NightMode};
use crate::island::service::IslandService;
use crate::later::Later;
use crate::modules;

// the pieces Focus turned on, none while it is off
static HELD: Mutex<Option<Focus>> = Mutex::new(None);

// what `kanade focus` asks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    On,
    Off,
    Toggle,
    Status,
}

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        match arguments {
            ["on"] => Some(Request::On),
            ["off"] => Some(Request::Off),
            ["toggle"] => Some(Request::Toggle),
            ["status"] => Some(Request::Status),
            _ => None,
        }
    }
}

// whether Focus is on
pub fn on() -> bool {
    HELD.locked().is_some()
}

/*
 * does what `request` asks and says how Focus is left, with why a piece did not turn. Call it from
 * the draw thread, which caffeine's inhibitor and the settings file's writes belong to
 */
pub fn request(request: Request) -> Result<String, String> {
    let problems = match request {
        Request::Status => Vec::new(),
        Request::On => turn(true),
        Request::Off => turn(false),
        Request::Toggle => turn(!on()),
    };

    Ok(std::iter::once(status())
        .chain(problems)
        .collect::<Vec<_>>()
        .join("\n"))
}

// the Controls tile pressed: a piece that did not turn is said on stderr
pub fn toggle() {
    for problem in turn(!on()) {
        eprintln!("kanade: focus: {problem}");
    }
}

// "off", or "on" with the pieces it turned on
pub fn status() -> String {
    let Some(held) = *HELD.locked() else {
        return String::from("off");
    };
    let pieces: Vec<&str> = [
        (held.dnd, "do not disturb"),
        (held.caffeine, "caffeine"),
        (held.night_light, "night light"),
    ]
    .into_iter()
    .filter_map(|(turned, name)| turned.then_some(name))
    .collect();

    if pieces.is_empty() {
        String::from("on")
    } else {
        format!("on: {}", pieces.join(", "))
    }
}

// turns Focus on or off, a no-op when it already is, and returns why a piece did not turn
fn turn(on: bool) -> Vec<String> {
    let mut held = HELD.locked();
    let mut problems = Vec::new();

    if on {
        if held.is_none() {
            *held = Some(engage(&mut problems));
        }
    } else if let Some(turned) = held.take() {
        release(turned, &mut problems);
    }

    problems
}

// turns on each piece `focus.*` wants that is available and off, and says which it did
fn engage(problems: &mut Vec<String>) -> Focus {
    let wanted = config::get().focus;
    let mut turned = Focus {
        dnd: false,
        caffeine: false,
        night_light: false,
    };

    if wanted.dnd && modules::on("notifications") && !IslandService::read().dnd() {
        notifications::set_dnd(true, Instant::now());
        turned.dnd = true;
    }

    // a refusal, like a missing program, shows as caffeine's own failure
    if wanted.caffeine && modules::on("caffeine") && !caffeine::holding() {
        caffeine::request(caffeine::Request::On(None), Later::detached());
        turned.caffeine = true;
    }

    if wanted.night_light
        && modules::on("night-light")
        && config::get().night_light.mode == NightMode::Off
    {
        match night_light::turn(true) {
            Ok(()) => turned.night_light = true,
            Err(why) => problems.push(format!("night light: {why}")),
        }
    }

    turned
}

// turns off the pieces Focus turned on, but not one the user already turned off
fn release(turned: Focus, problems: &mut Vec<String>) {
    if turned.dnd && IslandService::read().dnd() {
        notifications::set_dnd(false, Instant::now());
    }

    if turned.caffeine {
        caffeine::request(caffeine::Request::Off, Later::detached());
    }

    if turned.night_light
        && let Err(why) = night_light::turn(false)
    {
        problems.push(format!("night light: {why}"));
    }
}
