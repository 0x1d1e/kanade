//! The battery's charge limit, through UPower 1.90 or later: its battery device says whether the
//! hardware can stop charging short of full (`ChargeThresholdSupported`), whether it does
//! (`ChargeThresholdEnabled`) and where (`ChargeEndThreshold`), and `EnableChargeThreshold` turns it
//! on or off. UPower writes the kernel's files; Kanade never does.
//!
//! Its thread reads the device once, then again on each of the device's property changes; while
//! UPower names no battery it asks again, ever less often, and says so in `kanade status`. Turning
//! it on or off is asked on a thread of its own, since UPower may take a moment, so neither the
//! Controls switch nor `kanade battery limit` waits on the draw thread; a refusal shows on the
//! switch, with UPower's reason, and `kanade` is answered once UPower has.

use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use kanade_runtime::service::Service;
use zbus::blocking::Connection;

use super::osd::UPOWER;
use crate::backoff::Backoff;
use crate::bus::{Bus, Value};
use crate::later::Later;

const DEVICE: &str = "org.freedesktop.UPower.Device";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const ROOT: &str = "/org/freedesktop/UPower";

// UPower's Type for a battery
const BATTERY: f64 = 2.0;

// the wait before asking again for a battery, doubled at each try up to `LONGEST`: UPower may still
// be starting, and a machine may have none at all
const FIRST: Duration = Duration::from_secs(1);
const LONGEST: Duration = Duration::from_secs(60);

// the serials of the asks, so an older one's answer never takes a newer one's place
static ISSUED: AtomicU64 = AtomicU64::new(0);

// told how an ask went: the state UPower left, or why it refused
pub type Done = Box<dyn FnOnce(Result<String, String>) + Send>;

// the battery's charge limit as UPower last said; written only by this module's threads
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChargeLimit {
    // the battery's UPower device, none without UPower or a battery it knows
    pub device: Option<String>,

    // the hardware can stop short of full and UPower can make it
    pub supported: bool,

    pub enabled: bool,

    // where charging stops while enabled, none when UPower does not say
    pub end: Option<u8>,

    // the last ask by serial, and how it ended: none while UPower has not answered
    pub asked: Option<(u64, Option<Result<(), String>>)>,
}

impl Service for ChargeLimit {
    fn new() -> Self {
        ChargeLimit::default()
    }

    fn listen() {}
}

// what `kanade battery limit` asks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    On,
    Off,
    Status,
}

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        match arguments {
            ["limit", "on"] => Some(Request::On),
            ["limit", "off"] => Some(Request::Off),
            ["limit", "status"] => Some(Request::Status),
            _ => None,
        }
    }
}

/*
 * does what `request` asks: the state for `status`; on and off are asked of UPower, which then
 * answers `later` with the state it left, or its refusal
 */
pub fn request(request: Request, later: Later) {
    match request {
        Request::Status => later.done(said(&ChargeLimit::read())),
        Request::On | Request::Off => {
            let on = request == Request::On;
            let answer = later.clone();
            let done: Done = Box::new(move |result| answer.result(result));

            if let Err(why) = ask(on, Some(done)) {
                later.refused(why);
            }
        }
    }
}

// like "on, charging stops at 80%", "off" or why it is unavailable
fn said(limit: &ChargeLimit) -> String {
    match (
        limit.device.is_some(),
        limit.supported,
        limit.enabled,
        limit.end,
    ) {
        (false, ..) => String::from("unavailable: UPower names no battery"),
        (true, false, ..) => String::from(
            "unavailable: this battery or UPower cannot limit charging (UPower 1.90 or later needed)",
        ),
        (true, true, true, Some(end)) => format!("on, charging stops at {end}%"),
        (true, true, true, None) => String::from("on"),
        (true, true, false, _) => String::from("off"),
    }
}

/*
 * asks UPower to turn the limit on or off on a thread of its own; `done` is told the state it left,
 * or UPower's reason for refusing. Refused at once while unsupported
 */
pub fn ask(on: bool, done: Option<Done>) -> Result<(), String> {
    let limit = ChargeLimit::read().clone();

    let Some(device) = limit.device.clone().filter(|_| limit.supported) else {
        return Err(said(&limit));
    };

    let serial = ISSUED.fetch_add(1, Ordering::Relaxed) + 1;
    ChargeLimit::write().asked = Some((serial, None));

    let spawned = thread::Builder::new()
        .name(String::from("charge-limit ask"))
        .spawn(move || {
            let answer = enable(&device, on);

            if let Err(why) = &answer {
                eprintln!("kanade: charge limit: {why}");
            }

            // read back, as UPower may have changed it even if it said no
            let read = read(&device);
            let mut limit = ChargeLimit::write();

            limit.supported = read.supported;
            limit.enabled = read.enabled;
            limit.end = read.end;

            if limit
                .asked
                .as_ref()
                .is_some_and(|(asked, _)| *asked == serial)
            {
                limit.asked = Some((serial, Some(answer.clone())));
            }

            let state = said(&limit);

            drop(limit);

            if let Some(done) = done {
                done(answer.map(|()| state));
            }
        });

    spawned.map(drop).map_err(|error| {
        let why = format!("cannot start a thread: {error}");
        ChargeLimit::write().asked = Some((serial, Some(Err(why.clone()))));
        why
    })
}

// over a connection of its own, which says why UPower refused, unlike `Bus`, which says nothing
fn enable(device: &str, on: bool) -> Result<(), String> {
    let bus = Connection::system().map_err(|error| said_by(&error))?;

    bus.call_method(
        Some(UPOWER),
        device,
        Some(DEVICE),
        "EnableChargeThreshold",
        &(on,),
    )
    .map(drop)
    .map_err(|error| said_by(&error))
}

fn said_by(error: &zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(_, Some(description), _) => description.clone(),
        error => error.to_string(),
    }
}

// the first battery UPower knows that powers the machine, not a mouse's or a phone's
fn battery(bus: Bus) -> Option<String> {
    let devices = bus.call(
        UPOWER,
        ROOT,
        "org.freedesktop.UPower",
        "EnumerateDevices",
        &[],
    );

    devices
        .list()
        .iter()
        .map(Value::text)
        .find(|&path| {
            bus.property(UPOWER, path, DEVICE, "Type").number() == BATTERY
                && bus.property(UPOWER, path, DEVICE, "PowerSupply").bool()
        })
        .map(str::to_owned)
}

// the device's limit as UPower says now
fn read(device: &str) -> ChargeLimit {
    let bus = Bus::system();
    let property = |name| bus.property(UPOWER, device, DEVICE, name);

    // 0 is unset, and 100 stops nowhere short of full
    let end = property("ChargeEndThreshold").number();

    ChargeLimit {
        device: Some(device.to_owned()),
        supported: property("ChargeThresholdSupported").bool(),
        enabled: property("ChargeThresholdEnabled").bool(),
        end: (1.0..100.0).contains(&end).then_some(end as u8),
        asked: None,
    }
}

// the limit as read now, keeping the last ask's outcome; written only when it changed
fn publish(read: ChargeLimit) {
    let same = {
        let limit = ChargeLimit::read();
        let ChargeLimit {
            device,
            supported,
            enabled,
            end,
            ..
        } = &*limit;

        (device, supported, enabled, end)
            == (&read.device, &read.supported, &read.enabled, &read.end)
    };

    if !same {
        // the ask taken under the same lock, so one asked meanwhile is kept
        let mut limit = ChargeLimit::write();
        let asked = limit.asked.take();

        *limit = ChargeLimit { asked, ..read };
    }
}

/*
 * runs on its own thread for good: the battery's limit read at start and on each change UPower
 * announces. With no battery it asks again after a backoff, as UPower may still be starting or a
 * battery turn up, and says so while it does; should the bus itself go away, it says that too and
 * ends, and `supervise` keeps the stop for `kanade status`
 */
pub fn follow() {
    let bus = Bus::system();
    let mut backoff = Backoff::new(FIRST, LONGEST);
    let mut said = false;

    loop {
        let Some(device) = battery(bus) else {
            publish(ChargeLimit::default());

            if !said {
                eprintln!("kanade: charge limit: UPower names no battery, asking again");
                said = true;
            }

            thread::sleep(backoff.failed());
            continue;
        };

        if said {
            eprintln!("kanade: charge limit: UPower names a battery again");
            said = false;
        }

        backoff.reset();

        // watched before the first read, so no change between them is missed
        let changes = bus.signals_from(&device, PROPERTIES, "PropertiesChanged");

        publish(read(&device));

        // a battery that has gone away ends the subscription, and is looked for again; the bus
        // going away leaves none to find, which the next look says
        for change in changes {
            if change.arguments().first().map(Value::text) == Some(DEVICE) {
                publish(read(&device));
            }
        }
    }
}
