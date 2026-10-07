//! The Controls Surface's Wi-Fi sub-surface: the networks in range, strongest after the joined and
//! saved ones, and the password a secured one asks for. Each network says how it is joined, or why
//! its last join failed. A password shows as dots only.

use amane::{
    Center, Column, Cursor, End, Network, Padding, Parent, Rectangle, Row, Scroll, Size,
    SpaceBetween, Stack, Start, Text, Widget, children,
};

use super::WIDTH;
use super::focus::{Act, At, Focus};
use crate::icon::Icon;
use crate::island::geometry;
use crate::sources::system::Radio;
use crate::sources::wifi::{self, Failure, Join, Link, Networks, Secret, Security};
use crate::surfaces::{RING, Ring};
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, DISABLED, radius};

const HEADER: f32 = TARGET;
const GAP: f32 = 14.0;

// where the networks go
const LIST: f32 = geometry::CONTROLS.height - 2.0 * INSET - HEADER - GAP;

const ROWS: usize = 4;
const ROW_GAP: f32 = 6.0;
const ROW: f32 = (LIST - (ROWS - 1) as f32 * ROW_GAP) / ROWS as f32;
const ROW_INSET: f32 = 12.0;

const ICON: f32 = 20.0;
const ICON_GAP: f32 = 12.0;

// the radio's switch in the header
const TOGGLE: f32 = 40.0;
const TOGGLE_KNOB: f32 = 18.0;
const TOGGLE_INSET: f32 = (TARGET - TOGGLE_KNOB) / 2.0;
const HALO: f32 = 4.0;

const FIELD: f32 = 44.0;
const FIELD_INSET: f32 = 16.0;
const DOTS: f32 = theme::text::TITLE;
const CARET: f32 = 1.5;

const PILL: f32 = 32.0;
const PILL_PADDING: f32 = 14.0;

// pixels per wheel line
const WHEEL: f32 = 40.0;

// how far `count` networks can scroll: none while they fit
pub fn most(count: usize) -> f32 {
    (content(count) - LIST).max(0.0)
}

fn content(count: usize) -> f32 {
    count as f32 * ROW + count.saturating_sub(1) as f32 * ROW_GAP
}

fn top(row: usize) -> f32 {
    row as f32 * (ROW + ROW_GAP)
}

// the least scroll from `offset` that shows network `row` of `count` whole
pub fn reveal(offset: f32, row: usize, count: usize) -> f32 {
    let top = top(row);

    offset
        .clamp(0.0, most(count))
        .min(top)
        .max(top + ROW - LIST)
}

/*
 * where the networks show from: `offset`, moved just enough to show the ringed one whole, which
 * moves in the order as networks come, go and change while the ring stays on it
 */
pub fn scrolled(offset: f32, networks: &[wifi::Network], ring: Option<&At>) -> f32 {
    let count = networks.len();
    let ringed = networks
        .iter()
        .position(|network| matches!(ring, Some(At::Network(ssid)) if *ssid == network.ssid));

    match ringed {
        Some(row) => reveal(offset, row, count),
        None => offset.clamp(0.0, most(count)),
    }
}

// the networks any part of shows at `offset`, as a range, so only these are built
fn shown(offset: f32, count: usize) -> (usize, usize) {
    let step = ROW + ROW_GAP;
    let first = (offset / step).floor() as usize;
    let end = ((offset + LIST) / step).ceil() as usize;

    (first.min(count), end.min(count))
}

/*
 * the back chevron, the name and the radio's switch, then the networks. `ring` is what the ring is
 * on, none while it hides
 */
pub fn networks(
    radio: Radio,
    networks: &Networks,
    join: &Join,
    focus: &Focus,
    ring: Option<&At>,
) -> Column {
    let list: Box<dyn Widget> = if radio != Radio::On {
        Box::new(state("Wi-Fi is off", "Turn it on to see networks"))
    } else if networks.list.is_empty() {
        Box::new(state("Looking for networks", ""))
    } else {
        Box::new(list(networks, join, focus.offset, ring))
    };

    let title = Row::new(children![
        back(ring == Some(&At::Back)),
        Text::new("Wi-Fi")
            .size(theme::text::TITLE)
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::SEMIBOLD),
    ])
    .gap(8.0)
    .align(Center);

    let header = Row::new(children![title, toggle(radio, ring == Some(&At::Radio))])
        .width(WIDTH)
        .height(HEADER)
        .justify(SpaceBetween)
        .align(Center);

    Column::new(vec![Box::new(header) as Box<dyn Widget>, list])
        .width(WIDTH)
        .gap(GAP)
}

// pressing it goes back a level, as Escape does
fn back(ring: bool) -> Rectangle {
    Rectangle::new()
        .width(TARGET)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .border_if(ring)
        .on_click(super::super::on_left(|| {
            super::click(Act::Press(At::Back));
        }))
        .child(Icon::Back.draw(18.0))
}

// the radio on or off; a missing one has no sub-surface to show this in
fn toggle(radio: Radio, ring: bool) -> Rectangle {
    let on = radio == Radio::On;

    let (track, knob, at) = if on {
        (
            theme::ISLAND.primary,
            theme::ISLAND.on_primary,
            TOGGLE - TARGET,
        )
    } else {
        (
            theme::ISLAND.surface_container_high,
            theme::ISLAND.on_surface_variant,
            0.0,
        )
    };

    let toggle = Rectangle::new()
        .width(TOGGLE)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .fill(track)
        .padding(TOGGLE_INSET)
        .align_child(Start, Center)
        .child(
            Rectangle::new()
                .width(TOGGLE_KNOB)
                .height(TOGGLE_KNOB)
                .radius(TOGGLE_KNOB / 2.0)
                .fill(knob)
                .translate(at, 0.0),
        );

    // the ring around it, as it would not show on a filled track
    let toggle = Rectangle::new()
        .width(TOGGLE + 2.0 * HALO)
        .height(TARGET + 2.0 * HALO)
        .radius(TARGET / 2.0 + HALO)
        .align_child(Center, Center)
        .border_if(ring)
        .child(toggle);

    if radio == Radio::Missing {
        return toggle.opacity(DISABLED);
    }

    toggle
        .cursor(Cursor::Pointer)
        .on_click(super::super::on_left(move || {
            Network::set_wifi(!on);
        }))
}

// what it means, in the middle of where the networks go
fn state(title: &str, detail: &str) -> Rectangle {
    let mut lines = children![
        Icon::Wifi.on(28.0, theme::ISLAND.on_surface_variant),
        Text::new(title)
            .size(theme::text::BODY)
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::SEMIBOLD),
    ];

    if !detail.is_empty() {
        lines.push(Box::new(
            Text::new(detail)
                .size(theme::text::LABEL_SMALL)
                .color(theme::ISLAND.on_surface_variant)
                .weight(theme::text::MEDIUM),
        ));
    }

    Rectangle::new()
        .width(WIDTH)
        .height(LIST)
        .align_child(Center, Center)
        .child(Column::new(lines).gap(6.0).align(Center))
}

/*
 * the networks scrolled `offset` down, clipped to the list; a thumb in the inset says where while
 * they do not all fit
 */
fn list(networks: &Networks, join: &Join, offset: f32, ring: Option<&At>) -> Stack {
    let count = networks.list.len();
    let offset = scrolled(offset, &networks.list, ring);
    let (first, end) = shown(offset, count);

    let column = Column::new(
        networks.list[first..end]
            .iter()
            .map(|network| {
                let ringed = matches!(ring, Some(At::Network(ssid)) if *ssid == network.ssid);

                Box::new(row(network, join, ringed)) as Box<dyn Widget>
            })
            .collect(),
    )
    .width(WIDTH)
    .gap(ROW_GAP);

    let viewport = Rectangle::new()
        .width(WIDTH)
        .height(LIST)
        .clip()
        .align_child(Start, Start)
        .on_scroll(|Scroll { y, .. }| super::scroll(y * WHEEL))
        .child(
            Rectangle::new()
                .width(WIDTH)
                .height(content(end - first))
                .align_child(Start, Start)
                .translate(0.0, top(first) - offset)
                .child(column),
        );

    let mut layers = children![viewport];

    let most = most(count);

    if most > 0.0 {
        let length = (LIST * LIST / content(count)).max(TARGET);
        let at = (LIST - length) * offset / most;

        layers.push(Box::new(
            Rectangle::new()
                .width(3.0)
                .height(length)
                .radius(radius::HAIRLINE)
                .fill(theme::ISLAND.surface_container_high)
                .translate(WIDTH + 7.0, at),
        ));
    }

    Stack::new(layers).width(WIDTH).height(LIST)
}

// what a network's row says under its name, and whether that is an error
fn status(network: &wifi::Network, join: &Join) -> Option<(&'static str, bool)> {
    if network.link == Link::Joined {
        return Some(("Connected", false));
    }

    if network.link == Link::Joining || join.joining(&network.ssid) {
        return Some(("Connecting\u{2026}", false));
    }

    match join.failed(&network.ssid) {
        Some(Failure::WrongPassword) => return Some(("Wrong password", true)),
        Some(Failure::Other) => return Some(("Couldn\u{2019}t connect", true)),
        None => {}
    }

    if network.security == Security::Unsupported {
        Some(("Not supported", false))
    } else if network.profile.is_some() {
        Some(("Saved", false))
    } else {
        None
    }
}

/*
 * its signal, name and status, a lock while it needs a password; the joined one has a Disconnect
 * pill, which the ring goes on instead and which alone leaves it. Pressing another joins it
 */
fn row(network: &wifi::Network, join: &Join, ring: bool) -> Rectangle {
    let joined = network.link == Link::Joined;

    // the full fan faint behind the bars it has
    let signal = Stack::new(children![
        Icon::Wifi.on(ICON, theme::ISLAND.surface_container_high),
        Icon::Signal(network.bars).on(ICON, theme::ISLAND.on_surface),
    ])
    .width(ICON)
    .height(ICON);

    let mut lines = children![
        Text::new(&network.ssid)
            .size(theme::text::BODY)
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::SEMIBOLD)
            .elide()
    ];

    if let Some((status, error)) = status(network, join) {
        lines.push(Box::new(
            Text::new(status)
                .size(theme::text::LABEL_SMALL)
                .color(if error {
                    theme::SEMANTIC.critical
                } else {
                    theme::ISLAND.on_surface_variant
                })
                .weight(theme::text::MEDIUM)
                .elide(),
        ));
    }

    let mut trailing: Vec<Box<dyn Widget>> = Vec::new();

    if network.security.locked() {
        trailing.push(Box::new(
            Icon::Lock.on(16.0, theme::ISLAND.on_surface_variant),
        ));
    }

    if joined {
        trailing.push(Box::new(pill(
            Text::new("Disconnect")
                .size(theme::text::LABEL_SMALL)
                .color(theme::ISLAND.on_surface)
                .weight(theme::text::SEMIBOLD),
            96.0,
            ring,
            Some(Act::Press(At::Network(network.ssid.clone()))),
        )));
    }

    let row = Rectangle::new()
        .width(WIDTH)
        .height(ROW)
        .radius(radius::ROW)
        .padding(Padding {
            top: 0.0,
            right: ROW_INSET,
            bottom: 0.0,
            left: ROW_INSET,
        })
        .align_child(Start, Center)
        .child(
            Row::new(children![
                signal,
                Column::new(lines).width(Parent).gap(1.0),
                Row::new(trailing).gap(ICON_GAP).align(Center),
            ])
            .width(Parent)
            .gap(ICON_GAP)
            .align(Center),
        );

    if joined {
        return row;
    }

    let row = if ring {
        row.fill(theme::ISLAND.surface_container).border_if(true)
    } else {
        row
    };

    if network.security == Security::Unsupported {
        return row.opacity(DISABLED);
    }

    let ssid = network.ssid.clone();

    row.cursor(Cursor::Pointer)
        .on_click(super::super::on_left(move || {
            super::click(Act::Press(At::Network(ssid.clone())));
        }))
}

/*
 * joining `ssid`: the password as dots after the caret's place, what went wrong or what it takes,
 * then Cancel and Join, which waits for one that fits
 */
pub fn password(ssid: &str, secret: &Secret, failure: Option<Failure>) -> Column {
    let title = Row::new(children![
        back(false),
        Text::new(format!("Join \u{201c}{ssid}\u{201d}"))
            .size(theme::text::TITLE)
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::SEMIBOLD)
            .elide(),
    ])
    .width(WIDTH)
    .height(HEADER)
    .gap(8.0)
    .align(Center);

    let (note, error) = match failure {
        Some(Failure::WrongPassword) => ("Wrong password, try again", true),
        Some(Failure::Other) => ("Couldn\u{2019}t connect, try again", true),
        None => ("8 to 63 characters", false),
    };

    let note = Text::new(note)
        .size(theme::text::LABEL_SMALL)
        .color(if error {
            theme::SEMANTIC.critical
        } else {
            theme::ISLAND.on_surface_variant
        })
        .weight(theme::text::MEDIUM);

    let label = |text: &str| {
        Text::new(text)
            .size(theme::text::LABEL_SMALL)
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::SEMIBOLD)
    };

    let buttons = Row::new(children![
        pill(label("Cancel"), 84.0, false, Some(Act::Press(At::Back))),
        pill(
            label("Join"),
            84.0,
            false,
            secret.fits().then_some(Act::Join)
        ),
    ])
    .width(WIDTH)
    .gap(8.0)
    .justify(End);

    Column::new(children![title, field(secret), note, buttons])
        .width(WIDTH)
        .gap(GAP)
}

// a dot per character typed, the last ones while they do not fit, then the caret
fn field(secret: &Secret) -> Rectangle {
    let room = WIDTH - 2.0 * FIELD_INSET - TARGET - ICON_GAP - CARET;

    let caret = Rectangle::new()
        .width(CARET)
        .height(DOTS + 4.0)
        .fill(theme::ISLAND.on_surface);

    let dots = |count: usize| {
        Text::new("\u{2022}".repeat(count))
            .size(DOTS)
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::MEDIUM)
    };
    let fits = |text: &Text| matches!(text.width(), Size::Fixed(natural) if natural <= room);

    let typed: Box<dyn Widget> = if secret.is_empty() {
        Box::new(
            Row::new(children![
                caret,
                Text::new("Password")
                    .size(DOTS)
                    .color(theme::ISLAND.on_surface_variant)
                    .weight(theme::text::MEDIUM),
            ])
            .align(Center),
        )
    } else {
        let shown = (0..=secret.len())
            .rev()
            .map(dots)
            .find(fits)
            .unwrap_or_else(|| dots(0));

        Box::new(Row::new(children![shown, caret]).align(Center))
    };

    Rectangle::new()
        .width(WIDTH)
        .height(FIELD)
        .radius(FIELD / 2.0)
        .fill(theme::ISLAND.surface_container)
        .padding(Padding {
            top: 0.0,
            right: FIELD_INSET,
            bottom: 0.0,
            left: FIELD_INSET,
        })
        .align_child(Start, Center)
        .child(
            Row::new(vec![
                Box::new(Icon::Lock.on(TARGET, theme::ISLAND.on_surface_variant))
                    as Box<dyn Widget>,
                typed,
            ])
            .gap(ICON_GAP)
            .align(Center),
        )
}

// outlined, the ring in place of the outline; none to press leaves it faded and not pressable
fn pill(child: Text, width: f32, ring: bool, act: Option<Act>) -> Rectangle {
    let pill = Rectangle::new()
        .width(width)
        .height(PILL)
        .radius(PILL / 2.0)
        .padding(Padding {
            top: 0.0,
            right: PILL_PADDING,
            bottom: 0.0,
            left: PILL_PADDING,
        })
        .align_child(Center, Center)
        .child(child);

    let pill = if ring {
        pill.border(RING, theme::ISLAND.on_surface)
    } else {
        pill.border(1.0, theme::ISLAND.surface_container_high)
    };

    match act {
        Some(act) => pill
            .cursor(Cursor::Pointer)
            .on_click(super::super::on_left(move || {
                super::click(act.clone());
            })),
        None => pill.opacity(DISABLED),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_networks_fit_and_more_scroll() {
        assert_eq!(most(ROWS), 0.0);
        assert!(most(ROWS + 1) > 0.0);
    }

    #[test]
    fn revealing_a_network_scrolls_the_least() {
        // already showing, nothing moves
        assert_eq!(reveal(0.0, 1, 10), 0.0);

        // below, it comes up to the bottom edge
        assert_eq!(reveal(0.0, ROWS, 10), top(ROWS) + ROW - LIST);

        // above, it comes down to the top edge
        assert_eq!(reveal(most(10), 0, 10), 0.0);
    }

    #[test]
    fn the_list_follows_the_ringed_network_as_it_moves() {
        let networks: Vec<wifi::Network> = (0..10)
            .map(|n| wifi::Network {
                ssid: format!("n{n}"),
                bars: 2,
                security: Security::Psk,
                link: Link::None,
                profile: None,
                access_point: String::new(),
            })
            .collect();
        let ring = At::Network(String::from("n7"));

        // the ring on the eighth, now out of sight, brings it up
        assert_eq!(scrolled(0.0, &networks, Some(&ring)), reveal(0.0, 7, 10));

        // with no ring, or none on a network, the list stays where it was
        assert_eq!(scrolled(40.0, &networks, None), 40.0);
        assert_eq!(scrolled(40.0, &networks, Some(&At::Back)), 40.0);
        assert_eq!(scrolled(1e6, &networks, None), most(10));
    }
}
