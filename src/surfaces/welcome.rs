//! The one-time first-start tour of opening the Island and finding its keybinds (ADR 0123).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::MutexExt;
use kanade_runtime::service::{self, Service};
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{Button, Center, Column, Key, Rectangle, Row, Start, Text, children};

use super::visit::Visited;
use crate::atomic::{self, Mode};
use crate::config;
use crate::island::presentation::{Input, Presentation, Surface};
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

fn marker() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok();
    config::base("XDG_STATE_HOME", ".local/state", home.as_deref())
        .map(|dir| dir.join("kanade").join("welcome-tour"))
}

/*
 * the tour's one run: offered at a start with no marker, and over once the island no longer shows
 * the Welcome Surface, whichever path closed it
 */
#[derive(Debug, Default)]
struct Tour {
    // it was offered and has not closed yet
    offered: bool,
}

impl Tour {
    fn offer(&mut self) {
        self.offered = true;
    }

    // true the one time the tour is over: it was offered and the island no longer shows it
    fn over(&mut self, open: bool) -> bool {
        if open || !self.offered {
            return false;
        }

        self.offered = false;

        true
    }
}

static TOUR: Mutex<Tour> = Mutex::new(Tour { offered: false });

// Called from the core Module after its windows and Surface withholding are registered.
pub fn start_after_focus() {
    if marker().is_some_and(|marker| marker.exists()) {
        return;
    }

    // completion is recorded as the Surface closes, not by the Surface closing itself
    service::watch::<IslandService>(follow);

    let _ = thread::Builder::new()
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
                    /*
                     * offered in the same write that opens it, so the watcher that records
                     * completion never sees the tour offered and gone in between
                     */
                    let mut island = IslandService::write();
                    island.open(&monitor, Surface::Welcome, Instant::now());

                    if island.presentation(&monitor) == Presentation::Expanded(Surface::Welcome) {
                        TOUR.locked().offer();
                    }

                    return;
                }
                thread::sleep(Duration::from_millis(250));
            }
        });
}

// the island shows the Welcome Surface now or does not, on every change of the islands
fn follow() {
    let Some(marker) = marker() else {
        return;
    };

    // the guard is dropped here: recording fsyncs, which must not hold up the island's writes
    let open = IslandService::read().surface() == Some(Surface::Welcome);

    closed(&marker, open);
}

fn closed(marker: &Path, open: bool) {
    if TOUR.locked().over(open) {
        record(marker);
    }
}

/*
 * Dismissal is completion, including when the state directory cannot be written. In that case the
 * tour still closes for this run and may be offered again after a later restart.
 */
fn record(marker: &Path) {
    if let Err(error) = atomic::write(marker, env!("CARGO_PKG_VERSION").as_bytes(), Mode::Private) {
        eprintln!("kanade: could not record welcome tour completion: {error}");
    }
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
        .on_click(|button| {
            if button == Button::Left {
                press();
            }
        })
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

/*
 * a press on the tour dismisses it (ADR 0123). The press is the tour's own content's: the island's
 * own target beneath only claims the Surface it already shows, which would leave the keyboard on the
 * tour with no deadline. Collapsing and disarming as Escape does, since the press gave OnDemand the
 * focus while the pointer rests on the pill
 */
fn press() {
    let Some(monitor) = IslandService::read().expanded_on().map(str::to_owned) else {
        return;
    };

    let mut island = IslandService::write();
    island.input(&monitor, Input::Collapse, Instant::now());
    island.set_armed(&monitor, false);
}

pub fn key(monitor: &str, key: Key) -> bool {
    let step = {
        let island = IslandService::read();
        if island.presentation(monitor) != Presentation::Expanded(Surface::Welcome) {
            return false;
        }
        State::read().of(island.visit()).step
    };
    match key {
        Key::Escape => {
            view::collapse(monitor);
            true
        }
        Key::Right | Key::Enter | Key::Space if step + 1 < STEP_COUNT => {
            store(step + 1);
            true
        }
        Key::Right | Key::Enter | Key::Space => {
            view::collapse(monitor);
            true
        }
        Key::Left if step > 0 => {
            store(step - 1);
            true
        }
        // the tour takes no typing: a character at it was meant for the window beneath, as one at a
        // held island the pointer never reached (`stray` in `src/view/input.rs`)
        Key::Character(_) => {
            view::collapse(monitor);
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn temporary(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("kanade-welcome-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        dir.join("kanade/welcome-tour")
    }

    /*
     * the defect this fixed: the island closes the tour on its own, with no key and no Surface
     * code running, so completion has to follow the Surface leaving the island rather than a key
     */
    #[test]
    fn the_tour_is_recorded_however_its_surface_closes() {
        let marker = temporary("closes");

        // offered and still open: nothing recorded yet
        TOUR.locked().offer();
        closed(&marker, true);
        assert!(!marker.exists());

        // the island collapsed it, as a press elsewhere, a Surface opening over it or Escape does
        closed(&marker, false);
        assert_eq!(
            fs::read_to_string(&marker).unwrap(),
            env!("CARGO_PKG_VERSION")
        );

        // a later change of the islands records it no more, and it is readable by its owner only
        let bits = fs::metadata(&marker).unwrap().permissions().mode() & 0o777;
        assert_eq!(bits, 0o600);
        fs::remove_file(&marker).unwrap();
        closed(&marker, false);
        assert!(!marker.exists());

        let _ = fs::remove_dir_all(marker.parent().unwrap().parent().unwrap());
    }
}
