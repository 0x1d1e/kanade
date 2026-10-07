//! NetworkManager's Wi-Fi networks for the Controls Surface's Wi-Fi sub-surface (#130): the
//! networks in range, joining one, with a password when it needs one, and leaving it.
//!
//! Only the sub-surface reads them: `watch` asks for a scan as it opens, and `system::follow` asks
//! again for each change of the Wi-Fi device, an access point or the saved profiles while it
//! shows (`wanted`). Closed, those signals are dropped without a call, as before.
//!
//! A password lives in a `Secret`, which never prints, and only until NetworkManager has it: a
//! profile made to join with one is kept in memory and saved only once the network takes it, so a
//! wrong password leaves nothing behind. NetworkManager asks a secret agent, like nm-applet, for a
//! password it lacks or that failed; with none running, the join fails and says so here.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use amane::{Argument, Bus, Service, Value};

use super::network::{NAME, ROOT};
use crate::island::presentation::Surface;
use crate::island::service::IslandService;

const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const ACCESS_POINT: &str = "org.freedesktop.NetworkManager.AccessPoint";
const SETTINGS: &str = "org.freedesktop.NetworkManager.Settings";
const PROFILE: &str = "org.freedesktop.NetworkManager.Settings.Connection";

const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const DEVICES: &str = "/org/freedesktop/NetworkManager/Devices/";
const ACCESS_POINTS: &str = "/org/freedesktop/NetworkManager/AccessPoint/";

const WIRELESS_PROFILE: &str = "802-11-wireless";

// NetworkManager's numbers: a Wi-Fi device, and a device's states between choosing and being up
const WIFI_DEVICE: f64 = 2.0;
const PREPARING: f64 = 40.0;
const ACTIVATED_DEVICE: f64 = 100.0;

// a device failing to come up, and why: it had no password, or a wrong one
const FAILED: f64 = 120.0;
const NO_SECRETS: f64 = 7.0;

// an access point's flags: one with privacy, and the ways its WPA and RSN flags say it keys
const PRIVACY: u32 = 0x1;
const KEY_PSK: u32 = 0x100;
const KEY_8021X: u32 = 0x200;
const KEY_SAE: u32 = 0x400;
const KEY_OWE: u32 = 0x800;

/*
 * a password, or what is typed of one. It never prints, so no log, panic or `Debug` of a Service
 * holding one shows it
 */
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn push(&mut self, letter: char) {
        self.0.push(letter);
    }

    pub fn pop(&mut self) {
        self.0.pop();
    }

    // in characters, which is what the field draws a dot for
    pub fn len(&self) -> usize {
        self.0.chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    // what WPA takes: 8 to 63 characters, or the key itself as 64 hex digits
    pub fn fits(&self) -> bool {
        let length = self.0.len();

        (8..=63).contains(&length)
            || (length == 64 && self.0.chars().all(|letter| letter.is_ascii_hexdigit()))
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("Secret(..)")
    }
}

// how a network keeps others out
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    Open,

    // open, but encrypted (Enhanced Open)
    Owe,

    // a password, WPA2 style; WPA3 networks that also take WPA2 ones are this
    Psk,

    // a password, WPA3 only
    Sae,

    // enterprise sign-in or WEP, which this does not join
    Unsupported,
}

impl Security {
    // from an access point's flags, the best way both it and NetworkManager know
    fn of(flags: u32, wpa: u32, rsn: u32) -> Security {
        let keys = wpa | rsn;

        if keys & KEY_8021X != 0 {
            Security::Unsupported
        } else if keys & KEY_PSK != 0 {
            Security::Psk
        } else if keys & KEY_SAE != 0 {
            Security::Sae
        } else if keys & KEY_OWE != 0 {
            Security::Owe
        } else if flags & PRIVACY != 0 {
            Security::Unsupported
        } else {
            Security::Open
        }
    }

    pub fn locked(self) -> bool {
        matches!(self, Security::Psk | Security::Sae | Security::Unsupported)
    }

    pub fn password(self) -> bool {
        matches!(self, Security::Psk | Security::Sae)
    }

    // NetworkManager's key management for it, none for an open network
    fn key_management(self) -> Option<&'static str> {
        match self {
            Security::Psk => Some("wpa-psk"),
            Security::Sae => Some("sae"),
            Security::Owe => Some("owe"),
            Security::Open | Security::Unsupported => None,
        }
    }
}

// how the machine is on a network
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Link {
    #[default]
    None,
    Joining,
    Joined,
}

// one network in range, its access points as one
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Network {
    pub ssid: String,

    // its strongest access point's signal, 0 to 3 bars
    pub bars: u8,

    pub security: Security,
    pub link: Link,

    // its saved profile, none when never joined
    pub profile: Option<String>,

    // the strongest of its access points, which a join asks for
    pub access_point: String,
}

// the Wi-Fi device's networks for the sub-surface; written only while it shows
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Networks {
    // NetworkManager's object for the Wi-Fi device, empty without one
    pub device: String,

    // joined first, then saved, then by signal and name
    pub list: Vec<Network>,
}

impl Service for Networks {
    fn new() -> Self {
        Networks::default()
    }

    fn listen() {}
}

// why a join failed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    WrongPassword,
    Other,
}

// the last join asked from here, until another or a disconnect
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Join {
    #[default]
    Idle,
    Joining(String),
    Failed(String, Failure),
}

impl Service for Join {
    fn new() -> Self {
        Join::default()
    }

    fn listen() {}
}

impl Join {
    // how the join of `ssid` failed, if it was the last one asked
    pub fn failed(&self, ssid: &str) -> Option<Failure> {
        match self {
            Join::Failed(failed, failure) if failed == ssid => Some(*failure),
            _ => None,
        }
    }

    pub fn joining(&self, ssid: &str) -> bool {
        matches!(self, Join::Joining(joining) if joining == ssid)
    }
}

// the visit the sub-surface shows in, 0 while it does not
static WATCHED: AtomicU64 = AtomicU64::new(0);

// the last join or disconnect, so an older join that ends after it says nothing
static ASKED: AtomicU64 = AtomicU64::new(0);

/*
 * the sub-surface opening in `visit`: the networks are read now and on every change from here on,
 * and the device scans for new ones
 */
pub fn watch(visit: u64) {
    if WATCHED.swap(visit, Ordering::Relaxed) == visit {
        return;
    }

    thread::spawn(|| {
        scan();
        refresh();
    });
}

// the sub-surface closing; the island closing ends a watch too, see `wanted`
pub fn unwatch() {
    WATCHED.store(0, Ordering::Relaxed);
}

// while the sub-surface shows; it lasts one visit of the Controls Surface
pub fn wanted() -> bool {
    let watched = WATCHED.load(Ordering::Relaxed);
    let island = IslandService::read();

    watched != 0 && island.surface() == Some(Surface::Controls) && island.visit() == watched
}

/*
 * the device, one of its access points, or the saved profiles changing, which only the
 * sub-surface cares about; NetworkManager's root object is `network::concerns`
 */
pub fn concerns(path: &str, interface: &str) -> bool {
    let device = path.starts_with(DEVICES) && matches!(interface, DEVICE | WIRELESS);
    let access_point = path.starts_with(ACCESS_POINTS) && interface == ACCESS_POINT;
    let profiles = path == SETTINGS_PATH && interface == SETTINGS;

    device || access_point || profiles
}

// reads the networks again, writing only a change; nothing while the sub-surface is closed
pub fn refresh() {
    if !wanted() {
        return;
    }

    let networks = read();

    if *Networks::read() != networks {
        *Networks::write() = networks;
    }
}

// asks the device to look for networks; NetworkManager refuses one right after another
fn scan() {
    let device = device();

    if !device.is_empty() {
        Bus::system().call(
            NAME,
            &device,
            WIRELESS,
            "RequestScan",
            &[Argument::Map(BTreeMap::new())],
        );
    }
}

// the first Wi-Fi device, empty when there is none
fn device() -> String {
    let bus = Bus::system();

    bus.call(NAME, ROOT, NAME, "GetDevices", &[])
        .list()
        .iter()
        .map(Value::text)
        .find(|device| bus.property(NAME, device, DEVICE, "DeviceType").number() == WIFI_DEVICE)
        .unwrap_or_default()
        .to_owned()
}

// an access point as seen, before same-named ones are one network
#[derive(Debug, Clone, PartialEq)]
struct Seen {
    path: String,
    ssid: String,
    strength: u8,
    security: Security,
}

fn read() -> Networks {
    let device = device();

    if device.is_empty() {
        return Networks::default();
    }

    let bus = Bus::system();

    let seen: Vec<Seen> = bus
        .call(NAME, &device, WIRELESS, "GetAllAccessPoints", &[])
        .list()
        .iter()
        .filter_map(|path| seen(path.text()))
        .collect();

    let state = bus.property(NAME, &device, DEVICE, "State").number();
    let active = bus.property(NAME, &device, WIRELESS, "ActiveAccessPoint");

    let link = if state == ACTIVATED_DEVICE {
        Link::Joined
    } else if (PREPARING..ACTIVATED_DEVICE).contains(&state) {
        Link::Joining
    } else {
        Link::None
    };

    let on = seen
        .iter()
        .find(|seen| seen.path == active.text())
        .map(|seen| (seen.ssid.clone(), link));

    Networks {
        device,
        list: networks(seen, &profiles(), on),
    }
}

// one access point, none for a hidden one, which has no name to show
fn seen(path: &str) -> Option<Seen> {
    let properties = Bus::system().call(
        NAME,
        path,
        "org.freedesktop.DBus.Properties",
        "GetAll",
        &[Argument::from(ACCESS_POINT)],
    );

    let ssid = ssid(properties.get("Ssid"))?;
    let flag = |name| properties.get(name).number() as u32;

    Some(Seen {
        path: path.to_owned(),
        ssid,
        strength: properties.get("Strength").number() as u8,
        security: Security::of(flag("Flags"), flag("WpaFlags"), flag("RsnFlags")),
    })
}

// raw bytes, almost always UTF-8
fn ssid(bytes: &Value) -> Option<String> {
    let bytes: Vec<u8> = bytes
        .list()
        .iter()
        .map(|byte| byte.number() as u8)
        .collect();

    let ssid = String::from_utf8_lossy(&bytes).into_owned();

    (!ssid.is_empty()).then_some(ssid)
}

// the saved Wi-Fi profiles, by network, oldest first as NetworkManager lists them
fn profiles() -> Vec<(String, String)> {
    let bus = Bus::system();

    bus.call(NAME, SETTINGS_PATH, SETTINGS, "ListConnections", &[])
        .list()
        .iter()
        .filter_map(|path| {
            let settings = bus.call(NAME, path.text(), PROFILE, "GetSettings", &[]);

            Some((joins(&settings)?, path.text().to_owned()))
        })
        .collect()
}

/*
 * the network a profile's `settings` join, none for one that is not Wi-Fi or that hosts a network
 * rather than joining one, like a hotspot of the same name
 */
fn joins(settings: &Value) -> Option<String> {
    let wireless = settings.get(WIRELESS_PROFILE);
    let mode = wireless.get("mode").text();

    if settings.get("connection").get("type").text() != WIRELESS_PROFILE
        || !matches!(mode, "" | "infrastructure")
    {
        return None;
    }

    ssid(wireless.get("ssid"))
}

// a signal from 0 to 100 as 0 to 3 bars, as the Wi-Fi icon draws them
fn bars(strength: u8) -> u8 {
    match strength {
        0..=24 => 0,
        25..=49 => 1,
        50..=74 => 2,
        _ => 3,
    }
}

/*
 * one network per name, at its strongest access point; the one the device is on first, then saved
 * ones, then by bars and name. `on` is the network the device is on and how far
 */
fn networks(
    seen: Vec<Seen>,
    profiles: &[(String, String)],
    on: Option<(String, Link)>,
) -> Vec<Network> {
    let mut strongest: BTreeMap<String, Seen> = BTreeMap::new();

    for seen in seen {
        match strongest.get(&seen.ssid) {
            Some(kept) if kept.strength >= seen.strength => {}
            _ => {
                strongest.insert(seen.ssid.clone(), seen);
            }
        }
    }

    let mut list: Vec<Network> = strongest
        .into_values()
        .map(|seen| Network {
            bars: bars(seen.strength),
            security: seen.security,
            link: match &on {
                Some((ssid, link)) if *ssid == seen.ssid => *link,
                _ => Link::None,
            },

            // the newest of its profiles, which NetworkManager would choose too
            profile: profiles
                .iter()
                .rev()
                .find(|(ssid, _)| *ssid == seen.ssid)
                .map(|(_, path)| path.clone()),

            access_point: seen.path,
            ssid: seen.ssid,
        })
        .collect();

    list.sort_by(|a, b| {
        let on = |network: &Network| network.link == Link::None;

        on(a)
            .cmp(&on(b))
            .then(a.profile.is_none().cmp(&b.profile.is_none()))
            .then(b.bars.cmp(&a.bars))
            .then_with(|| a.ssid.to_lowercase().cmp(&b.ssid.to_lowercase()))
    });

    list
}

/*
 * joins `network`, with `secret` for one that needs a new password: a saved profile is used as it
 * is, otherwise one is made in memory and saved once the network takes it, replacing older ones
 */
pub fn join(device: &str, network: &Network, secret: Option<Secret>) {
    let asked = ASKED.fetch_add(1, Ordering::Relaxed) + 1;

    let ssid = network.ssid.clone();
    let device = device.to_owned();
    let network = network.clone();

    set(asked, Join::Joining(ssid.clone()));

    thread::spawn(move || {
        let outcome = joined(&device, &network, secret);

        set(
            asked,
            match outcome {
                Ok(()) => Join::Idle,
                Err(failure) => Join::Failed(ssid, failure),
            },
        );
    });
}

fn joined(device: &str, network: &Network, secret: Option<Secret>) -> Result<(), Failure> {
    let bus = Bus::system();

    /*
     * subscribed before asking, so the end of a quick join is not missed. The device says why it
     * failed; the active connection only says the device went down
     */
    let mut changes = bus.signals(DEVICE, "StateChanged");

    let (made, active) = match (&network.profile, secret) {
        (Some(profile), None) => {
            let active = bus.call(
                NAME,
                ROOT,
                NAME,
                "ActivateConnection",
                &[
                    Argument::Path(profile.clone()),
                    Argument::Path(device.to_owned()),
                    Argument::Path(network.access_point.clone()),
                ],
            );

            (None, active.text().to_owned())
        }
        (_, secret) => {
            let options = BTreeMap::from([(String::from("persist"), Argument::from("memory"))]);

            let reply = bus.call(
                NAME,
                ROOT,
                NAME,
                "AddAndActivateConnection2",
                &[
                    settings(network, secret.as_ref()),
                    Argument::Path(device.to_owned()),
                    Argument::Path(network.access_point.clone()),
                    Argument::Map(options),
                ],
            );

            let made = reply.list().first().map(Value::text).unwrap_or_default();
            let active = reply.list().get(1).map(Value::text).unwrap_or_default();

            (
                (!made.is_empty()).then(|| made.to_owned()),
                active.to_owned(),
            )
        }
    };

    let ended = if active.is_empty() {
        Err(Failure::Other)
    } else {
        let mut begun = false;

        changes
            .find_map(|signal| {
                let state = signal.arguments().first()?.number();
                let reason = signal.arguments().get(2)?.number();

                (signal.path() == device).then(|| ended(&mut begun, state, reason))?
            })
            .unwrap_or(Err(Failure::Other))
    };

    drop(changes);

    if let Some(made) = made {
        if ended.is_ok() {
            bus.call(NAME, &made, PROFILE, "Save", &[]);

            // a profile with the old password, which would otherwise come back
            if let Some(old) = &network.profile {
                bus.call(NAME, old, PROFILE, "Delete", &[]);
            }
        } else {
            bus.call(NAME, &made, PROFILE, "Delete", &[]);
        }
    }

    ended
}

/*
 * how the device's state change ends a join, none while it is still on its way. Changes before it
 * starts preparing are the network it leaves going down
 */
fn ended(begun: &mut bool, state: f64, reason: f64) -> Option<Result<(), Failure>> {
    *begun |= state == PREPARING;

    if !*begun {
        None
    } else if state == ACTIVATED_DEVICE {
        Some(Ok(()))
    } else if state == FAILED {
        Some(Err(if reason == NO_SECRETS {
            Failure::WrongPassword
        } else {
            Failure::Other
        }))
    } else if state < PREPARING {
        Some(Err(Failure::Other))
    } else {
        None
    }
}

// a new profile for `network`, `secret` its password
fn settings(network: &Network, secret: Option<&Secret>) -> Argument {
    let text = |text: &str| Argument::from(text);

    let mut groups = BTreeMap::from([
        (
            String::from("connection"),
            BTreeMap::from([
                (String::from("id"), text(&network.ssid)),
                (String::from("type"), text(WIRELESS_PROFILE)),
            ]),
        ),
        (
            String::from(WIRELESS_PROFILE),
            BTreeMap::from([(
                String::from("ssid"),
                Argument::Bytes(network.ssid.as_bytes().to_vec()),
            )]),
        ),
    ]);

    if let Some(key_management) = network.security.key_management() {
        let mut security = BTreeMap::from([(String::from("key-mgmt"), text(key_management))]);

        if let Some(secret) = secret {
            security.insert(String::from("psk"), text(&secret.0));
        }

        groups.insert(String::from("802-11-wireless-security"), security);
    }

    Argument::Groups(groups)
}

// leaves the network the device is on; NetworkManager joins nothing on its own until asked to
pub fn disconnect(device: &str) {
    let asked = ASKED.fetch_add(1, Ordering::Relaxed) + 1;
    let device = device.to_owned();

    set(asked, Join::Idle);

    thread::spawn(move || {
        Bus::system().call(NAME, &device, DEVICE, "Disconnect", &[]);
    });
}

// the outcome of the last thing asked, dropped when something newer was asked since
fn set(asked: u64, join: Join) {
    if ASKED.load(Ordering::Relaxed) == asked && *Join::read() != join {
        *Join::write() = join;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(path: &str, ssid: &str, strength: u8) -> Seen {
        Seen {
            path: path.into(),
            ssid: ssid.into(),
            strength,
            security: Security::Psk,
        }
    }

    fn names(list: &[Network]) -> Vec<&str> {
        list.iter().map(|network| network.ssid.as_str()).collect()
    }

    fn profile(kind: &str, ssid: &str, mode: Option<&str>) -> Value {
        let text = |text: &str| Value::Text(text.into());
        let mut wireless = BTreeMap::from([(
            String::from("ssid"),
            Value::List(
                ssid.bytes()
                    .map(|byte| Value::Number(byte.into()))
                    .collect(),
            ),
        )]);

        if let Some(mode) = mode {
            wireless.insert("mode".into(), text(mode));
        }

        Value::Map(BTreeMap::from([
            (
                String::from("connection"),
                Value::Map(BTreeMap::from([(String::from("type"), text(kind))])),
            ),
            (String::from(WIRELESS_PROFILE), Value::Map(wireless)),
        ]))
    }

    #[test]
    fn only_a_profile_that_joins_a_wifi_network_is_saved_for_it() {
        let wifi = WIRELESS_PROFILE;

        assert_eq!(joins(&profile(wifi, "home", None)).as_deref(), Some("home"));
        assert_eq!(
            joins(&profile(wifi, "home", Some("infrastructure"))).as_deref(),
            Some("home")
        );

        // a hotspot or other network this hosts, and what is not Wi-Fi
        for mode in ["ap", "adhoc", "mesh"] {
            assert_eq!(joins(&profile(wifi, "home", Some(mode))), None);
        }

        assert_eq!(joins(&profile("802-3-ethernet", "home", None)), None);
    }

    #[test]
    fn the_network_joined_comes_first_then_saved_then_strongest() {
        let list = networks(
            vec![
                seen("/1", "cafe", 90),
                seen("/2", "home", 30),
                seen("/3", "office", 60),
                seen("/4", "attic", 10),
                seen("/5", "Bakery", 95),
            ],
            &[("office".into(), "/s/1".into())],
            Some(("home".into(), Link::Joined)),
        );

        assert_eq!(names(&list), ["home", "office", "Bakery", "cafe", "attic"]);
        assert_eq!(list[0].link, Link::Joined);
        assert_eq!(list[1].profile.as_deref(), Some("/s/1"));
        assert!(list[2..].iter().all(|network| network.profile.is_none()));
    }

    #[test]
    fn same_bars_go_by_name_whatever_the_case() {
        let list = networks(
            vec![
                seen("/1", "beta", 80),
                seen("/2", "Alpha", 90),
                seen("/3", "gamma", 60),
            ],
            &[],
            None,
        );

        assert_eq!(names(&list), ["Alpha", "beta", "gamma"]);
    }

    #[test]
    fn access_points_of_one_network_are_one_at_the_strongest() {
        let list = networks(
            vec![
                seen("/1", "home", 40),
                seen("/2", "home", 80),
                seen("/3", "home", 60),
            ],
            &[
                ("home".into(), "/s/old".into()),
                ("home".into(), "/s/new".into()),
            ],
            None,
        );

        assert_eq!(list.len(), 1);
        assert_eq!(list[0].access_point, "/2");
        assert_eq!(list[0].bars, 3);
        assert_eq!(list[0].profile.as_deref(), Some("/s/new"));
    }

    #[test]
    fn a_network_being_joined_is_first_too() {
        let list = networks(
            vec![seen("/1", "cafe", 90), seen("/2", "home", 30)],
            &[("cafe".into(), "/s/1".into())],
            Some(("home".into(), Link::Joining)),
        );

        assert_eq!(names(&list), ["home", "cafe"]);
        assert_eq!(list[0].link, Link::Joining);
    }

    #[test]
    fn security_is_read_from_the_flags() {
        assert_eq!(Security::of(0, 0, 0), Security::Open);
        assert_eq!(Security::of(PRIVACY, 0, KEY_PSK), Security::Psk);
        assert_eq!(Security::of(PRIVACY, 0, KEY_PSK | KEY_SAE), Security::Psk);
        assert_eq!(Security::of(PRIVACY, 0, KEY_SAE), Security::Sae);
        assert_eq!(Security::of(0, 0, KEY_OWE), Security::Owe);
        assert_eq!(Security::of(PRIVACY, KEY_8021X, 0), Security::Unsupported);

        // privacy with no WPA is WEP
        assert_eq!(Security::of(PRIVACY, 0, 0), Security::Unsupported);
    }

    #[test]
    fn a_password_fits_wpa() {
        let secret = |text: &str| Secret(text.into());

        assert!(!secret("1234567").fits());
        assert!(secret("12345678").fits());
        assert!(secret(&"a".repeat(63)).fits());
        assert!(!secret(&"g".repeat(64)).fits());
        assert!(secret(&"0f".repeat(32)).fits());
        assert!(!secret(&"a".repeat(65)).fits());
    }

    #[test]
    fn a_secret_never_prints() {
        let secret = Secret("hunter22".into());

        assert!(!format!("{secret:?}").contains("hunter22"));
        assert!(!format!("{:?}", Some(secret)).contains("hunter22"));
    }

    #[test]
    fn a_join_ends_up_or_down_and_says_why() {
        // the network it leaves goes down first, which is not the join's end
        let mut begun = false;
        assert_eq!(ended(&mut begun, 110.0, 0.0), None);
        assert_eq!(ended(&mut begun, 30.0, 0.0), None);
        assert_eq!(ended(&mut begun, PREPARING, 0.0), None);
        assert_eq!(ended(&mut begun, 50.0, 0.0), None);
        assert_eq!(ended(&mut begun, ACTIVATED_DEVICE, 0.0), Some(Ok(())));

        let mut begun = true;
        assert_eq!(
            ended(&mut begun, FAILED, NO_SECRETS),
            Some(Err(Failure::WrongPassword))
        );
        assert_eq!(ended(&mut begun, FAILED, 8.0), Some(Err(Failure::Other)));
        assert_eq!(ended(&mut begun, 30.0, 0.0), Some(Err(Failure::Other)));
    }

    #[test]
    fn only_the_device_its_networks_and_profiles_concern_the_sub_surface() {
        assert!(concerns(
            "/org/freedesktop/NetworkManager/Devices/3",
            WIRELESS
        ));
        assert!(concerns(
            "/org/freedesktop/NetworkManager/Devices/3",
            DEVICE
        ));
        assert!(concerns(
            "/org/freedesktop/NetworkManager/AccessPoint/40",
            ACCESS_POINT
        ));
        assert!(concerns(SETTINGS_PATH, SETTINGS));

        assert!(!concerns(ROOT, NAME));
        assert!(!concerns(
            "/org/freedesktop/NetworkManager/Devices/3",
            "org.freedesktop.NetworkManager.Device.Statistics"
        ));
    }
}
