//! The Launcher Surface: a search over its providers, the apps `sources::apps`
//! lists and their own actions, the open windows (`windows`), the calculator, emoji, where Kanade goes (Wi-Fi, Bluetooth, each Surface, Settings and
//! each setting), with `wallpaper-surface` on, the Wallpapers Surface for anything after `@`, and with `clipboard` on, what was copied after `>`, best answer first.
//! It opens only from IPC or a keybind, so it always holds the keyboard: typing searches, the arrow
//! keys move the selection, Enter presses the selected answer and a click the one clicked. An app
//! starts and the island closes; a value or an emoji is copied, and the island
//! closes once it is, or says it was not. It says when the apps are still being found and when
//! nothing matches. `kanade dmenu` opens it on a script's lines alone (`choose`).

use std::thread;
use std::time::Instant;

use kanade_runtime::service::{self, Service};
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
use crate::sources::{self, clipboard};
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, radius};
use crate::{modules, view};

use super::visit::Visited;
use super::{Outline, controls, store};

mod apps;
mod calculator;
pub mod choose;
mod destinations;
mod emoji;
mod history;
mod provider;
mod units;
pub mod windows;

use apps::{Actions, Apps as AppsProvider};
use calculator::Calculator;
use choose::{Choices, Choosing};
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
            Action::Launch(..) | Action::Perform(..) | Action::Focus(_) | Action::Choose(_) => None,
        }
    }

    /*
     * pressing `action` in `visit` failed, for `why`; nothing once that visit or that press is
     * over, so a stale worker never touches a later visit's Search
     */
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

        let selected = match key {
            Key::Character(letter) => return Some((self.typed(Some(letter)), false)),
            Key::Space => return Some((self.typed(Some(' ')), false)),
            Key::Backspace => return Some((self.typed(None), false)),
            Key::Enter => return Some((self, count > 0)),
            Key::Up => self.selected.saturating_sub(1),
            Key::Down => (self.selected + 1).min(last),
            Key::Home => 0,
            Key::End => last,
            _ => return None,
        };

        self.selected = selected;
        self.offset = reveal(self.offset.clamp(0.0, most(count)), selected);

        Some((self, false))
    }

    // a letter typed, or the last one erased; a new query finds new answers, so it starts at the top
    fn typed(mut self, letter: Option<char>) -> Search {
        match letter {
            Some(letter) => self.query.push(letter),
            None => {
                self.query.pop();
            }
        }

        self.selected = 0;
        self.offset = 0.0;

        self
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
    let visit = IslandService::read().visit();

    let (fresh, open) = {
        let apps = Apps::read();
        let query = Search::read().of(visit).query;

        // a new visit lists every app
        (
            asks(apps.list().len()),
            asks(found(visit, apps.list(), &query).len()),
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

// what the providers find for the query in `visit`, best first; a dmenu's lines alone in its visit
fn found(visit: u64, apps: &[App], query: &str) -> Vec<Answer> {
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
            &Emoji,
            &History { entries: &entries },
        ],
        query,
    )
}

// the view's own read of the visit, so this never reads IslandService again
pub fn surface(monitor: &str, visit: u64) -> Rectangle {
    let apps = Apps::read();
    let search = Search::read().of(visit);
    let found = found(visit, apps.list(), &search.query);
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
 * a search icon, what is typed with the caret after it, or what to type faded; a query wider than
 * the field shows its end, where the typing is
 */
fn field(query: &str, placeholder: &str) -> Rectangle {
    let room = width() - 2.0 * FIELD_INSET - TARGET - ICON_GAP - CARET;

    let caret = Rectangle::new()
        .width(CARET)
        .height(QUERY + 4.0)
        .fill(theme::island().on_surface);

    let typed: Box<dyn Widget> = if query.is_empty() {
        Box::new(
            Row::new(children![
                caret,
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
    } else {
        Box::new(Row::new(children![tail(query, room), caret]).align(Center))
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
        .child(
            Row::new(vec![
                Box::new(Icon::Search.on(TARGET, theme::island().on_surface_variant))
                    as Box<dyn Widget>,
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
    let visit = {
        let island = IslandService::read();

        if island.presentation(monitor) != Presentation::Expanded(Surface::Launcher) {
            return false;
        }

        island.visit()
    };

    let search = Search::read().of(visit);
    let found = found(visit, Apps::read().list(), &search.query);

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
 * what starts, is copied or set is in the way of nothing, so the island closes; a copy or a
 * wallpaper once it is done. One of those at a time
 */
fn press(monitor: &str, visit: u64, action: Action) {
    if let Action::Choose(index) = action {
        choose::chosen(visit, index);
        view::collapse(monitor);
        return;
    }

    if let Action::Launch(launch, id) = &action {
        let launch = launch.clone();

        sources::recents::used(id);

        // off the view thread, as niri's or the bus's answer may take its patience
        thread::spawn(move || {
            if let Err(error) = launch.run() {
                eprintln!("launcher: {launch:?}: {error}");
            }
        });

        view::collapse(monitor);
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

    if let Action::Perform(launch, id, action) = &action {
        let (launch, action) = (launch.clone(), action.clone());

        sources::recents::used(id);

        // off the view thread, as the bus's or niri's answer may take its patience
        thread::spawn(move || {
            if let Err(error) = launch.run_action(&action) {
                eprintln!("launcher: {launch:?} {action}: {error}");
            }
        });

        view::collapse(monitor);
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
    let visit = IslandService::read().visit();

    let search = Search::read().of(visit);
    let count = found(visit, Apps::read().list(), &search.query).len();

    store(search.bounded(count).wheel(lines, count));
}
