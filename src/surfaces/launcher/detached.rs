//! The Launcher in a window of its own, `launcher.style = "detached"` (ADR 0131): one Overlay
//! window per output, centred on the one it was opened on, holding the keyboard only while it is
//! open and closing on Escape or on the keyboard leaving it. Its rows, its search and its presses
//! are the Island's own (`super::typed`), so the two hosts never drift apart.

use std::time::Instant;

use kanade_runtime::service::Service;
use kanade_runtime::{
    Horizontal, Key, Keyboard, Layer, LayerWindow, Margin, Monitor, Rectangle, Vertical, Zone,
};

use crate::island::presentation::{Input, LauncherStyle};
use crate::island::service::IslandService;
use crate::theme::{self, radius};
use crate::view;

use super::{Search, recorded, typed};
use crate::surfaces::visit::Visited;

// how far the body's shadow reaches past it, on every side
const SHADOW: f32 = 24.0;

/*
 * whether the detached Launcher holds the keyboard: only while it is open, and never for its Module
 * being off, so a closed window is never focused and Escape anywhere else stays wherever it was
 */
pub fn hold(island: &IslandService) -> Keyboard {
    match island.detached_on() {
        Some(_) if island.launcher_style() == LauncherStyle::Detached => Keyboard::Exclusive,
        _ => Keyboard::None,
    }
}

// the output the detached Launcher shows on, and the visit its search and its keys belong to
fn open_on(island: &IslandService) -> Option<(String, u64)> {
    island
        .detached_on()
        .map(|(monitor, visit)| (monitor.to_owned(), visit))
}

/*
 * the window on this output, from the Launcher's own open state: closed, never seen and never
 * focused; open, a body the size of the largest the Island's Surfaces take, centred on the output
 * by a computed margin, taking the pointer and the keyboard until it closes
 */
pub fn window(monitor: &Monitor) -> LayerWindow {
    let island = IslandService::read();
    let open = open_on(&island).filter(|(name, _)| *name == monitor.name);
    let keyboard = hold(&island);

    let Some((_, visit)) = open else {
        return LayerWindow::new()
            .width(1.0)
            .height(1.0)
            .anchor_vertical(Vertical::Top)
            .anchor_horizontal(Horizontal::Left)
            .layer(Layer::Overlay)
            .space(Zone::Ignore)
            .namespace("kanade-launcher")
            .visible(false)
            .click_through()
            .child(Rectangle::new().width(1.0).height(1.0));
    };

    let rec = recorded(&island);
    drop(island);

    let shape = view::largest();
    let body = Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .radius(radius::CARD)
        .fill(theme::island().surface)
        .border(1.0, theme::island().outline)
        .shadow(
            kanade_runtime::Shadow::drop(theme::SHADOW_AMBIENT)
                .blur(20.0)
                .offset(0.0, 6.0),
        )
        .child(super::surface(&monitor.name, visit, rec));

    let name = monitor.name.clone();

    LayerWindow::new()
        .width(shape.width + 2.0 * SHADOW)
        .height(shape.height + 2.0 * SHADOW)
        .anchor_vertical(Vertical::Top)
        .anchor_horizontal(Horizontal::Middle)
        .margin(Margin {
            top: ((monitor.height as f32 - shape.height) / 2.0 - SHADOW).max(0.0) as i32,
            ..Margin::default()
        })
        .layer(Layer::Overlay)
        .keyboard(keyboard)
        .space(Zone::Ignore)
        .namespace("kanade-launcher")
        .on_key(move |key| {
            key_press(&name, visit, key);
        })
        .on_lost(close)
        .child(body)
}

/*
 * a key while the detached Launcher shows: Escape with its actions closed ends the visit, and any
 * other key is the Launcher's own, which closes the actions, presses an answer or closes the whole
 * Launcher through the press itself
 */
fn key_press(monitor: &str, visit: u64, key: Key) -> bool {
    let rec = recorded(&IslandService::read());

    if key == Key::Escape && Search::read().of(visit).panel.is_none() {
        close();

        return true;
    }

    typed(monitor, visit, rec, key)
}

// closes the detached Launcher, on the output it stands on; nothing while none is open
pub fn close() {
    let mut island = IslandService::write();
    let monitor = island.detached_on().map(|(monitor, _)| monitor.to_owned());

    match monitor {
        Some(monitor) => island.input(&monitor, Input::Collapse, Instant::now()),
        None => island.quiet(),
    }
}

#[cfg(test)]
mod tests {
    use kanade_runtime::service::Service;

    use crate::island::presentation::{Input, LauncherStyle};
    use crate::island::service::IslandService;

    use super::hold;

    const MONITOR: &str = "eDP-1";

    /*
     * ADR 0131: the detached window takes the keyboard only while the Launcher is open in it, so a
     * closed one is never focused and never eats a key meant for the window in front
     */
    #[test]
    fn a_closed_detached_launcher_holds_no_keyboard() {
        let mut island = IslandService::new();
        island.set_launcher_style(LauncherStyle::Detached);

        let now = std::time::Instant::now();

        assert_eq!(hold(&island), kanade_runtime::Keyboard::None);

        island.input(
            MONITOR,
            Input::Open(crate::island::presentation::Surface::Launcher),
            now,
        );
        assert_eq!(hold(&island), kanade_runtime::Keyboard::Exclusive);

        island.input(MONITOR, Input::Collapse, now);
        assert_eq!(hold(&island), kanade_runtime::Keyboard::None);
    }
}
