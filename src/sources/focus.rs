//! Focus (ADR 0060): one switch for the pieces that quiet the session, Do Not Disturb, caffeine and
//! the night light, as `focus.*` chooses. It owns no thread and no state of its own beyond what it
//! turned on: each piece that was already on is left alone, and `off` turns off only a piece still
//! as `on` left it, so a Do Not Disturb, caffeine or night light mode the user set meanwhile is
//! theirs, even one set back to what Focus left. What it
//! turned on is kept in memory, so a restart forgets it and leaves those pieces as they are; the
//! night light, kept in the settings file, then stays on until turned off.
//!
//! A piece whose Module is off is skipped, so Focus works with any of them.

use std::sync::Mutex;
use std::time::Instant;

use kanade_runtime::MutexExt;
use kanade_runtime::service::Service;

use super::{caffeine, night_light, notifications};
use crate::config::{self, NightMode};
use crate::island::service::IslandService;
use crate::later::Later;
use crate::modules;

/*
 * what Focus turned on and how it left it, so `off` can tell a piece still as it left it from one
 * the user changed since: each by a number that changes whenever the piece does, even back to the
 * value it had: DND's setting, caffeine's holder serial, the night light's mode change
 */
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Held {
    dnd: Option<u64>,
    caffeine: Option<u64>,
    night_light: Option<u64>,
}

// what Focus turned on, none while it is off
static HELD: Mutex<Option<Held>> = Mutex::new(None);

// whether `on` turns a piece on: wanted, and observably off as it found it
fn turns_on(wanted: bool, was_off: bool) -> bool {
    wanted && was_off
}

/*
 * whether `off` turns a piece off: Focus turned it on and left it as `set`, and it still is. Any
 * other state is the user's, or the piece ended on its own, and is left as it is
 */
fn turns_off<S: PartialEq>(set: Option<S>, now: Option<S>) -> bool {
    set.is_some() && set == now
}

/*
 * whether Focus is on, as a Service so the Controls tile draws again when it changes, even when every
 * piece it turns on was already on and nothing else moved
 */
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Quiet {
    pub on: bool,
}

impl Service for Quiet {
    fn new() -> Self {
        Quiet::default()
    }

    fn listen() {}
}

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
        (held.dnd.is_some(), "do not disturb"),
        (held.caffeine.is_some(), "caffeine"),
        (held.night_light.is_some(), "night light"),
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

    Quiet::write().on = held.is_some();

    problems
}

// turns on each piece `focus.*` wants that is available and off, and says which it did
fn engage(problems: &mut Vec<String>) -> Held {
    let wanted = config::get().focus;
    let mut held = Held {
        dnd: None,
        caffeine: None,
        night_light: None,
    };

    let dnd_off = !IslandService::read().dnd();

    if modules::on("notifications") && turns_on(wanted.dnd, dnd_off) {
        notifications::set_dnd(true, Instant::now());
        held.dnd = Some(notifications::dnd_revision());
    }

    // starting counts as on, as the user's own `on` may be about to hold
    let caffeine_off = caffeine::newest().is_none();

    if modules::on("caffeine") && turns_on(wanted.caffeine, caffeine_off) {
        caffeine::request(caffeine::Request::On(None), Later::detached());

        // none if it did not start, which caffeine shows as its own failure
        held.caffeine = caffeine::newest();

        if held.caffeine.is_none() {
            problems.push(String::from("caffeine: did not turn on, see the island"));
        }
    }

    let light_off = config::get().night_light.mode == NightMode::Off;

    if modules::on("night-light") && turns_on(wanted.night_light, light_off) {
        match night_light::turn(true) {
            Ok(()) => held.night_light = Some(night_light::revision()),
            Err(why) => problems.push(format!("night light: {why}")),
        }
    }

    held
}

// turns off the pieces Focus turned on that are still as it left them
fn release(held: Held, problems: &mut Vec<String>) {
    if turns_off(held.dnd, Some(notifications::dnd_revision())) {
        notifications::set_dnd(false, Instant::now());
    }

    if turns_off(held.caffeine, caffeine::newest()) {
        caffeine::request(caffeine::Request::Off, Later::detached());
    }

    if turns_off(held.night_light, Some(night_light::revision()))
        && let Err(why) = night_light::turn(false)
    {
        problems.push(format!("night light: {why}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `off` undoes only what `on` did and the user has not changed since
    #[test]
    fn off_restores_only_what_on_did() {
        // on: a piece already on is left alone, one not wanted is not turned on
        assert!(turns_on(true, true));
        assert!(!turns_on(true, false));
        assert!(!turns_on(false, true));

        // off: a piece still as Focus left it, by the number it gave: a holder's serial, or the
        // count of times DND was set or the night light's mode changed
        assert!(turns_off(Some(3_u64), Some(3)));

        // never turned on by Focus, ended on its own or turned off, or the user's own `on` since
        assert!(!turns_off(None, Some(3_u64)));
        assert!(!turns_off(Some(3_u64), None));
        assert!(!turns_off(Some(3_u64), Some(4)));

        // a user who turned DND or the night light off and on again moved the number twice, so the
        // piece is back at the value Focus set and is still theirs
        assert!(!turns_off(Some(3_u64), Some(5)));
    }
}
