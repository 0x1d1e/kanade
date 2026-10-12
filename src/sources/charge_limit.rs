//! The battery's charge limit, through UPower 1.90 or later: its battery device says whether the
//! hardware can stop charging short of full (`ChargeThresholdSupported`), whether it does
//! (`ChargeThresholdEnabled`) and where (`ChargeEndThreshold`), and `EnableChargeThreshold` turns it
//! on or off. UPower writes the kernel's files; Kanade never does.
//!
//! Its thread reads the device once, then again on each of the device's property changes. Turning
//! it on or off is asked on a thread of its own, since UPower may take a moment, so neither the
//! Controls switch nor `kanade battery limit` waits on the draw thread; a refusal shows on the
//! switch and in `status`, with UPower's reason.

use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use kanade_runtime::service::Service;
use zbus::blocking::Connection;

use super::osd::UPOWER;
use crate::bus::{Bus, Value};

const DEVICE: &str = "org.freedesktop.UPower.Device";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const ROOT: &str = "/org/freedesktop/UPower";

// UPower's Type for a battery
const BATTERY: f64 = 2.0;

// the serials of the asks, so `kanade` waits on its own and not on one before it
static ISSUED: AtomicU64 = AtomicU64::new(0);

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

// how an ask of `kanade battery limit on|off` stands, as `status` says it
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    Waiting,
    Done(String),
    Refused(String),
}

/*
 * does what `request` asks: on or off answered at once as asking, by serial, which `status` then
 * says the outcome of
 */
pub fn request(request: Request) -> Result<String, String> {
    match request {
        Request::Status => Ok(status(&ChargeLimit::read())),
        Request::On => ask(true).map(|serial| format!("asking #{serial}")),
        Request::Off => ask(false).map(|serial| format!("asking #{serial}")),
    }
}

// the serial `request` answered with, for `kanade` to wait on
pub fn asking(reply: &str) -> Option<u64> {
    reply.strip_prefix("asking #")?.parse().ok()
}

/*
 * the limit's state, then the last ask's outcome, like "#3 done" or "#3 refused: why", so
 * `settled` can tell it
 */
fn status(limit: &ChargeLimit) -> String {
    let mut lines = vec![said(limit)];

    match &limit.asked {
        Some((serial, None)) => lines.push(format!("#{serial} waiting")),
        Some((serial, Some(Ok(())))) => lines.push(format!("#{serial} done")),
        Some((serial, Some(Err(why)))) => lines.push(format!("#{serial} refused: {why}")),
        None => {}
    }

    lines.join("\n")
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

// how the ask `serial` stands in `status`, none while `status` does not say
pub fn settled(status: &str, serial: u64) -> Option<Settled> {
    let mut lines = status.lines();
    let state = lines.next()?;
    let outcome = lines.next()?.strip_prefix(&format!("#{serial} "))?;

    Some(match outcome {
        "waiting" => Settled::Waiting,
        "done" => Settled::Done(state.to_owned()),
        refused => Settled::Refused(refused.strip_prefix("refused: ")?.to_owned()),
    })
}

/*
 * asks UPower to turn the limit on or off on a thread of its own, answered by serial; refused at
 * once while unsupported
 */
pub fn ask(on: bool) -> Result<u64, String> {
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
                limit.asked = Some((serial, Some(answer)));
            }
        });

    match spawned {
        Ok(_) => Ok(serial),
        Err(error) => {
            let why = format!("cannot start a thread: {error}");
            ChargeLimit::write().asked = Some((serial, Some(Err(why.clone()))));
            Err(why)
        }
    }
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
 * announces; with no battery UPower knows, it says so and ends
 */
pub fn follow() {
    let bus = Bus::system();

    let Some(device) = battery(bus) else {
        publish(ChargeLimit::default());
        return;
    };

    // watched before the first read, so no change between them is missed
    let changes = bus.signals_from(&device, PROPERTIES, "PropertiesChanged");

    publish(read(&device));

    for change in changes {
        if change.arguments().first().map(Value::text) == Some(DEVICE) {
            publish(read(&device));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `kanade battery limit on` tells its own ask's outcome from `status`, never an older one's
    #[test]
    fn an_ask_settles_by_its_own_serial() {
        let mut limit = ChargeLimit {
            device: Some(String::from("/org/freedesktop/UPower/devices/battery_BAT0")),
            supported: true,
            enabled: true,
            end: Some(80),
            asked: Some((3, None)),
        };

        assert_eq!(settled(&status(&limit), 3), Some(Settled::Waiting));
        assert_eq!(settled(&status(&limit), 4), None);

        limit.asked = Some((3, Some(Ok(()))));
        assert_eq!(
            settled(&status(&limit), 3),
            Some(Settled::Done(String::from("on, charging stops at 80%")))
        );

        limit.asked = Some((3, Some(Err(String::from("Not authorized")))));
        assert_eq!(
            settled(&status(&limit), 3),
            Some(Settled::Refused(String::from("Not authorized")))
        );
        assert_eq!(settled(&status(&limit), 2), None);
    }
}
