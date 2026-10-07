//! The Controls Surface (plan 7): Wi-Fi, Bluetooth, the microphone and Do Not Disturb as
//! switches, the speaker's volume and the screen's brightness as levels, and the power profile.
//! Each is one press or one drag, never a menu: it is not a settings app. What the machine does not
//! have, or whose daemon is not running, keeps its place, faded, says so, and does nothing. Its
//! header names the apps the privacy cluster stands for.
//!
//! The Wi-Fi and Bluetooth switches' tiles each open a sub-surface in its place, their knobs still
//! switching them: the networks in range, to join or leave, and the password a secured one asks
//! for; or the Bluetooth devices, to pair, connect, disconnect or forget (#131). The speaker's
//! level ends in a chevron to the Audio sub-surface: the devices to play on and record from, and
//! each app's level (#133). A sub-surface goes back a level by its chevron or Escape. Every target
//! is a key away, a ring on the one the arrows reach (`focus`).

use std::time::Instant;

use amane::{
    Audio, Brightness, Center, Color, Column, Cursor, End, Key, Network, Padding, Parent,
    Rectangle, Row, Scroll, Service, Start, Text, Widget, children,
};

use self::focus::{Act, At, Focus, Subsurface};
use super::Ring;
use super::slider::Slider;
use crate::cluster;
use crate::icon::Icon;
use crate::island::activity::Uplink;
use crate::island::geometry;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::modules;
use crate::sources::audio::{self as sound, Direction, Mixer, Switching};
use crate::sources::bluetooth::{self as bluez, Adapter, Prompt, Request};
use crate::sources::network::Connectivity;
use crate::sources::notifications;
use crate::sources::power::{self, Profile, Profiles};
use crate::sources::privacy::Privacy;
use crate::sources::system::Radio;
use crate::sources::wifi::{self as wireless, Failure, Join, Link, Networks, Security};
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, DISABLED};
use crate::view::bar;

mod audio;
mod bluetooth;
mod focus;
pub(super) mod list;
mod wifi;

// the content's width, which every row fills
const WIDTH: f32 = geometry::CONTROLS.width - 2.0 * INSET;

const HEADER: f32 = 20.0;
const GAP: f32 = 14.0;

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

// the Surface's height, which geometry::CONTROLS is
#[cfg(test)]
const HEIGHT: f32 =
    2.0 * INSET + HEADER + 2.0 * SWITCH + SWITCH_GAP + 2.0 * TARGET + LEVEL_GAP + TRACK + 3.0 * GAP;

/*
 * the top level, or the sub-surface entered from it. `visit`, `held` and `dnd` are the view's own
 * read of IslandService, so this never reads it again; `dnd` is none while the notifications Module
 * is off, since nothing heeds it then. `open` says the Surface is open rather than fading out, so
 * only then does the ring show
 */
pub fn surface(open: bool, visit: u64, held: bool, dnd: Option<bool>) -> Rectangle {
    let shape = geometry::CONTROLS;
    let focus = Focus::read().of(visit, held);

    let grid = focus.grid(rows(focus.sub.as_ref()));
    let focus = settled(focus, &grid);
    let ring = open.then(|| focus.ring(&grid)).flatten();

    let content = match &focus.sub {
        None => Column::new(children![
            header(),
            switches(dnd, ring.as_ref()),
            levels(ring.as_ref()),
            profiles(ring.as_ref())
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
    };

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .child(content)
}

// the targets a listing sub-surface lists under its header, read now
fn rows(sub: Option<&Subsurface>) -> Vec<Vec<(At, f32)>> {
    match sub {
        Some(Subsurface::Wifi) => wifi::rows(Connectivity::read().wifi, &Networks::read()),
        Some(Subsurface::Bluetooth) => bluetooth::rows(&Adapter::read(), &Prompt::read()),
        Some(Subsurface::Audio) => audio::rows(&mixer()),
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

fn header() -> Row {
    let mut header = children![
        Text::new("Controls")
            .size(theme::text::TITLE)
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::SEMIBOLD)
    ];

    // the cluster takes no pointer, so the apps behind its glyphs are named here
    if modules::on("privacy") {
        header.extend(capturing(&Privacy::read()));
    }

    Row::new(header)
        .width(WIDTH)
        .height(HEADER)
        .gap(ICON_GAP)
        .align(Center)
}

/*
 * the cluster's glyphs, then the apps using a microphone or camera; niri cannot say who casts the
 * screen, so a cast is its glyph alone
 */
fn capturing(privacy: &Privacy) -> Option<Box<dyn Widget>> {
    if !privacy.any() {
        return None;
    }

    let mut row = cluster::glyphs(privacy);

    if let Some(sensors) = privacy
        .sensors
        .as_ref()
        .filter(|sensors| !sensors.apps.is_empty())
    {
        row.push(Box::new(
            Text::new(sensors.apps.join(", "))
                .size(theme::text::LABEL_SMALL)
                .color(theme::ISLAND.on_surface_variant)
                .weight(theme::text::MEDIUM)
                .elide(),
        ));
    }

    Some(Box::new(Row::new(row).width(Parent).gap(6.0).align(Center)))
}

// Wi-Fi and Bluetooth, then the microphone and Do Not Disturb
fn switches(dnd: Option<bool>, ring: Option<&At>) -> Column {
    let wifi = self::wifi(&Connectivity::read());
    let bluetooth = self::bluetooth(&Adapter::read());
    let microphone =
        self::microphone(modules::on("audio").then(|| Audio::read().microphone_muted()));
    let dnd = self::dnd(dnd);

    let width = (WIDTH - SWITCH_GAP) / 2.0;
    let on = |at: At| ring == Some(&at);
    let row = |left: Rectangle, right: Rectangle| Row::new(children![left, right]).gap(SWITCH_GAP);

    // a radio's knob switches it, and the rest of its tile opens its sub-surface
    let wifi = switch(
        wifi,
        width,
        on(At::WifiSwitch),
        on(At::Wifi),
        Some(At::Wifi),
    );
    let bluetooth = switch(
        bluetooth,
        width,
        on(At::BluetoothSwitch),
        on(At::Bluetooth),
        Some(At::Bluetooth),
    );
    let microphone = switch(microphone, width, false, on(At::Microphone), None);
    let dnd = switch(dnd, width, false, on(At::Dnd), None);

    Column::new(children![row(wifi, bluetooth), row(microphone, dnd)]).gap(SWITCH_GAP)
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
    Microphone,
    Dnd(bool),
}

fn wifi(connectivity: &Connectivity) -> Switch {
    let joined = match &connectivity.uplink {
        Some(Uplink::Wifi(ssid)) => ssid.clone(),
        _ => String::from("Not connected"),
    };

    radio(Icon::Wifi, "Wi-Fi", connectivity.wifi, joined, Press::Wifi)
}

// the connected devices by name, as many as fit
fn bluetooth(adapter: &Adapter) -> Switch {
    let connected: Vec<&str> = adapter
        .devices
        .iter()
        .filter(|device| device.connected)
        .map(|device| device.name.as_str())
        .collect();

    let status = if connected.is_empty() {
        String::from("On")
    } else {
        connected.join(", ")
    };

    let path = adapter.path.clone();

    radio(Icon::Bluetooth, "Bluetooth", adapter.radio, status, |on| {
        Press::Bluetooth { adapter: path, on }
    })
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

// none while `audio` is off, which leaves it unavailable
fn microphone(muted: Option<bool>) -> Switch {
    let status = match muted {
        None => "Unavailable",
        Some(true) => "Muted",
        Some(false) => "On",
    };

    Switch {
        icon: if muted == Some(true) {
            Icon::MicrophoneMuted
        } else {
            Icon::Microphone
        },
        name: "Microphone",
        status: String::from(status),
        on: muted == Some(false),
        press: muted.map(|_| Press::Microphone),
    }
}

fn dnd(on: Option<bool>) -> Switch {
    let status = match on {
        None => "Unavailable",
        Some(true) => "On",
        Some(false) => "Off",
    };

    Switch {
        icon: Icon::Moon,
        name: "Do Not Disturb",
        status: String::from(status),
        on: on == Some(true),
        press: on.map(|on| Press::Dnd(!on)),
    }
}

/*
 * its knob filled while on, then its name and status. Pressing it switches it, or with `opens` only
 * its knob does, and the rest of it, a chevron at its end, presses `opens`, its sub-surface.
 * `knob_ring` and `ring` say the ring is on the knob or the whole of it
 */
fn switch(item: Switch, width: f32, knob_ring: bool, ring: bool, opens: Option<At>) -> Rectangle {
    let (fill, ink) = if item.on {
        (theme::ISLAND.primary, theme::ISLAND.on_primary)
    } else {
        (
            theme::ISLAND.surface_container_high,
            theme::ISLAND.on_surface,
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
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::SEMIBOLD)
            .elide(),
        Text::new(&item.status)
            .size(theme::text::LABEL_SMALL)
            .color(theme::ISLAND.on_surface_variant)
            .weight(theme::text::MEDIUM)
            .elide(),
    ])
    .width(width - KNOB_INSET - KNOB - ICON_GAP - right - chevron)
    .gap(2.0);

    let press = item.press;

    let knob = match (&press, &opens) {
        (Some(press), Some(_)) => {
            let press = press.clone();

            knob.cursor(Cursor::Pointer)
                .on_click(super::on_left(move || {
                    click_switch(press.clone());
                }))
        }
        _ => knob,
    };

    let mut parts = children![knob, words];

    if opens.is_some() {
        parts.push(Box::new(
            Icon::Forward.on(16.0, theme::ISLAND.on_surface_variant),
        ));
    }

    let switch = Rectangle::new()
        .width(width)
        .height(SWITCH)
        .radius(SWITCH / 2.0)
        .fill(theme::ISLAND.surface_container)
        .padding(Padding {
            top: 0.0,
            right,
            bottom: 0.0,
            left: KNOB_INSET - HALO,
        })
        .align_child(Start, Center)
        .border_if(ring)
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
        Press::Wifi(on) => Network::set_wifi(on),
        Press::Bluetooth { adapter, on } => bluez::power(adapter, on),
        Press::Microphone => Audio::toggle_microphone_mute(),
        Press::Dnd(on) => notifications::set_dnd(on, Instant::now()),
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
                .on_click(super::on_left(|| {
                    Audio::toggle_mute();
                }))
                .child(if muted {
                    Icon::SpeakerMuted.draw(20.0)
                } else {
                    Icon::Speaker(volume).draw(20.0)
                });

            let tone = if muted {
                theme::ISLAND.on_surface_variant
            } else {
                theme::ISLAND.on_surface
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
            theme::ISLAND.on_surface,
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
        theme::ISLAND.on_surface,
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
        .child(Icon::Forward.on(16.0, theme::ISLAND.on_surface_variant));

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
                .color(theme::ISLAND.on_surface_variant)
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
        .fill(theme::ISLAND.surface_container)
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
        .child(
            Text::new(profile.label())
                .size(theme::text::LABEL_SMALL)
                .color(if active {
                    theme::ISLAND.on_surface
                } else {
                    theme::ISLAND.on_surface_variant
                })
                .weight(theme::text::SEMIBOLD),
        );

    if active {
        segment.fill(theme::ISLAND.surface_container_high)
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
    let grid = focus.grid(rows(focus.sub.as_ref()));

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

// a target clicked: the ring hides, since the pointer is what moves now
fn click(act: Act) {
    let visit = hide();
    let focus = Focus::read().of(visit, false);

    set(self::act(focus, act, visit));
}

// hides the ring, giving the visit it hid in
fn hide() -> u64 {
    let visit = IslandService::read().visit();

    let mut focus = Focus::read().of(visit, false);
    focus.shown = false;

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
    let grid = focus.grid(rows(focus.sub.as_ref()));
    let mut focus = settled(focus, &grid);

    let most = list::most(grid.len().saturating_sub(1));

    focus.shown = false;
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

    focus.offset = list::scrolled(focus.offset, count, ringed);
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
        At::WifiSwitch => self::wifi(&Connectivity::read()).press,
        At::BluetoothSwitch => self::bluetooth(&Adapter::read()).press,
        At::Microphone => {
            microphone(modules::on("audio").then(|| Audio::read().microphone_muted())).press
        }
        At::Dnd => {
            let on = modules::on("notifications").then(|| IslandService::read().dnd());

            dnd(on).press
        }
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

/*
 * a write wakes the window even when nothing changed, so only write a real change. Out of a
 * sub-surface, its daemon stops being read for it
 */
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

    if *Focus::read() != focus {
        *Focus::write() = focus;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::bluetooth::Device;

    // with the ring on Cancel, Enter cancels the PIN typed rather than pairing with it
    #[test]
    fn enter_on_the_pin_prompt_presses_what_the_ring_is_on() {
        let prompt = Prompt::Pin {
            device: "/buds".into(),
            pin: "1234".into(),
        };
        let adapter = Adapter {
            radio: Radio::On,
            ..Adapter::default()
        };
        let focus = Focus::default().of(1, true).enter(Subsurface::Bluetooth);
        let grid = focus.grid(bluetooth::rows(&adapter, &prompt));

        let (focus, _) = focus.step(Key::Right, &grid).unwrap();
        let (focus, _) = focus.step(Key::Left, &grid).unwrap();
        assert_eq!(focus.ring(&grid), Some(At::Cancel));

        assert_eq!(typed_pin(Key::Enter, focus.ring(&grid).is_some()), None);
        let (_, act) = focus.step(Key::Enter, &grid).unwrap();
        assert_eq!(act, Some(Act::Press(At::Cancel)));

        // the ring hidden, Enter pairs, and typing goes to the PIN either way
        assert_eq!(typed_pin(Key::Enter, false), Some(Pin::Pair));
        assert_eq!(
            typed_pin(Key::Character('5'), true),
            Some(Pin::Letter(Some('5')))
        );
        assert_eq!(typed_pin(Key::Space, true), None);
    }

    #[test]
    fn the_surface_is_as_tall_as_its_rows() {
        assert_eq!(geometry::CONTROLS.height, HEIGHT);
    }

    #[test]
    fn wifi_says_the_network_it_is_on() {
        let wifi = |radio, uplink| {
            self::wifi(&Connectivity {
                uplink,
                wifi: radio,
            })
        };

        let home = wifi(Radio::On, Some(Uplink::Wifi("home".into())));
        assert_eq!((home.status.as_str(), home.on), ("home", true));
        assert_eq!(home.press, Some(Press::Wifi(false)));

        assert_eq!(wifi(Radio::On, Some(Uplink::Wired)).status, "Not connected");

        let off = wifi(Radio::Off, Some(Uplink::Wired));
        assert_eq!((off.status.as_str(), off.on), ("Off", false));
        assert_eq!(off.press, Some(Press::Wifi(true)));
    }

    #[test]
    fn a_missing_radio_is_unavailable_and_does_nothing() {
        let wifi = self::wifi(&Connectivity::default());
        let bluetooth = self::bluetooth(&Adapter::default());

        for missing in [wifi, bluetooth] {
            assert_eq!(missing.status, "Unavailable");
            assert!(!missing.on);
            assert_eq!(missing.press, None);
        }
    }

    #[test]
    fn bluetooth_names_what_is_connected_and_switches_its_adapter() {
        let peer = |name: &str, connected| Device {
            path: format!("/org/bluez/hci0/dev_{name}"),
            name: name.into(),
            paired: true,
            connected,
            battery: None,
        };
        let adapter = |radio, devices| Adapter {
            radio,
            path: "/org/bluez/hci0".into(),
            discovering: false,
            devices,
        };

        let on = bluetooth(&adapter(
            Radio::On,
            vec![peer("buds", true), peer("mouse", true), peer("pad", false)],
        ));
        assert_eq!(on.status, "buds, mouse");
        assert_eq!(
            on.press,
            Some(Press::Bluetooth {
                adapter: "/org/bluez/hci0".into(),
                on: false
            })
        );

        assert_eq!(bluetooth(&adapter(Radio::On, Vec::new())).status, "On");

        let off = bluetooth(&adapter(Radio::Off, vec![peer("pad", false)]));
        assert_eq!((off.status.as_str(), off.on), ("Off", false));
        assert_eq!(
            off.press,
            Some(Press::Bluetooth {
                adapter: "/org/bluez/hci0".into(),
                on: true
            })
        );
    }

    #[test]
    fn the_microphone_and_dnd_say_whether_they_are_on() {
        assert_eq!(
            (
                microphone(Some(true)).status.as_str(),
                microphone(Some(true)).on
            ),
            ("Muted", false)
        );
        assert_eq!(
            (
                microphone(Some(false)).status.as_str(),
                microphone(Some(false)).on
            ),
            ("On", true)
        );
        assert_eq!(microphone(None).press, None);
        assert_eq!(microphone(None).status, "Unavailable");

        assert_eq!(dnd(Some(true)).press, Some(Press::Dnd(false)));
        assert_eq!(dnd(Some(false)).press, Some(Press::Dnd(true)));
        assert_eq!(dnd(None).press, None);
        assert_eq!(dnd(None).status, "Unavailable");
    }
}
