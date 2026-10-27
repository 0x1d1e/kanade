//! The Notifications Surface: the history, newest first, each with its actions and a
//! dismiss, then Do Not Disturb and Clear all. An app with more than one notification shows them
//! as a stack: the newest one with a count, pressed to open the stack, whose head then closes it
//! and whose cross dismisses them all. A stack is closed again each time the Surface opens. Arrow
//! keys move a ring over every part of it and Enter presses what the ring is on, so it works
//! without a pointer. It says when another daemon has the notifications, and when there are none.

use std::cmp::Reverse;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Instant, SystemTime};

use crate::sources::notifications::{Notification, Notifications, Urgency};
use kanade_runtime::service::{self, Service};
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Column, Cursor, Key, Padding, Rectangle, Row, Scroll, Size, SpaceBetween, Stack, Start,
    Text, Widget, children,
};

use crate::icon::Icon;
use crate::island::activity::Toast;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::sources::notifications::{self, Daemon};
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, DISABLED, radius};
use crate::view;

use super::ring::Ring;
use super::{Outline, RING, store};

// the content's width, which every row fills, as wide as the largest body (`island.width`)
fn width() -> f32 {
    view::largest().width - 2.0 * INSET
}

const HEADER: f32 = 20.0;
const FOOTER: f32 = 28.0;
const GAP: f32 = 10.0;

// what the cards scroll in, as tall as the largest body (`island.height`) leaves
fn room() -> f32 {
    view::largest().height - 2.0 * INSET - HEADER - FOOTER - 2.0 * GAP
}

// what a state, as no notifications, takes of the list
const STATE: f32 = 120.0;

const CARD: f32 = 70.0;

// what a stack shows of the notifications behind its newest, and the head of an open stack
const STACK: f32 = 10.0;
const HEAD: f32 = 36.0;
const CARD_GAP: f32 = 6.0;
const PILL_PADDING: f32 = 10.0;
const CARD_INSET: f32 = 10.0;

// the action row a card with actions adds under its text
const ACTIONS: f32 = 32.0;

// past this many a card shows only the first ones; the sender's own window has the rest
const MOST_ACTIONS: usize = 3;
const ACTION_WIDTH: f32 = 120.0;

const TILE: f32 = 36.0;
const TILE_GAP: f32 = 10.0;

// Silence / Unsilence, beside the dismiss
const QUIET: f32 = 78.0;

// pixels per wheel line
const WHEEL: f32 = 40.0;

/*
 * the ring and the scroll for one visit of the Surface (`IslandService::visit`), so every opening
 * starts at the top with the ring on the newest card. Written by input only, never by the view
 */
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Focus {
    // none is the first part: the newest card, or Do Not Disturb with none
    ring: Ring<At>,

    // the stacks open, by `group`
    open: Vec<u64>,

    // how far the cards scrolled, in pixels
    offset: f32,
}

impl Service for Focus {
    fn new() -> Self {
        Focus::default()
    }

    fn listen() {}
}

impl Focus {
    // this visit's focus; one kept from an earlier visit is over
    fn of(&self, visit: u64, held: bool) -> Focus {
        if self.ring.current(visit) {
            self.clone()
        } else {
            Focus {
                ring: Ring::start(visit, held),
                ..Focus::default()
            }
        }
    }
}

/*
 * where the ring is, by what it is on rather than where, so a card arriving above it keeps it on
 * the same card. `row` is where the card was, for when it goes
 */
#[derive(Debug, Clone, Copy, PartialEq)]
enum At {
    Card { item: Item, row: usize, part: Part },
    Footer(Footer),
}

// what a row is: one notification, or the stack of an app, closed or open
#[derive(Debug, Clone, Copy, PartialEq)]
enum Item {
    Card(u32),
    Group(u64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Part {
    Body,
    Action(usize),
    // no Banner and no sound for this app, history kept
    Silence,
    Dismiss,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Footer {
    Dnd,
    Clear,
}

// a card's place in the grid of parts: rows are the cards, newest first, then the footer
#[derive(Debug, Clone, Copy, PartialEq)]
struct Place {
    row: usize,
    column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Look {
    Card,

    // the newest of an app's notifications, with the others behind it
    Stack,

    // the line over an app's notifications while they show
    Head,
}

// the parts of each row the ring moves over, newest first; a stack and its head have the body
// and the dismiss only
#[derive(Debug, Clone, PartialEq)]
struct Shape {
    item: Item,
    look: Look,
    actions: usize,
}

impl Shape {
    // the body, every action shown, silence, then the dismiss
    fn columns(&self) -> usize {
        self.actions + 3
    }

    fn height(&self) -> f32 {
        match (self.look, self.actions) {
            (Look::Stack, _) => CARD + STACK,
            (Look::Head, _) => HEAD,
            (Look::Card, 0) => CARD,
            (Look::Card, _) => CARD + ACTIONS,
        }
    }
}

// the footer has Clear all only while there is something to clear
fn footer_columns(cards: &[Shape]) -> usize {
    if cards.is_empty() { 1 } else { 2 }
}

fn columns(cards: &[Shape], row: usize) -> usize {
    cards
        .get(row)
        .map_or_else(|| footer_columns(cards), Shape::columns)
}

impl At {
    /*
     * where the ring is in the grid; a card gone leaves it on the one that took its row, or on the
     * footer, on the same kind of part
     */
    fn place(at: Option<At>, cards: &[Shape]) -> Place {
        let footer = cards.len();

        let Some(at) = at else {
            return Place { row: 0, column: 0 };
        };

        match at {
            At::Card { item, row, part } => {
                let row = cards
                    .iter()
                    .position(|card| card.item == item)
                    .unwrap_or(row.min(footer));
                let last = columns(cards, row) - 1;

                let column = match (cards.get(row), part) {
                    (_, Part::Body) => 0,
                    (_, Part::Silence) => last.saturating_sub(1),
                    (_, Part::Dismiss) => last,
                    (Some(card), Part::Action(index))
                        if card.item == item && index < card.actions =>
                    {
                        index + 1
                    }
                    _ => 0,
                };

                Place { row, column }
            }
            At::Footer(Footer::Dnd) => Place {
                row: footer,
                column: 0,
            },
            At::Footer(Footer::Clear) => Place {
                row: footer,
                column: footer_columns(cards) - 1,
            },
        }
    }

    // what the ring is on at `place`
    fn on(place: Place, cards: &[Shape]) -> At {
        match cards.get(place.row) {
            Some(card) => {
                let columns = card.columns();
                At::Card {
                    item: card.item,
                    row: place.row,
                    part: match place.column {
                        0 => Part::Body,
                        column if column + 1 == columns => Part::Dismiss,
                        column if column + 2 == columns => Part::Silence,
                        column => Part::Action(column.saturating_sub(1)),
                    },
                }
            }
            None if place.column == 0 => At::Footer(Footer::Dnd),
            None => At::Footer(Footer::Clear),
        }
    }
}

impl Place {
    /*
     * up and down keep to the right edge from a dismiss or Clear all and otherwise go to the left
     * one, so the ring never lands on an action it did not come from; left and right stop at the
     * row's ends; Tab reads on through every part and back to the first
     */
    fn moved(self, key: Key, cards: &[Shape]) -> Option<Place> {
        let footer = cards.len();
        let last = columns(cards, self.row) - 1;

        let to_row = |row: usize| {
            let edge = self.column == last && last > 0;

            Place {
                row,
                column: if edge { columns(cards, row) - 1 } else { 0 },
            }
        };

        let place = match key {
            Key::Up if self.row > 0 => to_row(self.row - 1),
            Key::Down if self.row < footer => to_row(self.row + 1),
            Key::Left if self.column > 0 => Place {
                column: self.column - 1,
                ..self
            },
            Key::Right if self.column < last => Place {
                column: self.column + 1,
                ..self
            },
            Key::Tab if self.column < last => Place {
                column: self.column + 1,
                ..self
            },
            Key::Tab if self.row < footer => Place {
                row: self.row + 1,
                column: 0,
            },
            Key::Tab => Place { row: 0, column: 0 },
            Key::Home => Place { row: 0, column: 0 },
            Key::End => Place {
                row: footer.saturating_sub(1),
                column: 0,
            },
            Key::Up | Key::Down | Key::Left | Key::Right => self,
            _ => return None,
        };

        Some(place)
    }
}

// how far the cards can scroll: none while they fit
fn most(cards: &[Shape]) -> f32 {
    (content(cards) - room()).max(0.0)
}

fn content(cards: &[Shape]) -> f32 {
    let heights: f32 = cards.iter().map(Shape::height).sum();

    heights + CARD_GAP * cards.len().saturating_sub(1) as f32
}

// how tall the cards ask the body to be, or a state for none; the island caps it (ADR 0030)
fn asks(cards: &[Shape]) -> f32 {
    let list = if cards.is_empty() {
        STATE
    } else {
        content(cards)
    };

    2.0 * INSET + HEADER + list + FOOTER + 2.0 * GAP
}

// the list as tall as its cards, scrolling once they reach past the room
fn listed(cards: &[Shape]) -> f32 {
    content(cards).min(room())
}

/*
 * tells the island how tall the Notifications Surface asks to be: as its stacks closed when a
 * visit opens it, and as the ones open in the visit now. After each change of the notifications,
 * the daemon or a stack, never from a view
 */
pub fn fit() {
    let _ordered = super::fitting();
    let visit = IslandService::read().visit();
    let open = Focus::read().of(visit, false).open;

    /*
     * the notifications are read and turned into cards once, and the rows are grouped from those:
     * the body asks for the list with the stacks closed and with the ones this visit opened, and
     * reading the daemon twice for the same list held every other Surface's fit behind both
     */
    let (fresh, now) = match *Daemon::read() {
        Daemon::Running => {
            let cards = cards();
            let now = shapes(&cards, &open);

            let fresh = match open.is_empty() {
                true => asks(&now),
                false => asks(&shapes(&cards, &[])),
            };

            (fresh, asks(&now))
        }
        Daemon::Starting | Daemon::Conflict(_) | Daemon::NoBus => {
            let none = Vec::new();

            (asks(&none), asks(&none))
        }
    };

    super::ask(Surface::Notifications, fresh, Some((visit, now)));
}

// the shapes the list has with `open`'s stacks showing
fn shapes(cards: &[Card], open: &[u64]) -> Vec<Shape> {
    rows_of(cards, open).iter().map(Entry::shape).collect()
}

// at start: the body follows the notifications and the stacks open from now on
pub fn start() {
    service::watch::<Notifications>(fit);
    service::watch::<Daemon>(fit);
    service::watch::<Focus>(fit);
    fit();
}

// the top of the card in `row`
fn top(cards: &[Shape], row: usize) -> f32 {
    cards[..row]
        .iter()
        .map(|card| card.height() + CARD_GAP)
        .sum()
}

// the least scroll from `offset` that shows the card in `row` whole; the footer needs none
fn reveal(offset: f32, cards: &[Shape], row: usize) -> f32 {
    let Some(card) = cards.get(row) else {
        return offset;
    };

    let top = top(cards, row);

    offset.min(top).max(top + card.height() - room())
}

// what the ring presses
#[derive(Debug, Clone, PartialEq)]
enum Press {
    // the notification itself: its default action, or a dismiss without one
    Open { id: u32, default: bool },
    Action { id: u32, key: String },
    Dismiss(u32),

    // no Banner and no sound for this app, including Critical; history stays
    Silence { app: String, desktop: String },

    // a stack: open it or close it, or dismiss all it holds
    Toggle(u64),
    DismissAll(Vec<u32>),
    Dnd,
    Clear,
}

// what a card shows, read from the daemon's list
#[derive(Clone)]
struct Card {
    id: u32,

    // groups the notifications into stacks, as the sender named itself
    app: String,
    toast: Toast,
    critical: bool,
    default: bool,
    desktop: String,

    // key and label, at most `MOST_ACTIONS`
    actions: Vec<(String, String)>,

    received: SystemTime,
}

impl Card {
    fn of(notification: &Notification) -> Card {
        Card {
            id: notification.id(),
            app: notification.app_name().to_owned(),
            toast: notifications::toast(notification),
            critical: notification.urgency() == Urgency::Critical,
            default: notification.has_default_action(),
            desktop: notification.desktop_entry().to_owned(),
            actions: notification
                .actions()
                .iter()
                .take(MOST_ACTIONS)
                .map(|action| (action.key().to_owned(), action.label().to_owned()))
                .collect(),
            received: notification.received(),
        }
    }

    fn shape(&self) -> Shape {
        Shape {
            item: Item::Card(self.id),
            look: Look::Card,
            actions: self.actions.len(),
        }
    }

    fn press(&self, part: Part) -> Press {
        match part {
            Part::Body => Press::Open {
                id: self.id,
                default: self.default,
            },
            Part::Action(index) => Press::Action {
                id: self.id,
                key: self.actions[index].0.clone(),
            },
            Part::Silence => Press::Silence {
                app: self.toast.app.clone(),
                desktop: self.desktop.clone(),
            },
            Part::Dismiss => Press::Dismiss(self.id),
        }
    }
}

// an app's notifications, newest first: `front` is the newest
struct Group {
    key: u64,
    ids: Vec<u32>,
    front: Card,

    // a Critical one among them shows on the stack
    critical: bool,
}

// what the list shows: a notification, or an app's stack closed or open
enum Entry {
    Card(Card),
    Stack(Group),
    Head(Group),
}

impl Entry {
    fn shape(&self) -> Shape {
        match self {
            Entry::Card(card) => card.shape(),
            Entry::Stack(group) => group.shape(Look::Stack),
            Entry::Head(group) => group.shape(Look::Head),
        }
    }

    fn press(&self, part: Part) -> Press {
        match self {
            Entry::Card(card) => card.press(part),
            Entry::Stack(group) | Entry::Head(group) => match part {
                Part::Dismiss => Press::DismissAll(group.ids.clone()),
                Part::Silence => Press::Silence {
                    app: group.front.toast.app.clone(),
                    desktop: group.front.desktop.clone(),
                },
                Part::Body | Part::Action(_) => Press::Toggle(group.key),
            },
        }
    }
}

impl Group {
    fn shape(&self, look: Look) -> Shape {
        Shape {
            item: Item::Group(self.key),
            look,
            actions: 0,
        }
    }
}

// the same for the same app within a run, which is as long as a stack open is kept
fn group(app: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    app.hash(&mut hasher);

    hasher.finish()
}

/*
 * newest first, a replaced one counting from its replacement; one its rule keeps out of the
 * history never shows here, only as its Banner. An app's notifications are a stack, placed by its
 * newest, or a head and each of them while its key is in `open`; one alone is a card
 */
fn rows(open: &[u64]) -> Vec<Entry> {
    rows_of(&cards(), open)
}

// the rows `cards` make, the stacks in `open` showing their own cards under their head
fn rows_of(cards: &[Card], open: &[u64]) -> Vec<Entry> {
    let mut apps: Vec<Vec<Card>> = Vec::new();

    for card in cards.iter().cloned() {
        match apps
            .iter_mut()
            .find(|cards| cards.first().is_some_and(|first| first.app == card.app))
        {
            Some(cards) => cards.push(card),
            None => apps.push(vec![card]),
        }
    }

    let mut rows = Vec::new();

    for mut cards in apps {
        if cards.len() == 1 {
            rows.extend(cards.pop().map(Entry::Card));

            continue;
        }

        let key = group(&cards[0].app);

        let stack = Group {
            key,
            ids: cards.iter().map(|card| card.id).collect(),
            critical: cards.iter().any(|card| card.critical),
            front: cards[0].clone(),
        };

        if open.contains(&key) {
            rows.push(Entry::Head(stack));
            rows.extend(cards.into_iter().map(Entry::Card));
        } else {
            rows.push(Entry::Stack(stack));
        }
    }

    rows
}

// newest first
fn cards() -> Vec<Card> {
    let notifications = Notifications::read();

    let mut cards: Vec<Card> = notifications
        .list()
        .iter()
        .filter(|notification| notification.effects().history)
        .map(Card::of)
        .collect();
    cards.sort_by_key(|card| Reverse(card.received));

    cards
}

/*
 * `open` says the Surface is open rather than fading out, so only then does the ring show. The
 * rest is the view's own read of IslandService, so this never reads it again
 */
pub fn surface(monitor: &str, open: bool, visit: u64, held: bool, dnd: bool) -> Rectangle {
    let focus = Focus::read().of(visit, held);
    let cards = rows(&focus.open);
    let shapes: Vec<Shape> = cards.iter().map(Entry::shape).collect();
    let daemon = Daemon::read().clone();

    let ring = (open && focus.ring.shown).then(|| At::place(focus.ring.at, &shapes));
    let offset = focus.offset.clamp(0.0, most(&shapes));

    let list: Box<dyn Widget> = match daemon {
        Daemon::Conflict(other) => Box::new(state(
            Icon::Bell.crossed(28.0),
            &format!("{other} is the notification daemon"),
            "Stop it and restart Kanade to see notifications here",
        )),
        // the daemon gets the bus name within a moment of starting, so nothing says anything yet
        Daemon::Starting | Daemon::NoBus => {
            Box::new(Rectangle::new().width(width()).height(STATE.min(room())))
        }
        Daemon::Running if cards.is_empty() => Box::new(state(
            Icon::Bell.draw(28.0),
            "No notifications",
            if dnd {
                "Do Not Disturb is on"
            } else {
                "New ones show here"
            },
        )),
        Daemon::Running => Box::new(list(monitor, &cards, &shapes, ring, offset)),
    };

    let footer_ring = ring
        .filter(|place| place.row == cards.len())
        .map(|place| place.column);

    let shape = view::largest();

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::Region, "notifications"))
        .child(
            Column::new(vec![
                Box::new(view::rise(0, header(count(&cards)))) as Box<dyn Widget>,
                Box::new(view::rise(1, list)),
                Box::new(view::rise(
                    2,
                    footer(monitor, dnd, !cards.is_empty(), footer_ring),
                )),
            ])
            .width(width())
            .gap(GAP),
        )
}

// how many notifications the rows hold: a stack holds all of an app's, its head none
fn count(rows: &[Entry]) -> usize {
    rows.iter()
        .map(|row| match row {
            Entry::Card(_) => 1,
            Entry::Stack(group) => group.ids.len(),
            Entry::Head(_) => 0,
        })
        .sum()
}

// the title and how many there are
fn header(count: usize) -> Row {
    let mut words = children![
        Text::new("Notifications")
            .size(theme::text::TITLE)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD)
    ];

    if count > 0 {
        words.push(Box::new(
            Text::new(count.to_string())
                .size(theme::text::BODY)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::SEMIBOLD),
        ));
    }

    Row::new(words)
        .width(width())
        .height(HEADER)
        .gap(8.0)
        .align(Center)
}

// an icon and what it means, in the middle of where the cards go
fn state(icon: Rectangle, title: &str, detail: &str) -> Rectangle {
    let mut lines = children![
        icon,
        Text::new(title)
            .size(theme::text::BODY)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD),
    ];

    if !detail.is_empty() {
        lines.push(Box::new(
            Text::new(detail)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::MEDIUM),
        ));
    }

    Rectangle::new()
        .width(width())
        .height(STATE.min(room()))
        .align_child(Center, Center)
        .child(Column::new(lines).gap(6.0).align(Center))
}

/*
 * the cards scrolled `offset` down, clipped to the list, which only reacts where a card shows;
 * a thumb in the inset says where while they do not all fit
 */
fn list(
    monitor: &str,
    cards: &[Entry],
    shapes: &[Shape],
    ring: Option<Place>,
    offset: f32,
) -> Stack {
    let column = Column::new(
        cards
            .iter()
            .enumerate()
            .map(|(row, card)| {
                let ring = ring
                    .filter(|place| place.row == row)
                    .map(|place| place.column);

                match card {
                    Entry::Card(card) => {
                        Box::new(self::card(monitor, card, ring)) as Box<dyn Widget>
                    }
                    Entry::Stack(group) => Box::new(stack(monitor, group, ring)),
                    Entry::Head(group) => Box::new(head(monitor, group, ring)),
                }
            })
            .collect(),
    )
    .width(width())
    .gap(CARD_GAP);

    let scrolled = monitor.to_owned();

    let viewport = Rectangle::new()
        .width(width())
        .height(listed(shapes))
        .clip()
        .align_child(Start, Start)
        .semantics(|| Semantics::new(Role::List, "cards"))
        .on_scroll(move |Scroll { y, .. }| wheel(&scrolled, y))
        .child(
            Rectangle::new()
                .width(width())
                .height(content(shapes))
                .align_child(Start, Start)
                .translate(0.0, -offset)
                .child(column),
        );

    let mut layers = children![viewport];

    let most = most(shapes);

    if most > 0.0 {
        let length = (room() * room() / content(shapes)).max(TARGET);
        let at = (room() - length) * offset / most;

        layers.push(Box::new(
            Rectangle::new()
                .width(3.0)
                .height(length)
                .radius(radius::HAIRLINE)
                .fill(theme::island().surface_container_high)
                .translate(width() + 7.0, at),
        ));
    }

    Stack::new(layers).width(width()).height(listed(shapes))
}

// the lines of text of a card: who sent it, the summary and the body. A stack counts its notifications
fn lines(card: &Card, critical: bool, count: Option<usize>) -> Vec<Box<dyn Widget>> {
    let sender = Text::new(&card.toast.app)
        .size(theme::text::LABEL_SMALL)
        .color(theme::island().on_surface_variant)
        .weight(theme::text::MEDIUM)
        .elide();

    let mut tags: Vec<Box<dyn Widget>> = Vec::new();

    if let Some(count) = count {
        tags.push(Box::new(badge(count)));
    }

    // said in words before the sender, so it never rests on color alone or elides away
    if critical {
        tags.push(Box::new(
            Text::new("Critical")
                .size(theme::text::LABEL_SMALL)
                .color(theme::signals().critical)
                .weight(theme::text::SEMIBOLD),
        ));
    }

    tags.push(Box::new(sender));

    let from: Box<dyn Widget> = if tags.len() == 1 {
        tags.remove(0)
    } else {
        Box::new(Row::new(tags).gap(6.0).align(Center))
    };

    let mut lines = vec![
        from,
        Box::new(
            Text::new(&card.toast.summary)
                .size(theme::text::BODY)
                .color(theme::island().on_surface)
                .weight(theme::text::SEMIBOLD)
                .elide(),
        ) as Box<dyn Widget>,
    ];

    if !card.toast.body.is_empty() {
        lines.push(Box::new(
            Text::new(&card.toast.body)
                .size(theme::text::LABEL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::MEDIUM)
                .elide(),
        ));
    }

    lines
}

// how many a stack holds, in a small round tag
fn badge(count: usize) -> Rectangle {
    let digits = count.to_string();

    Rectangle::new()
        .width(12.0 + 6.0 * digits.len() as f32)
        .height(16.0)
        .radius(8.0)
        .fill(theme::island().surface_container_high)
        .align_child(Center, Center)
        .child(
            Text::new(digits)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface)
                .weight(theme::text::SEMIBOLD),
        )
}

/*
 * the sender's picture, who sent it, the summary and the body, a dismiss in the corner, and the
 * actions under the text. Pressing it opens it. `ring` is the column the ring is on
 */
fn card(monitor: &str, card: &Card, ring: Option<usize>) -> Rectangle {
    let words = width() - 2.0 * CARD_INSET - TILE - 2.0 * TILE_GAP - TARGET - QUIET - 4.0;

    let top = Row::new(children![
        view::toast_tile(&card.toast, TILE, radius::TILE, &theme::island()),
        Column::new(lines(card, card.critical, None))
            .width(words)
            .gap(1.0),
        quiet(
            monitor,
            card.press(Part::Silence),
            notifications::silence::covers(&card.toast.app, &card.desktop),
            ring == Some(card.actions.len() + 1),
        ),
        dismiss(
            monitor,
            Press::Dismiss(card.id),
            ring == Some(card.actions.len() + 2)
        ),
    ])
    .gap(TILE_GAP)
    .align(Start);

    let mut parts = children![top];

    if !card.actions.is_empty() {
        parts.push(Box::new(actions(monitor, card, ring, words + TARGET)));
    }

    let press = card.press(Part::Body);
    let monitor = monitor.to_owned();

    Rectangle::new()
        .width(width())
        .height(card.shape().height())
        .radius(radius::CARD)
        .fill(theme::island().surface_container)
        .padding(CARD_INSET)
        .align_child(Start, Start)
        .identity(card.id)
        .cursor(Cursor::Pointer)
        .semantics(|| Semantics::new(Role::Item, "card").content(card.toast.summary.as_str()))
        .on_click(super::on_left(move || {
            click(&monitor, press.clone());
        }))
        .child(Column::new(parts).gap(8.0))
        .border_if(ring == Some(0))
}

/*
 * an app's newest notification with a count, and the edge of one or two more behind it. Pressing it
 * opens the stack; its cross dismisses every notification of the app. `ring` is the column the ring
 * is on
 */
fn stack(monitor: &str, group: &Group, ring: Option<usize>) -> Stack {
    let card = &group.front;
    let words = width() - 2.0 * CARD_INSET - TILE - 2.0 * TILE_GAP - TARGET - QUIET - 4.0;

    let top = Row::new(children![
        view::toast_tile(&card.toast, TILE, radius::TILE, &theme::island()),
        Column::new(lines(card, group.critical, Some(group.ids.len())))
            .width(words)
            .gap(1.0),
        quiet(
            monitor,
            Press::Silence {
                app: card.toast.app.clone(),
                desktop: card.desktop.clone(),
            },
            notifications::silence::covers(&card.toast.app, &card.desktop),
            ring == Some(1),
        ),
        dismiss(
            monitor,
            Press::DismissAll(group.ids.clone()),
            ring == Some(2)
        ),
    ])
    .gap(TILE_GAP)
    .align(Start);

    let press = Press::Toggle(group.key);
    let clicked = monitor.to_owned();

    let front = Rectangle::new()
        .width(width())
        .height(CARD)
        .radius(radius::CARD)
        .fill(theme::island().surface_container)
        .padding(CARD_INSET)
        .align_child(Start, Start)
        .identity(("stack", group.key))
        .cursor(Cursor::Pointer)
        .semantics(|| Semantics::new(Role::Item, "stack").content(card.toast.app.as_str()))
        .on_click(super::on_left(move || {
            click(&clicked, press.clone());
        }))
        .child(top)
        .border_if(ring == Some(0));

    // the notifications behind, each a little narrower and lower, the farthest first
    let behind = (group.ids.len() - 1).min(2);
    let mut layers: Vec<Box<dyn Widget>> = Vec::new();

    for depth in (1..=behind).rev() {
        let depth = depth as f32;

        layers.push(Box::new(
            Rectangle::new()
                .width(width() - 2.0 * STACK * depth)
                .height(CARD)
                .radius(radius::CARD)
                .fill(theme::island().surface_container_high)
                .translate(STACK * depth, STACK * depth / 2.0),
        ));
    }

    layers.push(Box::new(front));

    Stack::new(layers).width(width()).height(CARD + STACK)
}

/*
 * the line over an open stack's notifications: the app and how many, pressed to close the stack,
 * and a cross that dismisses them all. `ring` is the column the ring is on
 */
fn head(monitor: &str, group: &Group, ring: Option<usize>) -> Rectangle {
    let less = Text::new("Show less")
        .size(theme::text::LABEL_SMALL)
        .color(theme::island().on_surface_variant)
        .weight(theme::text::MEDIUM);

    let less_width = match less.width() {
        Size::Fixed(natural) => natural,
        _ => 70.0,
    };

    let words = width() - 2.0 * CARD_INSET - TARGET - QUIET - less_width - 3.0 * TILE_GAP - 4.0;

    let label = Text::new(format!("{} · {}", group.front.toast.app, group.ids.len()))
        .size(theme::text::LABEL_SMALL)
        .color(theme::island().on_surface)
        .weight(theme::text::SEMIBOLD)
        .elide();

    let head = Rectangle::new()
        .width(width())
        .height(HEAD)
        .radius(HEAD / 2.0)
        .padding(Padding {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: CARD_INSET,
        })
        .align_child(Start, Center)
        .identity(("head", group.key))
        .cursor(Cursor::Pointer);

    let head = if ring == Some(0) {
        head.border(RING, theme::island().on_surface)
    } else {
        head.border(1.0, theme::island().surface_container_high)
    };

    let press = Press::Toggle(group.key);
    let clicked = monitor.to_owned();

    head.semantics(|| Semantics::new(Role::Button, "head").content(group.front.toast.app.as_str()))
        .on_click(super::on_left(move || {
            click(&clicked, press.clone());
        }))
        .child(
            Row::new(children![
                Column::new(children![label]).width(words),
                less,
                quiet(
                    monitor,
                    Press::Silence {
                        app: group.front.toast.app.clone(),
                        desktop: group.front.desktop.clone(),
                    },
                    notifications::silence::covers(&group.front.toast.app, &group.front.desktop),
                    ring == Some(1),
                ),
                dismiss(
                    monitor,
                    Press::DismissAll(group.ids.clone()),
                    ring == Some(2)
                ),
            ])
            .gap(TILE_GAP)
            .align(Center),
        )
}

// a pill per action, as wide as fits, under the text
fn actions(monitor: &str, card: &Card, ring: Option<usize>, width: f32) -> Row {
    let count = card.actions.len() as f32;
    let each = ((width - 6.0 * (count - 1.0)) / count).min(ACTION_WIDTH);

    let pills = card
        .actions
        .iter()
        .enumerate()
        .map(|(index, (_, label))| {
            let press = card.press(Part::Action(index));

            Box::new(
                pill(
                    monitor,
                    label_in(label, each - 2.0 * PILL_PADDING),
                    (each, TARGET),
                    ring == Some(index + 1),
                    Some(press),
                )
                .semantics(|| Semantics::new(Role::Button, "action").content(label.as_str())),
            ) as Box<dyn Widget>
        })
        .collect();

    // under the text, not the picture
    Row::new(children![
        Rectangle::new().width(TILE).height(TARGET),
        Row::new(pills).gap(6.0),
    ])
    .gap(TILE_GAP)
}

// centred while it fits, elided only when it does not, since elided text fills its width
fn label_in(label: &str, width: f32) -> Text {
    let text = Text::new(label)
        .size(theme::text::LABEL_SMALL)
        .color(theme::island().on_surface)
        .weight(theme::text::SEMIBOLD);

    match text.width() {
        Size::Fixed(natural) if natural <= width => text,
        _ => text.elide(),
    }
}

// Silence or Unsilence for this app: no Banner and no sound, history kept
fn quiet(monitor: &str, press: Press, silenced: bool, ring: bool) -> Rectangle {
    let monitor = monitor.to_owned();
    let label = if silenced { "Unsilence" } else { "Silence" };

    Rectangle::new()
        .width(QUIET)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .semantics(|| Semantics::new(Role::Button, "silence").label(label))
        .on_click(super::on_left(move || {
            click(&monitor, press.clone());
        }))
        .child(
            Text::new(label)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface)
                .weight(theme::text::MEDIUM)
                .elide(),
        )
        .border_if(ring)
}

// a round target with a cross, pressed to close the notification, or a stack's
fn dismiss(monitor: &str, press: Press, ring: bool) -> Rectangle {
    let monitor = monitor.to_owned();

    // on the same 20 unit grid as every icon, so its 10 point cross keeps the icons' line
    let cross = Icon::Dismiss.on(20.0, theme::island().on_surface_variant);

    Rectangle::new()
        .width(TARGET)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .semantics(|| Semantics::new(Role::Button, "dismiss"))
        .on_click(super::on_left(move || {
            click(&monitor, press.clone());
        }))
        .child(cross)
        .border_if(ring)
}

// Do Not Disturb with a switch that says whether it is on, and Clear all while there is any
fn footer(monitor: &str, dnd: bool, any: bool, ring: Option<usize>) -> Row {
    let switch = {
        let (track, knob, at) = if dnd {
            (theme::island().primary, theme::island().on_primary, 12.0)
        } else {
            (
                theme::island().surface_container_high,
                theme::island().on_surface_variant,
                0.0,
            )
        };

        Rectangle::new()
            .width(26.0)
            .height(14.0)
            .radius(14.0 / 2.0)
            .fill(track)
            .padding(2.0)
            .align_child(Start, Center)
            .child(
                Rectangle::new()
                    .width(10.0)
                    .height(10.0)
                    .radius(10.0 / 2.0)
                    .fill(knob)
                    .translate(at, 0.0),
            )
    };

    let label = Row::new(children![
        Icon::Moon.draw(16.0),
        Text::new("Do Not Disturb")
            .size(theme::text::LABEL_SMALL)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD),
    ])
    .gap(7.0)
    .align(Center);

    let dnd = pill(
        monitor,
        Row::new(children![label, switch])
            .width(140.0)
            .justify(SpaceBetween)
            .align(Center),
        (170.0, FOOTER),
        ring == Some(0),
        Some(Press::Dnd),
    )
    .semantics(|| Semantics::new(Role::Toggle, "dnd").checked(dnd));

    let clear = pill(
        monitor,
        Text::new("Clear all")
            .size(theme::text::LABEL_SMALL)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD),
        (84.0, FOOTER),
        ring == Some(1),
        any.then_some(Press::Clear),
    )
    .semantics(|| Semantics::new(Role::Button, "clear").disabled(!any));

    Row::new(children![dnd, clear])
        .width(width())
        .height(FOOTER)
        .justify(SpaceBetween)
        .align(Center)
}

// outlined, the ring in place of the outline; none to press leaves it faded and not pressable
fn pill(
    monitor: &str,
    child: impl Widget + 'static,
    (width, height): (f32, f32),
    ring: bool,
    press: Option<Press>,
) -> Rectangle {
    let pill = Rectangle::new()
        .width(width)
        .height(height)
        .radius(height / 2.0)
        .padding(Padding {
            top: 0.0,
            right: PILL_PADDING,
            bottom: 0.0,
            left: PILL_PADDING,
        })
        .align_child(Center, Center)
        .child(child);

    let pill = super::pill_face(pill);
    let pill = if ring {
        pill.border(RING, theme::island().on_surface)
    } else {
        pill
    };

    let monitor = monitor.to_owned();

    match press {
        Some(press) => pill
            .cursor(Cursor::Pointer)
            .on_click(super::on_left(move || {
                click(&monitor, press.clone());
            })),
        None => pill.opacity(DISABLED),
    }
}

/*
 * a key while this island shows the Surface; false for one it does not use, which the window's own
 * keys then get. The first key only shows the ring where it is. A key it uses keeps a held island
 * open for another hold
 */
pub fn key(monitor: &str, key: Key) -> bool {
    let (visit, held) = {
        let island = IslandService::read();

        if island.presentation(monitor) != Presentation::Expanded(Surface::Notifications) {
            return false;
        }

        (island.visit(), island.held(monitor))
    };

    let focus = Focus::read().of(visit, held);
    let cards = rows(&focus.open);
    let shapes: Vec<Shape> = cards.iter().map(Entry::shape).collect();

    let Some((focus, press)) = focus.step(key, &cards, &shapes) else {
        return false;
    };

    store(focus);

    IslandService::write().attend(monitor, Instant::now());

    if let Some(press) = press {
        run(monitor, press);
    }

    true
}

impl Focus {
    /*
     * the focus after a key and what it presses, none when the key is not for the Surface. A key
     * while the ring is hidden only shows it, so nothing is pressed that the ring was not on
     */
    fn step(
        mut self,
        key: Key,
        cards: &[Entry],
        shapes: &[Shape],
    ) -> Option<(Focus, Option<Press>)> {
        let place = At::place(self.ring.at, shapes);

        let press = match key {
            Key::Enter | Key::Space => Some(press(cards, shapes, place)),
            Key::Backspace => cards.get(place.row).map(|card| card.press(Part::Dismiss)),
            _ => None,
        };

        let moved = place.moved(key, shapes);

        if press.is_none() && moved.is_none() {
            return None;
        }

        if !self.ring.shown {
            self.ring.shown = true;

            return Some((self, None));
        }

        if let Some(moved) = moved {
            self.ring = self.ring.onto(At::on(moved, shapes));
            self.offset = reveal(self.offset.clamp(0.0, most(shapes)), shapes, moved.row);
        }

        Some((self, press))
    }
}

fn press(cards: &[Entry], shapes: &[Shape], place: Place) -> Press {
    match At::on(place, shapes) {
        At::Card { part, .. } => cards[place.row].press(part),
        At::Footer(Footer::Dnd) => Press::Dnd,
        At::Footer(Footer::Clear) => Press::Clear,
    }
}

// a press from the pointer hides the ring, which is for the keyboard
fn click(monitor: &str, press: Press) {
    let focus = focus(monitor);

    store(Focus {
        ring: focus.ring.hidden(),
        ..focus
    });

    run(monitor, press);
}

// what opens something elsewhere closes the island, so it is not in the way of what opened
fn run(monitor: &str, press: Press) {
    match press {
        Press::Open { id, default } => {
            Notifications::click(id);

            if default {
                view::collapse(monitor);
            }
        }
        Press::Action { id, key } => {
            Notifications::invoke(id, &key);
            view::collapse(monitor);
        }
        Press::Dismiss(id) => Notifications::dismiss(id),
        Press::DismissAll(ids) => Notifications::dismiss_all(&ids),
        Press::Silence { app, desktop } => notifications::silence::toggle_named(&app, &desktop),
        Press::Toggle(key) => {
            let mut focus = focus(monitor);

            match focus.open.iter().position(|open| *open == key) {
                Some(index) => {
                    focus.open.remove(index);
                }
                None => focus.open.push(key),
            }

            store(focus);
        }
        Press::Dnd => {
            let dnd = IslandService::read().dnd();

            notifications::set_dnd(!dnd, Instant::now());
        }
        Press::Clear => Notifications::clear(),
    }
}

// down scrolls further down the list
fn wheel(monitor: &str, lines: f32) {
    let mut focus = focus(monitor);

    let shapes: Vec<Shape> = rows(&focus.open).iter().map(Entry::shape).collect();
    let most = most(&shapes);

    focus.offset = (focus.offset.clamp(0.0, most) + lines * WHEEL).clamp(0.0, most);

    store(focus);
}

// this visit's focus, a fresh one when the stored one is from an earlier opening
fn focus(monitor: &str) -> Focus {
    let (visit, held) = {
        let island = IslandService::read();

        (island.visit(), island.held(monitor))
    };

    Focus::read().of(visit, held)
}
