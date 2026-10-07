//! Network, Bluetooth and power profiles (plan 2, 5.1, 7): `follow` watches the system bus for
//! all three. None of it shows on the island (ADR 0007), so there is never a permanent Wi-Fi
//! indicator: the latest state is kept in `Connectivity`, `Adapter` and `Profiles` for the Controls
//! Surface. Each daemon belongs to its own Module (`network`, `bluetooth`,
//! `power`), and the watcher follows only those that are on; the State of one that is off stays at
//! its default, which Controls shows as missing.
//!
//! Amane's Network and Bluetooth poll every second or two for good once read, so this never reads
//! them. It waits on signals instead and asks a daemon again only when one of its objects changes,
//! comes or goes, or the daemon itself starts or stops: at idle its threads sleep, and a signal
//! about anything else, like an access point's strength, is dropped without a call. Only while the
//! Wi-Fi sub-surface shows are the Wi-Fi device and its access points followed too (`wifi`).

use std::sync::{Once, mpsc};
use std::{iter, mem};

use amane::{Bus, Service, Value};

use super::{bluetooth, network, power, wifi};
use crate::{modules, supervise};

// the bus itself, the only sender of NameOwnerChanged
const BUS: &str = "org.freedesktop.DBus";

// a radio the Controls Surface can switch, or show disabled (plan 7)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Radio {
    // no Wi-Fi device or Bluetooth adapter, or its daemon is not running
    #[default]
    Missing,

    Off,
    On,
}

impl Radio {
    pub fn of(present: bool, on: bool) -> Radio {
        match (present, on) {
            (false, _) => Radio::Missing,
            (true, false) => Radio::Off,
            (true, true) => Radio::On,
        }
    }
}

// which daemon said something changed
#[derive(Debug, Clone, Copy, PartialEq)]
enum Daemon {
    NetworkManager,

    // NetworkManager's Wi-Fi device, its access points or saved profiles, for the Wi-Fi
    // sub-surface only
    Wifi,

    BlueZ,
    PowerProfiles,
}

// the signals that can say so, each followed on its own
#[derive(Debug, Clone, Copy)]
enum Watch {
    // a property of an object that already exists
    Properties,

    // an object coming or going, like a device found or forgotten
    Added,
    Removed,

    // a daemon starting, stopping or restarting, which no object says
    Owner,
}

impl Watch {
    const ALL: [Watch; 4] = [
        Watch::Properties,
        Watch::Added,
        Watch::Removed,
        Watch::Owner,
    ];

    // its interface and name
    fn signal(self) -> (&'static str, &'static str) {
        match self {
            Watch::Properties => ("org.freedesktop.DBus.Properties", "PropertiesChanged"),
            Watch::Added => ("org.freedesktop.DBus.ObjectManager", "InterfacesAdded"),
            Watch::Removed => ("org.freedesktop.DBus.ObjectManager", "InterfacesRemoved"),
            Watch::Owner => ("org.freedesktop.DBus", "NameOwnerChanged"),
        }
    }
}

/*
 * which daemon to ask again, if any. Objects coming and going matter only for BlueZ:
 * NetworkManager's root object says when its devices or primary connection change, while its
 * access points come and go all the time
 */
fn route(watch: Watch, sender: &str, path: &str, arguments: &[Value]) -> Option<Daemon> {
    let first = arguments.first().map_or("", Value::text);

    match watch {
        Watch::Properties if network::concerns(path, first) => Some(Daemon::NetworkManager),
        Watch::Properties if wifi::concerns(path, first) => Some(Daemon::Wifi),
        Watch::Properties if bluetooth::concerns(path, first) => Some(Daemon::BlueZ),
        Watch::Properties if power::concerns(path, first) => Some(Daemon::PowerProfiles),

        // the first argument is the object
        Watch::Added | Watch::Removed if bluetooth::object(first) => Some(Daemon::BlueZ),

        // only the bus itself says who owns a name, the first argument
        Watch::Owner if sender == BUS && first == network::NAME => Some(Daemon::NetworkManager),
        Watch::Owner if sender == BUS && first == bluetooth::BLUEZ => Some(Daemon::BlueZ),
        Watch::Owner if sender == BUS && first == power::NAME => Some(Daemon::PowerProfiles),

        _ => None,
    }
}

// which daemons to follow, each by its Module
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Daemons {
    network: bool,
    bluetooth: bool,
    power: bool,
}

impl Daemons {
    fn follows(self, daemon: Daemon) -> bool {
        match daemon {
            Daemon::NetworkManager | Daemon::Wifi => self.network,
            Daemon::BlueZ => self.bluetooth,
            Daemon::PowerProfiles => self.power,
        }
    }

    fn followed(self) -> Vec<Daemon> {
        [Daemon::NetworkManager, Daemon::BlueZ, Daemon::PowerProfiles]
            .into_iter()
            .filter(|&daemon| self.follows(daemon))
            .collect()
    }

    // objects coming and going matter only for BlueZ, see `route`
    fn watches(self, watch: Watch) -> bool {
        match watch {
            Watch::Added | Watch::Removed => self.bluetooth,
            Watch::Properties | Watch::Owner => true,
        }
    }
}

static SPAWNED: Once = Once::new();

// called by each of the three Modules that is on; the first call starts the one watcher for all
pub fn spawn() {
    SPAWNED.call_once(|| {
        let daemons = Daemons {
            network: modules::on("network"),
            bluetooth: modules::on("bluetooth"),
            power: modules::on("power"),
        };

        supervise::spawn("system", move || follow(daemons));
    });
}

// runs on its own thread for good, or until the system bus goes away
fn follow(daemons: Daemons) {
    /*
     * signals queue up to 64 while unread, and a full queue stops the bus reading anything, replies
     * included: a read made while a burst arrives, as switching networks sends, would wait for
     * good. So each watch has a thread that only sorts its signals, and never calls
     */
    let (sender, changed) = mpsc::channel();

    for watch in Watch::ALL
        .into_iter()
        .filter(|&watch| daemons.watches(watch))
    {
        let (interface, name) = watch.signal();

        // subscribed before the first read, so no change falls between them
        let mut signals = Bus::system().signals(interface, name);
        let sender = sender.clone();

        // a restart goes on with the same subscription, so no signal is lost to it
        supervise::spawn("system bus watch", move || {
            for signal in signals.by_ref() {
                // the Wi-Fi device and its access points say much, heard only while the
                // sub-surface shows
                let Some(daemon) = route(watch, signal.sender(), signal.path(), signal.arguments())
                    .filter(|&daemon| daemons.follows(daemon))
                    .filter(|&daemon| daemon != Daemon::Wifi || wifi::wanted())
                else {
                    continue;
                };

                if sender.send(daemon).is_err() {
                    return;
                }
            }
        });
    }

    // once every watch has ended, so has the bus
    drop(sender);

    let mut first = true;

    supervise::run("system", || {
        if mem::take(&mut first) {
            // the first reads only set where things start, so starting the shell shows nothing
            if daemons.network {
                *network::Connectivity::write() = network::read();
            }

            if daemons.bluetooth {
                *bluetooth::Adapter::write() = bluetooth::read();
            }

            if daemons.power {
                *power::Profiles::write() = power::read();
            }
        } else {
            // a restart asks every daemon again, for what the panic dropped
            refresh_all(daemons.followed());
        }

        while let Ok(first) = changed.recv() {
            // a burst, or what came during the last read, asks once
            refresh_all(iter::once(first).chain(changed.try_iter()).collect());
        }
    });
}

fn refresh_all(daemons: Vec<Daemon>) {
    if daemons.contains(&Daemon::NetworkManager) {
        refresh(network::read());
    }

    // NetworkManager restarting, or the Wi-Fi switch, changes the networks too
    if daemons.contains(&Daemon::NetworkManager) || daemons.contains(&Daemon::Wifi) {
        wifi::refresh();
    }

    if daemons.contains(&Daemon::BlueZ) {
        refresh(bluetooth::read());
    }

    if daemons.contains(&Daemon::PowerProfiles) {
        refresh(power::read());
    }
}

// keeps `now` for Controls; a read that changes nothing writes nothing, so redraws nothing
fn refresh<S: Service + PartialEq>(now: S) {
    if *S::read() != now {
        *S::write() = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(texts: &[&str]) -> Vec<Value> {
        texts
            .iter()
            .map(|text| Value::Text((*text).into()))
            .collect()
    }

    const BUDS: &str = "/org/bluez/hci0/dev_26_03_27_B7_A2_ED";

    // NetworkManager's objects live elsewhere; BlueZ's manager is at "/"
    fn objects(watch: Watch, object: &str) -> Option<Daemon> {
        route(watch, ":1.13", "/", &text(&[object, ""]))
    }

    fn owner(sender: &str, name: &str, old: &str, new: &str) -> Option<Daemon> {
        route(
            Watch::Owner,
            sender,
            "/org/freedesktop/DBus",
            &text(&[name, old, new]),
        )
    }

    #[test]
    fn properties_go_to_their_daemon() {
        let changed = |path, interface| route(Watch::Properties, ":1.5", path, &text(&[interface]));

        assert_eq!(
            changed(network::ROOT, network::NAME),
            Some(Daemon::NetworkManager)
        );
        assert_eq!(
            changed(
                "/org/freedesktop/NetworkManager/AccessPoint/1",
                "org.freedesktop.NetworkManager.AccessPoint"
            ),
            Some(Daemon::Wifi)
        );
        assert_eq!(changed(BUDS, "org.bluez.Device1"), Some(Daemon::BlueZ));
        assert_eq!(
            changed("/org/bluez/hci0", "org.bluez.Adapter1"),
            Some(Daemon::BlueZ)
        );
    }

    #[test]
    fn power_profiles_go_to_their_daemon() {
        let changed = |path, interface| route(Watch::Properties, ":1.7", path, &text(&[interface]));

        assert_eq!(
            changed(power::PATH, power::NAME),
            Some(Daemon::PowerProfiles)
        );
        assert_eq!(
            owner(BUS, power::NAME, "", ":1.85"),
            Some(Daemon::PowerProfiles)
        );
    }

    #[test]
    fn properties_of_anything_else_go_nowhere() {
        let changed = |path, interface| route(Watch::Properties, ":1.5", path, &text(&[interface]));

        assert_eq!(
            changed(
                "/org/freedesktop/NetworkManager/Devices/2",
                "org.freedesktop.NetworkManager.Device.Statistics"
            ),
            None
        );
        assert_eq!(changed(BUDS, "org.bluez.MediaControl1"), None);
        assert_eq!(
            changed(
                "/org/freedesktop/UPower/devices/battery_BAT1",
                "org.freedesktop.UPower.Device"
            ),
            None
        );
    }

    #[test]
    fn a_bluez_device_coming_or_going_asks_bluez() {
        assert_eq!(objects(Watch::Added, BUDS), Some(Daemon::BlueZ));
        assert_eq!(objects(Watch::Removed, BUDS), Some(Daemon::BlueZ));
        assert_eq!(
            objects(Watch::Added, "/org/bluez/hci0"),
            Some(Daemon::BlueZ)
        );
    }

    #[test]
    fn other_objects_coming_or_going_ask_nothing() {
        let access_point = "/org/freedesktop/NetworkManager/AccessPoint/7";

        assert_eq!(objects(Watch::Added, access_point), None);
        assert_eq!(objects(Watch::Removed, access_point), None);
    }

    #[test]
    fn bluez_starting_or_stopping_asks_bluez() {
        assert_eq!(
            owner(BUS, bluetooth::BLUEZ, "", ":1.80"),
            Some(Daemon::BlueZ)
        );
        assert_eq!(
            owner(BUS, bluetooth::BLUEZ, ":1.80", ""),
            Some(Daemon::BlueZ)
        );
    }

    #[test]
    fn networkmanager_restarting_asks_networkmanager() {
        let restarted = Some(Daemon::NetworkManager);

        assert_eq!(owner(BUS, network::NAME, ":1.9", ""), restarted);
        assert_eq!(owner(BUS, network::NAME, "", ":1.81"), restarted);
    }

    #[test]
    fn other_owners_or_senders_ask_nothing() {
        assert_eq!(owner(BUS, "org.freedesktop.UPower", "", ":1.82"), None);
        assert_eq!(owner(BUS, ":1.83", "", ":1.83"), None);

        // anyone may send a signal by that name, only the bus's counts
        assert_eq!(owner(":1.84", bluetooth::BLUEZ, "", ":1.84"), None);
    }
}
