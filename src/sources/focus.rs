//! Focus (ADR 0060): one switch for the pieces that quiet the session, Do Not Disturb, caffeine and
//! the night light, as `focus.*` chooses. It owns no thread and no state of its own beyond what it
//! turned on: each piece that was already on is left alone, and `off` turns off only a piece still
//! as `on` left it, so a Do Not Disturb, caffeine or night light mode the user set meanwhile is
//! theirs, even one set back to what Focus left. What it
//! turned on is kept in memory, so a restart forgets it and leaves those pieces as they are; the
//! night light, kept in the settings file, then stays on until turned off.
//!
//! A piece whose Module is off is skipped, so Focus works with any of them.
//!
//! `focus.auto` may turn Focus on by itself (ADR 0078): a screen cast turns it on and its end turns
//! it off again, but only the Focus the cast turned on. Any `on`, `off` or `toggle` meanwhile
//! replaces it, or claims it, so it is the user's and stays.

use std::sync::Mutex;
use std::time::Instant;

use kanade_runtime::MutexExt;
use kanade_runtime::service::Service;

use super::{caffeine, night_light, notifications};
use crate::config::{self, NightMode};
use crate::island::service::IslandService;
use crate::later::Later;
use crate::modules;
use crate::sources::privacy::Privacy;

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

    // turned on by a trigger and not touched by the user since, so its end may turn it off
    auto: bool,
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

// what a request does to Focus
#[derive(Debug, PartialEq, Eq)]
enum Move {
    Engage,

    // the user's `on` while a trigger's Focus is on: it is theirs now
    Claim,
    Release,
    Keep,
}

/*
 * what turning Focus `on` or off does, given whether it is on and was turned on by a trigger
 * (`held`), and whether a trigger asks (`by_trigger`). A trigger's end releases only a Focus a
 * trigger turned on and the user has not touched
 */
fn next(held: Option<bool>, on: bool, by_trigger: bool) -> Move {
    match (held, on) {
        (None, true) => Move::Engage,
        (Some(true), true) if !by_trigger => Move::Claim,
        (Some(auto), false) if !by_trigger || auto => Move::Release,
        _ => Move::Keep,
    }
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
    let how = if held.auto { " (screen cast)" } else { "" };
    let pieces: Vec<&str> = [
        (held.dnd.is_some(), "do not disturb"),
        (held.caffeine.is_some(), "caffeine"),
        (held.night_light.is_some(), "night light"),
    ]
    .into_iter()
    .filter_map(|(turned, name)| turned.then_some(name))
    .collect();

    if pieces.is_empty() {
        format!("on{how}")
    } else {
        format!("on{how}: {}", pieces.join(", "))
    }
}

// turns Focus on or off, a no-op when it already is, and returns why a piece did not turn
fn turn(on: bool) -> Vec<String> {
    change(on, false)
}

// as `turn`, for a trigger when `by_trigger`
fn change(on: bool, by_trigger: bool) -> Vec<String> {
    let mut held = HELD.locked();
    let mut problems = Vec::new();

    match next(held.as_ref().map(|held| held.auto), on, by_trigger) {
        Move::Engage => {
            *held = Some(Held {
                auto: by_trigger,
                ..engage(&mut problems)
            });
        }
        Move::Claim => {
            if let Some(held) = held.as_mut() {
                held.auto = false;
            }
        }
        Move::Release => {
            if let Some(turned) = held.take() {
                release(turned, &mut problems);
            }
        }
        Move::Keep => {}
    }

    Quiet::write().on = held.is_some();

    problems
}

/*
 * a watcher of `Privacy` for `focus.auto`: a screen cast turns Focus on when it starts and off when
 * it ends, the latter only for a Focus it turned on itself. Not read again when the setting
 * changes mid-cast, nor before the first change of `Privacy`
 */
pub fn follow_cast() {
    let casting = Privacy::read().casting();
    let problems = if casting {
        if !config::get().focus.cast {
            return;
        }

        change(true, true)
    } else {
        change(false, true)
    };

    for problem in problems {
        eprintln!("kanade: focus: {problem}");
    }
}

// turns on each piece `focus.*` wants that is available and off, and says which it did
fn engage(problems: &mut Vec<String>) -> Held {
    let wanted = config::get().focus;
    let mut held = Held {
        dnd: None,
        caffeine: None,
        night_light: None,
        auto: false,
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

    // a cast ending turns off only the Focus a cast turned on, never one the user touched
    #[test]
    fn a_trigger_releases_only_the_focus_it_turned_on() {
        // not on: a cast turns it on, its end does nothing
        assert_eq!(next(None, true, true), Move::Engage);
        assert_eq!(next(None, false, true), Move::Keep);

        // on by a cast: its end releases it, the user's `on` claims it, their `off` releases it
        assert_eq!(next(Some(true), false, true), Move::Release);
        assert_eq!(next(Some(true), true, false), Move::Claim);
        assert_eq!(next(Some(true), false, false), Move::Release);

        // the user's own, or claimed: a cast ending leaves it, and another cast changes nothing
        assert_eq!(next(Some(false), false, true), Move::Keep);
        assert_eq!(next(Some(false), true, true), Move::Keep);
        assert_eq!(next(Some(true), true, true), Move::Keep);
    }
}
