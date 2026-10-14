//! Device events: a Bluetooth device connecting or going, the charger plugged in or out, the
//! default sound output changing, a Wi-Fi network joined. Each shows as a Transient Glance on the
//! focused output's Island, like a workspace switch, while `island.device_events` is on.
//!
//! Each Module's own Service says what happened: `Adapter` for `bluetooth`, `BatteryLevel` for
//! `battery`, `Mixer` for `audio`, `Connectivity` for `network`. A watcher of it (started by that
//! Module, so a Module that is off has none) tells what changed from what it saw last. The first
//! thing it sees is only where things start, so starting the shell or a daemon shows nothing, and a
//! source that goes missing forgets what it saw. Switching the config off still keeps up, so
//! switching it on shows nothing stale. One Activity per class, so a burst takes one place, and
//! the same event again within `BURST` shows nothing.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use kanade_runtime::{MutexExt, service::Service};

use super::audio::Mixer;
use super::battery::BatteryLevel;
use super::bluetooth::Adapter;
use super::network::Connectivity;
use super::system::Radio;
use crate::config;
use crate::island::activity::{
    Activity, Class, Detail, Hardware, Id, Interrupt, Kind, Lifetime, Priority, Scope, Uplink,
};
use crate::island::service::IslandService;

// the same event within this shows once: a device that flaps, a network that rejoins
const BURST: Duration = Duration::from_secs(10);

// it is read rather than glanced at, so it stays this many times as long as the OSD
const LINGER: u32 = 3;

// what each watcher has seen, and when each class last showed
#[derive(Default)]
struct Seen {
    bluetooth: Bluetooth,
    charger: Charger,
    output: Output,
    wifi: Wifi,
    shown: HashMap<Class, (Hardware, Instant)>,
}

static SEEN: Mutex<Option<Seen>> = Mutex::new(None);

// a watcher of `Adapter`
pub fn follow_bluetooth() {
    let adapter = Adapter::read().clone();

    run(|seen, now| seen.bluetooth.step(&adapter, now));
}

// a watcher of `BatteryLevel`
pub fn follow_charger() {
    let level = *BatteryLevel::read();

    run(|seen, _| seen.charger.step(level));
}

// a watcher of `Mixer`
pub fn follow_output() {
    let default = default_output(&Mixer::read());

    run(|seen, _| seen.output.step(default));
}

// a watcher of `Connectivity`
pub fn follow_wifi() {
    let connectivity = Connectivity::read().clone();

    run(|seen, _| seen.wifi.step(&connectivity));
}

// the event `step` finds, unless it is a repeat, posted while the config has them on
fn run(step: impl FnOnce(&mut Seen, Instant) -> Option<Hardware>) {
    let now = Instant::now();
    let mut seen = SEEN.locked();
    let seen = seen.get_or_insert_with(Seen::default);

    let Some(event) = step(seen, now).filter(|event| seen.fresh(event, now)) else {
        return;
    };

    if config::get().device_events {
        IslandService::write().post(activity(event), now);
    }
}

impl Seen {
    // whether `event` is not the one its class showed within `BURST`; remembers it
    fn fresh(&mut self, event: &Hardware, now: Instant) -> bool {
        let last = self.shown.get(&event.class);

        if last.is_some_and(|(last, at)| last == event && now.duration_since(*at) < BURST) {
            return false;
        }

        self.shown.insert(event.class, (event.clone(), now));
        true
    }
}

// Glance like a workspace switch: it shows when nothing more important does, and interrupts nothing
fn activity(event: Hardware) -> Activity {
    let (kind, key) = match event.class {
        Class::Bluetooth => (Kind::Bluetooth, "device"),
        Class::Charger => (Kind::Battery, "charger"),
        Class::Output => (Kind::Volume, "output"),
        Class::Wifi => (Kind::Network, "wifi"),
    };

    Activity::new(
        Id::new(kind, key),
        Priority::Glance,
        Lifetime::Transient(config::get().osd * LINGER),
        Scope::FocusedOutput,
        Interrupt::None,
    )
    .expect("the config bounds osd above zero")
    .with_detail(Detail::Hardware(event))
}

// a Bluetooth device, by its object, as connected: its name and its battery if it reports one
type Connected = BTreeMap<String, (String, Option<u8>)>;

#[derive(Default)]
struct Bluetooth {
    // none until the adapter was seen on
    connected: Option<Connected>,

    // the one device the last event was for, and when, to add its battery once it reports it
    announced: Option<(String, Instant)>,
}

impl Bluetooth {
    fn step(&mut self, adapter: &Adapter, now: Instant) -> Option<Hardware> {
        // an adapter turned off disconnects everything, which is no news
        if adapter.radio != Radio::On {
            self.connected = (adapter.radio == Radio::Off).then(Connected::new);
            return None;
        }

        let connected: Connected = adapter
            .devices
            .iter()
            .filter(|device| device.connected)
            .map(|device| (device.path.clone(), (device.name.clone(), device.battery)))
            .collect();

        let before = self.connected.replace(connected.clone())?;

        let came: Vec<_> = connected
            .iter()
            .filter(|(path, _)| !before.contains_key(*path))
            .collect();

        if !came.is_empty() {
            self.announced = match came[..] {
                [(path, _)] => Some((path.clone(), now)),
                _ => None,
            };

            return Some(bluetooth(
                "connected",
                came.iter().map(|(_, (name, battery))| (name, *battery)),
            ));
        }

        let went: Vec<_> = before
            .iter()
            .filter(|(path, _)| !connected.contains_key(*path))
            .collect();

        if !went.is_empty() {
            return Some(bluetooth(
                "disconnected",
                went.iter().map(|(_, (name, _))| (name, None)),
            ));
        }

        // a battery often reports a moment after the connection, which the event then shows
        let (path, at) = self.announced.as_ref()?;
        let (name, battery) = connected.get(path)?;

        (now.duration_since(*at) < BURST
            && battery.is_some()
            && before.get(path).is_some_and(|(_, was)| was.is_none()))
        .then(|| bluetooth("connected", [(name, *battery)].into_iter()))
    }
}

/*
 * "WH-1000XM5 connected" over "Bluetooth, 80%"; several at once, "2 devices connected" over their
 * names, without a battery, which one would not say whose
 */
fn bluetooth<'a>(verb: &str, devices: impl Iterator<Item = (&'a String, Option<u8>)>) -> Hardware {
    let devices: Vec<_> = devices.collect();

    let (title, line) = match devices[..] {
        [(name, battery)] => (
            format!("{name} {verb}"),
            match battery {
                Some(percent) => format!("Bluetooth, {percent}%"),
                None => String::from("Bluetooth"),
            },
        ),
        _ => (
            format!("{} devices {verb}", devices.len()),
            devices
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        ),
    };

    Hardware {
        class: Class::Bluetooth,
        title,
        line,
    }
}

#[derive(Default)]
struct Charger {
    // whether it was plugged in, none until a battery with a number was seen
    plugged: Option<bool>,
}

impl Charger {
    fn step(&mut self, level: BatteryLevel) -> Option<Hardware> {
        let Some(percent) = level.percent else {
            self.plugged = None;
            return None;
        };

        let before = self.plugged.replace(level.charging)?;

        (before != level.charging).then(|| Hardware {
            class: Class::Charger,
            title: String::from(if level.charging {
                "Plugged in"
            } else {
                "Unplugged"
            }),
            line: format!("{percent}%"),
        })
    }
}

// the name of the sound output that is the default, none while there is none
fn default_output(mixer: &Mixer) -> Option<Option<String>> {
    (!mixer.outputs.is_empty()).then(|| {
        mixer
            .outputs
            .iter()
            .find(|device| device.default)
            .map(|device| device.name.clone())
    })
}

#[derive(Default)]
struct Output {
    // none until outputs were seen; inside, the default's name, none while it has none
    default: Option<Option<String>>,
}

impl Output {
    // `now` is none while no output exists
    fn step(&mut self, now: Option<Option<String>>) -> Option<Hardware> {
        let before = std::mem::replace(&mut self.default, now.clone())?;

        // a default that is gone, and no other yet, says nothing
        match (before, now?) {
            (Some(before), Some(now)) if before != now => Some(Hardware {
                class: Class::Output,
                title: format!("Sound to {now}"),
                line: String::from("Output device"),
            }),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Wifi {
    // none until a Wi-Fi device was seen; inside, the network the machine reaches out through
    joined: Option<Option<String>>,
}

impl Wifi {
    fn step(&mut self, connectivity: &Connectivity) -> Option<Hardware> {
        if connectivity.wifi == Radio::Missing {
            self.joined = None;
            return None;
        }

        let now = match &connectivity.uplink {
            Some(Uplink::Wifi(name)) => Some(name.clone()),
            _ => None,
        };

        let before = self.joined.replace(now.clone())?;
        let name = now.filter(|name| before.as_ref() != Some(name))?;

        Some(Hardware {
            class: Class::Wifi,
            title: String::from("Wi-Fi connected"),
            line: name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::bluetooth::Device;

    fn device(path: &str, name: &str, connected: bool, battery: Option<u8>) -> Device {
        Device {
            path: path.to_owned(),
            name: name.to_owned(),
            paired: true,
            connected,
            battery,
        }
    }

    fn adapter(radio: Radio, devices: Vec<Device>) -> Adapter {
        Adapter {
            radio,
            devices,
            ..Adapter::default()
        }
    }

    fn ago(seconds: u64, now: Instant) -> Instant {
        now.checked_sub(Duration::from_secs(seconds)).unwrap()
    }

    // what is already connected as the shell, or BlueZ, starts is no event
    #[test]
    fn the_first_look_is_a_baseline_and_a_change_after_it_an_event() {
        let (mut seen, now) = (Bluetooth::default(), Instant::now());
        let buds = || device("/a", "Buds", true, Some(80));

        assert_eq!(seen.step(&adapter(Radio::Missing, vec![]), now), None);
        assert_eq!(seen.step(&adapter(Radio::On, vec![buds()]), now), None);

        let came = seen.step(
            &adapter(Radio::On, vec![buds(), device("/b", "Mouse", true, None)]),
            now,
        );
        let came = came.expect("a device connected");
        assert_eq!(
            (came.title.as_str(), came.line.as_str()),
            ("Mouse connected", "Bluetooth")
        );

        let went = seen
            .step(
                &adapter(Radio::On, vec![device("/a", "Buds", false, None)]),
                now,
            )
            .expect("both went");
        assert_eq!(went.title, "2 devices disconnected");
    }

    #[test]
    fn a_connection_shows_its_battery_once_the_device_reports_it() {
        let (mut seen, now) = (Bluetooth::default(), Instant::now());
        let on = |battery| adapter(Radio::On, vec![device("/a", "Buds", true, battery)]);

        seen.step(&adapter(Radio::On, vec![]), now);

        assert_eq!(seen.step(&on(None), now).unwrap().line, "Bluetooth");
        assert_eq!(seen.step(&on(None), now), None);
        assert_eq!(
            seen.step(&on(Some(80)), now).unwrap().line,
            "Bluetooth, 80%"
        );

        // a later reading of the battery is not news
        assert_eq!(seen.step(&on(Some(79)), now), None);

        // nor is one long after the connection
        let mut seen = Bluetooth::default();
        seen.step(&adapter(Radio::On, vec![]), ago(60, now));
        seen.step(&on(None), ago(60, now));
        assert_eq!(seen.step(&on(Some(80)), now), None);
    }

    #[test]
    fn an_adapter_turned_off_or_missing_makes_no_event() {
        let (mut seen, now) = (Bluetooth::default(), Instant::now());
        let buds = |connected| vec![device("/a", "Buds", connected, None)];

        seen.step(&adapter(Radio::On, buds(true)), now);
        assert_eq!(seen.step(&adapter(Radio::Off, buds(false)), now), None);

        // it comes back and reconnects: that is news
        assert!(seen.step(&adapter(Radio::On, buds(true)), now).is_some());

        // BlueZ going away forgets, so what is connected when it returns is the baseline
        assert_eq!(seen.step(&adapter(Radio::Missing, vec![]), now), None);
        assert_eq!(seen.step(&adapter(Radio::On, buds(true)), now), None);
    }

    #[test]
    fn the_charger_shows_when_it_goes_in_or_out_and_not_while_it_is_unknown() {
        let mut seen = Charger::default();
        let level = |percent, charging| BatteryLevel { percent, charging };

        assert_eq!(seen.step(level(Some(50), false)), None);

        let plugged = seen.step(level(Some(51), true)).unwrap();
        assert_eq!(
            (plugged.title.as_str(), plugged.line.as_str()),
            ("Plugged in", "51%")
        );
        assert_eq!(seen.step(level(Some(52), true)), None);
        assert_eq!(
            seen.step(level(Some(52), false)).unwrap().title,
            "Unplugged"
        );

        // no number is no battery to say anything of, and what it was is forgotten
        assert_eq!(seen.step(level(None, true)), None);
        assert_eq!(seen.step(level(Some(52), false)), None);
    }

    #[test]
    fn a_new_default_output_shows_and_a_missing_one_does_not() {
        let mut seen = Output::default();
        let named = |name: &str| Some(Some(name.to_owned()));

        assert_eq!(seen.step(None), None);
        assert_eq!(seen.step(named("Speakers")), None);
        assert_eq!(seen.step(named("Speakers")), None);
        assert_eq!(seen.step(named("Buds")).unwrap().title, "Sound to Buds");

        // gone with no other yet, then another: shown when it is chosen
        assert_eq!(seen.step(Some(None)), None);
        assert_eq!(seen.step(named("Speakers")), None);
        assert_eq!(seen.step(named("Buds")).unwrap().title, "Sound to Buds");

        // the audio graph gone forgets
        assert_eq!(seen.step(None), None);
        assert_eq!(seen.step(named("Speakers")), None);
    }

    #[test]
    fn joining_a_wifi_network_shows_and_wired_or_leaving_does_not() {
        let mut seen = Wifi::default();
        let state = |wifi, uplink| Connectivity { uplink, wifi };
        let at = |name: &str| Some(Uplink::Wifi(name.to_owned()));

        assert_eq!(seen.step(&state(Radio::Missing, at("Home"))), None);
        assert_eq!(seen.step(&state(Radio::On, at("Home"))), None);
        assert_eq!(seen.step(&state(Radio::On, Some(Uplink::Wired))), None);
        assert_eq!(seen.step(&state(Radio::On, None)), None);

        let joined = seen.step(&state(Radio::On, at("Cafe"))).unwrap();
        assert_eq!(
            (joined.title.as_str(), joined.line.as_str()),
            ("Wi-Fi connected", "Cafe")
        );
        assert_eq!(seen.step(&state(Radio::On, at("Cafe"))), None);
    }

    #[test]
    fn the_same_event_within_a_burst_shows_once() {
        let mut seen = Seen::default();
        let now = Instant::now();
        let event = Hardware {
            class: Class::Wifi,
            title: String::from("Wi-Fi connected"),
            line: String::from("Home"),
        };
        let other = Hardware {
            class: Class::Charger,
            ..event.clone()
        };

        assert!(seen.fresh(&event, ago(5, now)));
        assert!(seen.fresh(&other, ago(5, now)));
        assert!(!seen.fresh(&event, now));
        assert!(seen.fresh(&event, now + BURST));
    }
}
