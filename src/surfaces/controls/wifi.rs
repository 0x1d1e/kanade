//! The Controls Surface's Wi-Fi sub-surface: the networks in range, strongest after the joined and
//! saved ones, and the password a secured one asks for. Each network says how it is joined, or why
//! its last join failed. A password shows as dots only.

use amane::{
    Center, Column, End, Padding, Rectangle, Row, Size, Stack, Start, Text, Widget, children,
};

use super::WIDTH;
use super::focus::{Act, At};
use super::list::{self, GAP, HEADER, ICON, ICON_GAP};
use crate::icon::Icon;
use crate::sources::system::Radio;
use crate::sources::wifi::{self, Failure, Join, Link, Networks, Secret, Security};
use crate::theme::space::TARGET;
use crate::theme::{self, DISABLED};

const FIELD: f32 = 44.0;
const FIELD_INSET: f32 = 16.0;
const DOTS: f32 = theme::text::TITLE;
const CARET: f32 = 1.5;

// the networks it lists, by name, one a row: none while the radio is not on
pub fn rows(radio: Radio, networks: &Networks) -> Vec<Vec<(At, f32)>> {
    if radio != Radio::On {
        return Vec::new();
    }

    networks
        .list
        .iter()
        .map(|network| vec![(At::Network(network.ssid.clone()), 0.5)])
        .collect()
}

/*
 * the header, then the networks scrolled `offset` down. `ring` is what the ring is on, none while
 * it hides
 */
pub fn networks(
    radio: Radio,
    networks: &Networks,
    join: &Join,
    offset: f32,
    ring: Option<&At>,
) -> Column {
    let list: Box<dyn Widget> = if radio != Radio::On {
        Box::new(list::state(
            Icon::Wifi,
            "Wi-Fi is off",
            "Turn it on to see networks",
        ))
    } else if networks.list.is_empty() {
        Box::new(list::state(Icon::Wifi, "Looking for networks", ""))
    } else {
        Box::new(list::list(networks.list.len(), offset, |index| {
            let network = &networks.list[index];
            let ringed = matches!(ring, Some(At::Network(ssid)) if *ssid == network.ssid);

            Box::new(row(network, join, ringed))
        }))
    };

    Column::new(vec![
        Box::new(list::header("Wi-Fi", radio, ring)) as Box<dyn Widget>,
        list,
    ])
    .width(WIDTH)
    .gap(GAP)
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
    let at = At::Network(network.ssid.clone());

    // the full fan faint behind the bars it has
    let signal = Stack::new(children![
        Icon::Wifi.on(ICON, theme::ISLAND.surface_container_high),
        Icon::Signal(network.bars).on(ICON, theme::ISLAND.on_surface),
    ])
    .width(ICON)
    .height(ICON);

    let mut trailing: Vec<Box<dyn Widget>> = Vec::new();

    if network.security.locked() {
        trailing.push(Box::new(
            Icon::Lock.on(16.0, theme::ISLAND.on_surface_variant),
        ));
    }

    if joined {
        trailing.push(Box::new(list::pill(
            list::label("Disconnect"),
            96.0,
            ring,
            Some(Act::Press(at.clone())),
        )));
    }

    let unsupported = network.security == Security::Unsupported;
    let press = (!joined && !unsupported).then_some(at);

    let row = list::row(
        Box::new(signal),
        &network.ssid,
        status(network, join),
        trailing,
        ring && !joined,
        press,
    );

    if unsupported {
        row.opacity(DISABLED)
    } else {
        row
    }
}

/*
 * joining `ssid`: the password as dots after the caret's place, what went wrong or what it takes,
 * then Cancel and Join, which waits for one that fits
 */
pub fn password(ssid: &str, secret: &Secret, failure: Option<Failure>) -> Column {
    let title = Row::new(children![
        list::back(false),
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

    let buttons = Row::new(children![
        list::pill(
            list::label("Cancel"),
            84.0,
            false,
            Some(Act::Press(At::Back))
        ),
        list::pill(
            list::label("Join"),
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
