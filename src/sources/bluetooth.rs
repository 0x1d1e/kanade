//! BlueZ for `system.rs`: the adapter and the devices it knows, for the Controls Surface.

use std::thread;

use amane::{Argument, Bus, Service, Value};

use super::system::Radio;
use crate::island::activity::Peer;

pub const BLUEZ: &str = "org.bluez";

const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const BATTERY: &str = "org.bluez.Battery1";

// Bluetooth as the Controls Surface shows it; written only by `system::follow`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Adapter {
    pub radio: Radio,

    // BlueZ's object for it, empty while Missing
    pub path: String,

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
    object(path) && matches!(interface, ADAPTER | DEVICE | BATTERY)
}

// one of BlueZ's adapters or devices
pub fn object(path: &str) -> bool {
    path.starts_with("/org/bluez/")
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
            adapter.path.clone_from(path);
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

// BlueZ answers with the change, which `system::follow` reads back
pub fn power(adapter: String, on: bool) {
    thread::spawn(move || {
        Bus::system().set_property(BLUEZ, &adapter, ADAPTER, "Powered", Argument::from(on));
    });
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
