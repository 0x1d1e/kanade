//! BlueZ for `connectivity.rs`: the adapter and the devices it knows, and the Transient a device
//! connecting or going away shows.

use amane::{Bus, Service, Value};

use super::connectivity::{Radio, SHOWN};
use crate::island::activity::{Activity, Detail, Id, Kind, Peer, Priority};

const BLUEZ: &str = "org.bluez";

const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const BATTERY: &str = "org.bluez.Battery1";

// Bluetooth as the Controls Surface (#29) shows it; written only by `connectivity::follow`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Adapter {
    pub radio: Radio,

    // paired or connected, connected first, then by name
    pub devices: Vec<Peer>,
}

impl Service for Adapter {
    fn new() -> Self {
        Adapter::default()
    }

    fn listen() {}
}

// the adapter powering, a device connecting, renamed, or its battery
pub fn concerns(path: &str, interface: &str) -> bool {
    path.starts_with("/org/bluez/") && matches!(interface, ADAPTER | DEVICE | BATTERY)
}

// one call lists the adapter and every device at once; nothing when BlueZ is not running
pub fn read() -> Adapter {
    let objects = Bus::system().call(
        BLUEZ,
        "/",
        "org.freedesktop.DBus.ObjectManager",
        "GetManagedObjects",
        &[],
    );

    let mut adapter = Adapter::default();

    let Value::Map(objects) = objects else {
        return adapter;
    };

    for (path, interfaces) in &objects {
        let properties = interfaces.get(ADAPTER);

        // the first wins, most machines have one
        if properties != &Value::Nothing && adapter.radio == Radio::Missing {
            adapter.radio = Radio::of(true, properties.get("Powered").bool());
        }

        if let Some(peer) = peer(path, interfaces) {
            adapter.devices.push(peer);
        }
    }

    adapter
        .devices
        .sort_by_key(|peer| (!peer.connected, peer.name.to_lowercase()));

    adapter
}

// a device worth showing: one nearby that was only seen in a scan is not
fn peer(path: &str, interfaces: &Value) -> Option<Peer> {
    let device = interfaces.get(DEVICE);

    let connected = device.get("Connected").bool();

    if !connected && !device.get("Paired").bool() {
        return None;
    }

    let battery = match interfaces.get(BATTERY).get("Percentage") {
        Value::Nothing => None,
        percent => Some(percent.number() as u8),
    };

    // unnamed devices go by their address, as other Bluetooth menus show them
    let name = match device.get("Alias").text() {
        "" => device.get("Address").text(),
        alias => alias,
    };

    Some(Peer {
        path: path.to_owned(),
        name: name.to_owned(),
        connected,
        battery,
    })
}

/*
 * a device that connects shows; one that disconnects, or is forgotten or its adapter turned off
 * while connected, shows going away. A battery that ticks or a rename shows nothing
 */
pub fn changes(before: &Adapter, now: &Adapter) -> Vec<Activity> {
    let connected = |adapter: &Adapter, path: &str| {
        adapter
            .devices
            .iter()
            .any(|peer| peer.connected && peer.path == path)
    };

    let joined = now
        .devices
        .iter()
        .filter(|peer| peer.connected && !connected(before, &peer.path))
        .cloned();

    let left = before
        .devices
        .iter()
        .filter(|peer| peer.connected && !connected(now, &peer.path))
        .map(|peer| Peer {
            connected: false,
            battery: None,
            ..peer.clone()
        });

    joined.chain(left).map(activity).collect()
}

// one per device, so one that drops and comes back replaces its own
fn activity(peer: Peer) -> Activity {
    let id = Id::new(Kind::Bluetooth, peer.path.clone());

    Activity::transient(id, Priority::Passive, SHOWN).with_detail(Detail::Bluetooth(peer))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::activity::{Interrupt, Lifetime};

    fn peer(name: &str, connected: bool) -> Peer {
        Peer {
            path: format!("/org/bluez/hci0/dev_{name}"),
            name: name.into(),
            connected,
            battery: connected.then_some(80),
        }
    }

    fn with(devices: Vec<Peer>) -> Adapter {
        Adapter {
            radio: Radio::On,
            devices,
        }
    }

    // what each Transient says, as (name, connected)
    fn shown(before: Vec<Peer>, now: Vec<Peer>) -> Vec<(String, bool)> {
        changes(&with(before), &with(now))
            .into_iter()
            .map(|activity| {
                assert_eq!(activity.lifetime(), Lifetime::Transient(SHOWN));
                assert_eq!(activity.interrupt(), Interrupt::Transient);

                match activity.detail() {
                    Detail::Bluetooth(peer) => (peer.name.clone(), peer.connected),
                    detail => panic!("{detail:?}"),
                }
            })
            .collect()
    }

    #[test]
    fn connecting_and_disconnecting_show() {
        let paired = || vec![peer("buds", false)];
        let connected = || vec![peer("buds", true)];

        assert_eq!(shown(paired(), connected()), [("buds".into(), true)]);
        assert_eq!(shown(connected(), paired()), [("buds".into(), false)]);
    }

    // turning the adapter off, or forgetting a device, drops it from the list while connected
    #[test]
    fn a_connected_device_that_vanishes_shows_going_away() {
        assert_eq!(
            shown(vec![peer("buds", true)], Vec::new()),
            [("buds".into(), false)]
        );
    }

    #[test]
    fn each_device_shows_its_own() {
        let before = vec![peer("buds", true), peer("mouse", false)];
        let now = vec![peer("mouse", true), peer("buds", false)];

        let changes = changes(&with(before), &with(now));

        assert_eq!(changes.len(), 2);
        assert_ne!(changes[0].id(), changes[1].id());
    }

    #[test]
    fn a_battery_or_a_rename_shows_nothing() {
        let drained = Peer {
            battery: Some(20),
            name: "renamed".into(),
            ..peer("buds", true)
        };

        assert!(shown(vec![peer("buds", true)], vec![drained]).is_empty());
        assert!(shown(vec![peer("buds", false)], Vec::new()).is_empty());
    }
}
