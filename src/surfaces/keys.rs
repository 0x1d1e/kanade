//! The Keys Surface: niri's keybinds, as its config binds them, then the keys Kanade's own Surfaces
//! take, searched as it is typed. It opens only from IPC, a keybind or the Launcher, so it always
//! holds the keyboard: typing searches, Up, Down, Tab, Home and End move a ring through the list,
//! which scrolls to keep it in view.
//!
//! The binds come grouped by what they act on: Kanade, Windows, Workspaces, then Other (see
//! `sources::binds`). A config niri's binds cannot be read from says which file and why above the
//! rest, which still lists Kanade's own keys.

use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Column, End, Key, Padding, Parent, Rectangle, Row, Scroll, Size, Stack, Start, Text,
    Widget, children,
};
use kanade_runtime::{MutexExt, service::Service};

use crate::icon::Icon;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::sources::binds::{self, Bind, Group, Unread};
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, radius};
use crate::view;

use super::search::{self, FIELD};
use super::visit::Visited;
use super::{Outline, store};

// the content's width, which every row fills, as wide as the largest body (`island.width`)
fn width() -> f32 {
    view::largest().width - 2.0 * INSET
}

const FOOTER: f32 = 28.0;
const GAP: f32 = 12.0;

// what the list scrolls in, as tall as the largest body (`island.height`) leaves
fn room() -> f32 {
    view::largest().height - 2.0 * INSET - FIELD - FOOTER - 2.0 * GAP
}

// a bind's row, a group's title above its rows, and the gap between any two
const ROW: f32 = 34.0;
const TITLE: f32 = 28.0;
const ITEM_GAP: f32 = 2.0;
const ROW_INSET: f32 = 10.0;

// a row of Kanade's keys: where, in a column this wide, then what the keys do, wrapped
const PLACE: f32 = 120.0;
const HELP_INSET: f32 = 8.0;

// what says the config could not be read
const NOTICE: f32 = 52.0;

// a keycap: its height, and the room either side of its label
const CAP: f32 = 22.0;
const CAP_INSET: f32 = 7.0;
const CAP_GAP: f32 = 4.0;

// pixels per wheel line
const WHEEL: f32 = 40.0;

/*
 * the query, the ring and the scroll for one visit of the Surface (`IslandService::visit`), so every
 * opening starts at the top. Written by input only, never by the view
 */
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lookup {
    visit: u64,
    query: String,

    // in the rows the ring takes: the binds and Kanade's keys the query finds
    selected: usize,

    // how far the list scrolled, in pixels
    offset: f32,
}

impl Service for Lookup {
    fn new() -> Self {
        Lookup::default()
    }

    fn listen() {}
}

impl Visited for Lookup {
    fn visit(&self) -> u64 {
        self.visit
    }

    fn begin(visit: u64) -> Self {
        Lookup {
            visit,
            ..Lookup::default()
        }
    }
}

// one line of the list
#[derive(Debug, Clone, PartialEq)]
enum Item {
    // a group's title
    Title(&'static str),
    Bind(Bind),

    // where in Kanade, and what its keys do there
    Help(String, String),

    // why niri's binds are not listed
    Notice(String, String),
}

impl Item {
    // whether the ring stops on it
    fn ringed(&self) -> bool {
        matches!(self, Item::Bind(_) | Item::Help(..))
    }
}

// the list laid out: each item with its top and height, and which the ring stops on, in order
#[derive(Debug, Default)]
struct Laid {
    items: Vec<(Item, f32, f32)>,
    rings: Vec<usize>,
    content: f32,

    // the binds found, and in all
    found: usize,
    binds: usize,
}

impl Laid {
    fn most(&self) -> f32 {
        (self.content - room()).max(0.0)
    }

    // the top and height of ring stop `ring`
    fn place(&self, ring: usize) -> Option<(f32, f32)> {
        let &item = self.rings.get(ring)?;
        let (_, top, height) = &self.items[item];

        Some((*top, *height))
    }

    // the least scroll from `offset` that shows ring stop `ring` whole, its top when it is taller
    fn reveal(&self, offset: f32, ring: usize) -> f32 {
        let Some((top, height)) = self.place(ring) else {
            return offset;
        };

        offset
            .max(top + height - room())
            .min(top)
            .clamp(0.0, self.most())
    }

    // the ring stops that show whole at `offset`, as a range
    fn whole(&self, offset: f32) -> Option<(usize, usize)> {
        let shown = |ring: &usize| {
            self.place(*ring)
                .is_some_and(|(top, height)| top >= offset && top + height <= offset + room())
        };

        let first = (0..self.rings.len()).find(shown)?;
        let last = (0..self.rings.len()).rev().find(shown)?;

        Some((first, last))
    }
}

/*
 * the list for `query`: each group of binds found under its title, then Kanade's own keys; a query
 * of only spaces finds them all
 */
fn lay(read: &Result<Vec<Bind>, Unread>, query: &str) -> Laid {
    let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let finds = |text: &str| {
        let text = text.to_lowercase();

        terms.iter().all(|term| text.contains(term.as_str()))
    };

    let mut items = Vec::new();
    let mut found = 0;
    let mut all = 0;

    match read {
        Ok(binds) => {
            all = binds.len();

            for group in Group::ALL {
                let mut listed = binds
                    .iter()
                    .filter(|bind| bind.group == group)
                    .filter(|bind| {
                        finds(&format!(
                            "{} {} {}",
                            group.title(),
                            bind.title,
                            bind.keys.join(" ")
                        ))
                    })
                    .cloned()
                    .map(Item::Bind)
                    .peekable();

                if listed.peek().is_some() {
                    items.push(Item::Title(group.title()));

                    for item in listed {
                        found += 1;
                        items.push(item);
                    }
                }
            }

            if binds.is_empty() && terms.is_empty() {
                items.push(Item::Notice(
                    String::from("niri binds no keys"),
                    binds::config()
                        .map(|path| format!("{} has no binds", path.display()))
                        .unwrap_or_default(),
                ));
            }
        }
        Err(unread) if terms.is_empty() => items.push(Item::Notice(
            String::from("niri's keybinds could not be read"),
            format!("{}: {}", unread.file.display(), unread.why),
        )),
        Err(_) => {}
    }

    let help: Vec<Item> = binds::surface_keys()
        .into_iter()
        .filter(|(place, keys)| finds(&format!("kanade {place} {keys}")))
        .map(|(place, keys)| Item::Help(place, keys))
        .collect();

    if !help.is_empty() {
        items.push(Item::Title("In Kanade's Surfaces"));
        items.extend(help);
    }

    let mut laid = Laid {
        found,
        binds: all,
        ..Laid::default()
    };
    let mut top = 0.0;

    for (at, item) in items.into_iter().enumerate() {
        let height = match &item {
            Item::Title(_) => TITLE,
            Item::Bind(_) => ROW,
            Item::Help(place, keys) => {
                let text = help_text(keys).height_in(help_width());
                let place = place_text(place).height_in(PLACE);

                (text.max(place) + 2.0 * HELP_INSET).max(ROW)
            }
            Item::Notice(..) => NOTICE,
        };

        if item.ringed() {
            laid.rings.push(at);
        }

        laid.items.push((item, top, height));
        top += height + ITEM_GAP;
    }

    laid.content = (top - ITEM_GAP).max(0.0);

    laid
}

// what a search last laid out, so the view, which runs every frame of a morph, lays it out again
// only when the query, the binds or the width changed
#[derive(Default)]
struct Memo {
    query: String,
    read: Option<Arc<Result<Vec<Bind>, Unread>>>,
    width: f32,
    laid: Arc<Laid>,
}

static MEMO: LazyLock<Mutex<Memo>> = LazyLock::new(Mutex::default);

fn laid(query: &str) -> Arc<Laid> {
    let read = binds::binds();
    let mut memo = MEMO.locked();

    let same = memo.query == query
        && memo.width == width()
        && memo
            .read
            .as_ref()
            .is_some_and(|was| Arc::ptr_eq(was, &read));

    if !same {
        memo.laid = Arc::new(lay(&read, query));
        memo.query = query.to_owned();
        memo.width = width();
        memo.read = Some(read);
    }

    memo.laid.clone()
}

impl Lookup {
    // what the view draws: the selection on a ring stop found, the scroll within the list
    fn bounded(mut self, laid: &Laid) -> Lookup {
        self.selected = self.selected.min(laid.rings.len().saturating_sub(1));
        self.offset = self.offset.clamp(0.0, laid.most());

        self
    }

    /*
     * the lookup after a key, none when the key is not for the Surface. Nothing here is pressed,
     * so Enter only keeps the island, and Space types, as in the Launcher
     */
    fn step(mut self, key: Key, laid: &Laid) -> Option<Lookup> {
        let last = laid.rings.len().saturating_sub(1);

        match key {
            Key::Character(letter) => return Some(self.typed(Some(letter))),
            Key::Space => return Some(self.typed(Some(' '))),
            Key::Backspace => return Some(self.typed(None)),
            Key::Enter | Key::Left | Key::Right => return Some(self),
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => self.selected = (self.selected + 1).min(last),
            Key::Tab if self.selected < last => self.selected += 1,
            Key::Tab | Key::Home => self.selected = 0,
            Key::End => self.selected = last,
            _ => return None,
        }

        self.offset = laid.reveal(self.offset, self.selected);

        Some(self)
    }

    // a letter typed, or the last one erased; a new query finds new rows, so it starts at the top
    fn typed(mut self, letter: Option<char>) -> Lookup {
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
    fn wheel(mut self, lines: f32, laid: &Laid) -> Lookup {
        self.offset = (self.offset + lines * WHEEL).clamp(0.0, laid.most());

        if let Some((first, last)) = laid.whole(self.offset) {
            self.selected = self.selected.clamp(first, last);
        }

        self
    }
}

// the view's own read of the visit, so this never reads IslandService again
pub fn surface(_monitor: &str, visit: u64) -> Rectangle {
    let lookup = Lookup::read().of(visit);
    let laid = laid(&lookup.query);
    let lookup = lookup.bounded(&laid);

    let list: Box<dyn Widget> = if laid.items.is_empty() {
        Box::new(search::state(
            Icon::Keyboard,
            "No matching keys",
            &format!("Nothing found for \u{201c}{}\u{201d}", lookup.query.trim()),
            width(),
            room(),
        ))
    } else {
        Box::new(list(&laid, &lookup))
    };

    let shape = view::largest();

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::Region, "keys"))
        .child(
            Column::new(vec![
                Box::new(view::rise(
                    0,
                    search::field(Icon::Search, &lookup.query, "Search keys", width()),
                )) as Box<dyn Widget>,
                Box::new(view::rise(1, list)),
                Box::new(view::rise(2, footer(&laid, &lookup))),
            ])
            .width(width())
            .gap(GAP),
        )
}

/*
 * the items any part of shows, scrolled `offset` down and clipped to the list; a thumb in the inset
 * says where while they do not all fit
 */
fn list(laid: &Laid, lookup: &Lookup) -> Stack {
    let ring = laid.rings.get(lookup.selected).copied();
    let (from, to) = (lookup.offset, lookup.offset + room());

    let shown: Vec<Box<dyn Widget>> = laid
        .items
        .iter()
        .enumerate()
        .filter(|(_, (_, top, height))| top + height > from && *top < to)
        .map(|(at, (item, top, height))| {
            Box::new(row(item, *height, ring == Some(at)).translate(0.0, top - lookup.offset))
                as Box<dyn Widget>
        })
        .collect();

    let listed = laid.content.min(room());

    let viewport = Rectangle::new()
        .width(width())
        .height(listed)
        .clip()
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::List, "binds"))
        .on_scroll(|Scroll { y, .. }| wheel(y))
        .child(Stack::new(shown).width(width()).height(listed));

    let mut layers = children![viewport];

    let most = laid.most();

    if most > 0.0 {
        let length = (room() * room() / laid.content).max(TARGET);
        let at = (room() - length) * lookup.offset / most;

        layers.push(Box::new(
            Rectangle::new()
                .width(3.0)
                .height(length)
                .radius(radius::HAIRLINE)
                .fill(theme::island().surface_container_high)
                .translate(width() + 7.0, at),
        ));
    }

    Stack::new(layers).width(width()).height(listed)
}

fn row(item: &Item, height: f32, ring: bool) -> Rectangle {
    let line = Rectangle::new()
        .width(width())
        .height(height)
        .radius(radius::ROW)
        .padding(Padding {
            top: 0.0,
            right: ROW_INSET,
            bottom: 0.0,
            left: ROW_INSET,
        });

    let line = match item {
        Item::Title(title) => line.align_child(Start, End).child(
            Text::new(*title)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::SEMIBOLD),
        ),
        Item::Bind(bind) => line.align_child(Start, Center).child(
            Row::new(children![
                Column::new(children![
                    Text::new(&bind.title)
                        .size(theme::text::BODY)
                        .color(theme::island().on_surface)
                        .weight(theme::text::MEDIUM)
                        .elide()
                ])
                .width(Parent),
                Row::new(bind.keys.iter().map(|key| cap(key)).collect()).gap(CAP_GAP),
            ])
            .width(Parent)
            .gap(GAP)
            .align(Center),
        ),
        Item::Help(place, keys) => line
            .padding(Padding {
                top: HELP_INSET,
                right: ROW_INSET,
                bottom: HELP_INSET,
                left: ROW_INSET,
            })
            .align_child(Start, Start)
            .child(
                Row::new(children![
                    Column::new(children![place_text(place)]).width(PLACE),
                    Column::new(children![help_text(keys)]).width(help_width()),
                ])
                .gap(GAP),
            ),
        Item::Notice(title, detail) => line.align_child(Start, Center).child(
            Row::new(children![
                Rectangle::new()
                    .width(TARGET)
                    .height(TARGET)
                    .align_child(Center, Center)
                    .child(Icon::Keyboard.crossed(16.0).opacity(theme::DISABLED)),
                Column::new(children![
                    Text::new(title)
                        .size(theme::text::BODY)
                        .color(theme::island().on_surface)
                        .weight(theme::text::SEMIBOLD)
                        .elide(),
                    Text::new(detail)
                        .size(theme::text::LABEL_SMALL)
                        .color(theme::island().on_surface_variant)
                        .weight(theme::text::MEDIUM)
                        .elide(),
                ])
                .width(Parent)
                .gap(2.0),
            ])
            .width(Parent)
            .gap(GAP)
            .align(Center),
        ),
    };

    if ring {
        line.fill(theme::island().surface_container).border_if(true)
    } else {
        line
    }
}

// where in Kanade, wrapped in its column
fn place_text(place: &str) -> Text {
    Text::new(place)
        .size(theme::text::LABEL)
        .color(theme::island().on_surface)
        .weight(theme::text::SEMIBOLD)
        .wrap()
}

// what Kanade's keys do somewhere, wrapped beside where
fn help_text(keys: &str) -> Text {
    Text::new(keys)
        .size(theme::text::LABEL)
        .color(theme::island().on_surface_variant)
        .weight(theme::text::MEDIUM)
        .wrap()
}

fn help_width() -> f32 {
    width() - 2.0 * ROW_INSET - PLACE - GAP
}

// a key as a keycap: its name on a raised plate, as wide as the name
fn cap(key: &str) -> Box<dyn Widget> {
    let label = Text::new(key)
        .size(theme::text::LABEL_SMALL)
        .color(theme::island().on_surface)
        .weight(theme::text::SEMIBOLD);

    let wide = match label.width() {
        Size::Fixed(natural) => natural,
        _ => 0.0,
    };

    Box::new(
        Rectangle::new()
            .width((wide + 2.0 * CAP_INSET).max(CAP))
            .height(CAP)
            .radius(radius::ICON)
            .fill(theme::island().surface_container_high)
            .align_child(Center, Center)
            .child(label),
    )
}

// where the binds come from, and how many there are, of how many while searching
fn footer(laid: &Laid, lookup: &Lookup) -> Row {
    let from = binds::config()
        .map(|path| format!("From {}", shown(&path)))
        .unwrap_or_default();

    let count = match laid.binds {
        _ if !lookup.query.trim().is_empty() => {
            format!("{} of {} keybinds", laid.found, laid.binds)
        }
        1 => String::from("1 keybind"),
        all => format!("{all} keybinds"),
    };

    Row::new(children![
        Column::new(children![
            Text::new(from)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::MEDIUM)
                .elide()
        ])
        .width(Parent),
        Text::new(count)
            .size(theme::text::LABEL_SMALL)
            .color(theme::island().on_surface_variant)
            .weight(theme::text::MEDIUM),
    ])
    .width(width())
    .height(FOOTER)
    .gap(GAP)
    .align(Center)
}

// a path with home as `~`
fn shown(path: &Path) -> String {
    std::env::var_os("HOME")
        .and_then(|home| path.strip_prefix(home).ok())
        .map(|rest| format!("~/{}", rest.display()))
        .unwrap_or_else(|| path.display().to_string())
}

/*
 * a key while this island shows Keys; false for one it does not use, which the window's own keys
 * then get. A key it uses keeps the held island open for another hold
 */
pub fn key(monitor: &str, key: Key) -> bool {
    let visit = {
        let island = IslandService::read();

        if island.presentation(monitor) != Presentation::Expanded(Surface::Keys) {
            return false;
        }

        island.visit()
    };

    let lookup = Lookup::read().of(visit);
    let laid = laid(&lookup.query);

    let Some(lookup) = lookup.bounded(&laid).step(key, &laid) else {
        return false;
    };

    store(lookup);

    IslandService::write().attend(monitor, Instant::now());

    true
}

// down scrolls further down the list
fn wheel(lines: f32) {
    let visit = IslandService::read().visit();

    let lookup = Lookup::read().of(visit);
    let laid = laid(&lookup.query);

    store(lookup.bounded(&laid).wheel(lines, &laid));
}
