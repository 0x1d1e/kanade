//! NetworkManager for `connectivity.rs`: what the machine is online through, and the Transient
//! that joining or leaving it shows.

use amane::{Argument, Bus, Service, Value};

use super::connectivity::{Radio, SHOWN};
use crate::island::activity::{Activity, Connection, Detail, Id, Kind, Priority, Uplink};

const NAME: &str = "org.freedesktop.NetworkManager";

const ROOT: &str = "/org/freedesktop/NetworkManager";

const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";

// NetworkManager's numbers for an active connection that is up, and for a Wi-Fi device
const ACTIVATED: f64 = 2.0;
const WIFI_DEVICE: f64 = 2.0;

// the network as the Controls Surface (#29) shows it; written only by `connectivity::follow`
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

/*
 * joining an Uplink, a new one included, shows it; going offline shows the one left. The Wi-Fi
 * switch alone shows nothing: while wired it changes nothing, and on Wi-Fi going offline says it
 */
pub fn changes(before: &Connectivity, now: &Connectivity) -> Vec<Activity> {
    let change = match (&before.uplink, &now.uplink) {
        (before, Some(now)) if before.as_ref() != Some(now) => Connection {
            uplink: now.clone(),
            connected: true,
        },
        (Some(before), None) => Connection {
            uplink: before.clone(),
            connected: false,
        },
        _ => return Vec::new(),
    };

    // one, so leaving and joining in a row replaces rather than queues
    let id = Id::new(Kind::Network, "uplink");

    vec![Activity::transient(id, Priority::Passive, SHOWN).with_detail(Detail::Network(change))]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::activity::{Interrupt, Lifetime};

    fn on(uplink: Option<Uplink>) -> Connectivity {
        Connectivity {
            uplink,
            wifi: Radio::On,
        }
    }

    fn wifi(ssid: &str) -> Option<Uplink> {
        Some(Uplink::Wifi(ssid.into()))
    }

    fn shown(before: Option<Uplink>, now: Option<Uplink>) -> Option<Connection> {
        let mut changes = changes(&on(before), &on(now));

        assert!(changes.len() <= 1);

        let activity = changes.pop()?;

        assert_eq!(activity.lifetime(), Lifetime::Transient(SHOWN));
        assert_eq!(activity.interrupt(), Interrupt::Transient);

        match activity.detail() {
            Detail::Network(connection) => Some(connection.clone()),
            detail => panic!("{detail:?}"),
        }
    }

    fn joined(uplink: Option<Uplink>) -> Option<Connection> {
        Some(Connection {
            uplink: uplink.unwrap(),
            connected: true,
        })
    }

    fn left(uplink: Option<Uplink>) -> Option<Connection> {
        Some(Connection {
            uplink: uplink.unwrap(),
            connected: false,
        })
    }

    #[test]
    fn joining_and_leaving_show() {
        assert_eq!(shown(None, wifi("home")), joined(wifi("home")));
        assert_eq!(shown(wifi("home"), None), left(wifi("home")));
        assert_eq!(
            shown(None, Some(Uplink::Wired)),
            joined(Some(Uplink::Wired))
        );
    }

    #[test]
    fn a_new_uplink_shows_the_new_one() {
        assert_eq!(shown(wifi("home"), wifi("cafe")), joined(wifi("cafe")));
        assert_eq!(
            shown(wifi("home"), Some(Uplink::Wired)),
            joined(Some(Uplink::Wired))
        );
    }

    #[test]
    fn staying_put_shows_nothing() {
        assert_eq!(shown(None, None), None);
        assert_eq!(shown(wifi("home"), wifi("home")), None);
    }

    // the Wi-Fi switch is for Controls to show, not the island
    #[test]
    fn the_wifi_switch_alone_shows_nothing() {
        let wired = Some(Uplink::Wired);
        let off = Connectivity {
            uplink: wired.clone(),
            wifi: Radio::Off,
        };

        assert!(changes(&on(wired), &off).is_empty());
    }
}
