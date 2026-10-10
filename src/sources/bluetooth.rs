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

use crate::bus::{Argument, Bus, Value};
use kanade_runtime::service::Service;

use super::system::{Radio, Watched};
use crate::supervise;

mod agent;

pub use agent::{Prompt, confirm, type_pin};

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

/*
 * the last thing asked of a device from here, until another. What is under way, `Doing` or
 * `Cancelling`, lasts until the thread doing it ends, and only one is at a time
 */
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Request {
    #[default]
    Idle,
    Doing(String, Task),

    // a pairing cancelled, still running until BlueZ gives it up, which says nothing of itself
    Cancelling,

    Failed(String, Task),
}

impl Service for Request {
    fn new() -> Self {
        Request::default()
    }

    fn listen() {}
}

impl Request {
    // something is under way, so nothing else may be asked until it ends
    pub fn busy(&self) -> bool {
        matches!(self, Request::Doing(..) | Request::Cancelling)
    }

    // where its thread ending unfinished leaves it: what it was doing failed
    fn ended(&self) -> Option<Request> {
        match self {
            Request::Doing(path, task) => Some(Request::Failed(path.clone(), *task)),
            Request::Cancelling => Some(Request::Idle),
            Request::Idle | Request::Failed(..) => None,
        }
    }

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

// held while `Request` is checked and written, so nothing is asked between the two
static ASKING: Mutex<()> = Mutex::new(());

/*
 * the one thing under way, held by the thread doing it until it `finish`es. Dropped unfinished, as
 * by a panic, it is no longer under way. Finished, it touches nothing asked after, though its
 * thread may hold it a while longer
 */
struct Working {
    finished: bool,
}

impl Working {
    // how it goes on, still under way; a cancelled one says nothing more
    fn update(&self, request: Request) {
        let _asking = ASKING.lock().unwrap_or_else(PoisonError::into_inner);

        if !self.finished && matches!(*Request::read(), Request::Doing(..)) {
            write(request);
        }
    }

    // its outcome, no longer under way; a cancelled one's says nothing
    fn finish(&mut self, outcome: Request) {
        let _asking = ASKING.lock().unwrap_or_else(PoisonError::into_inner);

        if self.finished {
            return;
        }

        self.finished = true;

        let ended = match *Request::read() {
            Request::Doing(..) => Some(outcome),
            Request::Cancelling => Some(Request::Idle),
            Request::Idle | Request::Failed(..) => None,
        };

        if let Some(ended) = ended {
            write(ended);
        }
    }
}

impl Drop for Working {
    fn drop(&mut self) {
        let _asking = ASKING.lock().unwrap_or_else(PoisonError::into_inner);

        if self.finished {
            return;
        }

        let ended = Request::read().ended();

        if let Some(ended) = ended {
            write(ended);
        }
    }
}

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
    let Some(mut working) = ask(Request::Doing(path.clone(), Task::Pair)) else {
        return;
    };

    thread::spawn(move || {
        let bus = Bus::system();

        agent::register();
        bus.call(BLUEZ, &path, DEVICE, "Pair", &[]);
        agent::over(&path);

        if !bus.property(BLUEZ, &path, DEVICE, "Paired").bool() {
            working.finish(Request::Failed(path, Task::Pair));
            return;
        }

        bus.set_property(BLUEZ, &path, DEVICE, "Trusted", Argument::from(true));

        working.update(Request::Doing(path.clone(), Task::Connect));
        working.finish(connected(&path));
    });
}

pub fn connect(device: &str) {
    let path = device.to_owned();
    let Some(mut working) = ask(Request::Doing(path.clone(), Task::Connect)) else {
        return;
    };

    thread::spawn(move || {
        working.finish(connected(&path));
    });
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
    let Some(mut working) = ask(Request::Doing(path.clone(), Task::Disconnect)) else {
        return;
    };

    thread::spawn(move || {
        let bus = Bus::system();

        bus.call(BLUEZ, &path, DEVICE, "Disconnect", &[]);

        let outcome = if bus.property(BLUEZ, &path, DEVICE, "Connected").bool() {
            Request::Failed(path, Task::Disconnect)
        } else {
            Request::Idle
        };

        working.finish(outcome);
    });
}

// unpairs the device and drops it from the adapter, as if never paired
pub fn forget(adapter: &str, device: &str) {
    let adapter = adapter.to_owned();
    let path = device.to_owned();
    let Some(mut working) = ask(Request::Doing(path.clone(), Task::Forget)) else {
        return;
    };

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

        working.finish(outcome);
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

    abandon();
    agent::over(&device);

    thread::spawn(move || {
        Bus::system().call(BLUEZ, &device, DEVICE, "CancelPairing", &[]);
    });
}

/*
 * `request` asked, under way until the thread given `Working` ends. One at a time, so none while
 * another is: BlueZ's answers say nothing of which call they end, and a pairing's agent asks the
 * user about one device
 */
fn ask(request: Request) -> Option<Working> {
    let _asking = ASKING.lock().unwrap_or_else(PoisonError::into_inner);

    if Request::read().busy() {
        return None;
    }

    write(request);

    Some(Working { finished: false })
}

// what is under way says nothing more of itself, though it runs until it ends
fn abandon() {
    let _asking = ASKING.lock().unwrap_or_else(PoisonError::into_inner);

    if matches!(*Request::read(), Request::Doing(..)) {
        write(Request::Cancelling);
    }
}

fn write(request: Request) {
    if *Request::read() != request {
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
