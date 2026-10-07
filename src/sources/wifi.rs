//! NetworkManager's Wi-Fi networks for the Controls Surface's Wi-Fi sub-surface (#130): the
//! networks in range, joining one, with a password when it needs one, and leaving it.
//!
//! Only the sub-surface reads them: `watch` asks for a scan as it opens, and `system::follow` asks
//! again for each change of the Wi-Fi device, an access point or the saved profiles while it
//! shows (`wanted`). Closed, those signals are dropped without a call, as before. A join waits on
//! what it hears there too (`hear`): its device, its active connection and NetworkManager itself.
//!
//! A password lives in a `Secret`, which never prints, and only until NetworkManager has it: a
//! profile made to join with one is kept in memory and saved only once the network takes it, so a
//! wrong password leaves nothing behind. NetworkManager asks a secret agent, like nm-applet, for a
//! password it lacks or that failed; with none running, the join fails and says so here.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use amane::{Argument, Bus, Service, Value};

use super::network::{NAME, ROOT};
use crate::island::presentation::Surface;
use crate::island::service::IslandService;

pub const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const ACCESS_POINT: &str = "org.freedesktop.NetworkManager.AccessPoint";
const SETTINGS: &str = "org.freedesktop.NetworkManager.Settings";
const PROFILE: &str = "org.freedesktop.NetworkManager.Settings.Connection";
pub const ATTEMPT: &str = "org.freedesktop.NetworkManager.Connection.Active";

const BUS: &str = "org.freedesktop.DBus";
const BUS_PATH: &str = "/org/freedesktop/DBus";

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

// an active connection's states once it is up, and once it is over
const UP: f64 = 2.0;
const OVER: f64 = 4.0;

/*
 * how long a join that is over waits for why the device failed: NetworkManager says it before
 * the join is over, but the two are heard by different watches
 */
const LATE: Duration = Duration::from_secs(1);

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
    /*
     * the NetworkManager that made these objects, by its unique name: their paths are its own, so a
     * join or disconnect asks it, never one that replaced it
     */
    pub owner: String,

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

/*
 * the last join or disconnect, so an older join that ends after it says nothing. Held while its
 * outcome is written, so a newer one cannot be asked between checking and writing
 */
static ASKED: Mutex<u64> = Mutex::new(0);

/*
 * the joins waiting on NetworkManager, each by what it asked, told what `system::follow` hears for
 * them. Its watches are subscribed for good, so a join starts no thread of its own to listen, and
 * nothing it leaves waits on the bus
 */
static LISTENING: Mutex<Vec<(u64, Sender<Heard>)>> = Mutex::new(Vec::new());

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
    let owner = owner();
    let device = device(&owner);

    if !device.is_empty() {
        Bus::system().call(
            &owner,
            &device,
            WIRELESS,
            "RequestScan",
            &[Argument::Map(BTreeMap::new())],
        );
    }
}

/*
 * the NetworkManager running, by its unique name, empty when none is. Its object paths are its own,
 * so everything read or asked of one goes to it, never to a NetworkManager that replaced it
 */
fn owner() -> String {
    Bus::system()
        .call(BUS, BUS_PATH, BUS, "GetNameOwner", &[Argument::from(NAME)])
        .text()
        .to_owned()
}

// `owner`'s first Wi-Fi device, empty when there is none
fn device(owner: &str) -> String {
    let bus = Bus::system();

    bus.call(owner, ROOT, NAME, "GetDevices", &[])
        .list()
        .iter()
        .map(Value::text)
        .find(|device| bus.property(owner, device, DEVICE, "DeviceType").number() == WIFI_DEVICE)
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
    let bus = Bus::system();

    // all of it read from one NetworkManager; one replaced meanwhile answers nothing more
    let owner = owner();
    let device = device(&owner);

    if owner.is_empty() || device.is_empty() {
        return Networks::default();
    }

    let seen: Vec<Seen> = bus
        .call(&owner, &device, WIRELESS, "GetAllAccessPoints", &[])
        .list()
        .iter()
        .filter_map(|path| seen(&owner, path.text()))
        .collect();

    let state = bus.property(&owner, &device, DEVICE, "State").number();
    let active = bus.property(&owner, &device, WIRELESS, "ActiveAccessPoint");

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

    let profiles = profiles(&owner);

    Networks {
        owner,
        device,
        list: networks(seen, &profiles, on),
    }
}

// one of `owner`'s access points, none for a hidden one, which has no name to show
fn seen(owner: &str, path: &str) -> Option<Seen> {
    let properties = Bus::system().call(
        owner,
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
fn profiles(owner: &str) -> Vec<(String, String)> {
    let bus = Bus::system();

    bus.call(owner, SETTINGS_PATH, SETTINGS, "ListConnections", &[])
        .list()
        .iter()
        .filter_map(|path| {
            let settings = bus.call(owner, path.text(), PROFILE, "GetSettings", &[]);

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
 * joins `network`, one of `networks`, with `secret` for one that needs a new password: a saved
 * profile is used as it is, otherwise one is made in memory and saved once the network takes it,
 * replacing older ones. All of it is asked of the NetworkManager the networks were read from
 */
pub fn join(networks: &Networks, network: &Network, secret: Option<Secret>) {
    let asked = ask();

    let ssid = network.ssid.clone();
    let owner = networks.owner.clone();
    let device = networks.device.clone();
    let network = network.clone();

    set(asked, Join::Joining(ssid.clone()));

    thread::spawn(move || {
        let outcome = joined(asked, &owner, &device, &network, secret);

        set(
            asked,
            match outcome {
                Ended::Up => Join::Idle,
                Ended::Failed(failure) => Join::Failed(ssid, failure),
                Ended::Gone => Join::Failed(ssid, Failure::Other),
            },
        );
    });
}

fn joined(
    asked: u64,
    owner: &str,
    device: &str,
    network: &Network,
    secret: Option<Secret>,
) -> Ended {
    let bus = Bus::system();

    // listening before asking, so the end of a quick join is not missed
    let (listening, heard) = listen(asked);

    /*
     * the snapshot's NetworkManager may have been replaced while nothing listened. It can still
     * answer, but its going away would then say nothing more: no join is asked of it
     */
    if self::owner() != owner {
        return Ended::Gone;
    }

    let (made, active) = match (&network.profile, secret) {
        (Some(profile), None) => {
            let active = bus.call(
                owner,
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
                owner,
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
        Ended::Failed(Failure::Other)
    } else {
        let mut waiting = Waiting::new(owner, device, &active);
        let mut late: Option<Instant> = None;

        loop {
            // once over, why it failed may still be on its way, but not for long
            let next = match late {
                None => heard.recv().ok(),
                Some(by) => heard
                    .recv_timeout(by.saturating_duration_since(Instant::now()))
                    .ok(),
            };

            let Some(next) = next else {
                break waiting.failed();
            };

            if let Some(ended) = waiting.hear(&next) {
                break ended;
            }

            if waiting.over && late.is_none() {
                late = Some(Instant::now() + LATE);
            }
        }
    };

    drop(listening);

    for (profile, method) in tidy(made.as_deref(), network.profile.as_deref(), ended) {
        bus.call(owner, profile, PROFILE, method, &[]);
    }

    ended
}

/*
 * what becomes of the profile a join `made` and the `old` one it replaces once the join `ended`:
 * the one made is saved, and the old one, with the old password, deleted so it does not come back;
 * failed, the one made is deleted. With NetworkManager gone, so are its profiles: nothing is asked
 */
fn tidy<'a>(
    made: Option<&'a str>,
    old: Option<&'a str>,
    ended: Ended,
) -> Vec<(&'a str, &'static str)> {
    match (made, ended) {
        (Some(made), Ended::Up) => [(made, "Save")]
            .into_iter()
            .chain(old.map(|old| (old, "Delete")))
            .collect(),
        (Some(made), Ended::Failed(_)) => vec![(made, "Delete")],
        _ => Vec::new(),
    }
}

// how a join ends: up, failed, or with the NetworkManager it asked gone
#[derive(Debug, Clone, Copy, PartialEq)]
enum Ended {
    Up,
    Failed(Failure),
    Gone,
}

/*
 * a change a join hears, from the NetworkManager `owner`, by its unique name: the one that said
 * it, or the one gone. Paths are only that NetworkManager's, so a join hears only its own
 */
#[derive(Debug, Clone, PartialEq)]
pub struct Heard {
    pub owner: String,
    pub change: Change,
}

/*
 * of a device, in a state for a reason, of an active connection, an object removed, or
 * NetworkManager going away, which takes every attempt with it
 */
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    Device(String, f64, f64),
    Attempt(String, f64),
    Removed(String),
    Gone,
}

// a join listening, until dropped
struct Listening(u64);

impl Drop for Listening {
    fn drop(&mut self) {
        let mut listening = LISTENING.lock().unwrap_or_else(PoisonError::into_inner);

        listening.retain(|(asked, _)| *asked != self.0);
    }
}

// the join `asked` listening, and what it hears
fn listen(asked: u64) -> (Listening, Receiver<Heard>) {
    let (sender, heard) = mpsc::channel();
    let mut listening = LISTENING.lock().unwrap_or_else(PoisonError::into_inner);

    listening.push((asked, sender));

    (Listening(asked), heard)
}

/*
 * what `system::follow` hears for the joins waiting, none most of the time. The device and active
 * connection come from different watches, so a join may hear them in either order
 */
pub fn hear(heard: Heard) {
    let listening = LISTENING.lock().unwrap_or_else(PoisonError::into_inner);

    for (_, sender) in listening.iter() {
        // one that stops listening is removed by its `Listening`
        let _ = sender.send(heard.clone());
    }
}

/*
 * a join waiting on its device and active connection. Another join may take the device over, so
 * only the join's own active connection says it is up or over; the device says why it failed
 */
struct Waiting<'a> {
    owner: &'a str,
    device: &'a str,
    active: &'a str,

    // why the device last failed since it began preparing
    failure: Option<f64>,

    over: bool,
}

impl<'a> Waiting<'a> {
    fn new(owner: &'a str, device: &'a str, active: &'a str) -> Self {
        Self {
            owner,
            device,
            active,
            failure: None,
            over: false,
        }
    }

    // how what was heard ends the join: up, or over once it is known why; none while on its way
    fn hear(&mut self, heard: &Heard) -> Option<Ended> {
        // another NetworkManager's, one that replaced the join's, may reuse its paths
        if heard.owner != self.owner {
            return None;
        }

        match &heard.change {
            Change::Device(path, state, reason) if path == self.device => {
                if *state == PREPARING && !self.over {
                    self.failure = None;
                } else if *state == FAILED {
                    self.failure = Some(*reason);
                }
            }
            Change::Attempt(path, state) if path == self.active => {
                if *state == UP && !self.over {
                    return Some(Ended::Up);
                }

                self.over |= *state == OVER;
            }
            // nothing more is said of an attempt removed, nor by NetworkManager gone
            Change::Removed(path) if path == self.active => return Some(self.failed()),
            Change::Gone => return Some(Ended::Gone),
            _ => {}
        }

        (self.over && self.failure.is_some()).then(|| self.failed())
    }

    // the join failing, as a wrong password when the device said so
    fn failed(&self) -> Ended {
        Ended::Failed(if self.failure == Some(NO_SECRETS) {
            Failure::WrongPassword
        } else {
            Failure::Other
        })
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
pub fn disconnect(networks: &Networks) {
    let asked = ask();
    let owner = networks.owner.clone();
    let device = networks.device.clone();

    set(asked, Join::Idle);

    thread::spawn(move || {
        Bus::system().call(&owner, &device, DEVICE, "Disconnect", &[]);
    });
}

// a join or disconnect asked, newer than any before
fn ask() -> u64 {
    let mut asked = ASKED.lock().unwrap_or_else(PoisonError::into_inner);
    *asked += 1;
    *asked
}

// the outcome of the last thing asked, dropped when something newer was asked since
fn set(asked: u64, join: Join) {
    let last = ASKED.lock().unwrap_or_else(PoisonError::into_inner);

    if *last == asked && *Join::read() != join {
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

    // the NetworkManager a join asked, by its unique name
    const OWNER: &str = ":1.9";

    impl From<Change> for Heard {
        fn from(change: Change) -> Self {
            Self {
                owner: OWNER.into(),
                change,
            }
        }
    }

    #[test]
    fn a_join_ends_when_its_own_attempt_is_up_or_over_and_says_why() {
        const ON_ITS_WAY: f64 = 1.0;
        let device = |state, reason| Heard::from(Change::Device("/d".into(), state, reason));
        let attempt = |state| Heard::from(Change::Attempt("/a".into(), state));

        // the network it leaves goes down first, which is not the join's end
        let mut waiting = Waiting::new(OWNER, "/d", "/a");
        assert_eq!(waiting.hear(&device(110.0, 0.0)), None);
        assert_eq!(waiting.hear(&device(30.0, 0.0)), None);
        assert_eq!(waiting.hear(&attempt(ON_ITS_WAY)), None);
        assert_eq!(waiting.hear(&device(PREPARING, 0.0)), None);
        assert_eq!(waiting.hear(&device(ACTIVATED_DEVICE, 0.0)), None);
        assert_eq!(waiting.hear(&attempt(UP)), Some(Ended::Up));

        // a wrong password: the device fails, then the attempt is over
        let mut waiting = Waiting::new(OWNER, "/d", "/a");
        assert_eq!(waiting.hear(&device(PREPARING, 0.0)), None);
        assert_eq!(waiting.hear(&device(FAILED, NO_SECRETS)), None);
        assert_eq!(
            waiting.hear(&attempt(OVER)),
            Some(Ended::Failed(Failure::WrongPassword))
        );

        // heard the other way round, as the two threads may
        let mut waiting = Waiting::new(OWNER, "/d", "/a");
        assert_eq!(waiting.hear(&device(PREPARING, 0.0)), None);
        assert_eq!(waiting.hear(&attempt(OVER)), None);
        assert_eq!(
            waiting.hear(&device(FAILED, NO_SECRETS)),
            Some(Ended::Failed(Failure::WrongPassword))
        );

        // over without the device failing, once no reason came in time
        let mut waiting = Waiting::new(OWNER, "/d", "/a");
        assert_eq!(waiting.hear(&attempt(OVER)), None);
        assert_eq!(waiting.hear(&device(30.0, 0.0)), None);
        assert_eq!(waiting.failed(), Ended::Failed(Failure::Other));

        // a failure from before it began preparing is not this join's
        let mut waiting = Waiting::new(OWNER, "/d", "/a");
        assert_eq!(waiting.hear(&device(FAILED, NO_SECRETS)), None);
        assert_eq!(waiting.hear(&device(PREPARING, 0.0)), None);
        assert_eq!(waiting.hear(&device(FAILED, 8.0)), None);
        assert_eq!(
            waiting.hear(&attempt(OVER)),
            Some(Ended::Failed(Failure::Other))
        );
    }

    // only its own active connection and device count: another join's coming up is not this one's
    #[test]
    fn a_join_hears_only_its_own_attempt_and_device() {
        let mut waiting = Waiting::new(OWNER, "/d", "/a");
        assert_eq!(waiting.hear(&Change::Attempt("/b".into(), UP).into()), None);
        assert_eq!(
            waiting.hear(&Change::Device("/e".into(), FAILED, NO_SECRETS).into()),
            None
        );
        assert_eq!(
            waiting.hear(&Change::Device("/d".into(), ACTIVATED_DEVICE, 0.0).into()),
            None
        );

        // taken over before it began: over, with nothing from the device to say why
        assert_eq!(
            waiting.hear(&Change::Attempt("/a".into(), OVER).into()),
            None
        );
        assert_eq!(waiting.hear(&Change::Attempt("/a".into(), UP).into()), None);
        assert_eq!(waiting.failed(), Ended::Failed(Failure::Other));
    }

    // NetworkManager going away takes the attempt with it, saying no more
    #[test]
    fn a_join_ends_when_networkmanager_goes_away() {
        let mut waiting = Waiting::new(OWNER, "/d", "/a");
        assert_eq!(
            waiting.hear(&Change::Device("/d".into(), PREPARING, 0.0).into()),
            None
        );
        assert_eq!(waiting.hear(&Change::Gone.into()), Some(Ended::Gone));
    }

    /*
     * a NetworkManager that replaced the join's may reuse its paths, and one gone that is not the
     * join's takes nothing with it
     */
    #[test]
    fn a_join_hears_only_its_own_networkmanager() {
        let other = |change| Heard {
            owner: ":1.81".into(),
            change,
        };

        let mut waiting = Waiting::new(OWNER, "/d", "/a");
        assert_eq!(waiting.hear(&other(Change::Attempt("/a".into(), UP))), None);
        assert_eq!(
            waiting.hear(&other(Change::Device("/d".into(), FAILED, NO_SECRETS))),
            None
        );
        assert_eq!(waiting.hear(&other(Change::Removed("/a".into()))), None);
        assert_eq!(waiting.hear(&other(Change::Gone)), None);
        assert_eq!(waiting.hear(&Change::Gone.into()), Some(Ended::Gone));
    }

    // its attempt removed says no more either; another removed is not this join's end
    #[test]
    fn a_join_ends_when_its_attempt_is_removed() {
        let mut waiting = Waiting::new(OWNER, "/d", "/a");
        assert_eq!(waiting.hear(&Change::Removed("/b".into()).into()), None);
        assert_eq!(
            waiting.hear(&Change::Device("/d".into(), FAILED, NO_SECRETS).into()),
            None
        );
        assert_eq!(
            waiting.hear(&Change::Removed("/a".into()).into()),
            Some(Ended::Failed(Failure::WrongPassword))
        );
    }

    #[test]
    fn a_join_saves_or_deletes_what_it_made_but_nothing_once_networkmanager_is_gone() {
        let (made, old) = (Some("/s/9"), Some("/s/7"));

        assert_eq!(
            tidy(made, old, Ended::Up),
            [("/s/9", "Save"), ("/s/7", "Delete")]
        );
        assert_eq!(
            tidy(made, old, Ended::Failed(Failure::WrongPassword)),
            [("/s/9", "Delete")]
        );
        assert_eq!(tidy(None, old, Ended::Up), []);

        // the paths were the old NetworkManager's; a new one may have given them to others
        assert_eq!(tidy(made, old, Ended::Gone), []);
        assert_eq!(tidy(None, old, Ended::Gone), []);
    }

    // a join hears only while it listens, and leaves nothing behind
    #[test]
    fn a_join_listens_until_it_ends() {
        let heard = Heard::from(Change::Attempt("/a".into(), UP));

        let (listening, waiting) = listen(u64::MAX);
        hear(heard.clone());
        assert_eq!(waiting.try_recv(), Ok(heard.clone()));

        drop(listening);
        hear(heard);
        assert!(waiting.try_recv().is_err());
        assert!(
            LISTENING
                .lock()
                .unwrap()
                .iter()
                .all(|(asked, _)| *asked != u64::MAX)
        );
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
