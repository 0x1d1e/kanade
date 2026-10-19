//! The one-time first-start tour of opening the Island and finding its keybinds (ADR 0123).

use std::time::Instant;

use kanade_runtime::service::Service;
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{Center, Column, Key, Rectangle, Row, Start, Text, children};

use super::visit::Visited;
use crate::atomic::{self, Mode};
use crate::config;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::theme;
use crate::theme::space::INSET;
use crate::view;

const TITLES: [&str; 3] = ["Welcome to Kanade", "Open a Surface", "Find your keybinds"];
const STEP_COUNT: usize = TITLES.len();

fn body(step: usize) -> String {
    match step {
        0 if crate::modules::on("launcher") => String::from(
            "Click the Island at Rest to open Controls, or open the Launcher from the Island. Mod+Alt+Space is a bind from docs/wiki/Keyboard.md; add it to your niri config to use it.",
        ),
        0 => String::from(
            "Click the Island at Rest to open Controls, then open another available Surface from a tile or Activity. The Launcher Module is off; enable it with `kanade module enable launcher` and restart Kanade. Mod+Alt+Space is a bind from docs/wiki/Keyboard.md; add it to your niri config to use it.",
        ),
        1 => String::from(
            "Controls, Calendar, Media and other available Surfaces live inside the Island. Open one from a tile, an Activity, or a keybind.",
        ),
        2 if crate::modules::on("keys-surface") && crate::modules::on("launcher") => String::from(
            "Open Keys from the Launcher. Mod+Alt+Slash is a bind from docs/wiki/Keyboard.md; add it to your niri config to use it. Keys lists your niri binds and the keys Kanade's Surfaces use.",
        ),
        2 if crate::modules::on("keys-surface") => String::from(
            "Open Keys from an available Surface or its keybind. Mod+Alt+Slash is a bind from docs/wiki/Keyboard.md; add it to your niri config to use it. Keys lists your niri binds and the keys Kanade's Surfaces use.",
        ),
        2 => String::from(
            "The Keys Module is off; enable it with `kanade module enable keys-surface` and restart Kanade. Mod+Alt+Slash is a bind from docs/wiki/Keyboard.md; add it to your niri config to use it. The Keyboard page lists the keys Kanade's Surfaces use.",
        ),
        _ => unreachable!(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct State {
    visit: u64,
    step: usize,
}
impl Service for State {
    fn new() -> Self {
        State { visit: 0, step: 0 }
    }
    fn listen() {}
}
impl Visited for State {
    fn visit(&self) -> u64 {
        self.visit
    }
    fn begin(visit: u64) -> Self {
        State { visit, step: 0 }
    }
}

fn marker() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok();
    config::base("XDG_STATE_HOME", ".local/state", home.as_deref())
        .map(|dir| dir.join("kanade").join("welcome-tour"))
}

// Called from the core Module after its windows and Surface withholding are registered.
pub fn start_after_focus() {
    if marker().is_some_and(|marker| marker.exists()) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name(String::from("kanade-welcome"))
        .spawn(|| {
            for _ in 0..120 {
                if marker().is_some_and(|marker| marker.exists()) {
                    return;
                }
                let monitor = crate::sources::niri::Niri::read()
                    .seen
                    .focused_output
                    .clone();
                if let Some(monitor) = monitor {
                    IslandService::write().open(&monitor, Surface::Welcome, Instant::now());
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        });
}

// Dismissal is completion, including when the state directory cannot be written. In that case
// the tour still closes for this run and may be offered again after a later restart.
pub fn finish(monitor: &str) {
    if let Some(marker) = marker() {
        if let Err(error) =
            atomic::write(&marker, env!("CARGO_PKG_VERSION").as_bytes(), Mode::Private)
        {
            eprintln!("kanade: could not record welcome tour completion: {error}");
        }
    }
    view::collapse(monitor);
}

pub fn surface(open: bool, visit: u64) -> Rectangle {
    let step = State::read().of(visit).step.min(STEP_COUNT - 1);
    let title = TITLES[step];
    let body = body(step);
    let size = view::largest();
    Rectangle::new()
        .width(size.width)
        .height(size.height)
        .padding(INSET)
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::Region, "welcome"))
        .child(
            Column::new(children![
                Text::new(title)
                    .size(theme::text::TITLE)
                    .color(theme::island().on_surface)
                    .weight(theme::text::SEMIBOLD),
                Text::new(&body)
                    .size(theme::text::BODY)
                    .color(theme::island().on_surface_variant)
                    .wrap(),
                Row::new(children![
                    Text::new(if step + 1 < STEP_COUNT {
                        "Enter or Right: continue"
                    } else {
                        "Enter or Right: finish"
                    })
                    .size(theme::text::LABEL_SMALL)
                    .color(theme::island().on_surface),
                    Text::new("Escape: close")
                        .size(theme::text::LABEL_SMALL)
                        .color(theme::island().on_surface_variant),
                ])
                .gap(16.0),
            ])
            .gap(18.0)
            .align(Center),
        )
        .inert(!open)
}

pub fn key(monitor: &str, key: Key) -> bool {
    let step = {
        let island = IslandService::read();
        if island.presentation(monitor) != Presentation::Expanded(Surface::Welcome) {
            return false;
        }
        let step = State::read().of(island.visit()).step;
        step
    };
    match key {
        Key::Escape => {
            finish(monitor);
            true
        }
        Key::Right | Key::Enter | Key::Space if step + 1 < STEP_COUNT => {
            store(step + 1);
            true
        }
        Key::Right | Key::Enter | Key::Space => {
            finish(monitor);
            true
        }
        Key::Left if step > 0 => {
            store(step - 1);
            true
        }
        _ => false,
    }
}

fn store(step: usize) {
    let visit = IslandService::read().visit();
    let next = State { visit, step };
    if *State::read() != next {
        *State::write() = next;
    }
}
