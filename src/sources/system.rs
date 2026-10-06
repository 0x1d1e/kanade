//! Network, Bluetooth and power profiles (plan 2, 5.1, 7): `follow` watches the system bus for
//! all three. Joining or leaving a network, or a Bluetooth device connecting or going away, shows
//! as a short Transient on the focused island; nothing about them stays on it, so there is never a
//! permanent Wi-Fi indicator. The latest state is kept in `Connectivity`, `Adapter` and `Profiles`
//! for the Controls Surface.
//!
//! Amane's Network and Bluetooth poll every second or two for good once read, so this never reads
//! them. It waits on signals instead and asks a daemon again only when one of its objects changes,
//! comes or goes, or the daemon itself starts or stops: at idle its threads sleep, and a signal
//! about anything else, like an access point's strength, is dropped without a call.

use std::sync::mpsc;
use std::time::{Duration, Instant};
use std::{iter, thread};

use amane::{Bus, Service, Value};

use super::{bluetooth, network, power};
use crate::island::activity::Activity;
use crate::island::service::IslandService;

// the bus itself, the only sender of NameOwnerChanged
const BUS: &str = "org.freedesktop.DBus";

// longer than the default OSD, so a network's name can be read; shorter than a toast, nothing to act on
pub const SHOWN: Duration = Duration::from_millis(2000);

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

// runs on its own thread for good, or until the system bus goes away
pub fn follow() {
    /*
     * signals queue up to 64 while unread, and a full queue stops the bus reading anything, replies
     * included: a read made while a burst arrives, as switching networks sends, would wait for
     * good. So each watch has a thread that only sorts its signals, and never calls
     */
    let (sender, changed) = mpsc::channel();

    for watch in Watch::ALL {
        let (interface, name) = watch.signal();

        // subscribed before the first read, so no change falls between them
        let signals = Bus::system().signals(interface, name);
        let sender = sender.clone();

        thread::spawn(move || {
            for signal in signals {
                let Some(daemon) = route(watch, signal.sender(), signal.path(), signal.arguments())
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

    // the first reads only set where things start, so starting the shell shows nothing
    *network::Connectivity::write() = network::read();
    *bluetooth::Adapter::write() = bluetooth::read();
    *power::Profiles::write() = power::read();

    while let Ok(first) = changed.recv() {
        // a burst, or what came during the last read, asks once
        let daemons: Vec<Daemon> = iter::once(first).chain(changed.try_iter()).collect();

        if daemons.contains(&Daemon::NetworkManager) {
            refresh(network::read(), network::changes);
        }

        if daemons.contains(&Daemon::BlueZ) {
            refresh(bluetooth::read(), bluetooth::changes);
        }

        // a profile switched shows on Controls only
        if daemons.contains(&Daemon::PowerProfiles) {
            refresh(power::read(), |_, _| Vec::new());
        }
    }
}

/*
 * keeps `now` for Controls and posts what changed since the last read. A read that changes nothing
 * writes nothing, so a signal about something Kanade does not show redraws nothing
 */
fn refresh<S: Service + PartialEq>(now: S, changes: fn(&S, &S) -> Vec<Activity>) {
    let posts = {
        let before = S::read();

        if *before == now {
            return;
        }

        changes(&before, &now)
    };

    *S::write() = now;

    if posts.is_empty() {
        return;
    }

    let at = Instant::now();
    let mut island = IslandService::write();

    for activity in posts {
        island.post(activity, at);
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
                "/org/freedesktop/NetworkManager/AccessPoint/1",
                "org.freedesktop.NetworkManager.AccessPoint"
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
