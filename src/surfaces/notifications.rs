//! The Notifications Surface (plan 7): the history, newest first, each with its actions and a
//! dismiss, then Do Not Disturb and Clear all. Arrow keys move a ring over every part of it and
//! Enter presses what the ring is on, so it works without a pointer. It says when another daemon
//! has the notifications, and when there are none.

use std::cmp::Reverse;
use std::time::{Instant, SystemTime};

use amane::{
    Button, Canvas, Cap, Center, Column, Cursor, Key, Line, Notification, Notifications, Padding,
    Rectangle, Row, Scroll, Service, Shape as _, Size, SpaceBetween, Stack, Start, Text, Urgency,
    Widget, children, shapes,
};

use crate::island::activity::Toast;
use crate::island::geometry;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::sources::notifications::{self, Daemon};
use crate::theme;
use crate::view::{self, Icon};

const INSET: f32 = 20.0;

// the content's width, which every row fills
const WIDTH: f32 = geometry::EXPANDED_MAX.width - 2.0 * INSET;

// clear of the queued badge in the body's top right corner
const BADGE: f32 = 36.0;

const HEADER: f32 = 20.0;
const FOOTER: f32 = 28.0;
const GAP: f32 = 10.0;

// what the cards scroll in, three without actions at a time
const LIST: f32 = geometry::EXPANDED_MAX.height - 2.0 * INSET - HEADER - FOOTER - 2.0 * GAP;

const CARD: f32 = 70.0;
const CARD_GAP: f32 = 6.0;
const PILL_PADDING: f32 = 10.0;
const CARD_RADIUS: f32 = 16.0;
const CARD_INSET: f32 = 10.0;

// the action row a card with actions adds under its text
const ACTIONS: f32 = 32.0;

// past this many a card shows only the first ones; the sender's own window has the rest
const MOST_ACTIONS: usize = 3;
const ACTION_WIDTH: f32 = 120.0;

const TILE: f32 = 36.0;
const TILE_RADIUS: f32 = 10.0;
const TILE_GAP: f32 = 10.0;

// a pressable target never smaller than plan 7's 24 px
const TARGET: f32 = 24.0;

// a control with nothing to do now
const DISABLED: f32 = 0.35;

// the keyboard focus, thick enough to see on any part
const RING: f32 = 2.0;

// pixels per wheel line
const WHEEL: f32 = 40.0;

/*
 * the ring and the scroll for one visit of the Surface (`IslandService::visit`), so every opening
 * starts at the top with the ring on the newest card. Written by input only, never by the view
 */
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Focus {
    visit: u64,

    // none is the first part: the newest card, or Do Not Disturb with none
    at: Option<At>,

    // the ring shows: from the start when opened from the keyboard, else from the first key
    shown: bool,

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
        if self.visit == visit {
            self.clone()
        } else {
            Focus {
                visit,
                shown: held,
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
    (content(cards) - LIST).max(0.0)
}

fn content(cards: &[Shape]) -> f32 {
    let heights: f32 = cards.iter().map(Shape::height).sum();

    heights + CARD_GAP * cards.len().saturating_sub(1) as f32
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

    offset.min(top).max(top + card.height() - LIST)
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

// what a card shows, read from Amane's list
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
    let ring = (open && focus.shown).then(|| At::place(focus.at, &shapes));
    let offset = focus.offset.clamp(0.0, most(&shapes));

    let list: Box<dyn Widget> = match daemon {
        Daemon::Conflict(other) => Box::new(state(
            Icon::Bell.crossed(28.0),
            &format!("{other} is the notification daemon"),
            "Stop it and restart Kanade to see notifications here",
        )),
        // Amane gets the bus name within a moment of starting, so nothing says anything yet
        Daemon::Starting => Box::new(Rectangle::new().width(WIDTH).height(LIST)),
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

    let shape = geometry::EXPANDED_MAX;

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
            .width(WIDTH)
            .gap(GAP),
        )
}

// the title and how many there are
fn header(count: usize) -> Row {
    let mut words = children![
        Text::new("Notifications")
            .size(16.0)
            .color(theme::FG)
            .weight(600)
    ];

    if count > 0 {
        words.push(Box::new(
            Text::new(count.to_string())
                .size(14.0)
                .color(theme::MUTED)
                .weight(600),
        ));
    }

    Row::new(words)
        .width(WIDTH - BADGE)
        .height(HEADER)
        .gap(8.0)
        .align(Center)
}

// an icon and what it means, in the middle of where the cards go
fn state(icon: Canvas, title: &str, detail: &str) -> Rectangle {
    let mut lines = children![
        icon,
        Text::new(title).size(14.0).color(theme::FG).weight(600),
    ];

    if !detail.is_empty() {
        lines.push(Box::new(
            Text::new(detail).size(12.0).color(theme::MUTED).weight(500),
        ));
    }

    Rectangle::new()
        .width(WIDTH)
        .height(LIST)
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
    .width(WIDTH)
    .gap(CARD_GAP);

    let scrolled = monitor.to_owned();

    let viewport = Rectangle::new()
        .width(WIDTH)
        .height(LIST)
        .clip()
        .align_child(Start, Start)
        .on_scroll(move |Scroll { y, .. }| wheel(&scrolled, y))
        .child(
            Rectangle::new()
                .width(WIDTH)
                .height(content(shapes))
                .align_child(Start, Start)
                .translate(0.0, -offset)
                .child(column),
        );

    let mut layers = children![viewport];

    let most = most(shapes);

    if most > 0.0 {
        let length = (LIST * LIST / content(shapes)).max(TARGET);
        let at = (LIST - length) * offset / most;

        layers.push(Box::new(
            Rectangle::new()
                .width(3.0)
                .height(length)
                .radius(1.5)
                .fill(theme::DOT)
                .translate(WIDTH + 7.0, at),
        ));
    }

    Stack::new(layers).width(WIDTH).height(LIST)
}

/*
 * the sender's picture, who sent it, the summary and the body, a dismiss in the corner, and the
 * actions under the text. Pressing it opens it. `ring` is the column the ring is on
 */
fn card(monitor: &str, card: &Card, ring: Option<usize>) -> Rectangle {
    let words = WIDTH - 2.0 * CARD_INSET - TILE - TILE_GAP - TARGET - 4.0;

    let sender = Text::new(&card.toast.app)
        .size(12.0)
        .color(theme::MUTED)
        .weight(500)
        .elide();

    // said in words before the sender, so it never rests on color alone or elides away
    let from: Box<dyn Widget> = if card.critical {
        Box::new(
            Row::new(children![
                Text::new("Critical")
                    .size(12.0)
                    .color(theme::RED)
                    .weight(600),
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
                .size(14.0)
                .color(theme::FG)
                .weight(600)
                .elide(),
        ),
    ];

    if !card.toast.body.is_empty() {
        lines.push(Box::new(
            Text::new(&card.toast.body)
                .size(13.0)
                .color(theme::MUTED)
                .weight(500)
                .elide(),
        ));
    }

    let top = Row::new(children![
        view::toast_tile(&card.toast, TILE, TILE_RADIUS),
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
        .width(WIDTH)
        .height(card.shape().height())
        .radius(CARD_RADIUS)
        .fill(theme::CARD)
        .padding(CARD_INSET)
        .align_child(Start, Start)
        .cursor(Cursor::Pointer)
        .on_click(move |button| {
            if button == Button::Left {
                click(&monitor, press.clone());
            }
        })
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
    let text = Text::new(label).size(12.0).color(theme::FG).weight(600);

    match text.width() {
        Size::Fixed(natural) if natural <= width => text,
        _ => text.elide(),
    }
}

// a round target with a cross, pressed to close the notification
fn dismiss(monitor: &str, id: u32, ring: bool) -> Rectangle {
    let monitor = monitor.to_owned();
    let side = 10.0;

    let cross = Canvas::new().width(side).height(side).shapes(shapes![
        Line::new()
            .from(0.0, 0.0)
            .to(side, side)
            .stroke(1.7, theme::MUTED)
            .cap(Cap::Round),
        Line::new()
            .from(side, 0.0)
            .to(0.0, side)
            .stroke(1.7, theme::MUTED)
            .cap(Cap::Round),
    ]);

    Rectangle::new()
        .width(TARGET)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(move |button| {
            if button == Button::Left {
                click(&monitor, Press::Dismiss(id));
            }
        })
        .child(cross)
        .border_if(ring)
}

// Do Not Disturb with a switch that says whether it is on, and Clear all while there is any
fn footer(monitor: &str, dnd: bool, any: bool, ring: Option<usize>) -> Row {
    let switch = {
        let (track, knob, at) = if dnd {
            (theme::FG, theme::BODY, 12.0)
        } else {
            (theme::DOT, theme::MUTED, 0.0)
        };

        Rectangle::new()
            .width(26.0)
            .height(14.0)
            .radius(7.0)
            .fill(track)
            .padding(2.0)
            .align_child(Start, Center)
            .child(
                Rectangle::new()
                    .width(10.0)
                    .height(10.0)
                    .radius(5.0)
                    .fill(knob)
                    .translate(at, 0.0),
            )
    };

    let label = Row::new(children![
        Icon::Moon.draw(16.0),
        Text::new("Do Not Disturb")
            .size(12.0)
            .color(theme::FG)
            .weight(600),
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
            .size(12.0)
            .color(theme::FG)
            .weight(600),
        (84.0, FOOTER),
        ring == Some(1),
        any.then_some(Press::Clear),
    );

    Row::new(children![dnd, clear])
        .width(WIDTH)
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
        pill.border(RING, theme::FG)
    } else {
        pill.border(1.0, theme::DOT)
    };

    let monitor = monitor.to_owned();

    match press {
        Some(press) => pill.cursor(Cursor::Pointer).on_click(move |button| {
            if button == Button::Left {
                click(&monitor, press.clone());
            }
        }),
        None => pill.opacity(DISABLED),
    }
}

trait Ring {
    fn border_if(self, ring: bool) -> Self;
}

impl Ring for Rectangle {
    fn border_if(self, ring: bool) -> Self {
        if ring {
            self.border(RING, theme::FG)
        } else {
            self
        }
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

    set(focus);

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
        let place = At::place(self.at, shapes);

        let press = match key {
            Key::Enter | Key::Space => Some(press(cards, shapes, place)),
            Key::Backspace => cards.get(place.row).map(|card| Press::Dismiss(card.id)),
            _ => None,
        };

        let moved = place.moved(key, shapes);

        if press.is_none() && moved.is_none() {
            return None;
        }

        if !self.shown {
            self.shown = true;

            return Some((self, None));
        }

        if let Some(moved) = moved {
            self.at = Some(At::on(moved, shapes));
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
    set(Focus {
        shown: false,
        ..focus(monitor)
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

            IslandService::write().set_dnd(!dnd, Instant::now());
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

    set(focus);
}

// this visit's focus, a fresh one when the stored one is from an earlier opening
fn focus(monitor: &str) -> Focus {
    let (visit, held) = {
        let island = IslandService::read();

        (island.visit(), island.held(monitor))
    };

    Focus::read().of(visit, held)
}

// a write wakes the window even when nothing changed, so only write a real change
fn set(focus: Focus) {
    if *Focus::read() != focus {
        *Focus::write() = focus;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: u32, actions: usize) -> Shape {
        Shape { id, actions }
    }

    fn place(row: usize, column: usize) -> Place {
        Place { row, column }
    }

    // three cards, the middle one with two actions
    fn three() -> Vec<Shape> {
        vec![card(3, 0), card(2, 2), card(1, 0)]
    }

    fn toast(id: u32, default: bool) -> Card {
        Card {
            id,
            toast: Toast::default(),
            critical: false,
            default,
            actions: vec![],
            received: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn a_hidden_ring_takes_the_first_press_and_the_second_presses() {
        let cards = vec![toast(7, true)];
        let shapes: Vec<Shape> = cards.iter().map(Card::shape).collect();

        for key in [Key::Enter, Key::Space, Key::Backspace] {
            let hidden = Focus::default().of(1, false);

            let (shown, first) = hidden.step(key, &cards, &shapes).unwrap();
            assert!(shown.shown);
            assert_eq!(first, None);

            let (_, second) = shown.step(key, &cards, &shapes).unwrap();
            assert!(second.is_some());
        }

        // with no cards the first press would otherwise flip Do Not Disturb
        let (shown, first) = Focus::default()
            .of(1, false)
            .step(Key::Enter, &[], &[])
            .unwrap();
        assert_eq!(first, None);
        assert_eq!(
            shown.step(Key::Enter, &[], &[]).unwrap().1,
            Some(Press::Dnd)
        );
    }

    #[test]
    fn a_ring_shown_from_the_keyboard_presses_at_once() {
        let cards = vec![toast(7, true)];
        let shapes: Vec<Shape> = cards.iter().map(Card::shape).collect();

        let (_, press) = Focus::default()
            .of(1, true)
            .step(Key::Enter, &cards, &shapes)
            .unwrap();
        assert_eq!(
            press,
            Some(Press::Open {
                id: 7,
                default: true
            })
        );
    }

    #[test]
    fn the_ring_starts_on_the_newest_card() {
        assert_eq!(At::place(None, &three()), place(0, 0));
        assert_eq!(At::place(None, &[]), place(0, 0));
        assert_eq!(At::on(place(0, 0), &[]), At::Footer(Footer::Dnd));
    }

    #[test]
    fn arrows_walk_every_part_and_stop_at_the_edges() {
        let cards = three();
        let walk = |from: Place, key| from.moved(key, &cards).unwrap();

        // body, two actions, dismiss
        assert_eq!(walk(place(1, 0), Key::Right), place(1, 1));
        assert_eq!(walk(place(1, 3), Key::Right), place(1, 3));
        assert_eq!(walk(place(1, 0), Key::Left), place(1, 0));
        assert_eq!(walk(place(0, 0), Key::Up), place(0, 0));
        assert_eq!(walk(place(3, 0), Key::Down), place(3, 0));

        // the footer is the last row: Do Not Disturb, Clear all
        assert_eq!(walk(place(2, 0), Key::Down), place(3, 0));
        assert_eq!(walk(place(3, 0), Key::Right), place(3, 1));

        assert_eq!(walk(place(3, 1), Key::Home), place(0, 0));
        assert_eq!(walk(place(0, 0), Key::End), place(2, 0));
        assert_eq!(place(0, 0).moved(Key::Character('a'), &cards), None);
    }

    #[test]
    fn up_and_down_keep_to_the_dismiss_and_leave_the_actions() {
        let cards = three();
        let walk = |from: Place, key| from.moved(key, &cards).unwrap();

        // a dismiss stays a dismiss, and Clear all is below it
        assert_eq!(walk(place(0, 1), Key::Down), place(1, 3));
        assert_eq!(walk(place(1, 3), Key::Down), place(2, 1));
        assert_eq!(walk(place(2, 1), Key::Down), place(3, 1));
        assert_eq!(walk(place(3, 1), Key::Up), place(2, 1));

        // an action goes to the next card's body, never to another action
        assert_eq!(walk(place(1, 1), Key::Down), place(2, 0));
        assert_eq!(walk(place(2, 0), Key::Up), place(1, 0));

        // with nothing to clear the footer is Do Not Disturb alone
        assert_eq!(walk(place(3, 0), Key::Up), place(2, 0));
        assert_eq!(place(0, 0).moved(Key::Down, &[]), Some(place(0, 0)));
        assert_eq!(place(0, 0).moved(Key::Right, &[]), Some(place(0, 0)));
    }

    #[test]
    fn tab_reads_on_and_comes_back() {
        let cards = vec![card(2, 1), card(1, 0)];
        let mut at = place(0, 0);
        let mut seen = vec![at];

        for _ in 0..7 {
            at = at.moved(Key::Tab, &cards).unwrap();
            seen.push(at);
        }

        assert_eq!(
            seen,
            [
                place(0, 0),
                place(0, 1),
                place(0, 2),
                place(1, 0),
                place(1, 1),
                place(2, 0),
                place(2, 1),
                place(0, 0)
            ]
        );
    }

    #[test]
    fn the_ring_follows_its_card_and_takes_the_next_when_it_goes() {
        let dismiss = At::Card {
            id: 2,
            row: 1,
            part: Part::Dismiss,
        };
        let action = At::Card {
            id: 2,
            row: 1,
            part: Part::Action(1),
        };

        assert_eq!(At::place(Some(dismiss), &three()), place(1, 3));
        assert_eq!(At::place(Some(action), &three()), place(1, 2));

        // a newer card above moves the card down, the ring with it
        let mut newer = three();
        newer.insert(0, card(4, 0));
        assert_eq!(At::place(Some(action), &newer), place(2, 2));

        // gone: the card in its row, on its dismiss, or its body for an action
        let gone = vec![card(3, 0), card(1, 1)];
        assert_eq!(At::place(Some(dismiss), &gone), place(1, 2));
        assert_eq!(At::place(Some(action), &gone), place(1, 0));

        // the last one gone: the footer
        assert_eq!(At::place(Some(dismiss), &[]), place(0, 0));
        assert_eq!(At::place(Some(At::Footer(Footer::Clear)), &[]), place(0, 0));
    }

    #[test]
    fn places_and_parts_round_trip() {
        let cards = three();

        for row in 0..=cards.len() {
            for column in 0..columns(&cards, row) {
                let at = At::on(place(row, column), &cards);

                assert_eq!(At::place(Some(at), &cards), place(row, column));
            }
        }
    }

    #[test]
    fn three_plain_cards_fit_and_more_scroll() {
        let fits = vec![card(3, 0), card(2, 0), card(1, 0)];
        assert!(content(&fits) <= LIST);
        assert_eq!(most(&fits), 0.0);

        let cards = vec![card(4, 0), card(3, 1), card(2, 0), card(1, 3)];
        assert_eq!(most(&cards), content(&cards) - LIST);
    }

    #[test]
    fn the_ring_scrolls_its_card_into_view_and_no_further() {
        let cards = vec![card(4, 0), card(3, 1), card(2, 0), card(1, 3)];

        // already whole
        assert_eq!(reveal(0.0, &cards, 0), 0.0);
        assert_eq!(reveal(0.0, &cards, 1), 0.0);

        // below: its bottom at the list's bottom
        let last = reveal(0.0, &cards, 3);
        assert_eq!(last, top(&cards, 3) + cards[3].height() - LIST);
        assert_eq!(last, most(&cards));

        // above: its top at the list's top
        assert_eq!(reveal(last, &cards, 0), 0.0);

        // the footer is always in view
        assert_eq!(reveal(last, &cards, 4), last);
    }

    #[test]
    fn a_new_visit_starts_over_with_the_ring_only_if_held() {
        let kept = Focus {
            visit: 1,
            at: Some(At::Footer(Footer::Clear)),
            shown: true,
            offset: 80.0,
        };

        assert_eq!(kept.of(1, false), kept);

        let fresh = kept.of(2, false);
        assert_eq!((fresh.at, fresh.shown, fresh.offset), (None, false, 0.0));
        assert!(kept.of(2, true).shown);
    }
}
