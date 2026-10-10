//! NetworkManager for `system.rs`: what the machine is online through, for the Controls Surface.

use crate::bus::{Argument, Bus, Value};
use kanade_runtime::service::Service;
use kanade_runtime::worker;

use super::system::Radio;
use crate::island::activity::Uplink;

pub const NAME: &str = "org.freedesktop.NetworkManager";

pub const ROOT: &str = "/org/freedesktop/NetworkManager";

const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";

// NetworkManager's numbers for an active connection that is up, and for a Wi-Fi device
const ACTIVATED: f64 = 2.0;
const WIFI_DEVICE: f64 = 2.0;

// the network as the Controls Surface shows it; written only by `system::follow`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Connectivity {
    // none while offline
    pub uplink: Option<Uplink>,

    pub wifi: Radio,
}

impl Service for Connectivity {
    fn new() -> Self {
        Connectivity::default()
    }

    fn listen() {}
}

/*
 * the root object says when its primary connection, its state or the Wi-Fi switch changes, which
 * is all `read` asks; each device and access point has its own object and says much more
 */
pub fn concerns(path: &str, interface: &str) -> bool {
    path == ROOT && interface == NAME
}

// asks NetworkManager from scratch; everything is missing when it is not running
pub fn read() -> Connectivity {
    let bus = Bus::system();

    let primary = bus.property(NAME, ROOT, NAME, "PrimaryConnection");
    let enabled = bus.property(NAME, ROOT, NAME, "WirelessEnabled").bool();

    Connectivity {
        uplink: uplink(primary.text()),
        wifi: Radio::of(has_wifi(), enabled),
    }
}

// turns the Wi-Fi radio on or off off the view thread; `read` shows it once NetworkManager says so
pub fn set_wifi(enabled: bool) {
    worker::run(move || {
        Bus::system().set_property(NAME, ROOT, NAME, "WirelessEnabled", Argument::from(enabled));
    });
}

// the connection that carries the default route, "/" while there is none
fn uplink(path: &str) -> Option<Uplink> {
    if path.is_empty() || path == "/" {
        return None;
    }

    let active = properties(path, ACTIVE);

    // still joining, or already leaving
    if active.get("State").number() != ACTIVATED {
        return None;
    }

    let name = active.get("Id").text().to_owned();

    Some(match active.get("Type").text() {
        "802-3-ethernet" => Uplink::Wired,
        "802-11-wireless" => {
            Uplink::Wifi(ssid(active.get("SpecificObject").text()).unwrap_or(name))
        }
        _ => Uplink::Other(name),
    })
}

// the joined network's own name, which its saved profile may name otherwise
fn ssid(access_point: &str) -> Option<String> {
    let properties = properties(access_point, "org.freedesktop.NetworkManager.AccessPoint");

    // raw bytes, almost always UTF-8
    let bytes: Vec<u8> = properties
        .get("Ssid")
        .list()
        .iter()
        .map(|byte| byte.number() as u8)
        .collect();

    let ssid = String::from_utf8_lossy(&bytes).into_owned();

    (!ssid.is_empty()).then_some(ssid)
}

fn has_wifi() -> bool {
    let bus = Bus::system();

    bus.call(NAME, ROOT, NAME, "GetDevices", &[])
        .list()
        .iter()
        .any(|device| {
            let kind = bus.property(
                NAME,
                device.text(),
                "org.freedesktop.NetworkManager.Device",
                "DeviceType",
            );

            kind.number() == WIFI_DEVICE
        })
}

fn properties(path: &str, interface: &str) -> Value {
    Bus::system().call(
        NAME,
        path,
        "org.freedesktop.DBus.Properties",
        "GetAll",
        &[Argument::from(interface)],
    )
}
