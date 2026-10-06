//! The Controls Surface (plan 7): Wi-Fi, Bluetooth, the microphone and Do Not Disturb as
//! switches, the speaker's volume and the screen's brightness as levels, and the power profile.
//! Each is one press or one drag, never a menu: it is not a settings app. What the machine does not
//! have, or whose daemon is not running, keeps its place, faded, says so, and does nothing. Its
//! header names the apps the privacy cluster stands for.

use std::time::Instant;

use amane::{
    Audio, Brightness, Center, Color, Column, Cursor, End, Network, Padding, Parent, Rectangle,
    Row, Scroll, Service, Start, Text, Widget, children,
};

use super::slider::Slider;
use crate::cluster;
use crate::icon::Icon;
use crate::island::activity::Uplink;
use crate::island::geometry;
use crate::modules;
use crate::sources::bluetooth::{self, Adapter};
use crate::sources::network::Connectivity;
use crate::sources::notifications;
use crate::sources::power::{self, Profile, Profiles};
use crate::sources::privacy::Privacy;
use crate::sources::system::Radio;
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, DISABLED};
use crate::view::bar;

// the content's width, which every row fills
const WIDTH: f32 = geometry::CONTROLS.width - 2.0 * INSET;

// clear of the queued badge in the body's top right corner
const BADGE: f32 = 36.0;

const HEADER: f32 = 20.0;
const GAP: f32 = 14.0;

// a switch, round ends concentric with its knob
const SWITCH: f32 = 48.0;
const SWITCH_GAP: f32 = 8.0;
const KNOB: f32 = 32.0;
const KNOB_INSET: f32 = (SWITCH - KNOB) / 2.0;

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
 * `dnd` is the view's own read of IslandService, so this never reads it again; none while the
 * notifications Module is off, since nothing heeds it then
 */
pub fn surface(dnd: Option<bool>) -> Rectangle {
    let shape = geometry::CONTROLS;

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .child(
            Column::new(children![header(), switches(dnd), levels(), profiles()])
                .width(WIDTH)
                .gap(GAP),
        )
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
        .width(WIDTH - BADGE)
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
fn switches(dnd: Option<bool>) -> Column {
    let wifi = self::wifi(&Connectivity::read());
    let bluetooth = self::bluetooth(&Adapter::read());
    let microphone = self::microphone(Audio::read().microphone_muted());
    let dnd = self::dnd(dnd);

    let width = (WIDTH - SWITCH_GAP) / 2.0;
    let row = |left: Switch, right: Switch| {
        Row::new(children![switch(left, width), switch(right, width)]).gap(SWITCH_GAP)
    };

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
        .filter(|peer| peer.connected)
        .map(|peer| peer.name.as_str())
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

fn microphone(muted: bool) -> Switch {
    Switch {
        icon: if muted {
            Icon::MicrophoneMuted
        } else {
            Icon::Microphone
        },
        name: "Microphone",
        status: String::from(if muted { "Muted" } else { "On" }),
        on: !muted,
        press: Some(Press::Microphone),
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

// its knob filled while on, then its name and status
fn switch(item: Switch, width: f32) -> Rectangle {
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

    let right = 16.0;
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
    .width(width - KNOB_INSET - KNOB - ICON_GAP - right)
    .gap(2.0);

    let switch = Rectangle::new()
        .width(width)
        .height(SWITCH)
        .radius(SWITCH / 2.0)
        .fill(theme::ISLAND.surface_container)
        .padding(Padding {
            top: 0.0,
            right,
            bottom: 0.0,
            left: KNOB_INSET,
        })
        .align_child(Start, Center)
        .child(Row::new(children![knob, words]).gap(ICON_GAP).align(Center));

    match item.press {
        Some(press) => switch
            .cursor(Cursor::Pointer)
            .on_click(super::on_left(move || {
                run(press.clone());
            })),
        None => switch.opacity(DISABLED),
    }
}

fn run(press: Press) {
    match press {
        Press::Wifi(on) => Network::set_wifi(on),
        Press::Bluetooth { adapter, on } => bluetooth::power(adapter, on),
        Press::Microphone => Audio::toggle_microphone_mute(),
        Press::Dnd(on) => notifications::set_dnd(on, Instant::now()),
    }
}

// the speaker, its icon pressed to mute, then the screen
fn levels() -> Column {
    let audio = Audio::read();
    let (volume, muted) = (audio.volume(), audio.muted());
    drop(audio);

    let brightness = Brightness::read();
    let screen = brightness.present().then(|| brightness.percent());
    drop(brightness);

    let speaker = Rectangle::new()
        .width(TARGET)
        .height(TARGET)
        .align_child(Center, Center)
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

    let volume = level(speaker, Some(Slider::Speaker), Some(volume), tone);

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
    );

    Column::new(children![volume, brightness]).gap(LEVEL_GAP)
}

/*
 * its icon, its bar, then its number; the wheel anywhere on the row moves it. None to set leaves
 * the row faded and empty
 */
fn level(icon: Rectangle, slider: Option<Slider>, percent: Option<u8>, tone: Color) -> Rectangle {
    let width = WIDTH - TARGET - NUMBER - 2.0 * ICON_GAP;
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

    let row = Rectangle::new()
        .width(WIDTH)
        .height(TARGET)
        .align_child(Start, Center)
        .child(
            Row::new(children![icon, bar, number])
                .gap(ICON_GAP)
                .align(Center),
        );

    match slider {
        Some(slider) => row.on_scroll(move |Scroll { y, .. }| slider.wheel(y)),
        None => row.opacity(DISABLED),
    }
}

/*
 * the bolt, then every profile, the active one filled. One the machine lacks is faded in its place,
 * and with no daemon running so is the whole row
 */
fn profiles() -> Rectangle {
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

fn segment(profile: Profile, width: f32, active: bool, available: bool) -> Rectangle {
    let segment = Rectangle::new()
        .width(width)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .align_child(Center, Center)
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
                power::set(profile);
            }))
    } else {
        segment.opacity(DISABLED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::activity::Peer;

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
        let peer = |name: &str, connected| Peer {
            path: format!("/org/bluez/hci0/dev_{name}"),
            name: name.into(),
            connected,
            battery: None,
        };
        let adapter = |radio, devices| Adapter {
            radio,
            path: "/org/bluez/hci0".into(),
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
            (microphone(true).status.as_str(), microphone(true).on),
            ("Muted", false)
        );
        assert_eq!(
            (microphone(false).status.as_str(), microphone(false).on),
            ("On", true)
        );

        assert_eq!(dnd(Some(true)).press, Some(Press::Dnd(false)));
        assert_eq!(dnd(Some(false)).press, Some(Press::Dnd(true)));
        assert_eq!(dnd(None).press, None);
        assert_eq!(dnd(None).status, "Unavailable");
    }
}
