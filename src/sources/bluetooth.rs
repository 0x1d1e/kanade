//! BlueZ for the Controls Surface: the adapter and its devices, for the Bluetooth switch and the
//! Bluetooth sub-surface (#131), which pairs, connects, disconnects and forgets them.
//!
//! `system::follow` reads the adapter and every device again on each change BlueZ announces. The
//! devices nearby are known only while the adapter discovers: `watch` has the scanner start
//! discovering as the sub-surface opens, and it stops once the sub-surface closes, the island
//! closing too, which the scanner notices within a `TICK`. Closed, nothing discovers and the scanner
//! sleeps without waking.
//!
//! A pairing asked from here that needs the user, a code to compare or to type, asks this shell's
//! own agent (`agent`), which shows it in the sub-surface as the `Prompt`.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use amane::{Argument, Bus, Service, Value};

use super::system::{Radio, Watched};
use crate::supervise;

mod agent;

pub use agent::{Prompt, confirm};

pub const BLUEZ: &str = "org.bluez";

const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const BATTERY: &str = "org.bluez.Battery1";

// how often the scanner looks whether the sub-surface still shows, only while it does
const TICK: Duration = Duration::from_secs(1);

// a Bluetooth device the adapter knows, like a headset
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    // BlueZ's object for it, stable across renames, so it keys the device
    pub path: String,

    // its alias, or the address when it names none
    pub name: String,

    pub paired: bool,
    pub connected: bool,

    // 0 to 100, only for devices that report it
    pub battery: Option<u8>,
}

// Bluetooth as the Controls Surface shows it; written only by `system::follow`
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Adapter {
    pub radio: Radio,

    // BlueZ's object for it, empty while Missing
    pub path: String,

    // looking for devices nearby, for this shell or another program
    pub discovering: bool,

    // connected first, then paired, then nearby, each by name
    pub devices: Vec<Device>,
}

impl Service for Adapter {
    fn new() -> Self {
        Adapter::default()
    }

    fn listen() {}
}

// what is asked of a device
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    Pair,
    Connect,
    Disconnect,
    Forget,
}

// the last thing asked of a device from here, until another
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Request {
    #[default]
    Idle,
    Doing(String, Task),
    Failed(String, Task),
}

impl Service for Request {
    fn new() -> Self {
        Request::default()
    }

    fn listen() {}
}

impl Request {
    // what is being done to the device at `path`, if it was the last one asked
    pub fn doing(&self, path: &str) -> Option<Task> {
        match self {
            Request::Doing(doing, task) if doing == path => Some(*task),
            _ => None,
        }
    }

    // what failed for the device at `path`, if it was the last one asked
    pub fn failed(&self, path: &str) -> Option<Task> {
        match self {
            Request::Failed(failed, task) if failed == path => Some(*task),
            _ => None,
        }
    }
}

static WATCHED: Watched = Watched::new();

/*
 * the last thing asked, so an older one that ends after it says nothing. Held while its outcome is
 * written, so a newer one cannot be asked between checking and writing
 */
static ASKED: Mutex<u64> = Mutex::new(0);

// the scanner, told to look again; none until the sub-surface first opens
static SCANNER: Mutex<Option<Sender<()>>> = Mutex::new(None);

// the sub-surface opening in `visit`: the adapter discovers from now until it closes
pub fn watch(visit: u64) {
    if WATCHED.watch(visit) {
        nudge();
    }
}

// the sub-surface closing; the island closing ends a watch too, see `wanted`
pub fn unwatch() {
    if WATCHED.unwatch() {
        nudge();
    }
}

// while the sub-surface shows; it lasts one visit of the Controls Surface
pub fn wanted() -> bool {
    WATCHED.wanted()
}

// the adapter powering, discovering, a device found, connecting, renamed, or its battery
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

    let mut devices = Vec::new();

    for (path, interfaces) in &objects {
        let properties = interfaces.get(ADAPTER);

        // the first wins, most machines have one
        if properties != &Value::Nothing && adapter.radio == Radio::Missing {
            adapter.radio = Radio::of(true, properties.get("Powered").bool());
            adapter.discovering = properties.get("Discovering").bool();
            adapter.path.clone_from(path);
        }

        if let Some(device) = device(path, interfaces) {
            devices.push(device);
        }
    }

    adapter.devices = sorted(devices, &adapter.path);
    adapter
}

// the devices of the adapter at `adapter`, connected first, then paired, then nearby, each by name
fn sorted(devices: Vec<Device>, adapter: &str) -> Vec<Device> {
    let mut devices: Vec<Device> = devices
        .into_iter()
        .filter(|device| {
            device
                .path
                .strip_prefix(adapter)
                .is_some_and(|rest| rest.starts_with('/'))
        })
        .collect();

    devices.sort_by_key(|device| {
        (
            !device.connected,
            !device.paired,
            device.name.to_lowercase(),
        )
    });

    devices
}

/*
 * a device worth showing: one only seen nearby shows by the name it gives itself, and nameless
 * ones, mostly beacons, not at all
 */
fn device(path: &str, interfaces: &Value) -> Option<Device> {
    let device = interfaces.get(DEVICE);

    if device == &Value::Nothing {
        return None;
    }

    let paired = device.get("Paired").bool();
    let connected = device.get("Connected").bool();

    if !paired && !connected && device.get("Name").text().is_empty() {
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

    Some(Device {
        path: path.to_owned(),
        name: name.to_owned(),
        paired,
        connected,
        battery,
    })
}

// BlueZ answers with the change, which `system::follow` reads back
pub fn power(adapter: String, on: bool) {
    thread::spawn(move || {
        Bus::system().set_property(BLUEZ, &adapter, ADAPTER, "Powered", Argument::from(on));
    });
}

/*
 * pairs a device nearby, then connects it, trusted so it may connect again on its own. BlueZ says
 * nothing of why a pairing failed, so whether it did is read back
 */
pub fn pair(device: &str) {
    let path = device.to_owned();
    let asked = ask(Request::Doing(path.clone(), Task::Pair));

    thread::spawn(move || {
        let bus = Bus::system();

        agent::register();
        bus.call(BLUEZ, &path, DEVICE, "Pair", &[]);
        agent::over(&path);

        if !bus.property(BLUEZ, &path, DEVICE, "Paired").bool() {
            set(asked, Request::Failed(path, Task::Pair));
            return;
        }

        bus.set_property(BLUEZ, &path, DEVICE, "Trusted", Argument::from(true));

        set(asked, Request::Doing(path.clone(), Task::Connect));
        set(asked, connected(&path));
    });
}

pub fn connect(device: &str) {
    let path = device.to_owned();
    let asked = ask(Request::Doing(path.clone(), Task::Connect));

    thread::spawn(move || set(asked, connected(&path)));
}

// connects the device at `path`, saying whether it did
fn connected(path: &str) -> Request {
    let bus = Bus::system();

    bus.call(BLUEZ, path, DEVICE, "Connect", &[]);

    if bus.property(BLUEZ, path, DEVICE, "Connected").bool() {
        Request::Idle
    } else {
        Request::Failed(path.to_owned(), Task::Connect)
    }
}

pub fn disconnect(device: &str) {
    let path = device.to_owned();
    let asked = ask(Request::Doing(path.clone(), Task::Disconnect));

    thread::spawn(move || {
        let bus = Bus::system();

        bus.call(BLUEZ, &path, DEVICE, "Disconnect", &[]);

        let outcome = if bus.property(BLUEZ, &path, DEVICE, "Connected").bool() {
            Request::Failed(path, Task::Disconnect)
        } else {
            Request::Idle
        };

        set(asked, outcome);
    });
}

// unpairs the device and drops it from the adapter, as if never paired
pub fn forget(adapter: &str, device: &str) {
    let adapter = adapter.to_owned();
    let path = device.to_owned();
    let asked = ask(Request::Doing(path.clone(), Task::Forget));

    thread::spawn(move || {
        let bus = Bus::system();

        bus.call(
            BLUEZ,
            &adapter,
            ADAPTER,
            "RemoveDevice",
            &[Argument::Path(path.clone())],
        );

        // a device forgotten is gone, and has no address left to read
        let outcome = if bus.property(BLUEZ, &path, DEVICE, "Address") == Value::Nothing {
            Request::Idle
        } else {
            Request::Failed(path, Task::Forget)
        };

        set(asked, outcome);
    });
}

/*
 * stops the pairing asked from here, by Cancel on its prompt or the sub-surface closing: it ends
 * with nothing said of it failing
 */
pub fn cancel() {
    let pairing = match &*Request::read() {
        Request::Doing(path, Task::Pair) => Some(path.clone()),
        _ => None,
    };

    let Some(device) = pairing.or_else(|| Prompt::read().device().map(str::to_owned)) else {
        return;
    };

    ask(Request::Idle);
    agent::over(&device);

    thread::spawn(move || {
        Bus::system().call(BLUEZ, &device, DEVICE, "CancelPairing", &[]);
    });
}

// something asked, newer than any before, `request` until it ends
fn ask(request: Request) -> u64 {
    let mut asked = ASKED.lock().unwrap_or_else(PoisonError::into_inner);
    *asked += 1;

    if *Request::read() != request {
        *Request::write() = request;
    }

    *asked
}

// the outcome of the last thing asked, dropped when something newer was asked since
fn set(asked: u64, request: Request) {
    let last = ASKED.lock().unwrap_or_else(PoisonError::into_inner);

    if *last == asked && *Request::read() != request {
        *Request::write() = request;
    }
}

// has the scanner look again at whether to discover, starting it the first time
pub fn nudge() {
    let mut scanner = SCANNER.lock().unwrap_or_else(PoisonError::into_inner);

    let scanner = scanner.get_or_insert_with(|| {
        let (sender, nudged) = mpsc::channel();

        supervise::spawn("bluetooth scan", move || scan(&nudged));

        sender
    });

    let _ = scanner.send(());
}

/*
 * discovers on the adapter while the sub-surface shows and the adapter is on, and stops once it
 * does not; BlueZ stops it too when the adapter goes off. Closing the sub-surface cancels a pairing
 * still waiting on its prompt, since nothing could answer it. Runs for good, waking only while the
 * sub-surface shows
 */
fn scan(nudged: &Receiver<()>) {
    let bus = Bus::system();

    // the adapter this asked to discover
    let mut discovering: Option<String> = None;

    loop {
        let wanted = wanted();
        let adapter = Adapter::read().clone();
        let want = (wanted && adapter.radio == Radio::On).then_some(adapter.path);

        if discovering != want || (want.is_some() && !adapter.discovering) {
            if let Some(path) = discovering.take() {
                bus.call(BLUEZ, &path, ADAPTER, "StopDiscovery", &[]);
            }

            if let Some(path) = &want {
                bus.call(BLUEZ, path, ADAPTER, "StartDiscovery", &[]);
            }

            discovering = want;
        }

        if !wanted && Prompt::read().device().is_some() {
            cancel();
        }

        let next = if wanted {
            nudged.recv_timeout(TICK)
        } else {
            nudged.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };

        if next == Err(RecvTimeoutError::Disconnected) {
            return;
        }

        // a burst asks once
        nudged.try_iter().for_each(drop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(path: &str, name: &str, paired: bool, connected: bool) -> Device {
        Device {
            path: path.into(),
            name: name.into(),
            paired,
            connected,
            battery: None,
        }
    }

    fn names(devices: &[Device]) -> Vec<&str> {
        devices.iter().map(|device| device.name.as_str()).collect()
    }

    #[test]
    fn connected_come_first_then_paired_then_nearby_by_name() {
        let devices = sorted(
            vec![
                device("/org/bluez/hci0/dev_1", "speaker", false, false),
                device("/org/bluez/hci0/dev_2", "mouse", true, false),
                device("/org/bluez/hci0/dev_3", "buds", true, true),
                device("/org/bluez/hci0/dev_4", "Keyboard", true, false),
                device("/org/bluez/hci0/dev_5", "phone", false, false),
            ],
            "/org/bluez/hci0",
        );

        assert_eq!(
            names(&devices),
            ["buds", "Keyboard", "mouse", "phone", "speaker"]
        );
    }

    #[test]
    fn only_the_adapters_own_devices_show() {
        let devices = sorted(
            vec![
                device("/org/bluez/hci0/dev_1", "buds", true, false),
                device("/org/bluez/hci1/dev_2", "mouse", true, false),
                device("/org/bluez/hci10/dev_3", "pad", true, false),
            ],
            "/org/bluez/hci1",
        );

        assert_eq!(names(&devices), ["mouse"]);
    }

    fn interfaces(device: &[(&str, Value)]) -> Value {
        let device = device
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect();

        Value::Map([(DEVICE.to_owned(), Value::Map(device))].into())
    }

    #[test]
    fn a_device_nearby_shows_only_by_its_own_name() {
        let text = |text: &str| Value::Text(text.into());
        let address = ("Address", text("AA:BB:CC:DD:EE:FF"));

        // BlueZ aliases a nameless device by its address
        let beacon = interfaces(&[address.clone(), ("Alias", text("AA-BB-CC-DD-EE-FF"))]);
        assert_eq!(super::device("/org/bluez/hci0/dev_1", &beacon), None);

        let speaker = interfaces(&[
            address.clone(),
            ("Name", text("Speaker")),
            ("Alias", text("Kitchen")),
        ]);
        let speaker = super::device("/org/bluez/hci0/dev_1", &speaker).unwrap();
        assert_eq!(speaker.name, "Kitchen");

        // a paired one shows whatever its name, by address when it has none
        let paired = interfaces(&[address, ("Paired", Value::Bool(true))]);
        let paired = super::device("/org/bluez/hci0/dev_1", &paired).unwrap();
        assert_eq!(paired.name, "AA:BB:CC:DD:EE:FF");

        // an adapter is no device
        let adapter = Value::Map([(ADAPTER.to_owned(), Value::Map([].into()))].into());
        assert_eq!(super::device("/org/bluez/hci0", &adapter), None);
    }

    #[test]
    fn a_request_says_what_it_does_to_its_own_device_only() {
        let doing = Request::Doing("/d/1".into(), Task::Connect);
        assert_eq!(doing.doing("/d/1"), Some(Task::Connect));
        assert_eq!(doing.doing("/d/2"), None);
        assert_eq!(doing.failed("/d/1"), None);

        let failed = Request::Failed("/d/1".into(), Task::Pair);
        assert_eq!(failed.failed("/d/1"), Some(Task::Pair));
        assert_eq!(failed.doing("/d/1"), None);
    }

    #[test]
    fn only_bluez_adapters_devices_and_batteries_concern_it() {
        assert!(concerns("/org/bluez/hci0", ADAPTER));
        assert!(concerns("/org/bluez/hci0/dev_1", DEVICE));
        assert!(concerns("/org/bluez/hci0/dev_1", BATTERY));
        assert!(!concerns(
            "/org/bluez/hci0/dev_1",
            "org.bluez.MediaControl1"
        ));
        assert!(!concerns("/org/freedesktop/NetworkManager", DEVICE));
    }
}
