//! The Launcher Surface: a search over its providers, the apps `sources::apps`
//! lists and their own actions, the open windows (`windows`), the calculator, emoji, where Kanade goes (Wi-Fi, Bluetooth, each Surface, Settings and
//! each setting), with `wallpaper-surface` on, the Wallpapers Surface for anything after `@`, and with `clipboard` on, what was copied after `>`, best answer first.
//! It opens only from IPC or a keybind, so it always holds the keyboard: typing searches, the arrow
//! keys move the selection, Enter presses the selected answer and a click the one clicked. An app
//! starts and the island closes; a value or an emoji is copied, and the island
//! closes once it is, or says it was not; a command (`commands`, Kanade's own verbs) runs through the socket and closes the island once answered, or says `Not run`. It says when the apps are still being found and when
//! nothing matches. `kanade dmenu` opens it on a script's lines alone (`choose`).

use std::thread;
use std::time::Instant;

use kanade_runtime::service::{self, Service};
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Column, Cursor, Image, Key, Padding, Parent, Rectangle, Row, Scroll, Size, Stack,
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

mod apps;
mod calculator;
pub mod choose;
mod commands;
mod destinations;
mod emoji;
mod history;
mod provider;
mod units;
pub mod windows;

use apps::{Actions, Apps as AppsProvider};
use calculator::Calculator;
use choose::{Choices, Choosing};
use commands::{Commands, Rec};
use destinations::{Destination, Destinations};
use emoji::Emoji;
use history::History;
use provider::{Action, Answer, Mark};

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

const ICON: f32 = 28.0;
const ICON_GAP: f32 = 12.0;

// the typed text's size, and the caret after it
const QUERY: f32 = theme::text::TITLE;
const CARET: f32 = 1.5;

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
     * the search after a key and whether it presses the selected answer, none when the key is not
     * for the Launcher; `count` is how many answers the query finds before the key
     */
    fn step(mut self, key: Key, count: usize) -> Option<(Search, bool)> {
        let last = count.saturating_sub(1);

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
                Some((self, false))
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
                Some((self, false))
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
                Some((self, false))
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
                Some((self, false))
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
                Some((self, false))
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
                Some((self, false))
            }
            Key::Home => {
                self.select_all = false;
                self.caret = 0;
                Some((self, false))
            }
            Key::End => {
                self.select_all = false;
                self.caret = self.query.len();
                Some((self, false))
            }
            Key::SelectAll => {
                if !self.query.is_empty() {
                    self.select_all = true;
                    self.caret = self.query.len();
                }
                Some((self, false))
            }
            Key::Enter => Some((self, count > 0)),
            Key::Up => {
                self.select_all = false;
                self.selected = self.selected.saturating_sub(1);
                self.offset = reveal(self.offset.clamp(0.0, most(count)), self.selected);
                Some((self, false))
            }
            Key::Down => {
                self.select_all = false;
                self.selected = (self.selected + 1).min(last);
                self.offset = reveal(self.offset.clamp(0.0, most(count)), self.selected);
                Some((self, false))
            }
            _ => None,
        }
    }

    // scrolled by the wheel, the selection kept on a row that shows whole
    fn wheel(mut self, lines: f32, count: usize) -> Search {
        let most = most(count);

        self.offset = (self.offset.clamp(0.0, most) + lines * WHEEL).clamp(0.0, most);

        let (first, last) = whole(self.offset, count);
        self.selected = self.selected.clamp(first, last);

        self
    }

    // what the view draws; a query no longer finding as many leaves the selection on the last one
    fn bounded(mut self, count: usize) -> Search {
        self.selected = self.selected.min(count.saturating_sub(1));
        self.offset = self.offset.clamp(0.0, most(count));
        self.caret = self.caret.min(self.query.len());
        while !self.query.is_char_boundary(self.caret) {
            self.caret = self.caret.saturating_sub(1);
        }
        if self.query.is_empty() {
            self.select_all = false;
        }

        self
    }
}

// how far `count` rows can scroll: none while they fit
fn most(count: usize) -> f32 {
    (content(count) - room()).max(0.0)
}

fn content(count: usize) -> f32 {
    count as f32 * ROW + count.saturating_sub(1) as f32 * ROW_GAP
}

// how tall `count` answers ask the body to be, or a state for none; the island caps it (ADR 0030)
fn asks(count: usize) -> f32 {
    let list = if count == 0 { STATE } else { content(count) };

    2.0 * INSET + FIELD + GAP + list
}

// the list as tall as its rows, scrolling once they reach past the room
fn listed(count: usize) -> f32 {
    content(count).min(room())
}

/*
 * tells the island how tall the Launcher asks to be: a new visit lists every app, the one open what
 * its query found. After each change of the apps or the search, never from a view
 */
pub fn fit() {
    let _ordered = super::fitting();
    let (visit, rec) = {
        let island = IslandService::read();

        (island.visit(), recorded(&island))
    };

    let (fresh, open) = {
        let apps = Apps::read();
        let query = Search::read().of(visit).query;

        // a new visit lists every app
        (
            asks(apps.list().len()),
            asks(found(visit, apps.list(), &query, rec).len()),
        )
    };

    super::ask(Surface::Launcher, fresh, Some((visit, open)));
}

// at start: the body follows the apps and the search from now on
pub fn start() {
    service::watch::<Connectivity>(follow_radios);
    service::watch::<Adapter>(follow_radios);
    follow_radios();
    service::watch::<Radios>(fit);
    service::watch::<Apps>(fit);
    service::watch::<Search>(fit);
    service::watch::<Choosing>(fit);
    service::watch::<windows::Held>(fit);
    service::watch::<IslandService>(choose::follow);
    fit();
}

fn top(row: usize) -> f32 {
    row as f32 * (ROW + ROW_GAP)
}

// the least scroll from `offset` that shows `row` whole
fn reveal(offset: f32, row: usize) -> f32 {
    let top = top(row);

    offset.min(top).max(top + ROW - room())
}

/*
 * the rows any part of shows at `offset`, as a range; only these are built, so a morph doesn't
 * lay out and draw every answer each frame
 */
fn shown(offset: f32, count: usize) -> (usize, usize) {
    let step = ROW + ROW_GAP;
    let first = (offset / step).floor() as usize;
    let end = ((offset + room()) / step).ceil() as usize;

    (first.min(count), end.min(count))
}

// the first and last rows that show whole at `offset`
fn whole(offset: f32, count: usize) -> (usize, usize) {
    let step = ROW + ROW_GAP;
    let first = (offset / step).ceil() as usize;
    let last = ((offset + room() + ROW_GAP) / step).floor() as usize;

    (
        first.min(count.saturating_sub(1)),
        last.saturating_sub(1).min(count.saturating_sub(1)),
    )
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

// what the providers find for the query in `visit`, best first; a dmenu's lines alone in its visit
fn found(visit: u64, apps: &[App], query: &str, recording: Rec) -> Vec<Answer> {
    if let Some((_, choices)) = choose::of(visit) {
        return provider::ranked(&[&Choices(&choices)], query);
    }

    // read only for a history search, which also subscribes the view to the clipboard
    let entries = if history(query) {
        clipboard::Clipboard::read().entries().to_vec()
    } else {
        Vec::new()
    };

    let destinations = destinations();
    let titles = windows::titles();

    provider::ranked(
        &[
            &Calculator,
            &AppsProvider(apps, destinations.wallpapers),
            &Actions(apps, config::get().launcher_actions),
            &windows::Titles(&titles),
            &destinations,
            &commands(config::get().launcher_commands, recording),
            &Emoji,
            &History { entries: &entries },
        ],
        query,
    )
}

// the view's own read of the visit and the recording, so this never reads IslandService again
pub fn surface(monitor: &str, visit: u64, recording: Rec) -> Rectangle {
    let apps = Apps::read();
    let search = Search::read().of(visit);
    let found = found(visit, apps.list(), &search.query, recording);
    let search = search.bounded(found.len());
    let choosing = choose::of(visit);

    let query = search.query.trim();
    let emoji = query.starts_with(emoji::PREFIX);
    let history = history(query);

    let list: Box<dyn Widget> = if !found.is_empty() {
        Box::new(list(monitor, &found, &search))
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
        Box::new(
            Row::new(children![
                Rectangle::new()
                    .width(room)
                    .height(QUERY + 4.0)
                    .fill(theme::island().surface_container_high)
                    .radius(radius::HAIRLINE)
                    .align_child(Start, Center)
                    .child(tail(query, room)),
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

// the end of `query` that fits in `width`, an ellipsis before it when the start is cut
fn tail(query: &str, width: f32) -> Text {
    let text = |shown: &str| {
        Text::new(shown)
            .size(QUERY)
            .color(theme::island().on_surface)
            .weight(theme::text::MEDIUM)
    };
    let fits = |text: &Text| matches!(text.width(), Size::Fixed(natural) if natural <= width);

    let whole = text(query);

    if fits(&whole) {
        return whole;
    }

    let letters: Vec<char> = query.chars().collect();

    (1..letters.len())
        .map(|cut| {
            text(&format!(
                "\u{2026}{}",
                letters[cut..].iter().collect::<String>()
            ))
        })
        .find(fits)
        .unwrap_or_else(|| text("\u{2026}"))
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
        return Box::new(Row::new(children![tail(query, room), caret_widget]).align(Center));
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
 * the rows scrolled `offset` down, clipped to the list; a thumb in the inset says where while they
 * do not all fit
 */
fn list(monitor: &str, found: &[Answer], search: &Search) -> Stack {
    let (first, end) = shown(search.offset, found.len());

    let column = Column::new(
        found[first..end]
            .iter()
            .zip(first..)
            .map(|(answer, index)| {
                let selected = index == search.selected;
                let failed = search.failed(&answer.action);

                Box::new(row(
                    monitor,
                    search.visit,
                    answer,
                    selected,
                    failed.as_deref(),
                )) as Box<dyn Widget>
            })
            .collect(),
    )
    .width(width())
    .gap(ROW_GAP);

    let viewport = Rectangle::new()
        .width(width())
        .height(listed(found.len()))
        .clip()
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::List, "results"))
        .on_scroll(|Scroll { y, .. }| wheel(y))
        .child(
            Rectangle::new()
                .width(width())
                .height(content(end - first))
                .align_child(Start, Start)
                .translate(0.0, top(first) - search.offset)
                .child(column),
        );

    let mut layers = children![viewport];

    let most = most(found.len());

    if most > 0.0 {
        let length = (room() * room() / content(found.len())).max(TARGET);
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

    Stack::new(layers)
        .width(width())
        .height(listed(found.len()))
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

    row.identity(&answer.action)
        .semantics(|| {
            Semantics::new(Role::Item, "result")
                .content(answer.title.as_str())
                .selected(selected)
        })
        .on_click(super::on_left(move || {
            press(&monitor, visit, action.clone());
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

    let search = Search::read().of(visit);
    let found = found(visit, Apps::read().list(), &search.query, rec);

    let Some((mut search, presses)) = search.bounded(found.len()).step(key, found.len()) else {
        return false;
    };

    let selected = found
        .get(search.selected)
        .map(|answer| answer.action.clone());

    // a failed press is said until the next key
    if matches!(search.pressing, Pressing::Failed(..)) {
        search.pressing = Pressing::Idle;
    }

    store(search);

    IslandService::write().attend(monitor, Instant::now());

    if presses && let Some(action) = selected {
        press(monitor, visit, action);
    }

    true
}

/*
 * what starts, is copied or set is in the way of nothing, so the island closes; a copy, a wallpaper
 * or a verb once it is done. One of those at a time
 */
fn press(monitor: &str, visit: u64, action: Action) {
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

        sources::recents::used(id);

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

    if let Action::Run(verb) = &action
        && commands::clears_island(verb)
    {
        run_cleared(monitor, verb);
        return;
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

        sources::recents::used(id);

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
        if let Err(why) = open(monitor, destination) {
            Search::write().open_refused(visit, action, why);
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

// down scrolls further down the list
fn wheel(lines: f32) {
    let (visit, rec) = {
        let island = IslandService::read();

        (island.visit(), recorded(&island))
    };

    let search = Search::read().of(visit);
    let count = found(visit, Apps::read().list(), &search.query, rec).len();

    store(search.bounded(count).wheel(lines, count));
}

#[cfg(test)]
mod tests {
    use super::*;
    use kanade_runtime::Key;

    #[test]
    fn search_typing_and_caret_navigation() {
        let mut search = Search::default();

        for c in "hello".chars() {
            let (next, pressed) = search.step(Key::Character(c), 1).unwrap();
            assert!(!pressed);
            search = next;
        }
        assert_eq!(search.query, "hello");
        assert_eq!(search.caret, 5);

        // Left arrow moves caret backwards
        let (next, _) = search.step(Key::Left, 1).unwrap();
        search = next;
        assert_eq!(search.caret, 4);

        let (next, _) = search.step(Key::Left, 1).unwrap();
        search = next;
        assert_eq!(search.caret, 3);

        // Home moves to start
        let (next, _) = search.step(Key::Home, 1).unwrap();
        search = next;
        assert_eq!(search.caret, 0);

        // Left at 0 stays at 0
        let (next, _) = search.step(Key::Left, 1).unwrap();
        search = next;
        assert_eq!(search.caret, 0);

        // Right moves forward
        let (next, _) = search.step(Key::Right, 1).unwrap();
        search = next;
        assert_eq!(search.caret, 1);

        // End moves to query end
        let (next, _) = search.step(Key::End, 1).unwrap();
        search = next;
        assert_eq!(search.caret, 5);

        // Right at query end stays at end
        let (next, _) = search.step(Key::Right, 1).unwrap();
        search = next;
        assert_eq!(search.caret, 5);
    }

    #[test]
    fn search_insert_at_caret() {
        let mut search = Search::default();
        for c in "hllo".chars() {
            search = search.step(Key::Character(c), 1).unwrap().0;
        }
        // Move caret to between 'h' and 'l'
        search = search.step(Key::Left, 1).unwrap().0;
        search = search.step(Key::Left, 1).unwrap().0;
        search = search.step(Key::Left, 1).unwrap().0;
        assert_eq!(search.caret, 1);

        // Insert 'e'
        search = search.step(Key::Character('e'), 1).unwrap().0;
        assert_eq!(search.query, "hello");
        assert_eq!(search.caret, 2);
    }

    #[test]
    fn search_backspace_and_delete_at_caret() {
        let mut search = Search::default();
        for c in "world".chars() {
            search = search.step(Key::Character(c), 1).unwrap().0;
        }

        // Backspace at end
        search = search.step(Key::Backspace, 1).unwrap().0;
        assert_eq!(search.query, "worl");
        assert_eq!(search.caret, 4);

        // Move to position 2 (after "wo")
        search = search.step(Key::Left, 1).unwrap().0;
        search = search.step(Key::Left, 1).unwrap().0;
        assert_eq!(search.caret, 2);

        // Delete deletes character after caret ('r')
        search = search.step(Key::Delete, 1).unwrap().0;
        assert_eq!(search.query, "wol");
        assert_eq!(search.caret, 2);

        // Backspace deletes character before caret ('o')
        search = search.step(Key::Backspace, 1).unwrap().0;
        assert_eq!(search.query, "wl");
        assert_eq!(search.caret, 1);

        // Home then Backspace does nothing
        search = search.step(Key::Home, 1).unwrap().0;
        assert_eq!(search.caret, 0);
        search = search.step(Key::Backspace, 1).unwrap().0;
        assert_eq!(search.query, "wl");
        assert_eq!(search.caret, 0);

        // End then Delete does nothing
        search = search.step(Key::End, 1).unwrap().0;
        assert_eq!(search.caret, 2);
        search = search.step(Key::Delete, 1).unwrap().0;
        assert_eq!(search.query, "wl");
        assert_eq!(search.caret, 2);
    }

    #[test]
    fn search_multibyte_utf8_caret() {
        let mut search = Search::default();
        for c in "café".chars() {
            search = search.step(Key::Character(c), 1).unwrap().0;
        }
        assert_eq!(search.query, "café");
        assert_eq!(search.caret, 5); // 'é' is 2 bytes

        // Left moves across 'é' (2 bytes)
        search = search.step(Key::Left, 1).unwrap().0;
        assert_eq!(search.caret, 3);

        // Backspace deletes 'f'
        search = search.step(Key::Backspace, 1).unwrap().0;
        assert_eq!(search.query, "caé");
        assert_eq!(search.caret, 2);

        // Delete deletes 'é'
        search = search.step(Key::Delete, 1).unwrap().0;
        assert_eq!(search.query, "ca");
        assert_eq!(search.caret, 2);
    }

    #[test]
    fn search_select_all_and_replacement() {
        let mut search = Search::default();
        for c in "abc".chars() {
            search = search.step(Key::Character(c), 1).unwrap().0;
        }
        assert_eq!(search.query, "abc");

        // Select all
        search = search.step(Key::SelectAll, 1).unwrap().0;
        assert!(search.select_all);

        // Typing replacement character replaces entire query
        search = search.step(Key::Character('z'), 1).unwrap().0;
        assert_eq!(search.query, "z");
        assert_eq!(search.caret, 1);
        assert!(!search.select_all);

        // Select all then Backspace clears
        search = search.step(Key::SelectAll, 1).unwrap().0;
        search = search.step(Key::Backspace, 1).unwrap().0;
        assert_eq!(search.query, "");
        assert_eq!(search.caret, 0);
        assert!(!search.select_all);

        // Select all then Delete clears
        for c in "test".chars() {
            search = search.step(Key::Character(c), 1).unwrap().0;
        }
        search = search.step(Key::SelectAll, 1).unwrap().0;
        search = search.step(Key::Delete, 1).unwrap().0;
        assert_eq!(search.query, "");
        assert_eq!(search.caret, 0);
        assert!(!search.select_all);

        // Select all then Left arrow cancels selection, caret at 0
        for c in "word".chars() {
            search = search.step(Key::Character(c), 1).unwrap().0;
        }
        search = search.step(Key::SelectAll, 1).unwrap().0;
        search = search.step(Key::Left, 1).unwrap().0;
        assert!(!search.select_all);
        assert_eq!(search.caret, 0);

        // Select all then Right arrow cancels selection, caret at end
        search = search.step(Key::SelectAll, 1).unwrap().0;
        search = search.step(Key::Right, 1).unwrap().0;
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
}
