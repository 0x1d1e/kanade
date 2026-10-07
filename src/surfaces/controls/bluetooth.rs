//! The Controls Surface's Bluetooth sub-surface: the devices paired, connected ones first, then
//! those nearby, to pair, connect, disconnect or forget. Each says what is being done to it, or
//! what last failed. A pairing that needs the user shows its code, or takes the device's PIN, in
//! place of the devices.

use amane::{Center, Column, End, Rectangle, Row, Text, Widget, children};

use super::WIDTH;
use super::focus::{Act, At};
use super::list::{self, GAP, ICON, LIST};
use crate::icon::Icon;
use crate::sources::bluetooth::{Adapter, Device, Prompt, Request, Task};
use crate::sources::system::Radio;
use crate::theme;

const DISCONNECT: f32 = 96.0;
const FORGET: f32 = 76.0;

// the passkey or code, to read off at a glance
const CODE: f32 = 28.0;

/*
 * its targets, one row a device, or the prompt's buttons while a pairing waits on the user. None
 * while the adapter is not on
 */
pub fn rows(adapter: &Adapter, prompt: &Prompt) -> Vec<Vec<(At, f32)>> {
    if adapter.radio != Radio::On {
        return Vec::new();
    }

    match prompt {
        Prompt::Confirm { .. } | Prompt::Pin { .. } => {
            return vec![vec![(At::Cancel, 0.6), (At::Confirm, 0.85)]];
        }
        Prompt::Show { .. } => return vec![vec![(At::Cancel, 0.85)]],
        Prompt::None => {}
    }

    adapter
        .devices
        .iter()
        .map(|device| {
            let at = At::Device(device.path.clone());
            let forget = (At::Forget(device.path.clone()), 0.9);

            if device.connected {
                vec![(at, 0.62), forget]
            } else if device.paired {
                vec![(at, 0.4), forget]
            } else {
                vec![(at, 0.5)]
            }
        })
        .collect()
}

/*
 * the header, then the devices scrolled `offset` down, or the prompt. `ring` is what the ring is
 * on, none while it hides
 */
pub fn devices(
    adapter: &Adapter,
    request: &Request,
    prompt: &Prompt,
    offset: f32,
    ring: Option<&At>,
) -> Column {
    let list: Box<dyn Widget> = match adapter.radio {
        Radio::Missing => Box::new(list::state(
            Icon::Bluetooth,
            "Bluetooth is unavailable",
            "No adapter, or bluetoothd isn\u{2019}t running",
        )),
        Radio::Off => Box::new(list::state(
            Icon::Bluetooth,
            "Bluetooth is off",
            "Turn it on to see devices",
        )),
        Radio::On if *prompt != Prompt::None => Box::new(self::prompt(adapter, prompt, ring)),
        Radio::On if adapter.devices.is_empty() => {
            Box::new(list::state(Icon::Bluetooth, "Looking for devices", ""))
        }
        Radio::On => Box::new(list::list(adapter.devices.len(), offset, |index| {
            Box::new(row(&adapter.devices[index], request, ring))
        })),
    };

    Column::new(vec![
        Box::new(list::header("Bluetooth", adapter.radio, ring)) as Box<dyn Widget>,
        list,
    ])
    .width(WIDTH)
    .gap(GAP)
}

// what a device's row says under its name, and whether that is an error
fn status(device: &Device, request: &Request) -> Option<(String, bool)> {
    if let Some(task) = request.doing(&device.path) {
        let doing = match task {
            Task::Pair => "Pairing\u{2026}",
            Task::Connect => "Connecting\u{2026}",
            Task::Disconnect => "Disconnecting\u{2026}",
            Task::Forget => "Forgetting\u{2026}",
        };

        return Some((doing.into(), false));
    }

    if let Some(task) = request.failed(&device.path) {
        let failed = match task {
            Task::Pair => "Couldn\u{2019}t pair",
            Task::Connect => "Couldn\u{2019}t connect",
            Task::Disconnect => "Couldn\u{2019}t disconnect",
            Task::Forget => "Couldn\u{2019}t forget",
        };

        return Some((failed.into(), true));
    }

    if device.connected {
        let connected = match device.battery {
            Some(percent) => format!("Connected \u{b7} {percent}%"),
            None => String::from("Connected"),
        };

        Some((connected, false))
    } else if device.paired {
        Some((String::from("Not connected"), false))
    } else {
        None
    }
}

/*
 * its name and status; a paired one has a Forget pill, a connected one a Disconnect pill too, which
 * the ring goes on instead and which alone disconnects it. Pressing another pairs or connects it.
 * While something is being done to any device, none of it presses
 */
fn row(device: &Device, request: &Request, ring: Option<&At>) -> Rectangle {
    let busy = request.busy();
    let at = At::Device(device.path.clone());
    let forget = At::Forget(device.path.clone());
    let press = |at: &At| (!busy).then(|| Act::Press(at.clone()));

    let mut trailing: Vec<Box<dyn Widget>> = Vec::new();

    if device.connected {
        trailing.push(Box::new(list::pill(
            list::label("Disconnect"),
            DISCONNECT,
            ring == Some(&at),
            press(&at),
        )));
    }

    if device.paired {
        trailing.push(Box::new(list::pill(
            list::label("Forget"),
            FORGET,
            ring == Some(&forget),
            press(&forget),
        )));
    }

    let status = status(device, request);

    list::row(
        Box::new(Icon::Bluetooth.on(ICON, theme::ISLAND.on_surface)),
        &device.name,
        status
            .as_ref()
            .map(|(status, error)| (status.as_str(), *error)),
        trailing,
        ring == Some(&at) && !device.connected,
        (!device.connected && !busy).then_some(at),
    )
}

/*
 * a pairing waiting on the user, in place of the devices: the code the device shows too, to say
 * whether it matches, the one to type on it, or the device's PIN as it is typed, which pairs once
 * there is one
 */
fn prompt(adapter: &Adapter, prompt: &Prompt, ring: Option<&At>) -> Rectangle {
    let name = |path: &str| {
        adapter
            .devices
            .iter()
            .find(|device| device.path == path)
            .map_or_else(|| String::from("the device"), |device| device.name.clone())
    };

    let (title, code, detail, pairs) = match prompt {
        Prompt::Confirm { device, passkey } => (
            format!("Pair with {}?", name(device)),
            passkey.as_str(),
            "Check it shows the same code",
            Some(true),
        ),
        Prompt::Show { device, code } => (
            format!("Type this code on {}", name(device)),
            code.as_str(),
            "Then press Enter on it",
            None,
        ),
        Prompt::Pin { device, pin } => (
            format!("Type the PIN of {}", name(device)),
            pin.as_str(),
            "Its manual says it, then press Enter",
            Some(!pin.is_empty()),
        ),
        Prompt::None => return Rectangle::new().width(WIDTH).height(LIST),
    };

    let mut buttons = children![list::pill(
        list::label("Cancel"),
        84.0,
        ring == Some(&At::Cancel),
        Some(Act::Press(At::Cancel)),
    )];

    if let Some(ready) = pairs {
        buttons.push(Box::new(list::pill(
            list::label("Pair"),
            84.0,
            ring == Some(&At::Confirm),
            ready.then_some(Act::Press(At::Confirm)),
        )));
    }

    let lines = Column::new(children![
        Text::new(title)
            .size(theme::text::BODY)
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::SEMIBOLD)
            .elide(),
        Text::new(code)
            .size(CODE)
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::SEMIBOLD),
        Text::new(detail)
            .size(theme::text::LABEL_SMALL)
            .color(theme::ISLAND.on_surface_variant)
            .weight(theme::text::MEDIUM),
    ])
    .width(WIDTH)
    .gap(6.0)
    .align(Center);

    Rectangle::new().width(WIDTH).height(LIST).child(
        Column::new(children![
            lines,
            Row::new(buttons).width(WIDTH).gap(8.0).justify(End)
        ])
        .width(WIDTH)
        .gap(GAP),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(name: &str, paired: bool, connected: bool) -> Device {
        Device {
            path: format!("/org/bluez/hci0/dev_{name}"),
            name: name.into(),
            paired,
            connected,
            battery: None,
        }
    }

    fn adapter(devices: Vec<Device>) -> Adapter {
        Adapter {
            radio: Radio::On,
            path: "/org/bluez/hci0".into(),
            discovering: true,
            devices,
        }
    }

    #[test]
    fn a_device_says_how_it_is_and_what_is_being_done_to_it() {
        let mut buds = device("buds", true, true);
        let path = buds.path.clone();
        assert_eq!(
            status(&buds, &Request::Idle),
            Some(("Connected".into(), false))
        );

        buds.battery = Some(80);
        assert_eq!(
            status(&buds, &Request::Idle),
            Some(("Connected \u{b7} 80%".into(), false))
        );

        assert_eq!(
            status(&device("pad", true, false), &Request::Idle),
            Some(("Not connected".into(), false))
        );
        assert_eq!(
            status(&device("speaker", false, false), &Request::Idle),
            None
        );

        let doing = Request::Doing(path.clone(), Task::Disconnect);
        assert_eq!(
            status(&buds, &doing),
            Some(("Disconnecting\u{2026}".into(), false))
        );

        let failed = Request::Failed(path, Task::Connect);
        assert_eq!(
            status(&buds, &failed),
            Some(("Couldn\u{2019}t connect".into(), true))
        );

        // what is asked of one device says nothing of another
        assert_eq!(
            status(&device("pad", true, false), &failed),
            Some(("Not connected".into(), false))
        );
    }

    #[test]
    fn paired_devices_can_be_forgotten_and_nearby_ones_only_paired() {
        let rows = rows(
            &adapter(vec![
                device("buds", true, true),
                device("pad", true, false),
                device("speaker", false, false),
            ]),
            &Prompt::None,
        );

        let targets: Vec<Vec<At>> = rows
            .into_iter()
            .map(|row| row.into_iter().map(|(at, _)| at).collect())
            .collect();

        let path = |name: &str| format!("/org/bluez/hci0/dev_{name}");

        assert_eq!(
            targets,
            [
                vec![At::Device(path("buds")), At::Forget(path("buds"))],
                vec![At::Device(path("pad")), At::Forget(path("pad"))],
                vec![At::Device(path("speaker"))],
            ]
        );
    }

    #[test]
    fn a_prompt_takes_the_place_of_the_devices() {
        let adapter = adapter(vec![device("buds", false, false)]);
        let confirm = Prompt::Confirm {
            device: "/org/bluez/hci0/dev_buds".into(),
            passkey: "012345".into(),
        };
        let show = Prompt::Show {
            device: "/org/bluez/hci0/dev_buds".into(),
            code: "012345".into(),
        };
        let pin = Prompt::Pin {
            device: "/org/bluez/hci0/dev_buds".into(),
            pin: String::new(),
        };

        let targets = |prompt| -> Vec<At> {
            rows(&adapter, prompt)
                .concat()
                .into_iter()
                .map(|(at, _)| at)
                .collect()
        };

        assert_eq!(targets(&confirm), [At::Cancel, At::Confirm]);
        assert_eq!(targets(&show), [At::Cancel]);
        assert_eq!(targets(&pin), [At::Cancel, At::Confirm]);
    }

    #[test]
    fn an_adapter_not_on_lists_nothing() {
        let mut off = adapter(vec![device("buds", true, false)]);
        off.radio = Radio::Off;

        assert!(rows(&off, &Prompt::None).is_empty());
    }
}
