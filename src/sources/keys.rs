//! The keys the OSD answers and the lock lights it shows (ADR 0021), read off the keyboards' evdev
//! devices beside niri, which keeps them ungrabbed. A keybind changes a level outside Kanade, so a
//! press at a limit changes nothing Kanade could hear; the press itself can be heard. niri lights a
//! keyboard's Caps Lock and Num Lock lights as it toggles them, which every reader of the device
//! hears too. Typing, and a held key repeating, need none of this: the runtime's keyboard does both
//! (ADR 0034).
//!
//! Each device is masked in the kernel before it is read (`clock::mask_input`): only the level
//! keys and the two lights ever reach Kanade, never a letter, and a device that cannot be masked is
//! not read at all. What it queued before the mask is thrown away unread. Reading needs the input
//! group; without it the OSD shows only level changes. A device plugged in later is read once udev
//! gave it to the group.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use inotify::{EventMask, Inotify, WatchMask};

use super::osd::{self, Key, Reads};
use crate::clock::{
    self, EV_KEY, EV_LED, INPUT_EVENT, InputEvent, KEY_BRIGHTNESSDOWN, KEY_BRIGHTNESSUP,
    KEY_VOLUMEDOWN, KEY_VOLUMEUP, LED_CAPSL, LED_NUML,
};
use crate::island::activity::Mode;
use crate::supervise;

const DEVICES: &str = "/dev/input";
const CLASS: &str = "/sys/class/input";
const LEDS: &str = "/sys/class/leds";

/*
 * every key that steps a level, by the level. Not the mute keys: they toggle, never at a limit, and
 * their press would show the state before the toggle, so the change they make shows instead
 */
const KEYS: [(u16, Key); 4] = [
    (KEY_VOLUMEDOWN, Key::Speaker),
    (KEY_VOLUMEUP, Key::Speaker),
    (KEY_BRIGHTNESSDOWN, Key::Brightness),
    (KEY_BRIGHTNESSUP, Key::Brightness),
];

const LIGHTS: [u16; 2] = [LED_NUML, LED_CAPSL];

// what is read: the lights and the keys of the OSD's levels that are on
#[derive(Debug, Clone, PartialEq, Eq)]
struct Wanted {
    keys: Vec<u16>,
}

impl Wanted {
    // a level not read never wakes Kanade
    fn of(reads: Reads) -> Wanted {
        let levels = KEYS.iter().filter(|(_, key)| match key {
            Key::Speaker => reads.audio,
            Key::Brightness => reads.brightness,
        });

        Wanted {
            keys: levels.map(|&(code, _)| code).collect(),
        }
    }

    fn lights(&self) -> &[u16] {
        &LIGHTS
    }
}

// what an event says, none for any other
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Input {
    Key(Key),
    Light(Mode),
}

/*
 * a level's key going down or repeating while held, a light going on or off. Anything else, like a
 * key let go, is nothing
 */
fn heard(event: InputEvent) -> Option<Input> {
    match event.kind {
        EV_KEY if event.value != 0 => KEYS
            .iter()
            .find(|&&(code, _)| code == event.code)
            .map(|&(_, key)| Input::Key(key)),
        EV_LED => {
            let on = event.value != 0;

            match event.code {
                LED_CAPSL => Some(Input::Light(Mode::CapsLock(on))),
                LED_NUML => Some(Input::Light(Mode::NumLock(on))),
                _ => None,
            }
        }
        _ => None,
    }
}

// the lights as last seen; every keyboard says the same change, which shows once
#[derive(Debug, Default)]
struct Lights {
    caps: Option<bool>,
    num: Option<bool>,
}

impl Lights {
    // as the kernel's LED class has them now, any keyboard's
    fn now() -> Lights {
        let lit = |suffix: &str| {
            fs::read_dir(LEDS)
                .ok()?
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(suffix))
                .find_map(|entry| fs::read_to_string(entry.path().join("brightness")).ok())
                .map(|brightness| brightness.trim() != "0")
        };

        Lights {
            caps: lit("::capslock"),
            num: lit("::numlock"),
        }
    }

    // whether `mode` is a change
    fn turn(&mut self, mode: Mode) -> bool {
        let light = match mode {
            Mode::CapsLock(_) => &mut self.caps,
            Mode::NumLock(_) => &mut self.num,
            Mode::Airplane(_) => return false,
        };

        light.replace(mode.on()) != Some(mode.on())
    }
}

static LIT: Mutex<Option<Lights>> = Mutex::new(None);

// the devices being read, so one is never read twice
static READ: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

// the devices that could not be read but for the group, tried again only once plugged again
static FAILED: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

// said once, not for every device
static REFUSED: AtomicBool = AtomicBool::new(false);

/*
 * runs on its own thread for good: reads every keyboard there is, and each one that comes, for the
 * OSD's levels and lights. Ends only when /dev/input cannot be watched, the keyboards there then
 * still read
 */
pub fn follow(reads: Reads) {
    let wanted = Wanted::of(reads);

    *LIT.lock().unwrap_or_else(PoisonError::into_inner) = Some(Lights::now());

    // watched before the first look, so a device that comes between is seen by one or the other
    let watch = Inotify::init().and_then(|inotify| {
        inotify
            .watches()
            .add(DEVICES, WatchMask::CREATE | WatchMask::ATTRIB)
            .map(|_| inotify)
    });

    for entry in fs::read_dir(DEVICES).into_iter().flatten().flatten() {
        read(&entry.path(), Seen::Started, reads, &wanted);
    }

    let mut inotify = match watch {
        Ok(inotify) => inotify,
        Err(error) => {
            supervise::stopped("keys", format!("cannot watch {DEVICES}: {error}"));
            return;
        }
    };

    let mut buffer = [0; 4096];

    loop {
        let events = match inotify.read_events_blocking(&mut buffer) {
            Ok(events) => events,
            Err(error) => {
                supervise::stopped("keys", format!("lost the watch on {DEVICES}: {error}"));
                return;
            }
        };

        for event in events {
            let seen = if event.mask.contains(EventMask::CREATE) {
                Seen::Created
            } else {
                Seen::Changed
            };

            if let Some(name) = event.name {
                read(&Path::new(DEVICES).join(name), seen, reads, &wanted);
            }
        }
    }
}

// how a device came to be looked at
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    // there when Kanade started, so a refusal is the user's group, said once
    Started,

    // just made by the kernel, root's alone until udev gives it to the group, which says so as
    // an ATTRIB: a refusal is that, not yet the user's group
    Created,

    // udev changed it, as when it gave it to the group
    Changed,
}

/*
 * opens and masks `device` here, on the watching thread, then reads it on its own, if it is a
 * keyboard with what is wanted and not read yet. Here, so every later inotify event of the same
 * device comes after this one is done with
 */
fn read(device: &Path, seen: Seen, reads: Reads, wanted: &Wanted) {
    let Some(name) = device.file_name().and_then(|name| name.to_str()) else {
        return;
    };

    if !name.starts_with("event") || !can(name, &wanted.keys, wanted.lights()) {
        return;
    }

    {
        let mut failed = FAILED.lock().unwrap_or_else(PoisonError::into_inner);

        // a new device under the old name is tried again
        if seen == Seen::Created {
            failed.remove(device);
        }

        if failed.contains(device)
            || READ
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(device)
        {
            return;
        }
    }

    let file = match open(device, wanted) {
        Ok(file) => file,
        Err(error) => {
            refused(device, seen, &error);
            return;
        }
    };

    READ.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(device.to_owned());

    let device = device.to_owned();
    let wanted = wanted.clone();
    let mut opened = Some(file);

    supervise::spawn("keys", move || {
        // opened again only when a panic ran it again
        let ended = match opened.take().map_or_else(|| open(&device, &wanted), Ok) {
            Ok(file) => listen(file, reads),
            Err(error) => error,
        };

        READ.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&device);

        // unplugged
        if ended.kind() != io::ErrorKind::NotFound && ended.raw_os_error() != Some(ENODEV) {
            eprintln!("kanade: stopped reading {} ({ended})", device.display());
        }
    });
}

// opened and masked: not masked, it would hear every key typed, so it is not read at all
fn open(device: &Path, wanted: &Wanted) -> io::Result<File> {
    let file = File::open(device)?;

    clock::mask_input(&file, &[(EV_KEY, &wanted.keys), (EV_LED, wanted.lights())])?;

    Ok(file)
}

// why a device was not read, said once; a refusal udev is about to lift is not said, nor kept
fn refused(device: &Path, seen: Seen, error: &io::Error) {
    match error.kind() {
        io::ErrorKind::PermissionDenied if seen == Seen::Created => {}
        io::ErrorKind::PermissionDenied => {
            if !REFUSED.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "kanade: cannot read {} ({error}); without the input group the OSD shows no key \
                     at a limit and no lock light",
                    device.display()
                );
            }
        }
        // gone before it was read
        io::ErrorKind::NotFound => {}
        _ => {
            FAILED
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(device.to_owned());

            eprintln!("kanade: cannot read {} ({error})", device.display());
        }
    }
}

// <errno.h>: the device was unplugged while read
const ENODEV: i32 = 19;

/*
 * for doctor: whether the devices with the keys or lights asked for can be read, masked as the
 * shell masks them, any level's keys asked for
 */
pub fn found() -> Result<String, String> {
    let wanted = Wanted::of(Reads {
        audio: true,
        brightness: true,
    });
    let mut read = 0;

    for entry in fs::read_dir(DEVICES).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();

        if !name.starts_with("event") || !can(&name, &wanted.keys, wanted.lights()) {
            continue;
        }

        match open(&entry.path(), &wanted) {
            Ok(_) => read += 1,
            // unplugged since it was listed
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    || error.raw_os_error() == Some(ENODEV) => {}
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                return Err(format!(
                    "cannot read {} ({error}), join the input group",
                    entry.path().display()
                ));
            }
            Err(error) => {
                return Err(format!("cannot read {} ({error})", entry.path().display()));
            }
        }
    }

    match read {
        0 => Err(String::from("no input device with the keys or lights")),
        read => Ok(format!("{read} input devices readable in {DEVICES}")),
    }
}

// whether the device can send one of `keys` or `lights`, as sysfs lists what it can
fn can(name: &str, keys: &[u16], lights: &[u16]) -> bool {
    let capability = |what: &str| {
        fs::read_to_string(
            Path::new(CLASS)
                .join(name)
                .join("device/capabilities")
                .join(what),
        )
        .unwrap_or_default()
    };

    let key = capability("key");
    let led = capability("led");

    keys.iter().any(|&code| has(&key, code)) || lights.iter().any(|&code| has(&led, code))
}

// a capability bitmap as sysfs prints it: hex longs, the highest first
fn has(bitmap: &str, code: u16) -> bool {
    let word = usize::from(code / 64);

    bitmap
        .split_whitespace()
        .rev()
        .nth(word)
        .and_then(|hex| u64::from_str_radix(hex, 16).ok())
        .is_some_and(|bits| bits & (1 << (code % 64)) != 0)
}

// reads one opened, masked device until it is gone; why it ended
fn listen(mut file: File, reads: Reads) -> io::Error {
    let mut buffer = [0; INPUT_EVENT * 64];

    loop {
        let length = match file.read(&mut buffer) {
            Ok(0) => return io::ErrorKind::UnexpectedEof.into(),
            Ok(length) => length,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return error,
        };

        for bytes in buffer[..length].as_chunks::<INPUT_EVENT>().0 {
            match heard(InputEvent::of(bytes)) {
                Some(Input::Key(key)) => osd::pressed(key, reads),
                Some(Input::Light(mode)) if turned(mode) => osd::mode(mode),
                _ => {}
            }
        }
    }
}

fn turned(mode: Mode) -> bool {
    LIT.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(Lights::default)
        .turn(mode)
}
