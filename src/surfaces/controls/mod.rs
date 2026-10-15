//! The Controls Surface: tiles for Wi-Fi, Bluetooth, caffeine, Do Not Disturb, the night light,
//! Focus and, where the battery supports it, its charge limit, as `controls.tiles` orders and sizes
//! them, the speaker's volume and the screen's brightness as levels, and the power profile.
//! Each is one press or one drag, never a menu: it is not a settings app. What the machine does not
//! have, or whose daemon is not running, keeps its place, faded, says so, and does nothing. Its
//! header names the apps the privacy cluster stands for, and ends a PipeWire screen cast with Stop sharing.
//!
//! The Wi-Fi and Bluetooth switches' tiles each open a sub-surface in its place, their knobs still
//! switching them: the networks in range, to join or leave, and the password a secured one asks
//! for; or the Bluetooth devices, to pair, connect, disconnect or forget. The speaker's
//! level ends in a chevron to the Audio sub-surface: the devices to play on and record from, the
//! microphone's level and mute, and each app's level. A sub-surface goes back a level by its chevron or Escape. Every target
//! is a key away, a ring on the one the arrows reach (`focus`). The pencil in the header arranges
//! the tiles (`arrange`): move, resize, hide or bring back, saved by Done.

use std::collections::BTreeSet;
use std::time::Instant;

use crate::sources::brightness::Brightness;
use crate::sources::pulse::Audio;
use kanade_runtime::service::Service;
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Color, Column, Cursor, End, Key, Padding, Parent, Rectangle, Row, Scroll, Start, Text,
    Widget, children,
};

use self::focus::{Act, At, Focus, Subsurface};
use super::slider::Slider;
use super::{Outline, store};
use crate::cluster;
use crate::config::{self, NightMode};
use crate::icon::Icon;
use crate::island::activity::Uplink;
use crate::island::geometry;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::later::Later;
use crate::modules;
use crate::sources::audio::{self as sound, Direction, Mixer, Switching};
use crate::sources::battery::Remaining;
use crate::sources::bluetooth::{self as bluez, Adapter, Prompt, Request};
use crate::sources::caffeine;
use crate::sources::charge_limit::{self, ChargeLimit};
use crate::sources::focus as quiet;
use crate::sources::network::{Connectivity, set_wifi};
use crate::sources::night_light::{self, Shade};
use crate::sources::niri::Target;
use crate::sources::notifications::{self, timed};
use crate::sources::power::{self, Profile, Profiles};
use crate::sources::privacy::{self, Privacy};
use crate::sources::system::Radio;
use crate::sources::wifi::{self as wireless, Failure, Join, Link, Networks, Security};
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, DISABLED};
use crate::tiles::{self, Placed, Size, Tile};
use crate::view::{bar, rise};

mod arrange;
mod audio;
mod bluetooth;
mod focus;
pub(super) mod list;
mod wifi;

// the content's width, which every row fills
const WIDTH: f32 = geometry::CONTROLS.width - 2.0 * INSET;

const HEADER: f32 = 20.0;
const GAP: f32 = 8.0;

// the Stop sharing pill
const STOP: f32 = 96.0;

// a switch, round ends concentric with its knob
const SWITCH: f32 = 48.0;
const SWITCH_GAP: f32 = 8.0;
const KNOB: f32 = 32.0;
const KNOB_INSET: f32 = (SWITCH - KNOB) / 2.0;

// room around the knob for the ring, which would not show on a filled knob
const HALO: f32 = 4.0;

const ICON_GAP: f32 = 10.0;
const LEVEL_GAP: f32 = 8.0;

// room for "100"
const NUMBER: f32 = 30.0;

// the power profiles' track, their segments a target high inside it
const TRACK: f32 = 28.0;
const TRACK_INSET: f32 = (TRACK - TARGET) / 2.0;

/*
 * the top level, or the sub-surface entered from it. `visit`, `held` and `dnd` are the view's own
 * read of IslandService, so this never reads it again; `dnd` is none while the notifications Module
 * is off, since nothing heeds it then. `open` says the Surface is open rather than fading out, so
 * only then does the ring show
 */
pub fn surface(open: bool, visit: u64, held: bool, dnd: Option<bool>) -> Rectangle {
    let shape = geometry::CONTROLS;
    let focus = Focus::read().of(visit, held);

    let grid = targets(&focus);
    let focus = settled(focus, &grid);
    let ring = open.then(|| focus.ring(&grid)).flatten();

    let content = match &focus.sub {
        None => Column::new(children![
            rise(0, header(ring.as_ref())),
            rise(1, switches(dnd, ring.as_ref())),
            rise(2, levels(ring.as_ref())),
            rise(3, profiles(ring.as_ref()))
        ])
        .width(WIDTH)
        .gap(GAP),
        Some(Subsurface::Wifi) => wifi::networks(
            Connectivity::read().wifi,
            &Networks::read(),
            &Join::read(),
            focus.offset,
            ring.as_ref(),
        ),
        Some(Subsurface::Password { ssid, secret }) => {
            wifi::password(ssid, secret, Join::read().failed(ssid))
        }
        Some(Subsurface::Bluetooth) => bluetooth::devices(
            &Adapter::read(),
            &Request::read(),
            &Prompt::read(),
            focus.offset,
            ring.as_ref(),
        ),
        Some(Subsurface::Audio) => {
            audio::sound(&mixer(), *Switching::read(), focus.offset, ring.as_ref())
        }
        Some(Subsurface::Arrange { draft, chosen }) => arrange::view(draft, *chosen, ring.as_ref()),
    };

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .child(content)
}

// every target of `focus`'s level, read now
fn targets(focus: &Focus) -> Vec<Vec<(At, f32)>> {
    focus.grid(rows(focus.sub.as_ref()), sharing().is_some())
}

/*
 * the sessions of the casts the header can stop, none while it does not name casts or no PipeWire
 * one runs; a wlr-screencopy cast, like a recording or Kanade's own capture, cannot be stopped
 */
fn sharing() -> Option<BTreeSet<u64>> {
    let named = modules::on("privacy") && cluster::on();

    named
        .then(|| Privacy::read().sessions.clone())
        .filter(|sessions| !sessions.is_empty())
}

/*
 * whether a tile has a place now: Focus needs its Module, the charge limit a battery that supports
 * it. The rest keep their place, faded, when they cannot act
 */
fn visible(tile: Tile) -> bool {
    match tile {
        Tile::Focus => modules::on("focus"),
        Tile::ChargeLimit => charge_limit().is_some(),
        Tile::PowerProfile => modules::on("power"),
        _ => true,
    }
}

// the tiles that show, in the rows `controls.tiles` packs them into
fn layout() -> Vec<Vec<Placed>> {
    let list: Vec<Placed> = config::get()
        .tiles
        .iter()
        .copied()
        .filter(|placed| visible(placed.tile))
        .collect();

    tiles::rows(&list)
}

// the targets of the tiles' rows, at the middle of where `switches` draws each
fn tile_rows() -> Vec<Vec<(At, f32)>> {
    layout()
        .iter()
        .map(|row| {
            let widths = tiles::widths(row, WIDTH, SWITCH_GAP, SWITCH);
            let mut targets = Vec::new();
            let mut left = 0.0;

            for (placed, width) in row.iter().zip(widths) {
                let at = |across: f32| (left + across) / WIDTH;

                match placed.tile {
                    Tile::Wifi | Tile::Bluetooth => {
                        let (knob, tile) = if placed.tile == Tile::Wifi {
                            (At::WifiSwitch, At::Wifi)
                        } else {
                            (At::BluetoothSwitch, At::Bluetooth)
                        };

                        targets.push((knob, at(KNOB_INSET + KNOB / 2.0)));
                        targets.push((tile, at((KNOB_INSET + KNOB + width) / 2.0)));
                    }
                    tile => targets.push((target(tile), at(width / 2.0))),
                }

                left += width + SWITCH_GAP;
            }

            targets
        })
        .collect()
}

// what the ring is on for a tile that is one target
fn target(tile: Tile) -> At {
    match tile {
        Tile::Wifi => At::Wifi,
        Tile::Bluetooth => At::Bluetooth,
        Tile::Caffeine => At::Caffeine,
        Tile::Dnd => At::Dnd,
        Tile::NightLight => At::NightLight,
        Tile::Focus => At::Focus,
        Tile::ChargeLimit => At::ChargeLimit,
        Tile::PowerProfile => At::PowerProfile,
    }
}

// the icon a tile has, on the Surface and in its arrangement
fn icon(tile: Tile) -> Icon {
    match tile {
        Tile::Wifi => Icon::Wifi,
        Tile::Bluetooth => Icon::Bluetooth,
        Tile::Caffeine => Icon::Cup,
        Tile::Dnd => Icon::Moon,
        Tile::NightLight => Icon::Sunset,
        Tile::Focus => Icon::Focus,
        Tile::ChargeLimit => Icon::BatteryLimit,
        Tile::PowerProfile => Icon::Bolt,
    }
}

// the targets a listing sub-surface lists under its header, or the tiles', read now
fn rows(sub: Option<&Subsurface>) -> Vec<Vec<(At, f32)>> {
    match sub {
        None => tile_rows(),
        Some(Subsurface::Wifi) => wifi::rows(Connectivity::read().wifi, &Networks::read()),
        Some(Subsurface::Bluetooth) => bluetooth::rows(&Adapter::read(), &Prompt::read()),
        Some(Subsurface::Audio) => audio::rows(&mixer()),
        Some(Subsurface::Arrange { draft, chosen }) => arrange::rows(draft, *chosen),
        _ => Vec::new(),
    }
}

// the devices and apps, none while `audio` is off, as nothing follows them then
fn mixer() -> Mixer {
    if modules::on("audio") {
        Mixer::read().clone()
    } else {
        Mixer::default()
    }
}

fn header(ring: Option<&At>) -> Row {
    let mut header = children![
        Text::new("Controls")
            .size(theme::text::TITLE)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD)
    ];

    // the dots only mark a capture, so the apps behind them are named here
    if modules::on("privacy") && cluster::on() {
        header.extend(capturing(&Privacy::read(), ring == Some(&At::Stop)));
    }

    // the time left at the header's end, over whatever room the capture leaves
    if let Some(left) = modules::on("battery")
        .then(|| Remaining::read().said())
        .flatten()
    {
        if header.len() == 1 {
            header.push(Box::new(Rectangle::new().width(Parent).height(HEADER)));
        }

        header.push(Box::new(
            Text::new(left)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::MEDIUM),
        ));
    }

    // the pencil at the end, over the room the capture or the time leaves
    if header.len() == 1 {
        header.push(Box::new(Rectangle::new().width(Parent).height(HEADER)));
    }

    header.push(Box::new(pencil(ring == Some(&At::Edit))));

    Row::new(header)
        .width(WIDTH)
        .height(HEADER)
        .gap(ICON_GAP)
        .align(Center)
}

// arranges the tiles
fn pencil(ring: bool) -> Rectangle {
    Rectangle::new()
        .width(HEADER)
        .height(HEADER)
        .radius(HEADER / 2.0)
        .align_child(Center, Center)
        .border_if(ring)
        .cursor(Cursor::Pointer)
        .semantics(|| Semantics::new(Role::Button, "edit"))
        .on_click(super::on_left(|| {
            click(Act::Press(At::Edit));
        }))
        .child(Icon::Sliders.on(14.0, theme::island().on_surface_variant))
}

/*
 * the cluster's glyphs, then the apps using a microphone or camera and what the screen casts
 * capture; niri cannot say who casts, so a cast is a window's or a screen's and no app's. At the
 * end Stop sharing, only while a PipeWire cast runs, `ring` saying the ring is on it
 */
fn capturing(privacy: &Privacy, ring: bool) -> Option<Box<dyn Widget>> {
    if !privacy.any() {
        return None;
    }

    let mut row = cluster::glyphs(privacy);
    let apps = privacy
        .sensors
        .iter()
        .flat_map(|sensors| sensors.apps.clone());
    let paused = if privacy.paused { " (paused)" } else { "" };
    let casts = privacy.casts.iter().filter_map(|target| match target {
        Target::Screen => Some(format!("Screen cast{paused}")),
        Target::Window => Some(format!("Window cast{paused}")),
        Target::Unknown => None,
    });
    let named: Vec<String> = apps.chain(casts).collect();

    if !named.is_empty() {
        row.push(Box::new(
            Text::new(named.join(", "))
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::MEDIUM)
                .elide(),
        ));
    }

    if sharing().is_some() {
        row.push(Box::new(Rectangle::new().width(Parent).height(HEADER)));
        row.push(Box::new(stop(ring)));
    }

    Some(Box::new(Row::new(row).width(Parent).gap(6.0).align(Center)))
}

// the pill that stops the PipeWire casts
fn stop(ring: bool) -> Rectangle {
    Rectangle::new()
        .width(STOP)
        .height(HEADER)
        .radius(HEADER / 2.0)
        .fill(theme::island().surface_container_high)
        .align_child(Center, Center)
        .border_if(ring)
        .cursor(Cursor::Pointer)
        .semantics(|| Semantics::new(Role::Button, "stop"))
        .on_click(super::on_left(|| {
            click(Act::Press(At::Stop));
        }))
        .child(
            Text::new("Stop sharing")
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface)
                .weight(theme::text::SEMIBOLD),
        )
}

/*
 * the tiles as `controls.tiles` arranges them: a half is a switch, a full one a switch across the
 * row, an icon a round button
 */
fn switches(dnd: Option<bool>, ring: Option<&At>) -> Column {
    let rows = layout()
        .into_iter()
        .map(|row| {
            let widths = tiles::widths(&row, WIDTH, SWITCH_GAP, SWITCH);
            let tiles = row
                .iter()
                .zip(widths)
                .filter_map(|(placed, width)| tile(placed, width, dnd, ring))
                .collect();

            Box::new(Row::new(tiles).gap(SWITCH_GAP)) as Box<dyn Widget>
        })
        .collect();

    Column::new(rows).gap(SWITCH_GAP)
}

// one tile, none for what has no place now
fn tile(
    placed: &Placed,
    width: f32,
    dnd: Option<bool>,
    ring: Option<&At>,
) -> Option<Box<dyn Widget>> {
    let on = |at: At| ring == Some(&at);

    // a radio's knob switches it, and the rest of its tile opens its sub-surface
    let (item, knob, opens) = match placed.tile {
        Tile::Wifi => (
            self::wifi(&Connectivity::read()),
            on(At::WifiSwitch),
            Some(At::Wifi),
        ),
        Tile::Bluetooth => (
            self::bluetooth(&Adapter::read()),
            on(At::BluetoothSwitch),
            Some(At::Bluetooth),
        ),
        Tile::Caffeine => (
            self::caffeine(modules::on("caffeine").then(|| caffeine::newest().is_some())),
            false,
            None,
        ),
        Tile::Dnd => (self::dnd(dnd), false, None),
        Tile::NightLight => (self::night_light(), false, None),
        Tile::Focus => (self::focus(), false, None),
        Tile::ChargeLimit => (self::charge_limit()?, false, None),
        Tile::PowerProfile => (self::power_profile(), false, None),
    };

    let ring = on(target(placed.tile));

    Some(if placed.size == Size::Icon {
        Box::new(round(item, ring))
    } else {
        Box::new(switch(item, width, knob, ring, opens))
    })
}

// one switch, as the Surface draws it
#[derive(Debug, Clone, PartialEq)]
struct Switch {
    icon: Icon,
    name: &'static str,

    // what it is on, or that it is off or unavailable
    status: String,

    on: bool,

    // none while unavailable
    press: Option<Press>,
}

// what pressing a switch asks for, decided when drawn so a press does what it showed
#[derive(Debug, Clone, PartialEq)]
enum Press {
    Wifi(bool),
    Bluetooth { adapter: String, on: bool },
    Caffeine(bool),
    Dnd(bool),
    NightLight(bool),
    ChargeLimit(bool),
    Profile(Profile),
    Focus,
}

fn wifi(connectivity: &Connectivity) -> Switch {
    let joined = match &connectivity.uplink {
        Some(Uplink::Wifi(ssid)) => ssid.clone(),
        _ => String::from("Not connected"),
    };

    radio(
        icon(Tile::Wifi),
        "Wi-Fi",
        connectivity.wifi,
        joined,
        Press::Wifi,
    )
}

// the connected device by name, or the first and how many more
fn bluetooth(adapter: &Adapter) -> Switch {
    let connected: Vec<&str> = adapter
        .devices
        .iter()
        .filter(|device| device.connected)
        .map(|device| device.name.as_str())
        .collect();

    let status = if connected.is_empty() {
        String::from("On")
    } else if let [only] = connected[..] {
        only.to_owned()
    } else {
        format!("{} +{}", connected[0], connected.len() - 1)
    };

    let path = adapter.path.clone();

    radio(
        icon(Tile::Bluetooth),
        "Bluetooth",
        adapter.radio,
        status,
        |on| Press::Bluetooth { adapter: path, on },
    )
}

// `status` says what an On radio is on; `press` asks for it on or off
fn radio(
    icon: Icon,
    name: &'static str,
    radio: Radio,
    status: String,
    press: impl FnOnce(bool) -> Press,
) -> Switch {
    let (status, on) = match radio {
        Radio::Missing => (String::from("Unavailable"), false),
        Radio::Off => (String::from("Off"), false),
        Radio::On => (status, true),
    };

    Switch {
        icon,
        name,
        status,
        on,
        press: (radio != Radio::Missing).then(|| press(!on)),
    }
}

// `on` is none while `caffeine` is off, which leaves it unavailable
fn caffeine(on: Option<bool>) -> Switch {
    let status = match on {
        None => "Unavailable",
        Some(true) => "On",
        Some(false) => "Off",
    };

    Switch {
        icon: icon(Tile::Caffeine),
        name: "Caffeine",
        status: String::from(status),
        on: on == Some(true),
        press: on.map(|on| Press::Caffeine(!on)),
    }
}

fn dnd(on: Option<bool>) -> Switch {
    // a timed one says when it ends
    let status = match on {
        None => String::from("Unavailable"),
        Some(true) => timed::ends()
            .and_then(timed::label)
            .map_or_else(|| String::from("On"), |at| format!("Until {at}")),
        Some(false) => String::from("Off"),
    };

    Switch {
        icon: icon(Tile::Dnd),
        name: "Do Not Disturb",
        status,
        on: on == Some(true),
        press: on.map(|on| Press::Dnd(!on)),
    }
}

/*
 * on while `night-light.mode` is not off, saying when, or why wlsunset is not running; unavailable
 * while its Module is off or wlsunset is not installed
 */
fn night_light() -> Switch {
    let shade = modules::on("night-light").then(|| Shade::read().clone());
    let night = config::get().night_light;
    let on = night.mode != NightMode::Off;

    let status = match &shade {
        None => String::from("Unavailable"),
        Some(shade) if shade.missing => format!("Needs {}", night_light::WLSUNSET),
        Some(Shade {
            failed: Some(why), ..
        }) if on => why.clone(),
        Some(_) => match (night.mode, night.sunset, night.sunrise) {
            (NightMode::Off, ..) => String::from("Off"),
            (NightMode::On, ..) => String::from("On"),
            (NightMode::Schedule, Some(sunset), Some(sunrise)) => format!("{sunset} to {sunrise}"),
            (NightMode::Schedule, ..) => String::from("Sunset to sunrise"),
        },
    };

    Switch {
        icon: icon(Tile::NightLight),
        name: "Night Light",
        status,
        on: on && shade.is_some(),
        press: shade
            .filter(|shade| !shade.missing)
            .map(|_| Press::NightLight(!on)),
    }
}

// on while Focus holds what it turned on
fn focus() -> Switch {
    let on = quiet::Quiet::read().on;

    Switch {
        icon: icon(Tile::Focus),
        name: "Focus",
        status: String::from(if on { "On" } else { "Off" }),
        on,
        press: Some(Press::Focus),
    }
}

// a switch as a round button, filled while on, faded and not pressable while unavailable
fn round(item: Switch, ring: bool) -> Rectangle {
    let (fill, ink) = if item.on {
        (theme::island().primary, theme::island().on_primary)
    } else {
        (
            theme::island().surface_container,
            theme::island().on_surface,
        )
    };

    let round = Rectangle::new()
        .width(SWITCH)
        .height(SWITCH)
        .radius(SWITCH / 2.0)
        .fill(fill)
        .align_child(Center, Center)
        .border_if(ring)
        .child(item.icon.on(18.0, ink));

    let round = round.semantics(|| {
        toggle(
            Role::Toggle,
            item.name,
            &item.status,
            item.on,
            item.press.is_none(),
        )
    });

    match item.press {
        None => round.opacity(DISABLED),
        Some(press) => round
            .cursor(Cursor::Pointer)
            .on_click(super::on_left(move || click_switch(press.clone()))),
    }
}

// a switch's node: named for the switch, its status being what a network or a device is called
fn toggle(role: Role, name: &'static str, status: &str, on: bool, disabled: bool) -> Semantics {
    let id = match name {
        "Wi-Fi" => "wifi",
        "Bluetooth" => "bluetooth",
        "Caffeine" => "caffeine",
        "Do Not Disturb" => "dnd",
        "Night Light" => "night-light",
        "Focus" => "focus",
        "Power Profile" => "power-profile",
        "Limit Charge" => "limit-charge",
        _ => "switch",
    };

    let node = Semantics::new(role, id)
        .label(name)
        .content(status)
        .disabled(disabled);

    if role == Role::Toggle {
        node.checked(on)
    } else {
        node
    }
}

/*
 * the active power profile, on while it is not Balanced; pressing it asks for the next one the
 * machine has, as `kanade power-profile cycle` does (ADR 0084). Not pressable without the daemon
 */
fn power_profile() -> Switch {
    let profiles = Profiles::read().clone();

    let next = profiles.active.and_then(|active| {
        let at = profiles.available.iter().position(|each| *each == active)?;

        Some(profiles.available[(at + 1) % profiles.available.len()])
    });

    Switch {
        icon: icon(Tile::PowerProfile),
        name: "Power Profile",
        status: profiles
            .active
            .map_or("Unavailable", Profile::label)
            .to_owned(),
        on: profiles
            .active
            .is_some_and(|active| active != Profile::Balanced),
        press: next
            .filter(|next| Some(*next) != profiles.active)
            .map(Press::Profile),
    }
}

/*
 * on while UPower stops charging short of full, saying where, or that UPower is being asked, or why
 * it refused; none, so not drawn, while the battery cannot limit its charge or its Module is off
 */
fn charge_limit() -> Option<Switch> {
    let limit = modules::on("battery")
        .then(|| ChargeLimit::read().clone())
        .filter(|limit| limit.supported)?;

    let status = match (&limit.asked, limit.enabled, limit.end) {
        (Some((_, None)), ..) => String::from("Asking"),
        (Some((_, Some(Err(why)))), ..) => why.clone(),
        (_, true, Some(end)) => format!("Stops at {end}%"),
        (_, true, None) => String::from("On"),
        (_, false, _) => String::from("Off"),
    };

    Some(Switch {
        icon: icon(Tile::ChargeLimit),
        name: "Limit Charge",
        status,
        on: limit.enabled,
        press: Some(Press::ChargeLimit(!limit.enabled)),
    })
}

/*
 * its knob filled while on, then its name and status. Pressing it switches it, or with `opens` only
 * its knob does, and the rest of it, a chevron at its end, presses `opens`, its sub-surface.
 * `knob_ring` and `ring` say the ring is on the knob or the whole of it
 */
fn switch(item: Switch, width: f32, knob_ring: bool, ring: bool, opens: Option<At>) -> Rectangle {
    let (fill, ink) = if item.on {
        (theme::island().primary, theme::island().on_primary)
    } else {
        (
            theme::island().surface_container_high,
            theme::island().on_surface,
        )
    };

    let knob = Rectangle::new()
        .width(KNOB)
        .height(KNOB)
        .radius(KNOB / 2.0)
        .fill(fill)
        .align_child(Center, Center)
        .child(item.icon.on(18.0, ink));

    let knob = Rectangle::new()
        .width(KNOB + 2.0 * HALO)
        .height(KNOB + 2.0 * HALO)
        .radius(KNOB / 2.0 + HALO)
        .align_child(Center, Center)
        .border_if(knob_ring)
        .child(knob);

    let right = 16.0;
    let chevron = if opens.is_some() {
        16.0 + ICON_GAP - HALO
    } else {
        0.0
    };
    let words = Column::new(children![
        Text::new(item.name)
            .size(theme::text::LABEL)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD)
            .elide(),
        Text::new(&item.status)
            .size(theme::text::LABEL_SMALL)
            .color(theme::island().on_surface_variant)
            .weight(theme::text::MEDIUM)
            .elide(),
    ])
    .width(width - KNOB_INSET - KNOB - ICON_GAP - right - chevron)
    .gap(2.0);

    let on = item.on;
    let press = item.press;

    let knob = match (&press, &opens) {
        (Some(press), Some(_)) => {
            let press = press.clone();

            knob.cursor(Cursor::Pointer)
                .semantics(|| Semantics::new(Role::Toggle, "toggle").checked(on))
                .on_click(super::on_left(move || {
                    click_switch(press.clone());
                }))
        }
        _ => knob,
    };

    let mut parts = children![knob, words];

    if opens.is_some() {
        parts.push(Box::new(
            Icon::Forward.on(16.0, theme::island().on_surface_variant),
        ));
    }

    let switch = Rectangle::new()
        .width(width)
        .height(SWITCH)
        .radius(SWITCH / 2.0)
        .fill(theme::island().surface_container)
        .padding(Padding {
            top: 0.0,
            right,
            bottom: 0.0,
            left: KNOB_INSET - HALO,
        })
        .align_child(Start, Center)
        .border_if(ring)
        .semantics(|| {
            // one that opens its list is a button, and its toggle is the knob inside it
            let role = if opens.is_some() {
                Role::Button
            } else {
                Role::Toggle
            };

            toggle(role, item.name, &item.status, on, press.is_none())
        })
        .child(Row::new(parts).gap(ICON_GAP - HALO).align(Center));

    match (press, opens) {
        (None, _) => switch.opacity(DISABLED),
        (Some(_), Some(opens)) => {
            switch
                .cursor(Cursor::Pointer)
                .on_click(super::on_left(move || {
                    click(Act::Press(opens.clone()));
                }))
        }
        (Some(press), None) => switch
            .cursor(Cursor::Pointer)
            .on_click(super::on_left(move || {
                click_switch(press.clone());
            })),
    }
}

// a switch clicked: the ring hides, as for any click
fn click_switch(press: Press) {
    hide();
    run(press);
}

fn run(press: Press) {
    match press {
        Press::Wifi(on) => set_wifi(on),
        Press::Bluetooth { adapter, on } => bluez::power(adapter, on),
        Press::Caffeine(on) => caffeine::request(
            if on {
                caffeine::Request::On(None)
            } else {
                caffeine::Request::Off
            },
            Later::detached(),
        ),
        Press::Dnd(on) => notifications::set_dnd(on, Instant::now()),
        Press::NightLight(on) => night_light::switch(on),
        Press::Focus => quiet::toggle(),
        // refused at once only when it is no longer supported, which the switch then stops showing
        Press::ChargeLimit(on) => {
            let _ = charge_limit::ask(on, None);
        }
        Press::Profile(profile) => power::set(profile),
    }
}

/*
 * the speaker, its icon pressed to mute and its chevron to open the Audio sub-surface, then the
 * screen; with `audio` or `brightness` off its Service is never read, and its row is faded and empty
 */
fn levels(ring: Option<&At>) -> Column {
    let speaker = modules::on("audio").then(|| {
        let audio = Audio::read();

        (audio.volume(), audio.muted())
    });

    let screen = modules::on("brightness")
        .then(|| {
            let brightness = Brightness::read();

            brightness.present().then(|| brightness.percent())
        })
        .flatten();

    let icon = Rectangle::new()
        .width(TARGET)
        .height(TARGET)
        .align_child(Center, Center);

    let volume = match speaker {
        Some((volume, muted)) => {
            let icon = icon
                .cursor(Cursor::Pointer)
                .semantics(|| Semantics::new(Role::Toggle, "mute").checked(muted))
                .on_click(super::on_left(|| {
                    Audio::toggle_mute();
                }))
                .child(if muted {
                    Icon::SpeakerMuted.draw(20.0)
                } else {
                    Icon::Speaker(volume).draw(20.0)
                });

            let tone = if muted {
                theme::island().on_surface_variant
            } else {
                theme::island().on_surface
            };

            level(
                icon,
                Some(Slider::Speaker),
                Some(volume),
                tone,
                ring == Some(&At::Speaker),
                chevron(ring == Some(&At::Audio), true),
            )
        }
        None => level(
            icon.child(Icon::Speaker(0).draw(20.0)),
            None,
            None,
            theme::island().on_surface,
            ring == Some(&At::Speaker),
            chevron(ring == Some(&At::Audio), false),
        ),
    };

    let sun = Rectangle::new()
        .width(TARGET)
        .height(TARGET)
        .align_child(Center, Center)
        .child(Icon::Sun.draw(20.0));

    // a desktop monitor has no backlight to set
    let brightness = level(
        sun,
        screen.map(|_| Slider::Brightness),
        screen,
        theme::island().on_surface,
        ring == Some(&At::Brightness),
        Rectangle::new().width(TARGET).height(TARGET),
    );

    Column::new(children![volume, brightness]).gap(LEVEL_GAP)
}

// opens the Audio sub-surface, faded and not pressable while `audio` is off
fn chevron(ring: bool, on: bool) -> Rectangle {
    let chevron = Rectangle::new()
        .width(TARGET)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .align_child(Center, Center)
        .border_if(ring)
        .semantics(|| Semantics::new(Role::Button, "sound").disabled(!on))
        .child(Icon::Forward.on(16.0, theme::island().on_surface_variant));

    if !on {
        return chevron.opacity(DISABLED);
    }

    chevron.cursor(Cursor::Pointer).on_click(super::on_left(|| {
        click(Act::Press(At::Audio));
    }))
}

/*
 * its icon, its bar, its number, then `end`, a chevron or the room one takes; the wheel on the
 * row before `end` moves it, as Left and Right do with the ring on it. None to set leaves that faded
 * and empty
 */
fn level(
    icon: Rectangle,
    slider: Option<Slider>,
    percent: Option<u8>,
    tone: Color,
    ring: bool,
    end: Rectangle,
) -> Row {
    let width = WIDTH - TARGET - NUMBER - 3.0 * ICON_GAP - TARGET;
    let fraction = f32::from(percent.unwrap_or(0)) / 100.0;

    let bar = match slider {
        Some(slider) => slider.bar(width, fraction, tone),
        None => Rectangle::new()
            .width(width)
            .height(TARGET)
            .align_child(Start, Center)
            .child(bar(width, 0.0, tone)),
    };

    let number = Rectangle::new()
        .width(NUMBER)
        .height(TARGET)
        .align_child(End, Center)
        .child(
            Text::new(percent.map_or_else(String::new, |percent| percent.to_string()))
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::SEMIBOLD),
        );

    let level = Rectangle::new()
        .width(WIDTH - ICON_GAP - TARGET)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .align_child(Start, Center)
        .border_if(ring)
        .child(
            Row::new(children![icon, bar, number])
                .gap(ICON_GAP)
                .align(Center),
        );

    let level = match slider {
        Some(slider) => level.on_scroll(move |Scroll { y, .. }| slider.wheel(y)),
        None => level.opacity(DISABLED),
    };

    Row::new(children![level, end])
        .width(WIDTH)
        .gap(ICON_GAP)
        .align(Center)
}

/*
 * the bolt, then every profile, the active one filled. One the machine lacks is faded in its place,
 * and with no daemon running so is the whole row
 */
fn profiles(ring: Option<&At>) -> Rectangle {
    let profiles = Profiles::read().clone();

    let width = WIDTH - TARGET - ICON_GAP;
    let segment = (width - 2.0 * TRACK_INSET) / Profile::ALL.len() as f32;

    let segments = Profile::ALL
        .into_iter()
        .map(|profile| {
            Box::new(self::segment(
                profile,
                segment,
                profiles.active == Some(profile),
                profiles.available.contains(&profile),
                ring == Some(&At::Profile(profile)),
            )) as Box<dyn Widget>
        })
        .collect();

    let track = Rectangle::new()
        .width(width)
        .height(TRACK)
        .radius(TRACK / 2.0)
        .fill(theme::island().surface_container)
        .padding(TRACK_INSET)
        .align_child(Start, Center)
        .child(Row::new(segments));

    let bolt = Rectangle::new()
        .width(TARGET)
        .height(TARGET)
        .align_child(Center, Center)
        .child(Icon::Bolt.draw(20.0));

    let row = Rectangle::new()
        .width(WIDTH)
        .height(TRACK)
        .align_child(Start, Center)
        .child(Row::new(children![bolt, track]).gap(ICON_GAP).align(Center));

    if profiles.available.is_empty() {
        row.opacity(DISABLED)
    } else {
        row
    }
}

fn segment(profile: Profile, width: f32, active: bool, available: bool, ring: bool) -> Rectangle {
    let segment = Rectangle::new()
        .width(width)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .align_child(Center, Center)
        .border_if(ring)
        .semantics(|| {
            Semantics::new(Role::Button, "profile")
                .label(profile.label())
                .selected(active)
                .disabled(!available)
        })
        .child(
            Text::new(profile.label())
                .size(theme::text::LABEL_SMALL)
                .color(if active {
                    theme::island().on_primary
                } else {
                    theme::island().on_surface_variant
                })
                .weight(theme::text::SEMIBOLD),
        );

    // filled like a switch that is on
    if active {
        segment.fill(theme::island().primary)
    } else if available {
        segment
            .cursor(Cursor::Pointer)
            .on_click(super::on_left(move || {
                click(Act::Press(At::Profile(profile)));
            }))
    } else {
        segment.opacity(DISABLED)
    }
}

/*
 * a key while this island shows the Surface; false for one it does not use, which the window's own
 * keys then get, like Escape at the top level, which closes the island. The first key only shows
 * the ring where it is. A key it uses keeps a held island open for another hold
 */
pub fn key(monitor: &str, key: Key) -> bool {
    let (visit, held) = {
        let island = IslandService::read();

        if island.presentation(monitor) != Presentation::Expanded(Surface::Controls) {
            return false;
        }

        (island.visit(), island.held(monitor))
    };

    let focus = Focus::read().of(visit, held);
    let grid = targets(&focus);

    // read apart, as typing writes it
    let pin = matches!(*Prompt::read(), Prompt::Pin { .. });

    if focus.sub == Some(Subsurface::Bluetooth)
        && pin
        && let Some(typed) = typed_pin(key, focus.ring(&grid).is_some())
    {
        match typed {
            Pin::Letter(letter) => bluez::type_pin(letter),
            Pin::Pair => bluez::confirm(),
        }

        IslandService::write().attend(monitor, Instant::now());

        return true;
    }

    let focus = settled(focus, &grid);
    let Some((focus, act)) = focus.step(key, &grid) else {
        return false;
    };

    let focus = match act {
        Some(act) => self::act(focus, act, visit),
        None => focus,
    };

    set(focus);

    IslandService::write().attend(monitor, Instant::now());

    true
}

// what a key on the Pin prompt does beyond the ring
#[derive(Debug, PartialEq)]
enum Pin {
    // a letter typed, or the last erased for none
    Letter(Option<char>),
    Pair,
}

/*
 * a key on the Pin prompt, none for one the ring takes. Enter pairs only while the ring hides, so
 * it never does other than the button the ring is on
 */
fn typed_pin(key: Key, ringed: bool) -> Option<Pin> {
    match key {
        Key::Character(letter) => Some(Pin::Letter(Some(letter))),
        Key::Backspace => Some(Pin::Letter(None)),
        Key::Enter if !ringed => Some(Pin::Pair),
        _ => None,
    }
}

/*
 * opens the Surface on the Wi-Fi networks, as pressing the tile does, for a radio the machine has.
 * The same for Bluetooth. Nothing opens when Controls is withheld
 */
// whether Controls opened on the Wi-Fi list; not with no Wi-Fi device
pub fn open_wifi(monitor: &str) -> bool {
    Connectivity::read().wifi != Radio::Missing
        && open_into(monitor, Subsurface::Wifi, wireless::watch)
}

// whether Controls opened on the Bluetooth list; not with no adapter
pub fn open_bluetooth(monitor: &str) -> bool {
    Adapter::read().radio != Radio::Missing
        && open_into(monitor, Subsurface::Bluetooth, bluez::watch)
}

fn open_into(monitor: &str, sub: Subsurface, watch: fn(u64)) -> bool {
    let Some(visit) = opened(&mut IslandService::write(), monitor, Instant::now()) else {
        return false;
    };

    watch(visit);

    // read apart from the write in `set`
    let focus = Focus::read().of(visit, true).enter(sub);

    set(focus);

    true
}

// opens Controls on `monitor`, the visit it starts; none when it is withheld, which opens nowhere
fn opened(island: &mut IslandService, monitor: &str, now: Instant) -> Option<u64> {
    let before = island.visit();

    island.open(monitor, Surface::Controls, now);

    (island.visit() != before).then(|| island.visit())
}

// a target clicked: the ring hides, since the pointer is what moves now
fn click(act: Act) {
    let visit = hide();
    let focus = Focus::read().of(visit, false);

    set(self::act(focus, act, visit));
}

// hides the ring, giving the visit it hid in
fn hide() -> u64 {
    let visit = IslandService::read().visit();

    let focus = Focus::read().of(visit, false).hidden();

    set(focus);

    visit
}

/*
 * a listing sub-surface's rows scrolled by `pixels`, down further down, from where they show. The
 * ring hides, since the pointer is what moves now and the ring would hold the list on its row
 */
fn scroll(pixels: f32) {
    let visit = IslandService::read().visit();
    let focus = Focus::read().of(visit, false);
    let grid = targets(&focus);
    let mut focus = settled(focus, &grid);

    let most = list::ROOMY.most(grid.len().saturating_sub(1));

    focus = focus.hidden();
    focus.offset = (focus.offset + pixels).clamp(0.0, most);

    set(focus);
}

/*
 * `focus` scrolled to where its rows show, the ringed one whole, so a key or the wheel moves on
 * from what is seen. The rows are `grid`'s after the header
 */
fn settled(mut focus: Focus, grid: &[Vec<(At, f32)>]) -> Focus {
    if !focus.listing() {
        return focus;
    }

    let count = grid.len().saturating_sub(1);
    let ringed = focus.row(grid).and_then(|row| row.checked_sub(1));

    focus.offset = list::ROOMY.scrolled(focus.offset, count, ringed);
    focus
}

// what a key or click asks for done, giving where that leaves the focus
fn act(focus: Focus, act: Act, visit: u64) -> Focus {
    match act {
        Act::Press(at) => press(focus, at, visit),
        Act::Adjust(at, lines) => {
            let slider = match at {
                At::Speaker => Some(Slider::Speaker),
                At::Brightness => Some(Slider::Brightness),
                At::MicrophoneLevel => Some(Slider::Microphone),
                At::Stream(node) => Some(Slider::Stream(node)),
                _ => None,
            };

            if let Some(slider) = slider {
                slider.wheel(lines);
            }

            focus
        }
        Act::Join => join(focus),
    }
}

/*
 * what pressing `at` does, read now, so the keyboard does what a click on the same target would.
 * What is unavailable does nothing
 */
fn press(focus: Focus, at: At, visit: u64) -> Focus {
    let switched = match at {
        At::Stop => {
            if let Some(sessions) = sharing() {
                privacy::stop_sharing(&sessions);
            }

            None
        }
        At::WifiSwitch => self::wifi(&Connectivity::read()).press,
        At::BluetoothSwitch => self::bluetooth(&Adapter::read()).press,
        At::Caffeine => {
            caffeine(modules::on("caffeine").then(|| caffeine::newest().is_some())).press
        }
        At::Dnd => {
            let on = modules::on("notifications").then(|| IslandService::read().dnd());

            dnd(on).press
        }
        At::NightLight => night_light().press,
        At::ChargeLimit => charge_limit().and_then(|limit| limit.press),
        At::PowerProfile => power_profile().press,
        At::Focus => modules::on("focus").then_some(Press::Focus),
        At::Edit => return focus.arrange(config::get().tiles.clone()),
        At::Pick(_)
        | At::Earlier
        | At::Later
        | At::Resize(_)
        | At::Remove
        | At::Add(_)
        | At::Done => return arrange::press(focus, at),
        At::Wifi => {
            if Connectivity::read().wifi == Radio::Missing {
                return focus;
            }

            wireless::watch(visit);

            return focus.enter(Subsurface::Wifi);
        }
        At::Bluetooth => {
            if Adapter::read().radio == Radio::Missing {
                return focus;
            }

            bluez::watch(visit);

            return focus.enter(Subsurface::Bluetooth);
        }
        At::Speaker => {
            if modules::on("audio") {
                Audio::toggle_mute();
            }

            None
        }
        At::Audio => {
            if !modules::on("audio") {
                return focus;
            }

            return focus.enter(Subsurface::Audio);
        }
        At::MicrophoneLevel => {
            if modules::on("audio") {
                Audio::toggle_microphone_mute();
            }

            None
        }
        At::Output(node) => {
            default(&mixer().outputs, node, Direction::Output);
            None
        }
        At::Input(node) => {
            default(&mixer().inputs, node, Direction::Input);
            None
        }
        At::Stream(node) => {
            if let Some(stream) = mixer().streams.iter().find(|stream| stream.node == node) {
                sound::ask_muted(node, !stream.level.muted);
            }

            None
        }
        At::Route(node) => {
            let mixer = mixer();

            if let Some(stream) = mixer.streams.iter().find(|stream| stream.node == node) {
                sound::ask_route(node, audio::next_route(stream, &mixer.outputs));
            }

            None
        }
        At::Brightness => None,
        At::Profile(profile) => {
            let profiles = Profiles::read().clone();

            if profiles.active != Some(profile) && profiles.available.contains(&profile) {
                power::set(profile);
            }

            None
        }
        At::Back => return focus.out(),
        At::Radio if focus.sub == Some(Subsurface::Bluetooth) => {
            self::bluetooth(&Adapter::read()).press
        }
        At::Radio => {
            let radio = Connectivity::read().wifi;

            (radio != Radio::Missing).then_some(Press::Wifi(radio != Radio::On))
        }
        At::Network(ssid) => return network(focus, &ssid),
        At::Device(path) => {
            device(&path);
            None
        }
        At::Forget(path) => {
            forget(&path);
            None
        }
        At::Confirm => {
            bluez::confirm();
            None
        }
        At::Cancel => {
            bluez::cancel();
            None
        }
    };

    if let Some(press) = switched {
        run(press);
    }

    focus
}

/*
 * a network pressed: the joined one is left, a secured one not yet joined, or whose password was
 * wrong, asks for its password first, and any other is joined. One joining or that this cannot
 * join does nothing
 */
fn network(focus: Focus, ssid: &str) -> Focus {
    let networks = Networks::read().clone();
    let join = Join::read().clone();

    let Some(network) = networks.list.iter().find(|network| network.ssid == ssid) else {
        return focus;
    };

    let joining = network.link == Link::Joining || join.joining(ssid);
    let asks = network.profile.is_none() || join.failed(ssid) == Some(Failure::WrongPassword);

    if network.link == Link::Joined {
        wireless::disconnect(&networks);
    } else if joining || network.security == Security::Unsupported {
    } else if network.security.password() && asks {
        return focus.into_password(ssid);
    } else {
        wireless::join(&networks, network, None);
    }

    focus
}

// a device of `devices` made the default of `direction`, unless it is already or is gone
fn default(devices: &[sound::Device], node: sound::Node, direction: Direction) {
    if let Some(device) = devices
        .iter()
        .find(|device| device.node == node && !device.default)
    {
        sound::ask_default(device, direction);
    }
}

/*
 * a device pressed: a connected one is disconnected, a paired one connected, and one nearby paired,
 * then connected. One gone does nothing, as does any while something is being done to a device
 */
fn device(path: &str) {
    let Some(device) = Adapter::read()
        .devices
        .iter()
        .find(|device| device.path == path)
        .cloned()
    else {
        return;
    };

    if device.connected {
        bluez::disconnect(path);
    } else if device.paired {
        bluez::connect(path);
    } else {
        bluez::pair(path);
    }
}

// a paired device forgotten, unless something is being done to a device
fn forget(path: &str) {
    let adapter = Adapter::read().clone();

    if adapter
        .devices
        .iter()
        .any(|device| device.path == path && device.paired)
    {
        bluez::forget(&adapter.path, path);
    }
}

// joins with the password typed, back to the networks, where the join says how it goes
fn join(focus: Focus) -> Focus {
    let Some(Subsurface::Password { ssid, secret }) = &focus.sub else {
        return focus;
    };

    let networks = Networks::read().clone();

    if let Some(network) = networks.list.iter().find(|network| network.ssid == *ssid) {
        wireless::join(&networks, network, Some(secret.clone()));
    }

    focus.out()
}

// out of a sub-surface, its daemon stops being read for it
fn set(focus: Focus) {
    if !matches!(
        focus.sub,
        Some(Subsurface::Wifi | Subsurface::Password { .. })
    ) {
        wireless::unwatch();
    }

    if focus.sub != Some(Subsurface::Bluetooth) {
        bluez::unwatch();
    }

    store(focus);
}
