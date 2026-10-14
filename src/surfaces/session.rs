//! The Session Surface: lock, sleep, restart, power off and log out, each a tile. A restart,
//! power off or log out counts down first (`sources::session`), and while one does the Surface
//! says what is left, with Cancel and Now. The arrows move a ring between them, Enter or Space
//! presses it.

use std::time::Instant;

use kanade_runtime::service::Service;
use kanade_runtime::{Key, Rectangle, children};

use super::pad::{self, Focus};
use crate::icon::Icon;
use crate::island::activity::{Ending, Leave};
use crate::island::presentation::Surface;
use crate::modules;
use crate::sources::session::{self, Counting};

// what the ring can be on
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum At {
    Tile(Leave),
    // the countdown drawn, so a press meant for one since replaced does nothing
    Cancel(Counting),
    Now(Counting),
}

impl Service for Focus<At> {
    fn new() -> Self {
        Focus::default()
    }

    fn listen() {}
}

// the tiles, the lock's only while its Module is on
fn leaves() -> Vec<Leave> {
    let mut leaves = Vec::new();

    if modules::on("lock") {
        leaves.push(Leave::Lock);
    }

    leaves.extend([
        Leave::Sleep,
        Leave::Ending(Ending::Restart),
        Leave::Ending(Ending::PowerOff),
        Leave::Ending(Ending::LogOut),
    ]);
    leaves
}

// the targets: the tiles, or Cancel and Now while one counts down
fn grid(counting: Option<&Counting>) -> Vec<Vec<(At, f32)>> {
    let row: Vec<At> = match counting {
        Some(counting) => vec![At::Cancel(*counting), At::Now(*counting)],
        None => leaves().into_iter().map(At::Tile).collect(),
    };
    let count = row.len() as f32;

    vec![
        row.into_iter()
            .enumerate()
            .map(|(index, at)| (at, (index as f32 + 0.5) / count))
            .collect(),
    ]
}

/*
 * the tiles, or what counts down. `open` says the Surface is open rather than fading out, so only
 * then does the ring show
 */
pub fn surface(open: bool, visit: u64, held: bool) -> Rectangle {
    let counting = session::counting();
    let grid = grid(counting.as_ref());
    let ring = open
        .then(|| Focus::<At>::read().of(visit, held).ring(&grid))
        .flatten();

    match counting {
        Some(counting) => pad::frame("Session", countdown(&counting, ring)),
        None => pad::frame("Session", tiles(ring)),
    }
}

fn tiles(ring: Option<At>) -> Rectangle {
    let leaves = leaves();
    let width = pad::tile_width(leaves.len());

    let row = leaves
        .into_iter()
        .map(|leave| {
            Box::new(pad::tile(
                icon(leave),
                name(leave),
                width,
                ring == Some(At::Tile(leave)),
                move || click(At::Tile(leave)),
            )) as _
        })
        .collect();

    pad::tiles(row)
}

// what is left, then Cancel and Now along the bottom
fn countdown(counting: &Counting, ring: Option<At>) -> impl kanade_runtime::Widget + use<> {
    let left = session::left(&counting.countdown, Instant::now());
    let said = pad::reading(
        icon(Leave::Ending(counting.end)),
        format!("{} in {left} s", session::doing(counting.end)),
    );

    let (cancel, now) = (At::Cancel(*counting), At::Now(*counting));

    let pills = pad::pills(children![
        pad::pill("Cancel", ring == Some(cancel), move || click(cancel)),
        pad::pill(
            session::at_once(counting.end),
            ring == Some(now),
            move || click(now)
        )
    ]);

    pad::stacked(said, pills)
}

pub fn icon(leave: Leave) -> Icon {
    match leave {
        Leave::Lock => Icon::Lock,
        Leave::Sleep => Icon::Moon,
        Leave::Ending(Ending::Restart) => Icon::Restart,
        Leave::Ending(Ending::PowerOff) => Icon::Power,
        Leave::Ending(Ending::LogOut) => Icon::LogOut,
        Leave::Ending(Ending::Suspend | Ending::Hibernate) => Icon::Moon,
    }
}

pub fn name(leave: Leave) -> &'static str {
    match leave {
        Leave::Lock => "Lock",
        Leave::Sleep => "Sleep",
        Leave::Ending(Ending::Restart) => "Restart",
        Leave::Ending(Ending::PowerOff) => "Power off",
        Leave::Ending(Ending::LogOut) => "Log out",
        Leave::Ending(Ending::Suspend) => "Sleep",
        Leave::Ending(Ending::Hibernate) => "Hibernate",
    }
}

/*
 * a key while this island shows the Session Surface; false for one it does not use, as Escape,
 * which the window's own keys then get to close it
 */
pub fn key(monitor: &str, key: Key) -> bool {
    pad::key(
        monitor,
        Surface::Session,
        key,
        || grid(session::counting().as_ref()),
        press,
    )
}

fn click(at: At) {
    pad::click(Surface::Session, at, press);
}

/*
 * `at` pressed, giving where that leaves the focus. A restart, power off or log out keeps the
 * Surface open on its countdown, the ring on Cancel, and Cancel brings the ring back to its tile,
 * so a key repeated only counts down again; the rest close it, as does a countdown already gone.
 * One refused shows its refusal on the island, which closes the Surface
 */
fn press(focus: Focus<At>, at: At) -> Focus<At> {
    match at {
        At::Tile(leave) => {
            let requested = session::request(leave);

            if let (Leave::Ending(_), Some(counting)) =
                (leave, requested.as_ref().ok().and(session::counting()))
            {
                return Focus {
                    ring: focus.ring.onto(At::Cancel(counting)),
                };
            }

            // refused, its refusal closed the Surface already
            if requested.is_err() {
                return focus;
            }
        }
        At::Cancel(counting) => {
            session::act(session::CANCEL, &counting.serial.to_string());

            return Focus {
                ring: focus.ring.onto(At::Tile(Leave::Ending(counting.end))),
            };
        }
        At::Now(counting) => session::act(session::NOW, &counting.serial.to_string()),
    }

    pad::close();
    focus
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::ring::Ring;
    use super::*;
    use crate::island::activity::Countdown;

    #[test]
    fn a_hidden_ring_is_shown_by_a_key_before_any_key_presses() {
        let grid = vec![vec![
            (At::Tile(Leave::Sleep), 0.25),
            (At::Tile(Leave::Lock), 0.75),
        ]];
        let hidden = Focus::<At>::default().of(1, false);

        let (shown, act) = hidden.step(Key::Enter, &grid).unwrap();

        assert_eq!(act, None);
        assert_eq!(shown.ring(&grid), Some(At::Tile(Leave::Sleep)));
        assert_eq!(
            shown.step(Key::Enter, &grid).unwrap().1,
            Some(At::Tile(Leave::Sleep))
        );
    }

    fn counting(serial: u64) -> Counting {
        Counting {
            end: Ending::Restart,
            countdown: Countdown::new(Duration::from_secs(30), Instant::now()),
            serial,
        }
    }

    // a key meant for one countdown must never press another that replaced it unseen
    #[test]
    fn a_replaced_countdown_hides_the_ring_until_a_key_shows_it() {
        let (a, b) = (counting(1), counting(2));
        let before = grid(Some(&a));
        let after = grid(Some(&b));

        let ringed = Focus {
            ring: Ring::start(1, true).onto(At::Cancel(a)),
        };

        assert_eq!(ringed.ring(&before), Some(At::Cancel(a)));
        assert_eq!(ringed.ring(&after), None);

        let (revealed, act) = ringed.step(Key::Enter, &after).unwrap();

        assert_eq!(act, None);
        assert_eq!(revealed.ring(&after), Some(At::Cancel(b)));

        let (_, act) = revealed.step(Key::Enter, &after).unwrap();

        assert_eq!(act, Some(At::Cancel(b)));
    }
}
