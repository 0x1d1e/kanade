//! Banners (ADR 0007): each notification as it arrives, top right on the focused output, below the
//! privacy cluster. UI over `notifications`, which keeps the bus, the history and DND; closing a
//! Banner leaves the notification in the history. Plain text only, and an action runs only on a
//! click, never from the keyboard.

mod stack;

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use amane::{
    Button, Center, Column, Cursor, Horizontal, Layer, LayerWindow, Margin, Monitor, Notification,
    Notifications, Padding, Rectangle, Row, Service, Size, Stack as Layers, Start, Text, Urgency,
    Vertical, Widget, Zone, children,
};

pub use stack::Banner;
use stack::{MOST, Stack};

use crate::icon::Icon;
use crate::island::geometry::{self, Rect};
use crate::modules;
use crate::shadow::{self, ShadowStyle};
use crate::sources::notifications;
use crate::theme::space::TARGET;
use crate::theme::{self, ThemeRoles, radius};
use crate::view;

const WIDTH: f32 = 360.0;
const GAP: f32 = 8.0;
const INSET: f32 = 12.0;

const TILE: f32 = 36.0;
const TILE_GAP: f32 = 10.0;

// past this many a Banner shows only the first ones; the sender's own window has the rest
const MOST_ACTIONS: usize = 3;
const ACTION_WIDTH: f32 = 120.0;
const PILL_PADDING: f32 = 10.0;

// the body wraps to at most this many lines, elided past them
const BODY_LINES: usize = 2;

// from the monitor's right edge, lined up with the privacy cluster
const MARGIN: f32 = 8.0;

// below the island's resting row, which the privacy cluster shares
const TOP: f32 = geometry::TOP + geometry::REST.height + GAP;

/*
 * a deadline change nudges listen(), which otherwise sleeps until the next deadline; one pending
 * nudge is enough, since listen() reads the deadline again when it wakes
 */
static NUDGE: LazyLock<(SyncSender<()>, Mutex<Receiver<()>>)> = LazyLock::new(|| {
    let (sender, receiver) = mpsc::sync_channel(1);

    (sender, Mutex::new(receiver))
});

/*
 * written by the notifications and niri sources, and by a Banner's own pointer input; a write
 * wakes every Banners window even when nothing changed, so every writer checks first
 */
pub struct Banners {
    stack: Stack,

    // from niri; none while unknown or without niri, which counts every monitor as focused
    focused_output: Option<String>,
}

impl Service for Banners {
    fn new() -> Self {
        Self {
            stack: Stack::default(),
            focused_output: None,
        }
    }

    // sleeps until a Banner's time runs out or a nudge, so no Banner showing never wakes
    fn listen() {
        let receiver = NUDGE.1.lock().unwrap_or_else(PoisonError::into_inner);

        loop {
            // no deadline waits forever, recv_timeout falls back to recv on overflow
            let wait = Self::read()
                .stack
                .deadline()
                .map_or(Duration::MAX, |deadline| {
                    deadline.saturating_duration_since(Instant::now())
                });

            if receiver.recv_timeout(wait) != Err(RecvTimeoutError::Timeout) {
                continue;
            }

            let now = Instant::now();

            if Self::read()
                .stack
                .deadline()
                .is_some_and(|deadline| deadline <= now)
            {
                Self::write().stack.expire(now);
            }
        }
    }
}

impl Banners {
    pub fn arrive(&mut self, banner: Banner, dnd: bool, now: Instant) {
        self.stack.arrive(banner, dnd, now);
        nudge();
    }

    pub fn contains(&self, id: u32) -> bool {
        self.stack.contains(id)
    }

    pub fn close(&mut self, id: u32, now: Instant) {
        self.stack.close(id, now);
        nudge();
    }

    // Banners follow the focus to its output; the pointer is not on the new one yet
    pub fn focus(&mut self, output: Option<String>, now: Instant) {
        self.focused_output = output;
        self.stack.hover(false, now);
        nudge();
    }

    // newest first, and only on the focused output
    fn shown_on(&self, monitor: &str) -> Vec<Banner> {
        let focused = self
            .focused_output
            .as_deref()
            .is_none_or(|focused| focused == monitor);

        if focused {
            self.stack.shown().cloned().collect()
        } else {
            Vec::new()
        }
    }
}

fn nudge() {
    // full means a nudge is already pending, which is all listen() needs
    let _ = NUDGE.0.try_send(());
}

impl Banner {
    pub fn of(notification: &Notification) -> Banner {
        Banner {
            id: notification.id(),
            toast: notifications::toast(notification),
            urgency: notification.urgency(),
            default: notification.has_default_action(),
            actions: notification
                .actions()
                .iter()
                .take(MOST_ACTIONS)
                .map(|action| (action.key().to_owned(), action.label().to_owned()))
                .collect(),
        }
    }
}

// DND came on: only Critical Banners stay
pub fn silence(now: Instant) {
    if modules::on("banners") && Banners::read().stack.silences() {
        Banners::write().stack.silence(now);
        nudge();
    }
}

// one window per monitor, shown on the focused one while any Banner shows, so it draws nothing else
pub fn window(monitor: &Monitor) -> LayerWindow {
    // reading subscribes this window to the Banners
    let shown = Banners::read().shown_on(&monitor.name);

    let roles = theme::roles();
    let reach = ShadowStyle::island().reach() as f32;

    // room for the shadow on every side but the right, where the monitor ends anyway
    let width = reach + WIDTH + MARGIN;
    let height =
        reach + (0..MOST).map(|_| tallest()).sum::<f32>() + GAP * (MOST - 1) as f32 + reach;

    let mut layers: Vec<Box<dyn Widget>> = Vec::new();
    let mut areas = Vec::new();
    let mut cards: Vec<Box<dyn Widget>> = Vec::new();
    let mut y = reach;

    for banner in &shown {
        let body = Rect {
            x: reach,
            y,
            width: WIDTH,
            height: card_height(banner),
        };

        shadow::draw(&mut layers, body, radius::CARD, ShadowStyle::island());
        areas.push(view::input_area(body));
        cards.push(Box::new(card(banner, body.height, &roles)));

        y += body.height + GAP;
    }

    layers.push(Box::new(
        Rectangle::new()
            .width(width)
            .height(height)
            .padding(Padding {
                top: reach,
                right: 0.0,
                bottom: 0.0,
                left: reach,
            })
            .align_child(Start, Start)
            .child(Column::new(cards).gap(GAP)),
    ));

    LayerWindow::new()
        .width(width)
        .height(height)
        .anchor_vertical(Vertical::Top)
        .anchor_horizontal(Horizontal::Right)
        .margin(Margin {
            top: (TOP - reach) as i32,
            ..Margin::default()
        })
        .layer(Layer::Top)
        .space(Zone::Ignore)
        .namespace("kanade-banners")
        .visible(!shown.is_empty())
        .input_region(areas)
        .child(
            // the input region is the Banners, so this is on while the pointer is on any of them
            Rectangle::new()
                .width(width)
                .height(height)
                .on_hover(hover)
                .child(Layers::new(layers).width(width).height(height)),
        )
}

fn hover(inside: bool) {
    let now = Instant::now();

    // its own statement, so the read is released before the write
    let changes = Banners::read().stack.hovered() != inside;

    if changes && Banners::write().stack.hover(inside, now) {
        nudge();
    }
}

fn close(id: u32) {
    let now = Instant::now();

    if Banners::read().contains(id) {
        Banners::write().close(id, now);
    }
}

// the sender's picture, who sent it, the summary, the body and a close, then the actions
fn card(banner: &Banner, height: f32, roles: &ThemeRoles) -> Rectangle {
    let words = words();

    let sender = Text::new(&banner.toast.app)
        .size(theme::text::LABEL_SMALL)
        .color(roles.on_surface_variant)
        .weight(theme::text::MEDIUM)
        .elide();

    // said in words before the sender, so it never rests on color alone or elides away
    let from: Box<dyn Widget> = if banner.critical() {
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

    let mut lines: Vec<Box<dyn Widget>> = vec![from, Box::new(summary(banner, roles))];

    if !banner.toast.body.is_empty() {
        lines.push(Box::new(body(banner, roles)));
    }

    let top = Row::new(children![
        view::toast_tile(&banner.toast, TILE, radius::TILE, roles),
        Column::new(lines).width(words).gap(1.0),
        dismiss(banner.id, roles),
    ])
    .gap(TILE_GAP)
    .align(Start);

    let mut parts = children![top];

    if !banner.actions.is_empty() {
        parts.push(Box::new(actions(banner, words + TARGET, roles)));
    }

    let id = banner.id;
    let default = banner.default;

    Rectangle::new()
        .width(WIDTH)
        .height(height)
        .radius(radius::CARD)
        .fill(roles.surface)
        .padding(INSET)
        .align_child(Start, Start)
        .cursor(Cursor::Pointer)
        .on_click(on_left(move || {
            // Amane runs the default action; without one the click only puts the Banner away
            if default {
                Notifications::click(id);
            }

            close(id);
        }))
        .child(Column::new(parts).gap(GAP))
}

fn summary(banner: &Banner, roles: &ThemeRoles) -> Text {
    Text::new(&banner.toast.summary)
        .size(theme::text::BODY)
        .color(roles.on_surface)
        .weight(theme::text::SEMIBOLD)
        .elide()
}

fn body(banner: &Banner, roles: &ThemeRoles) -> Text {
    let text = || {
        Text::new(&banner.toast.body)
            .size(theme::text::LABEL)
            .color(roles.on_surface_variant)
            .weight(theme::text::MEDIUM)
            .elide()
    };

    // a wrapped Text is as tall as its most lines, so one that fits a line never wraps
    let wrapped = text().wrap().max_lines(BODY_LINES);

    if wrapped.height_in(words()) > tall(&text()) {
        wrapped
    } else {
        text()
    }
}

// what the text column takes, between the picture and the close
fn words() -> f32 {
    WIDTH - 2.0 * INSET - TILE - TARGET - 2.0 * TILE_GAP
}

fn card_height(banner: &Banner) -> f32 {
    let roles = theme::ISLAND;

    // the sender line is as tall whether it says Critical or not
    let sender = Text::new(&banner.toast.app)
        .size(theme::text::LABEL_SMALL)
        .weight(theme::text::MEDIUM);

    let mut text = tall(&sender) + 1.0 + tall(&summary(banner, &roles));

    if !banner.toast.body.is_empty() {
        text += 1.0 + tall(&body(banner, &roles));
    }

    let actions = if banner.actions.is_empty() {
        0.0
    } else {
        GAP + TARGET
    };

    2.0 * INSET + text.max(TILE) + actions
}

// how tall a Text lays out in the text column
fn tall(text: &Text) -> f32 {
    match text.height() {
        Size::Fixed(height) => height,
        _ => text.height_in(words()),
    }
}

// the tallest a Banner gets: a body of every line, and actions
fn tallest() -> f32 {
    let mut banner = Banner {
        id: 0,
        toast: Default::default(),
        urgency: Urgency::Normal,
        default: false,
        actions: vec![(String::new(), String::new())],
    };
    banner.toast.body = "Ag ".repeat(400);

    card_height(&banner)
}

// a pill per action, as wide as fits, under the text
fn actions(banner: &Banner, width: f32, roles: &ThemeRoles) -> Row {
    let count = banner.actions.len() as f32;
    let each = ((width - 6.0 * (count - 1.0)) / count).min(ACTION_WIDTH);
    let id = banner.id;

    let pills = banner
        .actions
        .iter()
        .map(|(key, label)| {
            let key = key.clone();

            Box::new(
                Rectangle::new()
                    .width(each)
                    .height(TARGET)
                    .radius(TARGET / 2.0)
                    .padding(Padding {
                        top: 0.0,
                        right: PILL_PADDING,
                        bottom: 0.0,
                        left: PILL_PADDING,
                    })
                    .border(1.0, roles.surface_container_high)
                    .align_child(Center, Center)
                    .cursor(Cursor::Pointer)
                    .on_click(on_left(move || {
                        // Amane closes the notification unless its sender keeps it
                        Notifications::invoke(id, &key);
                        close(id);
                    }))
                    .child(label_in(label, each - 2.0 * PILL_PADDING, roles)),
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
fn label_in(label: &str, width: f32, roles: &ThemeRoles) -> Text {
    let text = Text::new(label)
        .size(theme::text::LABEL_SMALL)
        .color(roles.on_surface)
        .weight(theme::text::SEMIBOLD);

    match text.width() {
        Size::Fixed(natural) if natural <= width => text,
        _ => text.elide(),
    }
}

// a round target with a cross; it puts the Banner away, the notification stays in the history
fn dismiss(id: u32, roles: &ThemeRoles) -> Rectangle {
    Rectangle::new()
        .width(TARGET)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(on_left(move || close(id)))
        .child(Icon::Dismiss.on(20.0, roles.on_surface_variant))
}

// only the left button acts, so a stray press of another never runs an action
fn on_left(press: impl Fn() + 'static) -> impl Fn(Button) + 'static {
    move |button| {
        if button == Button::Left {
            press();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::activity::Toast;

    fn banner(id: u32) -> Banner {
        Banner {
            id,
            toast: Toast::default(),
            urgency: Urgency::Normal,
            default: false,
            actions: Vec::new(),
        }
    }

    fn ids(banners: &Banners, monitor: &str) -> Vec<u32> {
        banners
            .shown_on(monitor)
            .iter()
            .map(|banner| banner.id)
            .collect()
    }

    #[test]
    fn banners_show_only_on_the_focused_output_and_follow_the_focus() {
        let now = Instant::now();
        let mut banners = Banners::new();

        banners.arrive(banner(1), false, now);

        // focus unknown counts every output as focused
        assert_eq!(ids(&banners, "DP-1"), [1]);
        assert_eq!(ids(&banners, "eDP-1"), [1]);

        banners.focus(Some("eDP-1".into()), now);
        assert_eq!(ids(&banners, "eDP-1"), [1]);
        assert!(ids(&banners, "DP-1").is_empty());

        banners.focus(Some("DP-1".into()), now);
        assert_eq!(ids(&banners, "DP-1"), [1]);
        assert!(ids(&banners, "eDP-1").is_empty());
    }

    #[test]
    fn the_focus_moving_away_resumes_the_timers() {
        let now = Instant::now();
        let mut banners = Banners::new();

        banners.focus(Some("eDP-1".into()), now);
        banners.arrive(banner(1), false, now);
        assert!(banners.stack.hover(true, now));

        banners.focus(Some("DP-1".into()), now);
        assert!(!banners.stack.hovered());
    }
}
