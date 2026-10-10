//! Airplane mode as the kernel's rfkill has it (ADR 0021): every radio blocked, by a key, a switch
//! or a program, on a machine with more than one kind of radio; one with only Wi-Fi has no airplane
//! mode, only Wi-Fi off. `/dev/rfkill` says each radio's change as it lands, so the OSD shows the mode
//! turning whoever turned it. Reading needs the rfkill group, or the device's default ACL.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

use super::osd;
use crate::clock::{RFKILL_EVENT, RFKILL_OP_ADD, RFKILL_OP_CHANGE, RFKILL_OP_DEL, RfkillEvent};
use crate::island::activity::Mode;
use crate::supervise;

const DEVICE: &str = "/dev/rfkill";
const CLASS: &str = "/sys/class/rfkill";

// one radio, by its index
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Radio {
    kind: u8,
    blocked: bool,
}

#[derive(Debug, Default)]
struct Radios(BTreeMap<u32, Radio>);

impl Radios {
    // as sysfs has them now, so the radios the device first lists change nothing
    fn now() -> Radios {
        let radios = fs::read_dir(CLASS)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let read = |what: &str| fs::read_to_string(path.join(what)).ok();
                let index = read("index")?.trim().parse().ok()?;
                let blocked = ["soft", "hard"]
                    .iter()
                    .any(|what| read(what).is_some_and(|value| value.trim() != "0"));

                Some((
                    index,
                    Radio {
                        kind: kind(&read("type")?),
                        blocked,
                    },
                ))
            })
            .collect();

        Radios(radios)
    }

    // airplane mode: radios of more than one kind, all blocked
    fn airplane(&self) -> bool {
        let mut kinds = self.0.values().map(|radio| radio.kind);
        let first = kinds.next();

        kinds.any(|kind| Some(kind) != first) && self.0.values().all(|radio| radio.blocked)
    }

    /*
     * whether `event` turned airplane mode, and to what. Only a radio's own change turns it: one
     * that comes or goes, a dongle unplugged or a driver reloaded, changes the radios, not the mode.
     * A CHANGE_ALL written to the device reaches readers as each radio's CHANGE
     */
    fn apply(&mut self, event: RfkillEvent) -> Option<bool> {
        let was = self.airplane();
        let radio = Radio {
            kind: event.kind,
            blocked: event.soft || event.hard,
        };

        match event.op {
            RFKILL_OP_ADD => {
                self.0.insert(event.index, radio);
                return None;
            }
            RFKILL_OP_DEL => {
                self.0.remove(&event.index);
                return None;
            }
            RFKILL_OP_CHANGE => {
                self.0.insert(event.index, radio);
            }
            _ => return None,
        }

        let is = self.airplane();

        (is != was).then_some(is)
    }
}

// <linux/rfkill.h>'s type, as sysfs names it
fn kind(name: &str) -> u8 {
    match name.trim() {
        "wlan" => 1,
        "bluetooth" => 2,
        "uwb" => 3,
        "wimax" => 4,
        "wwan" => 5,
        "gps" => 6,
        "fm" => 7,
        "nfc" => 8,
        _ => u8::MAX,
    }
}

// for doctor: whether the device can be read
pub fn found() -> Result<String, String> {
    match File::open(DEVICE) {
        Ok(_) => Ok(format!("{DEVICE} readable")),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Err(format!(
            "cannot read {DEVICE} ({error}), join the rfkill group"
        )),
        Err(error) => Err(format!("cannot read {DEVICE} ({error})")),
    }
}

// runs on its own thread for good, until the device cannot be read
pub fn follow() {
    if !Path::new(DEVICE).exists() {
        supervise::stopped("radios", format!("there is no {DEVICE}"));
        return;
    }

    let ended = listen();

    let why = match ended.kind() {
        io::ErrorKind::PermissionDenied => format!(
            "cannot read {DEVICE} ({ended}); airplane mode shows nothing without the rfkill group"
        ),
        _ => format!("stopped reading {DEVICE} ({ended})"),
    };

    eprintln!("kanade: {why}");
    supervise::stopped("radios", why);
}

fn listen() -> io::Error {
    let mut device = match File::open(DEVICE) {
        Ok(device) => device,
        Err(error) => return error,
    };

    // after opening: a radio added between is in both, the same
    let mut radios = Radios::now();

    // each read is one event, its first fixed size; what a newer kernel adds is left
    let mut event = [0; RFKILL_EVENT];

    loop {
        match device.read(&mut event) {
            Ok(RFKILL_EVENT) => {}
            Ok(0) => return io::ErrorKind::UnexpectedEof.into(),
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return error,
        }

        if let Some(on) = radios.apply(RfkillEvent::of(&event)) {
            osd::mode(Mode::Airplane(on));
        }
    }
}
