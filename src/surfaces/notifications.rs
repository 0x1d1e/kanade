//! The Notifications Surface (plan 7): the history, newest first, each with its actions and a
//! dismiss, then Do Not Disturb and Clear all. Arrow keys move a ring over every part of it and
//! Enter presses what the ring is on, so it works without a pointer. It says when another daemon
//! has the notifications, and when there are none.

use std::cmp::Reverse;
use std::time::{Instant, SystemTime};

use crate::sources::notifications::{Notification, Notifications, Urgency};
use kanade_runtime::service::{self, Service};
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
    Card { id: u32, row: usize, part: Part },
    Footer(Footer),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Part {
    Body,
    Action(usize),
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

// the parts of each card the ring moves over, newest first
#[derive(Debug, Clone, PartialEq)]
struct Shape {
    id: u32,
    actions: usize,
}

impl Shape {
    // the body, every action shown, then the dismiss
    fn columns(&self) -> usize {
        self.actions + 2
    }

    fn height(&self) -> f32 {
        if self.actions == 0 {
            CARD
        } else {
            CARD + ACTIONS
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
            At::Card { id, row, part } => {
                let row = cards
                    .iter()
                    .position(|card| card.id == id)
                    .unwrap_or(row.min(footer));
                let last = columns(cards, row) - 1;

                let column = match (cards.get(row), part) {
                    (_, Part::Body) => 0,
                    (_, Part::Dismiss) => last,
                    (Some(card), Part::Action(index)) if card.id == id && index < card.actions => {
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
            Some(card) => At::Card {
                id: card.id,
                row: place.row,
                part: match place.column {
                    0 => Part::Body,
                    column if column > card.actions => Part::Dismiss,
                    column => Part::Action(column - 1),
                },
            },
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
 * tells the island how tall the Notifications Surface asks to be, the same in every visit. After
 * each change of the notifications or the daemon, never from a view
 */
pub fn fit() {
    let _ordered = super::fitting();
    let shapes: Vec<Shape> = match *Daemon::read() {
        Daemon::Running => cards().iter().map(Card::shape).collect(),
        Daemon::Starting | Daemon::Conflict(_) => Vec::new(),
    };

    IslandService::write().fit(Surface::Notifications, asks(&shapes), None, Instant::now());
}

// at start: the body follows the notifications from now on
pub fn start() {
    service::watch::<Notifications>(fit);
    service::watch::<Daemon>(fit);
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
    Dnd,
    Clear,
}

// what a card shows, read from the daemon's list
struct Card {
    id: u32,
    toast: Toast,
    critical: bool,
    default: bool,

    // key and label, at most `MOST_ACTIONS`
    actions: Vec<(String, String)>,

    received: SystemTime,
}

impl Card {
    fn of(notification: &Notification) -> Card {
        Card {
            id: notification.id(),
            toast: notifications::toast(notification),
            critical: notification.urgency() == Urgency::Critical,
            default: notification.has_default_action(),
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
            id: self.id,
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
            Part::Dismiss => Press::Dismiss(self.id),
        }
    }
}

// newest first, a replaced one counting from its replacement
fn cards() -> Vec<Card> {
    let notifications = Notifications::read();

    let mut cards: Vec<Card> = notifications.list().iter().map(Card::of).collect();
    cards.sort_by_key(|card| Reverse(card.received));

    cards
}

/*
 * `open` says the Surface is open rather than fading out, so only then does the ring show. The
 * rest is the view's own read of IslandService, so this never reads it again
 */
pub fn surface(monitor: &str, open: bool, visit: u64, held: bool, dnd: bool) -> Rectangle {
    let cards = cards();
    let shapes: Vec<Shape> = cards.iter().map(Card::shape).collect();
    let daemon = Daemon::read().clone();

    let focus = Focus::read().of(visit, held);
    let ring = (open && focus.ring.shown).then(|| At::place(focus.ring.at, &shapes));
    let offset = focus.offset.clamp(0.0, most(&shapes));

    let list: Box<dyn Widget> = match daemon {
        Daemon::Conflict(other) => Box::new(state(
            Icon::Bell.crossed(28.0),
            &format!("{other} is the notification daemon"),
            "Stop it and restart Kanade to see notifications here",
        )),
        // the daemon gets the bus name within a moment of starting, so nothing says anything yet
        Daemon::Starting => Box::new(Rectangle::new().width(width()).height(STATE.min(room()))),
        Daemon::Running if cards.is_empty() => Box::new(state(
            Icon::Bell.draw(28.0),
            "No notifications",
            if dnd { "Do Not Disturb is on" } else { "" },
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
        .child(
            Column::new(vec![
                Box::new(header(cards.len())) as Box<dyn Widget>,
                list,
                Box::new(footer(monitor, dnd, !cards.is_empty(), footer_ring)),
            ])
            .width(width())
            .gap(GAP),
        )
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
    cards: &[Card],
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

                Box::new(self::card(monitor, card, ring)) as Box<dyn Widget>
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

/*
 * the sender's picture, who sent it, the summary and the body, a dismiss in the corner, and the
 * actions under the text. Pressing it opens it. `ring` is the column the ring is on
 */
fn card(monitor: &str, card: &Card, ring: Option<usize>) -> Rectangle {
    let words = width() - 2.0 * CARD_INSET - TILE - TILE_GAP - TARGET - 4.0;

    let sender = Text::new(&card.toast.app)
        .size(theme::text::LABEL_SMALL)
        .color(theme::island().on_surface_variant)
        .weight(theme::text::MEDIUM)
        .elide();

    // said in words before the sender, so it never rests on color alone or elides away
    let from: Box<dyn Widget> = if card.critical {
        Box::new(
            Row::new(children![
                Text::new("Critical")
                    .size(theme::text::LABEL_SMALL)
                    .color(theme::SEMANTIC.critical)
                    .weight(theme::text::SEMIBOLD),
                sender,
            ])
            .gap(6.0),
        )
    } else {
        Box::new(sender)
    };

    let mut lines = vec![
        from,
        Box::new(
            Text::new(&card.toast.summary)
                .size(theme::text::BODY)
                .color(theme::island().on_surface)
                .weight(theme::text::SEMIBOLD)
                .elide(),
        ),
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

    let top = Row::new(children![
        view::toast_tile(&card.toast, TILE, radius::TILE, &theme::island()),
        Column::new(lines).width(words).gap(1.0),
        dismiss(monitor, card.id, ring == Some(card.actions.len() + 1)),
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
        .cursor(Cursor::Pointer)
        .on_click(super::on_left(move || {
            click(&monitor, press.clone());
        }))
        .child(Column::new(parts).gap(8.0))
        .border_if(ring == Some(0))
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

            Box::new(pill(
                monitor,
                label_in(label, each - 2.0 * PILL_PADDING),
                (each, TARGET),
                ring == Some(index + 1),
                Some(press),
            )) as Box<dyn Widget>
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

// a round target with a cross, pressed to close the notification
fn dismiss(monitor: &str, id: u32, ring: bool) -> Rectangle {
    let monitor = monitor.to_owned();

    // on the same 20 unit grid as every icon, so its 10 point cross keeps the icons' line
    let cross = Icon::Dismiss.on(20.0, theme::island().on_surface_variant);

    Rectangle::new()
        .width(TARGET)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(super::on_left(move || {
            click(&monitor, Press::Dismiss(id));
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
    );

    let clear = pill(
        monitor,
        Text::new("Clear all")
            .size(theme::text::LABEL_SMALL)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD),
        (84.0, FOOTER),
        ring == Some(1),
        any.then_some(Press::Clear),
    );

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

    let pill = if ring {
        pill.border(RING, theme::island().on_surface)
    } else {
        pill.border(1.0, theme::island().surface_container_high)
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

    let cards = cards();
    let shapes: Vec<Shape> = cards.iter().map(Card::shape).collect();

    let Some((focus, press)) = Focus::read().of(visit, held).step(key, &cards, &shapes) else {
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
        cards: &[Card],
        shapes: &[Shape],
    ) -> Option<(Focus, Option<Press>)> {
        let place = At::place(self.ring.at, shapes);

        let press = match key {
            Key::Enter | Key::Space => Some(press(cards, shapes, place)),
            Key::Backspace => cards.get(place.row).map(|card| Press::Dismiss(card.id)),
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

fn press(cards: &[Card], shapes: &[Shape], place: Place) -> Press {
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
        Press::Dnd => {
            let dnd = IslandService::read().dnd();

            notifications::set_dnd(!dnd, Instant::now());
        }
        Press::Clear => Notifications::clear(),
    }
}

// down scrolls further down the list
fn wheel(monitor: &str, lines: f32) {
    let shapes: Vec<Shape> = cards().iter().map(Card::shape).collect();
    let most = most(&shapes);

    let mut focus = focus(monitor);
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
