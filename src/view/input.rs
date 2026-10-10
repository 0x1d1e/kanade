//! What the Island's window does with the pointer and keys: it only routes them to `IslandService`.

use std::time::Instant;

use kanade_runtime::service::Service;
use kanade_runtime::{InputArea, Key};

use crate::island::geometry::{Canvas, Rect};
use crate::island::presentation::{Input, Segment, Surface};
use crate::island::service::IslandService;
use crate::modules;
use crate::surfaces;

// a write wakes the window even when nothing changed, so only write a real change
pub(super) fn expand(monitor: &str) {
    let island = IslandService::read();
    let (expanded, segment) = (island.expanded(monitor), island.segment(monitor));
    drop(island);

    if !expanded {
        IslandService::write().input(monitor, Input::Click(segment), Instant::now());
    } else {
        claim();
    }
}

// a press on an AutoExpand's Surface makes it the user's, so it is not given back (ADR 0009)
pub(crate) fn claim() {
    if IslandService::read().auto() {
        IslandService::write().claim(Instant::now());
    }
}

// closes an open Surface or ends a pinned Peek
pub(crate) fn collapse(monitor: &str) {
    let island = IslandService::read();
    let raised = island.expanded(monitor) || island.pinned(monitor);
    drop(island);

    if raised {
        IslandService::write().input(monitor, Input::Collapse, Instant::now());
    }
}

// in arms and starts the hover delay, out disarms and starts the grace; IslandService::listen times both
pub(super) fn hover(monitor: &str, inside: bool, peeks: bool) {
    if IslandService::read().inside(monitor) != inside {
        IslandService::write().hover(monitor, inside, peeks, Instant::now());
    }
}

// the wheel reaches no Presentation yet, so nothing is written for it (#27)
pub(super) fn route(monitor: &str, input: Input) {
    if input.decides() {
        IslandService::write().input(monitor, input, Instant::now());
    }
}

/*
 * opens `surface` from a target of the island's own, as a click on the island opens one, pinned
 * when it should stay with nobody on it
 */
pub(crate) fn open(monitor: &str, surface: Surface, pinned: bool) {
    route(monitor, Input::Open(surface));

    if pinned && !IslandService::read().pinned(monitor) {
        route(monitor, Input::RightClick(Segment::Primary));
    }
}

// a right click on the open Surface's own targets, which only the Expanded island shows
pub(crate) fn pin() {
    let monitor = IslandService::read().expanded_on().map(str::to_owned);

    if let Some(monitor) = monitor {
        route(&monitor, Input::RightClick(Segment::Primary));
    }
}

/*
 * a key on `monitor`'s island, as the runtime gives it, again while a Backspace or arrow is held: to
 * the open Surface that takes it, else Escape collapses the island
 */
pub fn key(monitor: &str, key: Key) {
    // a Surface whose Module is off never opens, so its keys read nothing. Controls, the
    // Tray and the Calendar go first, as Escape in a sub-surface or the agenda goes back a
    // level rather than closing
    if modules::on("controls") && surfaces::controls::key(monitor, key) {
        return;
    }

    if modules::on("tray") && surfaces::tray::key(monitor, key) {
        return;
    }

    if modules::on("calendar-surface") && surfaces::calendar::key(monitor, key) {
        return;
    }

    if modules::on("session") && surfaces::session::key(monitor, key) {
        return;
    }

    if key == Key::Escape {
        collapse(monitor);

        // OnDemand would keep the focus the press gave while the pointer rests on the pill
        set_armed(monitor, false);
    } else {
        let used = (modules::on("notification-surface")
            && surfaces::notifications::key(monitor, key))
            || (modules::on("launcher") && surfaces::launcher::key(monitor, key))
            || (modules::on("clipboard-surface") && surfaces::clipboard::key(monitor, key));

        if !used {
            stray(monitor, key);
        }
    }
}

/*
 * the Launcher and the Clipboard take typing, the Calendar a few letters, and they consume their
 * keys first, so a character typed into any other held island the pointer never reached was meant
 * for the window beneath: the island lets go of the keyboard before a Space or Enter presses
 * anything
 */
fn stray(monitor: &str, key: Key) {
    let island = IslandService::read();
    let stray = island.expanded(monitor)
        && island.held(monitor)
        && !island.inside(monitor)
        && matches!(key, Key::Character(_));
    drop(island);

    if stray {
        collapse(monitor);
    }
}

pub(super) fn set_segment(monitor: &str, segment: Segment) {
    if IslandService::read().segment(monitor) != segment {
        IslandService::write().set_segment(monitor, segment);
    }
}

pub(super) fn moved_to(monitor: &str, x: f32, y: f32, canvas: Canvas) {
    if IslandService::read().under(monitor).is_some() {
        IslandService::write().moved(monitor, x, y, canvas);
    }
}

pub(super) fn set_armed(monitor: &str, armed: bool) {
    if IslandService::read().armed(monitor) != armed {
        IslandService::write().set_armed(monitor, armed);
    }
}

// geometry already rounded it to whole pixels
pub(crate) fn input_area(area: Rect) -> InputArea {
    InputArea {
        x: area.x as i32,
        y: area.y as i32,
        width: area.width as i32,
        height: area.height as i32,
    }
}
