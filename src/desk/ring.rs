//! The Desk's keyboard ring over the media buttons that can act (ADR 0148). It is not an Island
//! visit and not a Surface ring: the pointer already has the window's `Keyboard::OnDemand`, asked
//! for while it is on a button that can act, and the first Left or Right only draws the ring. The
//! next Enter or Space runs that button's existing `media::request`. Escape clears the ring and
//! presses nothing.
//!
//! A note being typed into and a face held by its handle take their keys first, so this ring never
//! sees them. One visit exists at a time, and it belongs to one monitor: a key on another output
//! leaves it and does nothing. A player change that disables the visited button clears the ring
//! rather than leaving it on a dead control. No timer: the key that arrives already wakes the window.

use std::sync::Mutex;

use kanade_runtime::Key;
use kanade_runtime::MutexExt;

use crate::sources::media::Transport;
use crate::sources::playback::Keys;

// previous, play-pause, next, in the order the row draws them
const ORDER: [Transport; 3] = [Transport::Previous, Transport::PlayPause, Transport::Next];

// the ring on one output: hidden until a Left or Right shows it, then on one button that can act
#[derive(Debug, Clone, PartialEq, Eq)]
struct Visit {
    monitor: String,
    at: Transport,
    shown: bool,
}

// one visit across the outputs: the monitor that holds it is the only one a key may move or show
#[derive(Debug, Default)]
struct Rings {
    visit: Option<Visit>,
    // the output whose pointer is on a media button that can act, so that window asks for the keyboard
    over: Option<String>,
}

static RINGS: Mutex<Rings> = Mutex::new(Rings {
    visit: None,
    over: None,
});

impl Rings {
    // whether this output's window asks for the keyboard: the pointer is on a button that can act
    fn keyboard(&self, monitor: &str) -> bool {
        self.over.as_deref() == Some(monitor)
    }

    // the button the ring shows on this output, none while it hides or the output is not its
    fn on(&self, monitor: &str) -> Option<Transport> {
        let visit = self.visit.as_ref()?;

        (visit.shown && visit.monitor == monitor).then_some(visit.at)
    }

    /*
     * a key the ring may take, once notes and a held drag have had it. None when the key is not the
     * ring's, so the window below can have the next character. The first Left or Right only shows
     * the ring; a later one moves it among buttons that can act; Enter or Space presses the one it
     * shows; Escape clears it
     */
    fn key(&mut self, monitor: &str, key: Key, keys: Option<Keys>) -> Option<Transport> {
        match key {
            Key::Left | Key::Right => self.move_to(monitor, key, keys),
            Key::Enter | Key::Space => self.press(monitor, keys),
            Key::Escape => self.clear(monitor),
            _ => None,
        }
    }

    // the first arrow shows the ring and presses nothing; one already shown moves. A visit another
    // monitor holds is left where it is: this key neither moves it nor starts one of its own
    fn move_to(&mut self, monitor: &str, key: Key, keys: Option<Keys>) -> Option<Transport> {
        let Some(keys) = keys.filter(|keys| any(*keys)) else {
            self.drop_visit(monitor);

            return None;
        };

        if self
            .visit
            .as_ref()
            .is_some_and(|visit| visit.monitor != monitor)
        {
            return None;
        }

        let visit = self.kept(monitor, keys);

        let Some(visit) = visit else {
            let at = if key == Key::Left {
                last(keys)
            } else {
                first(keys)
            };

            self.visit = at.map(|at| Visit {
                monitor: monitor.to_owned(),
                at,
                shown: true,
            });

            return None;
        };

        if !visit.shown {
            visit.shown = true;

            return None;
        }

        let forward = key == Key::Right;
        let at = step(visit.at, keys, forward).unwrap_or(visit.at);

        visit.at = at;

        None
    }

    // Enter or Space, only once the ring shows on a button that can still act
    fn press(&mut self, monitor: &str, keys: Option<Keys>) -> Option<Transport> {
        let Some(keys) = keys else {
            self.drop_visit(monitor);

            return None;
        };

        let visit = self.kept(monitor, keys)?;

        if !visit.shown || !able(visit.at, keys) {
            return None;
        }

        Some(visit.at)
    }

    // Escape clears the ring and presses nothing. The pointer no longer asks for the keyboard,
    // the same as a notes card left with Escape, so the next character is not the Desk's
    fn clear(&mut self, monitor: &str) -> Option<Transport> {
        self.drop_visit(monitor);
        self.leave(monitor);

        None
    }

    // what this output draws of the ring: a visit whose button can no longer act is dropped, and
    // an output that draws no button that can act, including one hidden, drops the hover too
    fn shown(&mut self, monitor: &str, keys: Option<Keys>) -> Option<Transport> {
        if let Some(keys) = keys.filter(|keys| any(*keys)) {
            self.kept(monitor, keys);
        } else {
            self.drop_visit(monitor);
            self.leave(monitor);
        }

        self.on(monitor)
    }

    // this output's visit, dropped when the button it is on can no longer act
    fn kept(&mut self, monitor: &str, keys: Keys) -> Option<&mut Visit> {
        let gone = self
            .visit
            .as_ref()
            .is_some_and(|visit| visit.monitor == monitor && (!any(keys) || !able(visit.at, keys)));

        if gone {
            self.visit = None;
        }

        self.visit.as_mut().filter(|visit| visit.monitor == monitor)
    }

    fn drop_visit(&mut self, monitor: &str) {
        if self
            .visit
            .as_ref()
            .is_some_and(|visit| visit.monitor == monitor)
        {
            self.visit = None;
        }
    }

    // this output's hover, dropped only when the pointer no longer has a button here to ask for
    fn leave(&mut self, monitor: &str) {
        if self.over.as_deref() == Some(monitor) {
            self.over = None;
        }
    }
}

// whether any media button can act, which is also whether the row has an input region
fn any(keys: Keys) -> bool {
    keys.can_previous || keys.can_play_pause || keys.can_next
}

fn able(transport: Transport, keys: Keys) -> bool {
    match transport {
        Transport::Previous => keys.can_previous,
        Transport::PlayPause => keys.can_play_pause,
        Transport::Next => keys.can_next,
    }
}

fn first(keys: Keys) -> Option<Transport> {
    ORDER.into_iter().find(|transport| able(*transport, keys))
}

fn last(keys: Keys) -> Option<Transport> {
    ORDER
        .into_iter()
        .rev()
        .find(|transport| able(*transport, keys))
}

// the next button that can act, wrapping; none when `from` is the only one
fn step(from: Transport, keys: Keys, forward: bool) -> Option<Transport> {
    let order: Vec<Transport> = if forward {
        ORDER.into_iter().collect()
    } else {
        ORDER.into_iter().rev().collect()
    };
    let start = order.iter().position(|transport| *transport == from)?;

    order[start + 1..]
        .iter()
        .chain(order[..start].iter())
        .copied()
        .find(|transport| able(*transport, keys))
}

/*
 * the button this output's ring shows, so the row can draw it. A visit whose button can no longer
 * act, or an output with no player, is cleared here rather than drawn on a dead control. None
 * while the ring hides
 */
pub fn shown(monitor: &str, keys: Option<Keys>) -> Option<Transport> {
    RINGS.locked().shown(monitor, keys)
}

/*
 * a key for the ring, after a note and a held drag have taken theirs. Some is the button Enter or
 * Space pressed; None is a key the ring did not take, Left and Right included once they have only
 * shown or moved the ring. A note being typed never reaches here: the Desk's key handler returns
 * first, arrows included
 */
pub fn key(monitor: &str, key: Key, keys: Option<Keys>) -> Option<Transport> {
    RINGS.locked().key(monitor, key, keys)
}

// the keyboard left the window: the ring goes with it, and the window below has its keys again.
// The hover stays. The pointer may still be on the button, and the next click must find OnDemand
// already asked for, rather than waiting for a pointer move that never comes
pub fn clear(monitor: &str) {
    RINGS.locked().drop_visit(monitor);
}

// the pointer came onto a media button of this output that can act, or left every one of them.
// Leaving one output does not clear another output's hover
pub fn hover(monitor: &str, inside: bool) {
    let mut rings = RINGS.locked();

    if inside {
        rings.over = Some(monitor.to_owned());
    } else if rings.over.as_deref() == Some(monitor) {
        rings.over = None;
    }
}

// whether this output's window asks for the keyboard: the pointer is on a button that can act, the
// same shape as a notes card under the pointer. The ring itself does not keep the ask once the
// pointer has left, so Escape and the next character reach the window below
pub fn keyboard(monitor: &str) -> bool {
    RINGS.locked().keyboard(monitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: &str = "eDP-1";

    fn keys(previous: bool, play: bool, next: bool) -> Keys {
        Keys {
            playing: false,
            can_play_pause: play,
            can_previous: previous,
            can_next: next,
        }
    }

    fn fresh() -> Rings {
        Rings {
            visit: None,
            over: None,
        }
    }

    #[test]
    fn the_first_arrow_only_shows_the_ring() {
        let mut rings = fresh();
        let keys = keys(true, true, true);

        assert_eq!(rings.key(MONITOR, Key::Right, Some(keys)), None);
        assert_eq!(rings.on(MONITOR), Some(Transport::Previous));

        // a following arrow moves among buttons that can act, and still presses nothing
        assert_eq!(rings.key(MONITOR, Key::Right, Some(keys)), None);
        assert_eq!(rings.on(MONITOR), Some(Transport::PlayPause));

        assert_eq!(
            rings.key(MONITOR, Key::Enter, Some(keys)),
            Some(Transport::PlayPause)
        );
        assert_eq!(
            rings.key(MONITOR, Key::Space, Some(keys)),
            Some(Transport::PlayPause)
        );
    }

    #[test]
    fn a_dead_button_is_skipped_and_a_player_that_disables_the_visit_clears_it() {
        let mut rings = fresh();

        assert_eq!(
            rings.key(MONITOR, Key::Right, Some(keys(false, true, false))),
            None
        );
        assert_eq!(rings.on(MONITOR), Some(Transport::PlayPause));

        // the only button that can act: an arrow stays on it
        assert_eq!(
            rings.key(MONITOR, Key::Right, Some(keys(false, true, false))),
            None
        );
        assert_eq!(rings.on(MONITOR), Some(Transport::PlayPause));

        // the player can no longer do what the ring is on
        assert_eq!(
            rings.key(MONITOR, Key::Enter, Some(keys(true, false, true))),
            None
        );
        assert_eq!(rings.on(MONITOR), None);

        // no button can act: an arrow draws nothing
        assert_eq!(
            rings.key(MONITOR, Key::Left, Some(keys(false, false, false))),
            None
        );
        assert_eq!(rings.on(MONITOR), None);
        assert_eq!(rings.key(MONITOR, Key::Right, None), None);
    }

    #[test]
    fn escape_drops_the_ask_and_no_able_button_does_too() {
        let kept = std::mem::take(&mut *RINGS.locked());
        let able = keys(true, true, true);

        hover(MONITOR, true);
        key(MONITOR, Key::Left, Some(able));

        assert!(keyboard(MONITOR));
        assert_eq!(shown(MONITOR, Some(able)), Some(Transport::Next));

        // Escape clears the visit and the hover, so the window is not OnDemand for the next key
        assert_eq!(key(MONITOR, Key::Escape, Some(able)), None);
        assert!(!keyboard(MONITOR));
        assert_eq!(shown(MONITOR, Some(able)), None);

        // another output's hover is not this Escape
        hover("HDMI-A-1", true);
        key(MONITOR, Key::Escape, Some(able));
        assert!(!keyboard(MONITOR));
        assert!(keyboard("HDMI-A-1"));

        // drawing with no button that can act, the player gone or media off, clears the hover too
        hover(MONITOR, true);
        key(MONITOR, Key::Right, Some(able));

        assert!(keyboard(MONITOR));
        assert_eq!(shown(MONITOR, None), None);
        assert!(!keyboard(MONITOR));

        hover(MONITOR, true);
        assert_eq!(shown(MONITOR, Some(keys(false, false, false))), None);
        assert!(!keyboard(MONITOR));

        // a button that can still act does not drop the hover: the next click needs it
        hover(MONITOR, true);
        assert_eq!(shown(MONITOR, Some(keys(true, false, false))), None);
        assert!(keyboard(MONITOR));

        // the keyboard leaving the window drops the visit and leaves the hover, so a click still
        // finds OnDemand already asked for
        key(MONITOR, Key::Left, Some(able));
        clear(MONITOR);
        assert!(keyboard(MONITOR));
        assert_eq!(shown(MONITOR, Some(able)), None);

        *RINGS.locked() = kept;
    }

    #[test]
    fn escape_clears_the_ring_and_a_character_is_not_the_rings() {
        let mut rings = fresh();
        let able = keys(true, true, true);

        rings.key(MONITOR, Key::Left, Some(able));

        assert_eq!(rings.on(MONITOR), Some(Transport::Next));
        assert_eq!(rings.key(MONITOR, Key::Escape, Some(able)), None);
        assert_eq!(rings.on(MONITOR), None);

        // the next character is not consumed here, so it can type into the window below
        assert_eq!(rings.key(MONITOR, Key::Character('a'), Some(able)), None);
        assert_eq!(rings.on(MONITOR), None);

        // Left starts at the end, and a dead button in the way is skipped
        let skipped = keys(true, false, true);

        rings.key(MONITOR, Key::Left, Some(skipped));
        assert_eq!(rings.on(MONITOR), Some(Transport::Next));
        rings.key(MONITOR, Key::Left, Some(skipped));
        assert_eq!(rings.on(MONITOR), Some(Transport::Previous));
    }

    #[test]
    fn a_key_on_another_monitor_does_not_replace_the_visit() {
        let mut rings = fresh();
        let able = keys(true, true, true);
        let other = "HDMI-A-1";

        rings.key(MONITOR, Key::Right, Some(able));

        assert_eq!(rings.on(MONITOR), Some(Transport::Previous));

        // B has buttons that can act, and still neither shows a ring nor takes A's
        assert_eq!(rings.key(other, Key::Right, Some(able)), None);
        assert_eq!(rings.on(other), None);
        assert_eq!(rings.on(MONITOR), Some(Transport::Previous));

        assert_eq!(rings.key(other, Key::Enter, Some(able)), None);
        assert_eq!(rings.on(MONITOR), Some(Transport::Previous));

        // Escape on B is not A's ring
        assert_eq!(rings.key(other, Key::Escape, Some(able)), None);
        assert_eq!(rings.on(MONITOR), Some(Transport::Previous));

        // a key on the monitor that holds it still moves it
        assert_eq!(rings.key(MONITOR, Key::Right, Some(able)), None);
        assert_eq!(rings.on(MONITOR), Some(Transport::PlayPause));
    }
}
