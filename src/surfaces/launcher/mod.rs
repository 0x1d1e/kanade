//! The Launcher Surface: a search over its providers, the apps `sources::apps`
//! lists and their own actions, the open windows (`windows`), the calculator, emoji, where Kanade goes (Wi-Fi, Bluetooth, each Surface, Settings and
//! each setting), with `wallpaper-surface` on, the Wallpapers Surface for anything after `@`, and with `clipboard` on, what was copied after `>`, best answer first.
//! It opens only from IPC or a keybind, so it always holds the keyboard: typing searches, the arrow
//! keys move the selection, Enter presses the selected answer and a click the one clicked. An app
//! starts and the island closes; a value or an emoji is copied, and the island
//! closes once it is, or says it was not; a command (`commands`, Kanade's own verbs) runs through the socket and closes the island once answered, or says `Not run`. It says when the apps are still being found and when
//! nothing matches. With nothing typed it lists sections, a heading over each: the Recents (by
//! frecency), then Kanade's Commands, the open Windows and the Apps. `Ctrl+K` opens the actions of the
//! selected answer (`actions`) in the list's place, `Ctrl+1` through `Ctrl+9` press the nth answer, and
//! `kanade dmenu` opens it on a script's lines alone (`choose`). A query nothing answers offers to
//! search Google for it or open it in the browser (`web`, ADR 0130).

use std::sync::{Arc, LazyLock, Mutex};
use std::thread;
use std::time::Instant;

use kanade_runtime::MutexExt;
use kanade_runtime::service::{self, Service};
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Column, Cursor, End, Image, Key, Padding, Parent, Rectangle, Row, Scroll, Size, Stack,
    Start, Text, Widget, children,
};

use crate::config;
use crate::icon::Icon;
use crate::island::geometry;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::settings;
use crate::sources::apps::{App, Apps};
use crate::sources::bluetooth::Adapter;
use crate::sources::network::Connectivity;
use crate::sources::niri::{self, Acted};
use crate::sources::system::Radio;
use crate::sources::{self, clipboard, live, recording};
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, radius};
use crate::{modules, view};

use super::visit::Visited;
use super::{Outline, capture, controls, store};

mod actions;
mod apps;
mod calculator;
pub mod choose;
mod commands;
mod destinations;
pub mod detached;
mod emoji;
mod history;
mod provider;
mod units;
pub mod web;
pub mod windows;

use actions::{Act, Offer, Panel};
use apps::{Actions, Apps as AppsProvider};
use calculator::Calculator;
use choose::{Choices, Choosing};
use commands::{Commands, Rec};
use destinations::{Destination, Destinations};
use emoji::Emoji;
use history::History;
use provider::{Action, Answer, Fit, LauncherProvider, Mark};

// the content's width, which every row fills, as wide as the largest body (`island.width`)
fn width() -> f32 {
    view::largest().width - 2.0 * INSET
}

const FIELD: f32 = 44.0;
const FIELD_INSET: f32 = 16.0;
const GAP: f32 = 12.0;

// what the rows scroll in, as tall as the largest body (`island.height`) leaves
fn room() -> f32 {
    view::largest().height - 2.0 * INSET - FIELD - GAP
}

// what a state, as no match, takes of the list
const STATE: f32 = 120.0;

// `ROWS` at a time in the least largest body, more in a taller one
const LEAST_LIST: f32 = geometry::CALENDAR.height - 2.0 * INSET - FIELD - GAP;

const ROWS: usize = 5;
const ROW_GAP: f32 = 6.0;
const ROW: f32 = (LEAST_LIST - (ROWS - 1) as f32 * ROW_GAP) / ROWS as f32;
const ROW_INSET: f32 = 8.0;

// a section's heading, above the answers it names
const HEADER: f32 = 26.0;

// a row of the actions `Ctrl+K` opens, and the line under them that says a pin was refused
const OFFER: f32 = 40.0;
const NOTE: f32 = 22.0;

const ICON: f32 = 28.0;
const ICON_GAP: f32 = 12.0;

// the typed text's size, and the caret after it
const QUERY: f32 = theme::text::TITLE;
const CARET: f32 = 1.5;

// the air the selection keeps around the letters it covers
const SELECTION_INSET: f32 = 4.0;

// pixels per wheel line
const WHEEL: f32 = 40.0;

/*
 * the query, the selection and the scroll for one visit of the Surface (`IslandService::visit`),
 * so every opening starts empty at the top. Written by input only, never by the view
 */
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Search {
    visit: u64,
    query: String,
    caret: usize,
    select_all: bool,

    // in the answers the query finds
    selected: usize,

    // how far the rows scrolled, in pixels
    offset: f32,

    pressing: Pressing,

    // the actions `Ctrl+K` opened over the selected answer, none while the list shows
    panel: Option<Panel>,

    // where the panel's ring is, and how far its rows scrolled
    chosen: usize,
    panel_offset: f32,

    // a pin or unpin the settings file refused, with why, said under the panel's rows
    note: Option<String>,
}

/*
 * a copy or a wallpaper, from its press until it is done or failed, of the answer pressed; so the
 * row of that answer says it failed, whichever is selected by then
 */
#[derive(Debug, Clone, Default, PartialEq)]
enum Pressing {
    #[default]
    Idle,
    Waiting(Action),

    // with why
    Failed(Action, String),
}

/*
 * what a key asks for once the search has taken it: nothing, the answer the search selects, the nth
 * answer of the list (`Ctrl+1`), the row of the open panel its ring is on, or the panel of the
 * answer selected
 */
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Intent {
    Idle,
    Selected,
    At(usize),
    Offer,
    Panel,
}

impl Service for Search {
    fn new() -> Self {
        Search::default()
    }

    fn listen() {}
}

impl Visited for Search {
    fn visit(&self) -> u64 {
        self.visit
    }

    fn begin(visit: u64) -> Self {
        Search {
            visit,
            ..Search::default()
        }
    }
}

impl Search {
    // what its row says when pressing `action` failed, none when it did not
    fn failed(&self, action: &Action) -> Option<String> {
        let Pressing::Failed(failed, why) = &self.pressing else {
            return None;
        };

        if failed != action {
            return None;
        }

        match action {
            Action::Copy(_) => Some(String::from("Not copied")),
            Action::Paste(_) => Some(String::from("Not copied")),
            Action::Browse(_) => Some(format!("Not opened: {why}")),
            Action::Open(_) => Some(format!("Not opened: {why}")),
            // `dispatch` worded it, as it knows whether the verb may have run
            Action::Run(_) => Some(why.clone()),
            Action::Launch(..) | Action::Perform(..) => Some(format!("Not launched: {why}")),
            Action::Focus(_) | Action::Choose(_) => None,
        }
    }

    // starts one press for this visit unless another press is still in flight
    fn press_waiting(&mut self, visit: u64, action: Action) -> bool {
        if self.visit != visit || matches!(self.pressing, Pressing::Waiting(_)) {
            return false;
        }

        self.pressing = Pressing::Waiting(action);
        true
    }

    // `Failed` is shown by the selected result until the next key
    fn press_failed(&mut self, visit: u64, action: Action, why: String) {
        if self.visit == visit
            && matches!(&self.pressing, Pressing::Waiting(waiting) if *waiting == action)
        {
            self.pressing = Pressing::Failed(action, why);
        }
    }

    // an `action` that opens another Surface was refused, for `why`; nothing once the visit is over
    fn open_refused(&mut self, visit: u64, action: Action, why: &str) {
        if self.visit == visit {
            self.pressing = Pressing::Failed(action, String::from(why));
        }
    }

    /*
     * the search after a key and what it asks for, none when the key is not for the Launcher;
     * `layout` is what the query finds before the key
     */
    fn step(mut self, key: Key, layout: &Layout) -> Option<(Search, Intent)> {
        // the panel takes the keys it can act on, and Escape and Ctrl+K close it; a key it has no
        // use for closes it unacted, so the list is never typed under a panel no key opened for
        if self.panel.is_some() {
            return self.stepped_panel(key);
        }

        let last = layout.answers.len().saturating_sub(1);

        match key {
            Key::Character(letter) => {
                if self.select_all {
                    self.query.clear();
                    self.query.push(letter);
                    self.caret = self.query.len();
                    self.select_all = false;
                } else {
                    self.query.insert(self.caret, letter);
                    self.caret += letter.len_utf8();
                }
                self.selected = 0;
                self.offset = 0.0;
                Some((self, Intent::Idle))
            }
            Key::Space => {
                if self.select_all {
                    self.query.clear();
                    self.query.push(' ');
                    self.caret = self.query.len();
                    self.select_all = false;
                } else {
                    self.query.insert(self.caret, ' ');
                    self.caret += 1;
                }
                self.selected = 0;
                self.offset = 0.0;
                Some((self, Intent::Idle))
            }
            Key::Shortcut('k') => self.opened(layout),

            /*
             * the nth answer, which nothing presses on a list shorter than n; any other letter
             * with ctrl is no key of the Launcher's
             */
            Key::Shortcut(letter) => {
                let at = letter
                    .to_digit(10)
                    .filter(|&digit| digit >= 1)
                    .map(|digit| digit as usize - 1)
                    .filter(|&at| at <= last)?;

                self.selected = at;
                Some((self, Intent::At(at)))
            }
            Key::Backspace => {
                if self.select_all {
                    self.query.clear();
                    self.caret = 0;
                    self.select_all = false;
                    self.selected = 0;
                    self.offset = 0.0;
                } else if self.caret > 0 {
                    let prev = self.query[..self.caret]
                        .char_indices()
                        .last()
                        .map(|(idx, _)| idx)
                        .unwrap_or(0);
                    self.query.drain(prev..self.caret);
                    self.caret = prev;
                    self.selected = 0;
                    self.offset = 0.0;
                }
                Some((self, Intent::Idle))
            }
            Key::Delete => {
                if self.select_all {
                    self.query.clear();
                    self.caret = 0;
                    self.select_all = false;
                    self.selected = 0;
                    self.offset = 0.0;
                } else if self.caret < self.query.len() {
                    let len = self.query[self.caret..]
                        .chars()
                        .next()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                    self.query.drain(self.caret..self.caret + len);
                    self.selected = 0;
                    self.offset = 0.0;
                }
                Some((self, Intent::Idle))
            }
            Key::Left => {
                if self.select_all {
                    self.select_all = false;
                    self.caret = 0;
                } else if self.caret > 0 {
                    let prev = self.query[..self.caret]
                        .char_indices()
                        .last()
                        .map(|(idx, _)| idx)
                        .unwrap_or(0);
                    self.caret = prev;
                }
                Some((self, Intent::Idle))
            }
            Key::Right => {
                if self.select_all {
                    self.select_all = false;
                    self.caret = self.query.len();
                } else if self.caret < self.query.len() {
                    let next = self.query[self.caret..]
                        .chars()
                        .next()
                        .map(|c| self.caret + c.len_utf8())
                        .unwrap_or(self.query.len());
                    self.caret = next;
                }
                Some((self, Intent::Idle))
            }
            Key::Home => {
                self.select_all = false;
                self.caret = 0;
                Some((self, Intent::Idle))
            }
            Key::End => {
                self.select_all = false;
                self.caret = self.query.len();
                Some((self, Intent::Idle))
            }
            Key::SelectAll => {
                if !self.query.is_empty() {
                    self.select_all = true;
                    self.caret = self.query.len();
                }
                Some((self, Intent::Idle))
            }
            Key::Enter => Some((
                self,
                if layout.answers.is_empty() {
                    Intent::Idle
                } else {
                    Intent::Selected
                },
            )),
            Key::Up => {
                self.select_all = false;
                self.selected = self.selected.saturating_sub(1);
                self.offset = layout.reveal(self.offset, self.selected);
                Some((self, Intent::Idle))
            }
            Key::Down => {
                self.select_all = false;
                self.selected = (self.selected + 1).min(last);
                self.offset = layout.reveal(self.offset, self.selected);
                Some((self, Intent::Idle))
            }
            _ => None,
        }
    }

    // `Ctrl+K`: the actions of the selected answer, or the list once they are open
    fn opened(mut self, layout: &Layout) -> Option<(Search, Intent)> {
        if layout.answers.is_empty() {
            return Some((self, Intent::Idle));
        }

        self.chosen = 0;
        self.panel_offset = 0.0;

        Some((self, Intent::Panel))
    }

    /*
     * a key while the actions are open: the ring moves over the rows, Enter or Space presses the
     * one it is on and a click the one clicked; Escape and `Ctrl+K` close the panel unacted, as does
     * a key no row takes, so the list comes back rather than typing under a panel
     */
    fn stepped_panel(mut self, key: Key) -> Option<(Search, Intent)> {
        let last = self
            .panel
            .as_ref()
            .map_or(0, |panel| panel.offers.len().saturating_sub(1));

        match key {
            Key::Escape | Key::Shortcut('k') => {
                self.closed();
                Some((self, Intent::Idle))
            }
            Key::Enter | Key::Space => Some((self, Intent::Offer)),
            Key::Up => {
                self.chosen = self.chosen.saturating_sub(1);
                self.panel_offset = self.reveal_offer(self.chosen);
                Some((self, Intent::Idle))
            }
            Key::Down => {
                self.chosen = (self.chosen + 1).min(last);
                self.panel_offset = self.reveal_offer(self.chosen);
                Some((self, Intent::Idle))
            }
            Key::Shortcut(letter) => {
                let at = letter
                    .to_digit(10)
                    .filter(|&digit| digit >= 1)
                    .map(|digit| digit as usize - 1)
                    .filter(|&at| at <= last)?;

                self.chosen = at;
                self.panel_offset = self.reveal_offer(at);
                Some((self, Intent::Offer))
            }
            // a key the panel has no use for closes it, unacted, and the list takes nothing
            _ => {
                self.closed();
                Some((self, Intent::Idle))
            }
        }
    }

    // the actions closed, the selection where it was
    fn closed(&mut self) {
        self.panel = None;
        self.chosen = 0;
        self.panel_offset = 0.0;
    }

    // the rows of the actions as tall as they ask, before the room caps them
    fn offers(&self) -> f32 {
        let rows = self.panel.as_ref().map_or(0, |panel| panel.offers.len());

        (rows as f32 * (OFFER + ROW_GAP) - ROW_GAP).max(0.0)
    }

    // the room the rows scroll in: what the note, when there is one, leaves under them
    fn panel_room(&self) -> f32 {
        (room() - self.note_room()).max(OFFER)
    }

    // what a note under the rows takes, none while there is nothing to say
    fn note_room(&self) -> f32 {
        match self.note.is_some() {
            true => NOTE + ROW_GAP,
            false => 0.0,
        }
    }

    // how far the rows can scroll: none while they fit
    fn panel_most(&self) -> f32 {
        (self.offers() - self.panel_room()).max(0.0)
    }

    // the rows as tall as they show, scrolling once they reach past the room
    fn panel_listed(&self) -> f32 {
        self.offers().min(self.panel_room())
    }

    // the least scroll from the one now that shows the row at `at` whole
    fn reveal_offer(&self, at: usize) -> f32 {
        let top = at as f32 * (OFFER + ROW_GAP);

        self.panel_offset
            .min(top)
            .max(top + OFFER - self.panel_room())
            .clamp(0.0, self.panel_most())
    }

    // the rows scrolled by the wheel, the ring left where it is
    fn scrolled_offers(mut self, lines: f32) -> Search {
        self.panel_offset = (self.panel_offset + lines * WHEEL).clamp(0.0, self.panel_most());

        self
    }

    // scrolled by the wheel, the selection kept on a row that shows whole
    fn wheel(mut self, lines: f32, layout: &Layout) -> Search {
        let most = layout.most();

        self.offset = (self.offset.clamp(0.0, most) + lines * WHEEL).clamp(0.0, most);

        let (first, last) = layout.whole(self.offset);
        self.selected = self.selected.clamp(first, last);

        self
    }

    // what the view draws; a query no longer finding as many leaves the selection on the last one
    fn bounded(mut self, layout: &Layout) -> Search {
        self.selected = self.selected.min(layout.answers.len().saturating_sub(1));
        self.offset = self.offset.clamp(0.0, layout.most());
        self.caret = self.caret.min(self.query.len());
        while !self.query.is_char_boundary(self.caret) {
            self.caret = self.caret.saturating_sub(1);
        }
        if self.query.is_empty() {
            self.select_all = false;
        }

        // the panel follows the answers: a query that finds nothing leaves no answer to act on
        if let Some(panel) = &self.panel {
            let rows = panel.offers.len().saturating_sub(1);

            self.chosen = self.chosen.min(rows);

            if layout.answers.is_empty() {
                self.panel = None;
            }
        }

        self.panel_offset = self.panel_offset.clamp(0.0, self.panel_most());

        self
    }
}

/*
 * the list as the search scrolls it: a row for each answer and, with nothing typed, a heading
 * before each section's first. A heading has its own height, so the rows below keep their place
 * whatever a section holds
 */
#[derive(Debug, Default, Clone, PartialEq)]
struct Layout {
    // each row top down, with its top and height
    rows: Vec<(Line, f32, f32)>,

    // the top of each answer's row
    answers: Vec<f32>,

    // the list's height, headings and gaps included
    content: f32,
}

// a row of the list: a section's heading, or the answer at that place
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    Heading(&'static str),
    Answer(usize),
}

// a heading over a run of answers, and the answer it starts at
#[derive(Debug, Clone, PartialEq, Eq)]
struct Section {
    title: &'static str,
    at: usize,
}

impl Layout {
    // the answers alone, as a ranked query finds them
    #[cfg(test)]
    fn plain(count: usize) -> Layout {
        Layout::of(&[], count)
    }

    fn of(sections: &[Section], count: usize) -> Layout {
        let mut rows = Vec::with_capacity(count + sections.len());
        let mut answers = Vec::with_capacity(count);
        let mut top = 0.0;
        let mut next = sections.iter().peekable();

        for at in 0..count {
            // a section with nothing to list shows no heading
            while let Some(section) = next.peek()
                && section.at <= at
            {
                rows.push((Line::Heading(section.title), top, HEADER));
                top += HEADER + ROW_GAP;
                next.next();
            }

            answers.push(top);
            rows.push((Line::Answer(at), top, ROW));
            top += ROW + ROW_GAP;
        }

        Layout {
            rows,
            answers,
            content: (top - ROW_GAP).max(0.0),
        }
    }

    // how far the list can scroll: none while it fits
    fn most(&self) -> f32 {
        (self.content - room()).max(0.0)
    }

    // the list as tall as its rows, scrolling once they reach past the room
    fn listed(&self) -> f32 {
        self.content.min(room())
    }

    // the least scroll from `offset` that shows the answer at `at` whole
    fn reveal(&self, offset: f32, at: usize) -> f32 {
        let Some(&top) = self.answers.get(at) else {
            return offset.clamp(0.0, self.most());
        };

        offset
            .min(top)
            .max(top + ROW - room())
            .clamp(0.0, self.most())
    }

    // the first and last answers that show whole at `offset`, the nearest when none does
    fn whole(&self, offset: f32) -> (usize, usize) {
        let end = offset + room();
        let mut first = None;
        let mut last = 0;

        for (at, &top) in self.answers.iter().enumerate() {
            if top + ROW <= end + 0.5 && top >= offset - 0.5 {
                first.get_or_insert(at);
                last = at;
            }
        }

        first.map_or_else(
            || {
                let nearest = self
                    .answers
                    .iter()
                    .position(|&top| top + ROW > offset)
                    .unwrap_or(self.answers.len().saturating_sub(1));

                (nearest, nearest)
            },
            |first| (first, last),
        )
    }
}

// how tall a list `list` pixels high asks the body to be, or a state for none; the island caps it
// (ADR 0030)
fn asks(list: f32) -> f32 {
    let list = if list <= 0.0 { STATE } else { list };

    2.0 * INSET + FIELD + GAP + list
}

/*
 * tells the island how tall the Launcher asks to be: a new visit lists the sections, the one open
 * what its query found. After each change of the apps or the search, never from a view
 */
pub fn fit() {
    let _ordered = super::fitting();
    let (visit, rec) = {
        let island = IslandService::read();

        (island.visit(), recorded(&island))
    };

    let (fresh, open) = {
        let apps = Apps::read();
        let search = Search::read().of(visit);
        let query = search.query.clone();

        // a new visit starts with nothing typed, so it lists the sections
        let read = apps.found();

        let fresh = asks(found(visit, apps.list(), "", rec, read).layout().listed());

        // the actions stand in the list's place, as tall as their rows and the note under them
        let open = match search.panel.is_some() {
            true => {
                let layout = found(visit, apps.list(), &query, rec, read).layout();
                let search = search.bounded(&layout);

                asks(search.panel_listed() + search.note_room())
            }
            false if query.trim().is_empty() => fresh,
            false => asks(
                found(visit, apps.list(), &query, rec, read)
                    .layout()
                    .listed(),
            ),
        };

        (fresh, open)
    };

    super::ask(Surface::Launcher, fresh, Some((visit, open)));
}

// at start: the body follows the apps and the search from now on
pub fn start() {
    service::watch::<config::Reloads>(follow_style);
    follow_style();
    service::watch::<Connectivity>(follow_radios);
    service::watch::<Adapter>(follow_radios);
    follow_radios();
    service::watch::<Radios>(fit);
    service::watch::<Apps>(fit);
    service::watch::<Search>(fit);
    service::watch::<Choosing>(fit);
    service::watch::<windows::Held>(fit);

    // and the ranking follows them, ranking again when any is written
    service::watch::<Radios>(forget);
    service::watch::<Apps>(forget);
    service::watch::<windows::Held>(forget);
    service::watch::<clipboard::Clipboard>(forget);
    service::watch::<config::Reloads>(forget);
    service::watch::<IslandService>(choose::follow);
    fit();
}

/*
 * `launcher.style`, pushed in: `island/` never reads the config, so the place the Launcher opens is
 * a field of its own Service, set here at start and on each reload. A change while the Launcher is
 * open closes it where it is rather than moving it: the place it opens is not the place it stands
 * (ADR 0131)
 */
fn follow_style() {
    let style = config::get().launcher_style;

    // the open Launcher, read under the same write that sets the style, so no opening lands between
    let open = {
        let mut island = IslandService::write();

        if island.launcher_style() == style {
            island.quiet();

            return;
        }

        island.set_launcher_style(style);

        island.open_launcher_on()
    };

    if let Some(monitor) = open {
        view::collapse(&monitor);
    }
}

// whether the query searches the clipboard history, which it does only with its Module on
fn history(query: &str) -> bool {
    query.trim().starts_with(history::PREFIX) && modules::on("clipboard")
}

/*
 * whether the machine has each radio, as far as Wi-Fi and Bluetooth are on. A Service of its own,
 * so the Launcher draws again when a radio comes or goes and not on each state it takes
 */
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Radios {
    wifi: bool,
    bluetooth: bool,
}

impl Service for Radios {
    fn new() -> Self {
        Radios::default()
    }

    fn listen() {}
}

// a radio is read only with its Module on
fn follow_radios() {
    let radios = Radios {
        wifi: modules::on("network") && Connectivity::read().wifi != Radio::Missing,
        bluetooth: modules::on("bluetooth") && Adapter::read().radio != Radio::Missing,
    };

    let mut write = Radios::write();

    if *write == radios {
        write.quiet();
    } else {
        *write = radios;
    }
}

// where Kanade goes that exists now: its Module is on and, for a radio, the machine has one
fn destinations() -> Destinations {
    let controls = modules::on("controls");
    let radios = Radios::read();

    Destinations {
        wifi: controls && radios.wifi,
        bluetooth: controls && radios.bluetooth,
        notifications: modules::on("notification-surface"),
        controls,
        weather: modules::on("weather-surface"),
        tray: modules::on("tray"),
        session: modules::on("session"),
        timer: modules::on("timer-surface"),
        capture: modules::on("capture-surface"),
        clipboard: modules::on("clipboard-surface"),
        calendar: modules::on("calendar-surface"),
        wallpapers: modules::on("wallpaper-surface"),
        keys: modules::on("keys-surface"),
        settings: modules::on("settings"),
    }
}

// the verbs that exist now: their Module is on and, for a radio, the machine has one
fn commands(offered: bool, recording: Rec) -> Commands {
    if !offered {
        return Commands::default();
    }

    let radios = Radios::read();

    Commands {
        caffeine: modules::on("caffeine"),
        night_light: modules::on("night-light"),
        focus: modules::on("focus"),
        notifications: modules::on("notifications"),
        wifi: radios.wifi,
        bluetooth: radios.bluetooth,
        power: modules::on("power"),
        capture: modules::on("capture"),
        lock: modules::on("lock"),
        wallpaper: modules::on("wallpaper"),
        dictation: modules::on("dictation"),
        recording,
    }
}

// how the recording stands, from the island the caller already holds: a view never reads it again
pub fn recorded(island: &IslandService) -> Rec {
    let standing = recording::standing(island);

    match (standing.path, standing.paused) {
        (None, _) => Rec::Idle,
        (Some(_), true) => Rec::Paused,
        (Some(_), false) => Rec::Running {
            pausable: standing.pausable,
        },
    }
}

// what the providers find for the query in `visit`: best first, or, with nothing typed, the
// empty query's sections under a heading each
#[derive(Debug, Default, Clone, PartialEq)]
struct Found {
    answers: Vec<Answer>,

    // a heading over the answers it starts at, in the order they list
    sections: Vec<Section>,
}

impl Found {
    // what a query finds, with no heading
    fn ranked(answers: Vec<Answer>) -> Found {
        Found {
            answers,
            sections: Vec::new(),
        }
    }

    fn layout(&self) -> Layout {
        Layout::of(&self.sections, self.answers.len())
    }

    fn is_empty(&self) -> bool {
        self.answers.is_empty()
    }

    fn get(&self, at: usize) -> Option<&Answer> {
        self.answers.get(at)
    }
}

/*
 * the empty query's sections, in the order the plan gives them: the Recents by frecency, then
 * Kanade's Commands, the open Windows and the Apps. An answer the Recents show is taken out of the
 * section it would list in, so nothing lists twice, and a recent that no longer exists is left out
 */
fn sections(apps: &[App], key: &Inputs, recording: Rec) -> Found {
    let destinations = destination(key.destinations);
    let commands = commands(key.launcher_commands, recording);
    let apps = AppsProvider(apps, destinations.wallpapers);

    let mut found_apps = apps.find("");
    let mut found_commands = commands.find("");
    let mut found_destinations = destinations.find("");
    let found_windows = windows::Titles(&key.titles).find("");

    // the Recents, highest score first, each taken from the answers it would list among
    let recent = sources::recents::with(|usage| {
        usage
            .recent(crate::clock::unix_seconds())
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    });

    let mut recents = Vec::new();

    for key in recent {
        let key = key.as_str();
        let found = if key.starts_with("command:") {
            &mut found_commands
        } else if key.starts_with("destination:") {
            &mut found_destinations
        } else {
            &mut found_apps
        };

        if let Some(at) = found
            .iter()
            .position(|answer| kept(answer).as_deref() == Some(key))
        {
            let mut answer = found.remove(at);
            answer.fit = Fit::Recent;
            recents.push(answer);
        }
    }

    gathered([
        ("Recents", recents),
        ("Commands", found_commands),
        ("Windows", found_windows),
        ("Apps", found_apps),
    ])
}

/*
 * the sections laid out in the order given, each under its heading, and none for a section that
 * found nothing: a heading over no answers says nothing
 */
fn gathered(sections: [(&'static str, Vec<Answer>); 4]) -> Found {
    let mut answers = Vec::new();
    let mut placed = Vec::new();

    for (title, found) in sections {
        if found.is_empty() {
            continue;
        }

        placed.push(Section {
            title,
            at: answers.len(),
        });
        answers.extend(found);
    }

    Found {
        answers,
        sections: placed,
    }
}

// the key `sources::recents` keeps an answer under, none for one it never tracks
fn kept(answer: &Answer) -> Option<String> {
    match &answer.action {
        Action::Launch(_, id) | Action::Perform(_, id, _) => Some(sources::recents::app(id)),
        Action::Run(verb) => Some(sources::recents::command(verb)),
        Action::Open(destination) => Some(sources::recents::destination(destination.name())),
        _ => None,
    }
}

/*
 * ADR 0127's order inside a Fit: the class of a query that launched the answer leads, then the
 * match score (ADR 0126), then the providers' own order. `provider::ranked` gave the last two, so
 * sorting stably on the Fit and the class alone moves a habit ahead of the noisy in-tier scores and
 * leaves every answer of an equal class where it lay: `fit` sometimes reads an alias where the
 * score reads the title, so the score alone is not the better order
 */
fn learned_first(answers: &mut [Answer], query: &str, now: i64, usage: &sources::recents::Usage) {
    let query = query.trim().to_lowercase();

    answers.sort_by_key(|answer| {
        let learned = kept(answer).map_or(0, |key| usage.learned(&key, &query, now));

        (answer.fit, std::cmp::Reverse(learned))
    });
}

/*
 * what the providers last found for a query, and what that depended on. The view runs every frame of
 * a morph (`crates/runtime/src/wayland/redraw.rs`), so ranking eight providers and sorting them
 * again each frame is work the frame does not change: it is kept until one of its inputs does
 */
#[derive(Default)]
struct Memo {
    key: Option<Inputs>,

    /*
     * the apps that `found` was ranked for, kept beside the key and compared rather than hashed: a
     * compare stops at the first app that differs, while hashing every name and action would cost
     * about as much as the ranking it saves. Only a ranking that runs copies them
     */
    apps: Vec<App>,
    found: Arc<Found>,
}

// everything a ranking depends on besides its query, so a change to any of it ranks again
#[derive(Default, PartialEq, Eq)]
struct Inputs {
    visit: u64,

    // the query as typed, before trimming
    query: String,

    // the query's case is its own: the fit folds it, the recents do not
    recording: Rec,

    // whether the apps were read yet, and the two config switches
    read: bool,
    actions: bool,
    launcher_commands: bool,

    // which destinations exist, as the flags they are: `Destinations` is not compared, its file
    // not this one's
    destinations: [bool; 14],

    // what the ranking read off the services, so a change to any of them ranks again
    radios: Radios,
    titles: Vec<windows::Entry>,

    // the clipboard's entries the history search reads, by id
    history: Option<Vec<u64>>,

    /*
     * the second the ranking was made in. A score decays with the clock, and the queries that
     * launched an app are remembered for a while, so the answer is worked out again once a second
     * at most: fresh enough to watch, and a frame of a morph never re-ranks
     */
    now: i64,
}

static MEMO: LazyLock<Mutex<Memo>> = LazyLock::new(Mutex::default);

/*
 * what was ranked is dropped, so the next draw ranks again. Called from a watcher: what a Service
 * was written with is not always what a ranking's inputs show, and the writes that leave them equal
 * are the ones a comparison alone would miss
 */
fn forget() {
    MEMO.locked().key = None;
}

/*
 * what the providers find for the query in `visit`, best first; a dmenu's lines alone in its visit.
 * `read` says whether the apps were read at all: until they are, an app may be the answer, so the
 * web is not offered for a query they could still match
 *
 * The answer is kept from the frame before, and what it depends on is read every time, so a change
 * still draws the window that shows it: the Services it reads are read here, not only when the
 * ranking runs
 */
fn found(visit: u64, apps: &[App], query: &str, recording: Rec, read: bool) -> Arc<Found> {
    if let Some((_, choices)) = choose::of(visit) {
        return Arc::new(Found::ranked(provider::ranked(
            &[&Choices(&choices)],
            query,
        )));
    }

    let destinations = destinations();
    let radios = Radios::read();
    let titles = windows::titles();
    let history = history(query).then(clipboard::Clipboard::read);
    let entries = history
        .as_deref()
        .map_or(&[][..], clipboard::Clipboard::entries);

    let key = Inputs {
        visit,
        query: query.to_owned(),
        recording,
        read,
        actions: config::get().launcher_actions,
        launcher_commands: config::get().launcher_commands,
        destinations: [
            destinations.wifi,
            destinations.bluetooth,
            destinations.notifications,
            destinations.controls,
            destinations.weather,
            destinations.tray,
            destinations.session,
            destinations.timer,
            destinations.capture,
            destinations.clipboard,
            destinations.calendar,
            destinations.wallpapers,
            destinations.keys,
            destinations.settings,
        ],
        radios: *radios,
        titles,
        history: history
            .as_deref()
            .map(|held| held.entries().iter().map(|entry| entry.id).collect()),
        now: crate::clock::unix_seconds(),
    };

    let mut memo = MEMO.locked();
    let same = memo.key.as_ref() == Some(&key) && memo.apps == apps;

    if !same {
        memo.found = Arc::new(rank(apps, query, recording, read, &key, entries));
        memo.key = Some(key);
        memo.apps = apps.to_vec();
    }

    // one reference, so a frame that changes nothing copies no answer
    Arc::clone(&memo.found)
}

// what the providers find now, which `found` keeps until its inputs change
fn rank(
    apps: &[App],
    query: &str,
    recording: Rec,
    read: bool,
    key: &Inputs,
    entries: &[clipboard::Entry],
) -> Found {
    grouped(
        query,
        || sections(apps, key, recording),
        || {
            let mut ranked = provider::ranked(
                &[
                    &Calculator,
                    &AppsProvider(apps, key.destinations[11]),
                    &Actions(apps, key.actions),
                    &windows::Titles(&key.titles),
                    &destination(key.destinations),
                    &commands(key.launcher_commands, recording),
                    &Emoji,
                    &History { entries },
                ],
                query,
            );

            sources::recents::with(|usage| learned_first(&mut ranked, query, key.now, usage));

            Found::ranked(offered(query, ranked, read, web::openable))
        },
    )
}

// the destinations the flags in a ranking's inputs name
fn destination(flags: [bool; 14]) -> Destinations {
    let [
        wifi,
        bluetooth,
        notifications,
        controls,
        weather,
        tray,
        session,
        timer,
        capture,
    ] = flags[..9].try_into().expect("nine of fourteen");

    Destinations {
        wifi,
        bluetooth,
        notifications,
        controls,
        weather,
        tray,
        session,
        timer,
        capture,
        clipboard: flags[9],
        calendar: flags[10],
        wallpapers: flags[11],
        keys: flags[12],
        settings: flags[13],
    }
}

/*
 * what a typed query lists: what the providers found, or, when none of them found anything, the
 * web's two rows. The fallback lives here, off the providers' own list, so a provider can never
 * answer it and the web can never outrank an answer (ADR 0130). `openable` is asked only when
 * something is offered, so a query the providers answer reads no PATH
 */
fn offered(
    query: &str,
    ranked: Vec<Answer>,
    read: bool,
    openable: impl FnOnce() -> bool,
) -> Vec<Answer> {
    match ranked.is_empty() && read {
        true => web::offers(query, openable),
        false => ranked,
    }
}

/*
 * the answers a query finds: with nothing typed the empty query's sections, and the ranking
 * itself for anything typed; each is built only when it is the one to list, so a typed query never
 * asks the providers for an empty one
 */
fn grouped(query: &str, sections: impl FnOnce() -> Found, ranked: impl FnOnce() -> Found) -> Found {
    if query.trim().is_empty() {
        sections()
    } else {
        ranked()
    }
}

// the view's own read of the visit and the recording, so this never reads IslandService again
pub fn surface(monitor: &str, visit: u64, recording: Rec) -> Rectangle {
    let apps = Apps::read();
    let search = Search::read().of(visit);
    let found = found(visit, apps.list(), &search.query, recording, apps.found());
    let layout = found.layout();
    let search = search.bounded(&layout);
    let choosing = choose::of(visit);

    let query = search.query.trim();
    let emoji = query.starts_with(emoji::PREFIX);
    let history = history(query);

    let list: Box<dyn Widget> = if let Some(panel) = &search.panel {
        Box::new(panel_view(monitor, &search, panel))
    } else if !found.is_empty() {
        Box::new(list(monitor, &found, &layout, &search))
    } else if choosing.is_some() {
        Box::new(state(
            "No matching choices",
            &format!("Nothing found for \u{201c}{query}\u{201d}"),
        ))
    } else if history {
        Box::new(state(
            "Nothing copied",
            &if query.len() > 1 {
                format!("Nothing found for \u{201c}{}\u{201d}", query[1..].trim())
            } else {
                String::from("What you copy shows here")
            },
        ))
    } else if !apps.found() && !emoji {
        // read once the Module starts, so only a Launcher opened at once waits
        Box::new(state("Finding apps", ""))
    } else if let Some((title, detail)) = web::refused(query, web::openable) {
        // nothing answers it and the browser cannot be opened, so the row says why rather than
        // offering a search that would do nothing (ADR 0130)
        Box::new(state(title, &detail))
    } else {
        Box::new(state(
            if emoji {
                "No matching emoji"
            } else {
                "No matching apps"
            },
            &format!("Nothing found for \u{201c}{query}\u{201d}"),
        ))
    };

    let shape = view::largest();

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .child(
            Column::new(vec![
                Box::new(view::rise(
                    0,
                    field(
                        &search.query,
                        search.caret,
                        search.select_all,
                        match &choosing {
                            Some((Some(prompt), _)) => prompt,
                            Some((None, _)) => "Search choices",
                            None => placeholder(),
                        },
                    ),
                )) as Box<dyn Widget>,
                Box::new(view::rise(1, list)),
            ])
            .width(width())
            .gap(GAP),
        )
}

/*
 * a search icon, what is typed with the caret, or what to type faded; a query wider than
 * the field shows the window around the caret
 */
fn field(query: &str, caret: usize, select_all: bool, placeholder: &str) -> Rectangle {
    let room = width() - 2.0 * FIELD_INSET - TARGET - ICON_GAP - CARET;

    let caret_widget = Rectangle::new()
        .width(CARET)
        .height(QUERY + 4.0)
        .fill(theme::island().on_surface);

    let typed: Box<dyn Widget> = if query.is_empty() {
        Box::new(
            Row::new(children![
                caret_widget,
                // a dmenu's prompt may be wider than the field
                Rectangle::new()
                    .width(room)
                    .height(QUERY + 4.0)
                    .align_child(Start, Center)
                    .child(
                        Text::new(placeholder)
                            .size(QUERY)
                            .color(theme::island().on_surface_variant)
                            .weight(theme::text::MEDIUM)
                            .elide(),
                    ),
            ])
            .align(Center),
        )
    } else if select_all {
        // measured on the whole query: one wider than the field shows its end instead
        let whole = Text::new(query)
            .size(QUERY)
            .color(theme::island().on_surface)
            .weight(theme::text::MEDIUM);

        /*
         * the selection is as wide as the letters it covers, with a little air around them, so
         * the caret follows the text; a query wider than the field fills it
         */
        let (selected, selected_width, inset): (Box<dyn Widget>, f32, f32) = match whole.width() {
            Size::Fixed(natural) if natural <= room => {
                // the air gives way rather than push the caret out of the field
                let inset = SELECTION_INSET.min((room - natural) / 2.0);

                (Box::new(whole), natural + 2.0 * inset, inset)
            }
            _ => (Box::new(super::search::tail(query, room)), room, 0.0),
        };

        Box::new(
            Row::new(children![
                Rectangle::new()
                    .width(selected_width)
                    .height(QUERY + 4.0)
                    .fill(theme::island().surface_container_high)
                    .radius(radius::HAIRLINE)
                    .padding(Padding {
                        top: 0.0,
                        right: inset,
                        bottom: 0.0,
                        left: inset,
                    })
                    .align_child(Start, Center)
                    .child(selected),
                caret_widget,
            ])
            .align(Center),
        )
    } else {
        cursor_text(query, caret, room, caret_widget)
    };

    Rectangle::new()
        .width(width())
        .height(FIELD)
        .radius(FIELD / 2.0)
        .fill(theme::island().surface_container)
        .padding(Padding {
            top: 0.0,
            right: FIELD_INSET,
            bottom: 0.0,
            left: FIELD_INSET,
        })
        .align_child(Start, Center)
        .semantics(|| {
            Semantics::new(Role::TextInput, "search")
                .text(query)
                .focused(true)
        })
        .child(
            Row::new(vec![
                Box::new(
                    Rectangle::new()
                        .width(TARGET)
                        .height(TARGET)
                        .align_child(Center, Center)
                        .child(Icon::Search.on(16.0, theme::island().on_surface_variant)),
                ) as Box<dyn Widget>,
                typed,
            ])
            .gap(ICON_GAP)
            .align(Center),
        )
}

// what to type, `@` only with the Wallpapers Surface on
fn placeholder() -> &'static str {
    match (modules::on("wallpaper-surface"), modules::on("clipboard")) {
        (true, true) => "Search apps, settings, 2+2, :emoji, @wallpaper or >clipboard",
        (true, false) => "Search apps, settings, 2+2, :emoji or @wallpaper",
        (false, true) => "Search apps, settings, 2+2, :emoji or >clipboard",
        (false, false) => "Search apps, settings, 2+2 or :emoji",
    }
}

fn cursor_text(query: &str, caret: usize, room: f32, caret_widget: Rectangle) -> Box<dyn Widget> {
    let text = |shown: &str| {
        Text::new(shown)
            .size(QUERY)
            .color(theme::island().on_surface)
            .weight(theme::text::MEDIUM)
    };
    let fits = |text: &Text| matches!(text.width(), Size::Fixed(natural) if natural <= room);

    let whole = text(query);
    if fits(&whole) {
        let caret = caret.min(query.len());
        let before = &query[..caret];
        let after = &query[caret..];
        let mut elements: Vec<Box<dyn Widget>> = Vec::with_capacity(3);
        if !before.is_empty() {
            elements.push(Box::new(text(before)));
        }
        elements.push(Box::new(caret_widget));
        if !after.is_empty() {
            elements.push(Box::new(text(after)));
        }
        return Box::new(Row::new(elements).align(Center));
    }

    if caret >= query.len() {
        return Box::new(
            Row::new(children![super::search::tail(query, room), caret_widget]).align(Center),
        );
    }

    let letters: Vec<(usize, char)> = query.char_indices().collect();
    let caret_char_idx = letters
        .iter()
        .position(|&(idx, _)| idx == caret)
        .unwrap_or(letters.len());

    let mut left = caret_char_idx;
    let mut right = caret_char_idx;
    while left > 0 || right < letters.len() {
        let try_left = left.saturating_sub(1);
        let try_right = (right + 1).min(letters.len());

        let start_byte = letters[try_left].0;
        let end_byte = if try_right < letters.len() {
            letters[try_right].0
        } else {
            query.len()
        };
        let prefix = if try_left > 0 { "\u{2026}" } else { "" };
        let suffix = if try_right < letters.len() {
            "\u{2026}"
        } else {
            ""
        };
        let candidate = format!("{prefix}{}{suffix}", &query[start_byte..end_byte]);
        if !fits(&text(&candidate)) {
            break;
        }
        left = try_left;
        right = try_right;
    }

    let start_byte = if left < letters.len() {
        letters[left].0
    } else {
        0
    };
    let end_byte = if right < letters.len() {
        letters[right].0
    } else {
        query.len()
    };
    let prefix = if left > 0 { "\u{2026}" } else { "" };
    let suffix = if right < letters.len() {
        "\u{2026}"
    } else {
        ""
    };
    let before_str = format!("{prefix}{}", &query[start_byte..caret]);
    let after_str = format!("{}{suffix}", &query[caret..end_byte]);

    let mut elements: Vec<Box<dyn Widget>> = Vec::with_capacity(3);
    if !before_str.is_empty() {
        elements.push(Box::new(text(&before_str)));
    }
    elements.push(Box::new(caret_widget));
    if !after_str.is_empty() {
        elements.push(Box::new(text(&after_str)));
    }
    Box::new(Row::new(elements).align(Center))
}

// what it means, in the middle of where the rows go
fn state(title: &str, detail: &str) -> Rectangle {
    let mut lines = children![
        Icon::Search.draw(28.0),
        Text::new(title)
            .size(theme::text::BODY)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD),
    ];

    if !detail.is_empty() {
        let text = Text::new(detail)
            .size(theme::text::LABEL_SMALL)
            .color(theme::island().on_surface_variant)
            .weight(theme::text::MEDIUM);

        // centred while it fits, elided only when a long query does not, since elided text fills
        // its width
        lines.push(Box::new(match text.width() {
            Size::Fixed(natural) if natural <= width() => text,
            _ => text.elide(),
        }));
    }

    Rectangle::new()
        .width(width())
        .height(STATE.min(room()))
        .align_child(Center, Center)
        .child(Column::new(lines).gap(6.0).align(Center))
}

/*
 * the actions of the answer the Launcher selected, in the list's place: a row for each, the ring on
 * the one Enter presses, scrolled by the wheel and pressed by a click, and under them why a pin the
 * settings file refused was not kept
 */
fn panel_view(monitor: &str, search: &Search, panel: &Panel) -> Stack {
    let end = search.panel_offset + search.panel_room();

    let shown: Vec<Box<dyn Widget>> = panel
        .offers
        .iter()
        .enumerate()
        .filter_map(|(at, offer)| {
            let top = at as f32 * (OFFER + ROW_GAP);

            (top + OFFER > search.panel_offset && top < end).then(|| {
                let chosen = at == search.chosen;

                Box::new(
                    offer_row(monitor, search.visit, offer, chosen)
                        .translate(0.0, top - search.panel_offset),
                ) as Box<dyn Widget>
            })
        })
        .collect();

    let viewport = Rectangle::new()
        .width(width())
        .height(search.panel_listed())
        .clip()
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::Menu, "actions"))
        .on_scroll(|Scroll { y, .. }| wheel_panel(y))
        .child(
            Stack::new(shown)
                .width(width())
                .height(search.panel_listed()),
        );

    let mut layers = children![viewport];

    let most = search.panel_most();

    if most > 0.0 {
        let length = (search.panel_room() * search.panel_room() / search.offers()).max(TARGET);
        let at = (search.panel_room() - length) * search.panel_offset / most;

        layers.push(Box::new(
            Rectangle::new()
                .width(3.0)
                .height(length)
                .radius(radius::HAIRLINE)
                .fill(theme::island().surface_container_high)
                .translate(width() + 7.0, at),
        ));
    }

    if let Some(note) = &search.note {
        layers.push(Box::new(
            Rectangle::new()
                .width(width())
                .height(NOTE)
                .align_child(Start, Center)
                .translate(0.0, search.panel_listed() + ROW_GAP)
                .child(
                    Text::new(note)
                        .size(theme::text::LABEL_SMALL)
                        .color(theme::signals().critical)
                        .weight(theme::text::MEDIUM)
                        .elide(),
                ),
        ));
    }

    Stack::new(layers)
        .width(width())
        .height(search.panel_listed() + search.note_room())
}

/*
 * one row of the actions: its name, what it acts on under it, and the ring on the one Enter presses.
 * A row that cannot act is dimmed and takes no press, its second line saying why
 */
fn offer_row(monitor: &str, visit: u64, offer: &Offer, chosen: bool) -> Rectangle {
    let mut lines = children![
        Text::new(&offer.label)
            .size(theme::text::BODY)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD)
            .elide()
    ];

    if !offer.detail.is_empty() {
        lines.push(Box::new(
            Text::new(&offer.detail)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::MEDIUM)
                .elide(),
        ));
    }

    let row = Rectangle::new()
        .width(width())
        .height(OFFER)
        .radius(radius::ROW)
        .padding(Padding {
            top: 0.0,
            right: ROW_INSET + 4.0,
            bottom: 0.0,
            left: ROW_INSET + ICON_GAP,
        })
        .align_child(Start, Center)
        .child(Column::new(lines).width(Parent).gap(1.0));

    let row = if chosen {
        row.fill(theme::island().surface_container)
    } else {
        row
    };

    let Some(act) = &offer.act else {
        return row.opacity(theme::DISABLED);
    };

    let monitor = monitor.to_owned();
    let act = act.clone();

    row.identity(("launcher-action", &offer.label))
        .cursor(Cursor::Pointer)
        .semantics(|| {
            Semantics::new(Role::MenuItem, "action")
                .label(offer.label.as_str())
                .selected(chosen)
        })
        .on_click(super::on_left(move || {
            act_on(&monitor, visit, act.clone());
        }))
        .border_if(chosen)
}

/*
 * the rows scrolled `offset` down, clipped to the list; a thumb in the inset says where while they
 * do not all fit. A heading is drawn where it stands, so the answers under it keep their place
 */
fn list(monitor: &str, found: &Found, layout: &Layout, search: &Search) -> Stack {
    let end = search.offset + room();

    let shown: Vec<Box<dyn Widget>> = layout
        .rows
        .iter()
        .filter(|(_, top, height)| top + height > search.offset && *top < end)
        .map(|(line, top, _)| {
            let at = top - search.offset;

            match line {
                Line::Heading(title) => {
                    Box::new(heading(title).translate(0.0, at)) as Box<dyn Widget>
                }
                Line::Answer(index) => {
                    let answer = &found.answers[*index];
                    let selected = *index == search.selected;
                    let failed = search.failed(&answer.action);

                    Box::new(
                        row(monitor, search.visit, answer, selected, failed.as_deref())
                            .translate(0.0, at),
                    ) as Box<dyn Widget>
                }
            }
        })
        .collect();

    let viewport = Rectangle::new()
        .width(width())
        .height(layout.listed())
        .clip()
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::List, "results"))
        .on_scroll(|Scroll { y, .. }| wheel(y))
        .child(Stack::new(shown).width(width()).height(layout.listed()));

    let mut layers = children![viewport];

    let most = layout.most();

    if most > 0.0 {
        let length = (room() * room() / layout.content).max(TARGET);
        let at = (room() - length) * search.offset / most;

        layers.push(Box::new(
            Rectangle::new()
                .width(3.0)
                .height(length)
                .radius(radius::HAIRLINE)
                .fill(theme::island().surface_container_high)
                .translate(width() + 7.0, at),
        ));
    }

    Stack::new(layers).width(width()).height(layout.listed())
}

// a section's heading, as the Keys Surface's groups draw theirs
fn heading(title: &str) -> Rectangle {
    Rectangle::new()
        .width(width())
        .height(HEADER)
        .padding(Padding {
            top: 0.0,
            right: ROW_INSET,
            bottom: 0.0,
            left: ROW_INSET,
        })
        .align_child(Start, End)
        .child(
            Text::new(title)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::SEMIBOLD),
        )
}

/*
 * the answer's mark, title and what it is, or that pressing it failed; the selected one stands
 * out, and pressing one does what it does
 */
fn row(
    monitor: &str,
    visit: u64,
    answer: &Answer,
    selected: bool,
    failed: Option<&str>,
) -> Rectangle {
    let mut lines = children![
        Text::new(&answer.title)
            .size(theme::text::BODY)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD)
            .elide()
    ];

    let detail = failed.unwrap_or(&answer.detail);

    if !detail.is_empty() {
        lines.push(Box::new(
            Text::new(detail)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::MEDIUM)
                .elide(),
        ));
    }

    let row = Rectangle::new()
        .width(width())
        .height(ROW)
        .radius(radius::ROW)
        .padding(Padding {
            top: 0.0,
            right: ROW_INSET + 4.0,
            bottom: 0.0,
            left: ROW_INSET,
        })
        .align_child(Start, Center)
        .cursor(Cursor::Pointer)
        .child(
            Row::new(vec![
                mark(&answer.mark),
                Box::new(Column::new(lines).width(Parent).gap(1.0)),
            ])
            .width(Parent)
            .gap(ICON_GAP)
            .align(Center),
        );

    let row = if selected {
        row.fill(theme::island().surface_container)
    } else {
        row
    };
    let row = row.border_if(selected);

    let monitor = monitor.to_owned();
    let action = answer.action.clone();
    let query = Search::read().of(visit).query;

    row.identity(&answer.action)
        .semantics(|| {
            Semantics::new(Role::Item, "result")
                .content(answer.title.as_str())
                .selected(selected)
        })
        .on_click(super::on_left(move || {
            press(&monitor, visit, action.clone(), &query);
        }))
}

/*
 * an app's icon, a wallpaper, a letter or sign on the quiet tile, or an emoji; nothing while an
 * image decodes, so no row shifts
 */
fn mark(mark: &Mark) -> Box<dyn Widget> {
    // decoded at twice its size, crisp at scale 2
    let pixels = (ICON * 2.0) as u32;

    match mark {
        Mark::Picture(path) => Box::new(
            Rectangle::new()
                .width(ICON)
                .height(ICON)
                .fill(Image::contain(path).thumbnail(pixels, pixels)),
        ),
        Mark::Tile(sign) => Box::new(view::tile(None, sign, ICON, radius::ICON, &theme::island())),
        Mark::Icon(icon) => Box::new(
            Rectangle::new()
                .width(ICON)
                .height(ICON)
                .align_child(Center, Center)
                .child(icon.draw(ICON * 0.8)),
        ),
        Mark::Glyph(glyph) => Box::new(
            Rectangle::new()
                .width(ICON)
                .height(ICON)
                .align_child(Center, Center)
                .child(Text::new(*glyph).size(ICON * 0.8)),
        ),
    }
}

/*
 * a key while this island shows the Launcher; false for one it does not use, which the window's
 * own keys then get. A key it uses keeps the held island open for another hold
 */
pub fn key(monitor: &str, key: Key) -> bool {
    let (visit, rec) = {
        let island = IslandService::read();

        if island.presentation(monitor) != Presentation::Expanded(Surface::Launcher) {
            return false;
        }

        (island.visit(), recorded(&island))
    };

    typed(monitor, visit, rec, key)
}

/*
 * the keys a Launcher's visit takes, once something says it is the one showing: the search, its
 * sections and its actions, and the press the key came to. Both hosts call this, so a detached
 * Launcher searches and presses exactly as one in the Island does (ADR 0131)
 */
fn typed(monitor: &str, visit: u64, rec: Rec, key: Key) -> bool {
    let search = Search::read().of(visit);
    let apps = Apps::read();
    let found = found(visit, apps.list(), &search.query, rec, apps.found());
    let layout = found.layout();

    let Some((mut search, intent)) = search.bounded(&layout).step(key, &layout) else {
        return false;
    };

    let query = search.query.clone();

    // a failed press is said until the next key
    if matches!(search.pressing, Pressing::Failed(..)) {
        search.pressing = Pressing::Idle;
    }
    search.note = None;

    // the answer the panel acts on, or the row of it its ring is on; read before the search is
    // stored, which moves it
    let selected = search.selected;
    let panel = search.panel.clone();
    let offer = panel
        .as_ref()
        .and_then(|panel| panel.offers.get(search.chosen))
        .and_then(|offer| offer.act.clone());

    store(search);

    IslandService::write().attend(monitor, Instant::now());

    match intent {
        Intent::Idle => {}

        // `Ctrl+K` opens the actions of the answer selected now, which the view then draws
        Intent::Panel => opened(visit, &found, selected),

        Intent::Selected => {
            if let Some(answer) = found.get(selected) {
                press(monitor, visit, answer.action.clone(), &query);
            }
        }

        // the nth answer, which the list already held
        Intent::At(at) => {
            if let Some(answer) = found.get(at) {
                press(monitor, visit, answer.action.clone(), &query);
            }
        }

        Intent::Offer => {
            if let Some(act) = offer {
                acted(monitor, visit, act, &query);
            }
        }
    }

    true
}

// the actions of the answer at `at`, read now so what it says of its app and the Dock is current
fn opened(visit: u64, found: &Found, at: usize) {
    let Some(answer) = found.get(at) else {
        return;
    };

    let apps = Apps::read();
    let pinned = config::get().pinned.clone();
    let panel = actions::of(answer, apps.list(), &pinned);

    let mut search = Search::read().of(visit);

    if search.visit != visit {
        return;
    }

    search.panel = Some(panel);
    search.chosen = 0;
    search.panel_offset = 0.0;
    store(search);
}

/*
 * a row of the actions pressed by pointer: the same as Enter on it, over the answer the Launcher
 * still has selected, and the query it was found by
 */
fn act_on(monitor: &str, visit: u64, act: Act) {
    let search = Search::read().of(visit);

    if search.visit != visit {
        return;
    }

    acted(monitor, visit, act, &search.query);
}

// the actions' rows scrolled by the wheel
fn wheel_panel(lines: f32) {
    let visit = IslandService::read().visit();
    let search = Search::read().of(visit);

    if search.panel.is_none() {
        return;
    }

    store(search.scrolled_offers(lines));
}

// what a row of the actions asked for
fn acted(monitor: &str, visit: u64, act: Act, query: &str) {
    match act {
        Act::Press(action) => {
            // the answer's own press closes the island itself, and a desktop action's does too;
            // only a verb that keeps the island (`Run`) leaves the actions open to be closed here
            close_panel(visit);
            press(monitor, visit, action, query);
        }

        Act::Copy(text) => {
            close_panel(visit);

            if !Search::write().press_waiting(visit, Action::Copy(text.clone())) {
                return;
            }

            let monitor = monitor.to_owned();
            let action = Action::Copy(text.clone());
            let done = move |done| pressed(&monitor, visit, action, done);

            // the action the press was registered with, which is the one `press_failed` matches
            let registered = Action::Copy(text.clone());

            if let Err(why) = copy(text, done) {
                Search::write().press_failed(visit, registered, why);
            }
        }

        Act::Pin { id, pinned } => pinned_to(visit, id, pinned),
    }
}

fn close_panel(visit: u64) {
    let mut search = Search::read().of(visit);

    if search.visit == visit {
        search.closed();
        store(search);
    }
}

/*
 * pins the app with desktop file id `id` to the Dock, or unpins it, through the settings file as the
 * Dock's own menu does; off the view thread, as the write may wait, and the Dock follows the reload.
 * A refusal is said under the answer's actions, which stay open to try another
 */
fn pinned_to(visit: u64, id: String, pinned: bool) {
    thread::spawn(move || match settings::set_pinned(&id, pinned) {
        Ok(()) => {}

        Err(why) => {
            eprintln!("launcher: cannot set the Dock's pin: {why}");

            let mut search = Search::read().of(visit);

            if search.visit == visit {
                search.note = Some(why);
                store(search);
            }
        }
    });
}

/*
 * what starts, is copied or set is in the way of nothing, so the island closes; a copy, a wallpaper
 * or a verb once it is done. One of those at a time
 */
fn press(monitor: &str, visit: u64, action: Action, query: &str) {
    if let Action::Choose(index) = action {
        choose::chosen(visit, index);
        view::collapse(monitor);
        return;
    }

    if let Action::Launch(launch, id) = &action {
        if !Search::write().press_waiting(visit, action.clone()) {
            return;
        }

        if let Err(why) = launch.validate() {
            Search::write().press_failed(visit, action.clone(), why);
            return;
        }

        let launch = launch.clone();
        let monitor = monitor.to_owned();
        let action_for_result = action.clone();

        sources::recents::used(&sources::recents::app(id), query);

        // off the view thread, as niri's or the bus's answer may take its patience
        thread::spawn(move || {
            let result = launch.run();
            if let Err(error) = &result {
                eprintln!("launcher: {launch:?}: {error}");
            }
            pressed(&monitor, visit, action_for_result, result);
        });

        return;
    }

    if let Action::Focus(id) = &action {
        let id = *id;

        // off the view thread, as niri's answer may take its patience
        thread::spawn(move || match niri::act(&windows::focus(id)) {
            Ok(Acted::Done) => {}
            Ok(Acted::Unknown(why)) => eprintln!("launcher: focus {id}: no clear answer: {why}"),
            Err(error) => eprintln!("launcher: focus {id}: {error}"),
        });

        view::collapse(monitor);
        return;
    }

    if let Action::Run(verb) = &action {
        // counted before it runs, as an app is: a verb that may have run was still asked for
        sources::recents::used(&sources::recents::command(verb), query);

        if commands::clears_island(verb) {
            run_cleared(monitor, verb);
            return;
        }
    }

    if let Action::Perform(launch, id, action_name) = &action {
        if !Search::write().press_waiting(visit, action.clone()) {
            return;
        }

        if let Err(why) = launch.validate() {
            Search::write().press_failed(visit, action.clone(), why);
            return;
        }

        let (launch, action_name) = (launch.clone(), action_name.clone());
        let monitor = monitor.to_owned();
        let action_for_result = action.clone();

        sources::recents::used(&sources::recents::app(id), query);

        // off the view thread, as the bus's or niri's answer may take its patience
        thread::spawn(move || {
            let result = launch.run_action(&action_name);
            if let Err(error) = &result {
                eprintln!("launcher: {launch:?} {action_name}: {error}");
            }
            pressed(&monitor, visit, action_for_result, result);
        });

        return;
    }

    // the island changes Surface, so this is no copy to wait on
    if let Action::Open(destination) = &action {
        let key = sources::recents::destination(destination.name());

        if let Err(why) = open(monitor, destination) {
            Search::write().open_refused(visit, action, why);
        } else {
            sources::recents::used(&key, query);
        }

        return;
    }

    let search = Search::read().of(visit);

    if matches!(search.pressing, Pressing::Waiting(_)) {
        return;
    }

    store(Search {
        pressing: Pressing::Waiting(action.clone()),
        ..search
    });

    let done = {
        let (monitor, action) = (monitor.to_owned(), action.clone());
        move |done| pressed(&monitor, visit, action, done)
    };

    let started = match &action {
        Action::Copy(text) => copy(text.clone(), done),
        Action::Paste(id) => paste(*id, done),
        Action::Run(verb) => run(verb, done),
        Action::Browse(address) => browse(address.clone(), done),
        Action::Launch(..)
        | Action::Perform(..)
        | Action::Focus(_)
        | Action::Open(_)
        | Action::Choose(_) => Ok(()),
    };

    if let Err(why) = started {
        pressed(monitor, visit, action, Err(why));
    }
}

// the island shows another Surface, or Settings opens and the island closes; why not, as when the
// radio went since the tile was drawn
fn open(monitor: &str, destination: &Destination) -> Result<(), &'static str> {
    match destination {
        Destination::Wifi => controls::open_wifi(monitor)
            .then_some(())
            .ok_or("Controls did not open"),
        Destination::Bluetooth => controls::open_bluetooth(monitor)
            .then_some(())
            .ok_or("Controls did not open"),
        Destination::Clipboard => {
            IslandService::write().open(monitor, Surface::Clipboard, Instant::now());
            Ok(())
        }
        Destination::Calendar => {
            IslandService::write().open(monitor, Surface::Calendar, Instant::now());
            Ok(())
        }
        Destination::Notifications => {
            IslandService::write().open(monitor, Surface::Notifications, Instant::now());
            Ok(())
        }
        Destination::Controls => {
            IslandService::write().open(monitor, Surface::Controls, Instant::now());
            Ok(())
        }
        Destination::Weather => {
            // an old forecast is fetched again, as when `kanade weather open` asks (`ipc::island`)
            sources::weather::opened();
            IslandService::write().open(monitor, Surface::Weather, Instant::now());
            Ok(())
        }
        Destination::Tray => {
            IslandService::write().open(monitor, Surface::Tray, Instant::now());
            Ok(())
        }
        Destination::Session => {
            IslandService::write().open(monitor, Surface::Session, Instant::now());
            Ok(())
        }
        Destination::Timer => {
            IslandService::write().open(monitor, Surface::Timer, Instant::now());
            Ok(())
        }
        Destination::Capture => {
            IslandService::write().open(monitor, Surface::Capture, Instant::now());
            Ok(())
        }
        Destination::Wallpapers => {
            IslandService::write().open(monitor, Surface::Wallpapers, Instant::now());
            Ok(())
        }
        Destination::Keys => {
            IslandService::write().open(monitor, Surface::Keys, Instant::now());
            Ok(())
        }
        Destination::Settings(page) => {
            settings::open(Some(page));
            view::collapse(monitor);
            Ok(())
        }
    }
}

// puts `text` on the clipboard off the draw thread, then tells `done` how it went
fn copy(
    text: String,
    done: impl FnOnce(Result<(), String>) + Send + 'static,
) -> Result<(), String> {
    thread::Builder::new()
        .name("launcher-copy".into())
        .spawn(move || {
            let copied = clipboard::put(&text).map_err(|error| error.to_string());

            if let Err(why) = &copied {
                eprintln!("kanade: cannot copy a Launcher answer ({why})");
            }

            done(copied);
        })
        .map(drop)
        .map_err(|error| {
            eprintln!("kanade: cannot copy a Launcher answer ({error})");
            error.to_string()
        })
}

/*
 * runs a Kanade verb of the table (`commands`) through the shell's own socket, as the CLI and a
 * gesture do (ADR 0107, 0112), so its Module gate and parse apply; the Launcher never calls a
 * source itself. Off the view thread, as the shell answers it there. Told to `done` once answered,
 * with the words its row shows if it was not
 */
fn run(
    verb: &'static [&'static str],
    done: impl FnOnce(Result<(), String>) + Send + 'static,
) -> Result<(), String> {
    thread::Builder::new()
        .name("launcher-verb".into())
        .spawn(move || done(commands::dispatch(verb)))
        .map(drop)
        .map_err(|error| {
            eprintln!("launcher: cannot run {verb:?} ({error})");
            commands::not_run(&error.to_string())
        })
}

/*
 * opens `address` in the default browser through `xdg-open`, by argv with no shell (ADR 0130), off
 * the view thread as an opener may live as long as the browser it started. The row says it is not
 * opened if it is not; the query is never logged, here or anywhere (ADR 0019)
 */
fn browse(
    address: String,
    done: impl FnOnce(Result<(), String>) + Send + 'static,
) -> Result<(), String> {
    thread::Builder::new()
        .name("launcher-browse".into())
        .spawn(move || {
            let browsed = web::browse(&address);

            if let Err(why) = &browsed {
                eprintln!("launcher: opening the browser: {why}");
            }

            done(browsed)
        })
        .map(drop)
        .map_err(|error| {
            eprintln!("launcher: cannot open the browser ({error})");
            error.to_string()
        })
}

/*
 * a verb that would catch the island in what it takes (`commands::clears_island`): the island
 * collapses, and the verb runs once it and the compositor's frame after it are gone, as the
 * Capture Surface does. The row is gone by then, so a failure shows as a failed Activity, as a
 * gesture's does
 */
fn run_cleared(monitor: &str, verb: &'static [&'static str]) {
    view::collapse(monitor);

    let spawned = thread::Builder::new()
        .name("launcher-verb".into())
        .spawn(move || {
            thread::sleep(capture::CLEAR);

            if let Err(why) = commands::dispatch(verb) {
                live::gesture(commands::title(verb), &why);
            }
        });

    if let Err(error) = spawned {
        eprintln!("launcher: cannot run {verb:?} ({error})");
    }
}

// puts the history's entry back on the clipboard; it is gone if the history dropped it since the row was drawn
fn paste(id: u64, done: impl FnOnce(Result<(), String>) + Send + 'static) -> Result<(), String> {
    let entry = clipboard::Clipboard::read()
        .entries()
        .iter()
        .find(|entry| entry.id == id)
        .cloned()
        .ok_or_else(|| String::from("The entry is gone"))?;

    clipboard::restore(&entry, move |put| {
        done(if put {
            Ok(())
        } else {
            Err(String::from("The clipboard did not take it"))
        })
    });

    Ok(())
}

/*
 * how pressing `action` in `visit` went: closes the island, or says it failed. Nothing once that
 * visit is over, so it never closes another
 */
fn pressed(monitor: &str, visit: u64, action: Action, done: Result<(), String>) {
    match done {
        Ok(()) => IslandService::write().finish(monitor, visit, Surface::Launcher, Instant::now()),

        // checked and written under one guard, as a later visit may be writing its own Search
        Err(why) => Search::write().press_failed(visit, action, why),
    }
}

// down scrolls further down the list, or the actions' rows while they show
fn wheel(lines: f32) {
    let (visit, rec) = {
        let island = IslandService::read();

        (island.visit(), recorded(&island))
    };

    let search = Search::read().of(visit);

    if search.panel.is_some() {
        store(search.scrolled_offers(lines));
        return;
    }

    let apps = Apps::read();
    let found = found(visit, apps.list(), &search.query, rec, apps.found());
    let layout = found.layout();

    store(search.bounded(&layout).wheel(lines, &layout));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{launch, launch::Launch};
    use kanade_runtime::Key;
    use std::path::Path;

    // a list of one answer, as the search tests step over
    fn one() -> Layout {
        Layout::plain(1)
    }

    // an app that starts `command`, as the desktop scan reads one
    fn app(id: &str) -> App {
        let id = format!("{id}.desktop");

        let launch = Launch::of(&launch::Fields {
            id: &id,
            name: id.as_str(),
            icon: None,
            file: Path::new(""),
            exec: Some(id.as_str()),
            path: None,
            terminal: false,
            dbus_activatable: false,
        })
        .expect("an Exec starts");

        App {
            name: id.clone(),
            id,
            description: None,
            icon: None,
            icon_file: None,
            launch,
            actions: Vec::new(),
        }
    }

    /*
     * the ranking is kept between frames, so what it answers has to follow its inputs: the same
     * inputs answer the same and copy nothing, and an app that arrives or leaves ranks again
     *
     * Its inputs are read from Services the process shares, and the window titles among them are
     * written by a test in this module's own tree: the turn keeps that write out of these calls,
     * which read the titles as equal between them
     */
    #[test]
    fn the_ranking_follows_its_inputs() {
        let _turn = windows::TITLES_TURN.locked();

        let (visit, rec) = (7, Rec::Idle);
        let one_app = [app("alpha")];

        let first = found(visit, &one_app, "al", rec, true);
        let again = found(visit, &one_app, "al", rec, true);

        assert_eq!(first, again);
        assert!(Arc::ptr_eq(&first, &again), "the frame after ranked again");

        // a typed query is another ranking, and the one after it is kept in turn
        let typed = found(visit, &one_app, "alpha", rec, true);
        assert!(!Arc::ptr_eq(&first, &typed));
        assert!(Arc::ptr_eq(
            &typed,
            &found(visit, &one_app, "alpha", rec, true)
        ));

        // an app that arrives is ranked, and answers beside the one before it
        let two = [app("alpha"), app("alpine")];
        let more = found(visit, &two, "al", rec, true);

        assert!(more.answers.len() > first.answers.len(), "{more:?}");

        // and one that leaves is ranked again, so its answer goes with it
        let fewer = found(visit, &one_app, "al", rec, true);

        assert_eq!(first, fewer);
        assert!(!Arc::ptr_eq(&first, &fewer), "a leaving app was not ranked");

        // a watcher's write drops what was ranked even when the inputs look the same, which is
        // what a Service written with what the ranking already read needs
        forget();
        let after = found(visit, &one_app, "al", rec, true);

        assert_eq!(first, after);
        assert!(
            !Arc::ptr_eq(&first, &after),
            "a dropped ranking was not ranked"
        );
    }

    /*
     * ADR 0127 puts the query that started an answer ahead of the match score inside a Fit: the
     * score reads the title while `fit` may read an alias, so a class is observed intent where an
     * in-tier score is positional noise. Fit stays outer, and ADR 0126's score still orders what no
     * class lifts
     */
    #[test]
    fn a_learned_query_leads_the_match_score_within_a_fit() {
        let now = crate::clock::unix_seconds();
        let apps = [app("Firefox"), app("Fable X")];
        let titles = |answers: &[Answer]| {
            answers
                .iter()
                .map(|answer| answer.title.clone())
                .collect::<Vec<_>>()
        };

        // both fit `fx` alike, and `Fable X` answers it better by the score alone
        let mut ranked = provider::ranked(&[&AppsProvider(&apps, false)], "fx");

        assert!(ranked.iter().all(|answer| answer.fit == Fit::Terms));
        assert_eq!(titles(&ranked), ["Fable X.desktop", "Firefox.desktop"]);

        // `Firefox` was started by `fx`, so it leads the better score, as the ranking orders it
        let mut usage = sources::recents::Usage::default();
        usage.start(&sources::recents::app("Firefox.desktop"), "fx", now);
        learned_first(&mut ranked, "fx", now, &usage);

        assert_eq!(titles(&ranked), ["Firefox.desktop", "Fable X.desktop"]);

        // the same class never lifts an answer over a better Fit
        usage.start(&sources::recents::app("Fable X.desktop"), "fx", now);
        let nearer = [app("fx"), app("Fable X")];
        let mut ranked = provider::ranked(&[&AppsProvider(&nearer, false)], "fx");
        learned_first(&mut ranked, "fx", now, &usage);

        assert_eq!(titles(&ranked), ["fx.desktop", "Fable X.desktop"]);
    }

    #[test]
    fn search_typing_and_caret_navigation() {
        let mut search = Search::default();

        for c in "hello".chars() {
            let (next, intent) = search.step(Key::Character(c), &one()).unwrap();
            assert_eq!(intent, Intent::Idle);
            search = next;
        }
        assert_eq!(search.query, "hello");
        assert_eq!(search.caret, 5);

        // Left arrow moves caret backwards
        let (next, _) = search.step(Key::Left, &one()).unwrap();
        search = next;
        assert_eq!(search.caret, 4);

        let (next, _) = search.step(Key::Left, &one()).unwrap();
        search = next;
        assert_eq!(search.caret, 3);

        // Home moves to start
        let (next, _) = search.step(Key::Home, &one()).unwrap();
        search = next;
        assert_eq!(search.caret, 0);

        // Left at 0 stays at 0
        let (next, _) = search.step(Key::Left, &one()).unwrap();
        search = next;
        assert_eq!(search.caret, 0);

        // Right moves forward
        let (next, _) = search.step(Key::Right, &one()).unwrap();
        search = next;
        assert_eq!(search.caret, 1);

        // End moves to query end
        let (next, _) = search.step(Key::End, &one()).unwrap();
        search = next;
        assert_eq!(search.caret, 5);

        // Right at query end stays at end
        let (next, _) = search.step(Key::Right, &one()).unwrap();
        search = next;
        assert_eq!(search.caret, 5);
    }

    #[test]
    fn search_insert_at_caret() {
        let mut search = Search::default();
        for c in "hllo".chars() {
            search = search.step(Key::Character(c), &one()).unwrap().0;
        }
        // Move caret to between 'h' and 'l'
        search = search.step(Key::Left, &one()).unwrap().0;
        search = search.step(Key::Left, &one()).unwrap().0;
        search = search.step(Key::Left, &one()).unwrap().0;
        assert_eq!(search.caret, 1);

        // Insert 'e'
        search = search.step(Key::Character('e'), &one()).unwrap().0;
        assert_eq!(search.query, "hello");
        assert_eq!(search.caret, 2);
    }

    #[test]
    fn search_backspace_and_delete_at_caret() {
        let mut search = Search::default();
        for c in "world".chars() {
            search = search.step(Key::Character(c), &one()).unwrap().0;
        }

        // Backspace at end
        search = search.step(Key::Backspace, &one()).unwrap().0;
        assert_eq!(search.query, "worl");
        assert_eq!(search.caret, 4);

        // Move to position 2 (after "wo")
        search = search.step(Key::Left, &one()).unwrap().0;
        search = search.step(Key::Left, &one()).unwrap().0;
        assert_eq!(search.caret, 2);

        // Delete deletes character after caret ('r')
        search = search.step(Key::Delete, &one()).unwrap().0;
        assert_eq!(search.query, "wol");
        assert_eq!(search.caret, 2);

        // Backspace deletes character before caret ('o')
        search = search.step(Key::Backspace, &one()).unwrap().0;
        assert_eq!(search.query, "wl");
        assert_eq!(search.caret, 1);

        // Home then Backspace does nothing
        search = search.step(Key::Home, &one()).unwrap().0;
        assert_eq!(search.caret, 0);
        search = search.step(Key::Backspace, &one()).unwrap().0;
        assert_eq!(search.query, "wl");
        assert_eq!(search.caret, 0);

        // End then Delete does nothing
        search = search.step(Key::End, &one()).unwrap().0;
        assert_eq!(search.caret, 2);
        search = search.step(Key::Delete, &one()).unwrap().0;
        assert_eq!(search.query, "wl");
        assert_eq!(search.caret, 2);
    }

    #[test]
    fn search_multibyte_utf8_caret() {
        let mut search = Search::default();
        for c in "café".chars() {
            search = search.step(Key::Character(c), &one()).unwrap().0;
        }
        assert_eq!(search.query, "café");
        assert_eq!(search.caret, 5); // 'é' is 2 bytes

        // Left moves across 'é' (2 bytes)
        search = search.step(Key::Left, &one()).unwrap().0;
        assert_eq!(search.caret, 3);

        // Backspace deletes 'f'
        search = search.step(Key::Backspace, &one()).unwrap().0;
        assert_eq!(search.query, "caé");
        assert_eq!(search.caret, 2);

        // Delete deletes 'é'
        search = search.step(Key::Delete, &one()).unwrap().0;
        assert_eq!(search.query, "ca");
        assert_eq!(search.caret, 2);
    }

    #[test]
    fn search_select_all_and_replacement() {
        let mut search = Search::default();
        for c in "abc".chars() {
            search = search.step(Key::Character(c), &one()).unwrap().0;
        }
        assert_eq!(search.query, "abc");

        // Select all
        search = search.step(Key::SelectAll, &one()).unwrap().0;
        assert!(search.select_all);

        // Typing replacement character replaces entire query
        search = search.step(Key::Character('z'), &one()).unwrap().0;
        assert_eq!(search.query, "z");
        assert_eq!(search.caret, 1);
        assert!(!search.select_all);

        // Select all then Backspace clears
        search = search.step(Key::SelectAll, &one()).unwrap().0;
        search = search.step(Key::Backspace, &one()).unwrap().0;
        assert_eq!(search.query, "");
        assert_eq!(search.caret, 0);
        assert!(!search.select_all);

        // Select all then Delete clears
        for c in "test".chars() {
            search = search.step(Key::Character(c), &one()).unwrap().0;
        }
        search = search.step(Key::SelectAll, &one()).unwrap().0;
        search = search.step(Key::Delete, &one()).unwrap().0;
        assert_eq!(search.query, "");
        assert_eq!(search.caret, 0);
        assert!(!search.select_all);

        // Select all then Left arrow cancels selection, caret at 0
        for c in "word".chars() {
            search = search.step(Key::Character(c), &one()).unwrap().0;
        }
        search = search.step(Key::SelectAll, &one()).unwrap().0;
        search = search.step(Key::Left, &one()).unwrap().0;
        assert!(!search.select_all);
        assert_eq!(search.caret, 0);

        // Select all then Right arrow cancels selection, caret at end
        search = search.step(Key::SelectAll, &one()).unwrap().0;
        search = search.step(Key::Right, &one()).unwrap().0;
        assert!(!search.select_all);
        assert_eq!(search.caret, 4);
    }

    #[test]
    fn late_press_failure_only_changes_the_matching_waiting_action() {
        let mut search = Search {
            visit: 42,
            pressing: Pressing::Waiting(Action::Copy(String::from("current"))),
            ..Search::default()
        };
        let pending = Action::Copy(String::from("current"));
        let late = Action::Copy(String::from("old"));

        search.press_failed(42, late, String::from("late error"));
        assert_eq!(search.pressing, Pressing::Waiting(pending.clone()));

        search.press_failed(42, pending.clone(), String::from("current error"));
        assert_eq!(
            search.pressing,
            Pressing::Failed(pending, String::from("current error"))
        );
    }

    #[test]
    fn launch_failure_formats_as_not_launched() {
        let mut search = Search {
            visit: 42,
            ..Search::default()
        };
        let action = Action::Launch(
            sources::launch::Launch::of(&sources::launch::Fields {
                id: "bad.desktop",
                name: "Bad",
                icon: None,
                file: std::path::Path::new("/usr/share/applications/bad.desktop"),
                exec: Some("bad-cmd"),
                path: Some("/nonexistent/directory/bad"),
                terminal: false,
                dbus_activatable: false,
            })
            .unwrap(),
            String::from("bad.desktop"),
        );

        assert!(search.press_waiting(42, action.clone()));
        search.press_failed(
            42,
            action.clone(),
            String::from("its Path /nonexistent/directory/bad is no directory"),
        );

        assert_eq!(
            search.failed(&action),
            Some(String::from(
                "Not launched: its Path /nonexistent/directory/bad is no directory"
            ))
        );
    }

    fn answer(title: &str) -> Answer {
        Answer {
            fit: Fit::Prefix,
            title: title.to_owned(),
            detail: String::new(),
            mark: Mark::Tile(title.to_owned()),
            action: Action::Copy(title.to_owned()),
        }
    }

    // the empty query lists the four sections in the plan's order, a heading over each, and a
    // section that found nothing shows none
    #[test]
    fn sections_list_in_order_under_a_heading_each() {
        let found = gathered([
            ("Recents", vec![answer("Files")]),
            ("Commands", vec![answer("Toggle Caffeine")]),
            ("Windows", vec![answer("kanade")]),
            ("Apps", vec![answer("Blender"), answer("Zed")]),
        ]);

        let titles: Vec<&str> = found.sections.iter().map(|section| section.title).collect();
        assert_eq!(titles, ["Recents", "Commands", "Windows", "Apps"]);
        assert_eq!(found.answers.len(), 5);

        let headings: Vec<&str> = found
            .layout()
            .rows
            .iter()
            .filter_map(|(line, ..)| match line {
                Line::Heading(title) => Some(*title),
                Line::Answer(_) => None,
            })
            .collect();
        assert_eq!(headings, ["Recents", "Commands", "Windows", "Apps"]);

        // a section with nothing to list leaves no heading behind
        let empty = gathered([
            ("Recents", Vec::new()),
            ("Commands", vec![answer("Toggle Caffeine")]),
            ("Windows", Vec::new()),
            ("Apps", vec![answer("Blender")]),
        ]);

        let titles: Vec<&str> = empty.sections.iter().map(|section| section.title).collect();
        assert_eq!(titles, ["Commands", "Apps"]);
        assert_eq!(empty.sections[1].at, 1);
    }

    // a heading takes its own place above the answers it names, so the rows below keep their place
    // and the answers' rows are the ones the selection moves over
    #[test]
    fn a_heading_offsets_the_rows_below_it() {
        let found = gathered([
            ("Recents", vec![answer("Files")]),
            ("Commands", vec![answer("Toggle Caffeine")]),
            ("Windows", Vec::new()),
            ("Apps", vec![answer("Blender")]),
        ]);
        let layout = found.layout();

        // heading, Recents' answer, heading, Commands' answer, heading, Apps' answer
        assert_eq!(layout.rows.len(), 6);
        assert_eq!(layout.answers.len(), 3);
        assert!(layout.answers[0] < layout.answers[1]);
        assert!(layout.answers[1] < layout.answers[2]);
        assert_eq!(
            layout.answers[1] - layout.answers[0],
            ROW + ROW_GAP + HEADER + ROW_GAP
        );
        assert_eq!(
            layout.answers[0],
            layout.rows[0].1 + HEADER + ROW_GAP,
            "the first answer sits under its heading"
        );

        // a ranked query, with no headings, keeps its rows in single steps
        let ranked = Layout::plain(3);
        assert_eq!(ranked.rows.len(), 3);
        assert_eq!(ranked.answers[1] - ranked.answers[0], ROW + ROW_GAP);
    }

    // scrolling moves the selection over answers only, never onto a heading
    #[test]
    fn the_selection_moves_over_answers_only() {
        let found = gathered([
            ("Recents", vec![answer("Files")]),
            ("Commands", vec![answer("Toggle Caffeine")]),
            ("Windows", Vec::new()),
            ("Apps", vec![answer("Blender")]),
        ]);
        let layout = found.layout();

        let mut search = Search::default();
        for _ in 0..10 {
            search = search.step(Key::Down, &layout).unwrap().0;
        }
        assert_eq!(search.selected, 2, "the last answer, not a heading");

        for _ in 0..10 {
            search = search.step(Key::Up, &layout).unwrap().0;
        }
        assert_eq!(search.selected, 0);
    }

    // a typed query goes back to the ranking, and only then asks the providers for one: the
    // sections' closure panics if it is reached
    #[test]
    fn typing_a_character_lists_the_ranking_again() {
        let sections = || panic!("a typed query must not build the empty query's sections");
        let ranked = || Found::ranked(vec![answer("Files")]);

        assert_eq!(grouped("f", sections, ranked).sections, []);
        assert_eq!(
            grouped("    ", Found::default, ranked).answers,
            Vec::new(),
            "a query of spaces is nothing typed"
        );
    }

    // the web stands in for the ranking only when the ranking found nothing, so it never outranks
    // an answer and a provider never offers it (ADR 0130)
    #[test]
    fn the_web_is_offered_only_when_nothing_answers() {
        let files = vec![answer("Files")];

        assert_eq!(
            offered("nothing matches this", files.clone(), true, || true),
            files
        );

        let web = offered("nothing matches this", Vec::new(), true, || true);
        assert_eq!(web.len(), 2, "the search and the address");
        assert!(
            web.iter()
                .all(|answer| matches!(answer.action, Action::Browse(_)))
        );

        // a query no web row is offered for is still nothing at all
        assert_eq!(offered("x", Vec::new(), true, || true), Vec::new());

        // without the opener there is no row to press, and the state under the field says why
        assert_eq!(
            offered("nothing matches this", Vec::new(), true, || false),
            Vec::new()
        );

        // an app may still be the answer until the apps are read, so nothing is sent meanwhile
        assert_eq!(
            offered("nothing matches this", Vec::new(), false, || true),
            Vec::new(),
            "the web waits for the desktop scan"
        );
    }
}
