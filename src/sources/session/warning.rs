//! What a restart, power off or log out would lose, for the countdown to say (ADR 0077): from
//! Kanade's own state (a recording, a Timer, live Activities) and, for a restart or power off, the
//! block inhibitors on `shutdown` that logind lists, which make it refuse. A warning only: nothing
//! here refuses or delays anything.
//!
//! logind is asked once per countdown, on a thread of its own, so the Surface never waits on the
//! bus. A logind that does not answer lists nothing. Who and why are the inhibiting program's own
//! words, so they are cleaned and cut short before they are kept, and they are never logged.

use std::sync::Mutex;
use std::thread;

use kanade_runtime::MutexExt;
use kanade_runtime::service::Service;
use zbus::blocking::Connection;

use super::{LOGIND, MANAGER, PATH, Seconds};
use crate::island::activity::{Detail, Ending};
use crate::island::service::IslandService;
use crate::sources::{recording, timer};

// how many lines the countdown has room for
pub const MOST: usize = 3;

// how much of an inhibitor's name and reason is kept
const WHO: usize = 24;
const WHY: usize = 40;

// one logind lists: what, who, why, mode, uid, pid
type Listed = (String, String, String, String, u32, u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocker {
    pub who: String,
    pub why: String,
}

// the blockers of the countdown with this serial, none until logind answered
static BLOCKERS: Mutex<Option<(u64, Vec<Blocker>)>> = Mutex::new(None);

// asks logind who blocks `end`, for the countdown `serial` names; a log out is blocked by none
pub fn look(serial: u64, end: Ending) {
    if !matches!(end, Ending::Restart | Ending::PowerOff) {
        return;
    }

    let spawned = thread::Builder::new()
        .name(String::from("session-inhibitors"))
        .spawn(move || {
            let blockers = list().map(blocking).unwrap_or_default();

            if blockers.is_empty() {
                return;
            }
            *BLOCKERS.locked() = Some((serial, blockers));

            // so the countdown draws them
            drop(Seconds::write());
        });

    // a countdown without the inhibitors still warns of Kanade's own state
    drop(spawned);
}

fn list() -> Option<Vec<Listed>> {
    Connection::system()
        .ok()?
        .call_method(Some(LOGIND), PATH, Some(MANAGER), "ListInhibitors", &())
        .ok()?
        .body()
        .deserialize()
        .ok()
}

// the shutdown blockers among those listed, cleaned
fn blocking(listed: Vec<Listed>) -> Vec<Blocker> {
    listed
        .into_iter()
        .filter(|(what, _, _, mode, ..)| {
            mode == "block" && what.split(':').any(|w| w == "shutdown")
        })
        .map(|(_, who, why, ..)| Blocker {
            who: clean(&who, WHO),
            why: clean(&why, WHY),
        })
        .collect()
}

// zero-width and bidi controls (Unicode Cf), which could reorder or hide what a blocker says
fn is_format(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}' | '\u{E0000}'..='\u{E007F}')
}

// `text` on one line without control or format characters, cut to `most` characters
fn clean(text: &str, most: usize) -> String {
    let one: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control() && !is_format(*c))
        .collect();

    if one.chars().count() > most {
        let mut cut: String = one.chars().take(most - 1).collect();

        cut.push('…');
        cut
    } else {
        one
    }
}

/*
 * what the countdown `serial` names would lose, at most `MOST` lines, the last saying how many
 * more there are. Read fresh at each draw, so a recording stopped meanwhile drops off
 */
pub fn lines(serial: u64) -> Vec<String> {
    let (recording, live) = {
        let island = IslandService::read();
        let live = island
            .activities()
            .filter(
                |activity| matches!(activity.detail(), Detail::Live(job) if job.ended.is_none()),
            )
            .count();

        (recording::recording(&island).is_some(), live)
    };
    let blockers = BLOCKERS
        .locked()
        .as_ref()
        .filter(|(held, _)| *held == serial)
        .map(|(_, blockers)| blockers.clone())
        .unwrap_or_default();

    summarize(recording, timer::standing().is_some(), live, &blockers)
}

fn summarize(recording: bool, timer: bool, live: usize, blockers: &[Blocker]) -> Vec<String> {
    let mut lines = Vec::new();

    if recording {
        lines.push(String::from("A recording is running"));
    }
    if timer {
        lines.push(String::from("A timer is running"));
    }
    match live {
        0 => {}
        1 => lines.push(String::from("1 live Activity is running")),
        count => lines.push(format!("{count} live Activities are running")),
    }
    lines.extend(
        blockers
            .iter()
            .map(|blocker| format!("Blocked by {}: {}", blocker.who, blocker.why)),
    );

    if lines.len() > MOST {
        let more = lines.len() - (MOST - 1);

        lines.truncate(MOST - 1);
        lines.push(format!("and {more} more"));
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(what: &str, who: &str, why: &str, mode: &str) -> Listed {
        (what.into(), who.into(), why.into(), mode.into(), 1000, 1)
    }

    // logind's list is outside input, its words an inhibiting program's own
    #[test]
    fn only_block_inhibitors_on_shutdown_stay_and_their_words_are_cleaned() {
        let kept = blocking(vec![
            listed("sleep", "Firefox", "playing", "block"),
            listed("shutdown", "Burner", "burning", "delay"),
            listed(
                "idle:shutdown",
                "Disk\x1b[31m\ncopy",
                &"x".repeat(100),
                "block",
            ),
            listed("handle-power-key", "systemd", "", "block"),
        ]);

        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].who, "Disk[31m copy");
        assert!(!kept[0].why.chars().any(char::is_control));
        assert_eq!(kept[0].why.chars().count(), WHY);
        assert!(kept[0].why.ends_with('…'));
    }

    #[test]
    fn too_many_lines_end_in_how_many_more() {
        let blocker = |who: &str| Blocker {
            who: who.into(),
            why: String::from("busy"),
        };

        assert_eq!(
            summarize(true, true, 2, &[blocker("a"), blocker("b")]),
            ["A recording is running", "A timer is running", "and 3 more"]
        );
        assert!(summarize(false, false, 0, &[]).is_empty());
    }
}
