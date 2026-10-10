//! The OSD (plan 3, 5.1, ADR 0021): a volume, brightness or microphone mute change, a press of a
//! level's key, or a mode turned on or off shows as a Transient on the focused output's Island, and
//! a held key keeps posting it again, so it extends one OSD.
//!
//! `Audio` and `Brightness` have no subscription for another Service, so this reads them again when PulseAudio or the
//! kernel announces a change (`wake`), and polls while a change settles or an announcer is down; a
//! read that finds the levels unchanged posts nothing, so an idle OSD never redraws. It reads only
//! what the `audio` and `brightness` Modules that are on allow.
//!
//! A level's key shows the level at once, before the keybind that changes it ran, and at its limit,
//! where nothing changes (`keys`); a lock key's light and airplane mode show as a Mode (`keys`,
//! `radios`). The keyboard's backlight shows as UPower announces it, the firmware's own key
//! included (`follow_keyboard`).
//!
//! `kanade osd volume|brightness` shows a level as it is, for a keybind that changes it outside
//! Kanade.

use std::fs;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crate::sources::brightness::{self, Brightness};
use crate::sources::pulse::Audio;
use kanade_runtime::service::Service;

use super::seat;
use super::wake::{Announcer, Pace, Wakes};
use crate::bus::{Bus, Value};
use crate::config;
use crate::island::activity::{
    Activity, Detail, Device, Id, Interrupt, Kind, Lifetime, Mode, Priority, Scope, Volume,
};
use crate::island::service::IslandService;
use crate::supervise;

const PACE: Pace = Pace {
    // plan 3: 50-100 ms; a held key repeats every 40 ms or so, so the bar moves about every other
    // step
    poll: Duration::from_millis(80),

    // a beat for PulseAudio to settle; the backlight is read fresh
    settle: Duration::from_secs(1),

    idle: None,
};

// PulseAudio announces each change to a device's volume or mute, and to which is the default
pub const PULSE: Announcer = Announcer {
    program: "pactl",
    args: &["subscribe"],
    announces: |line| {
        ["on sink #", "on source #", "on server"]
            .iter()
            .any(|on| line.contains(on))
    },
};

// the kernel announces each change to a backlight, from a hotkey or a tool like brightnessctl
pub const BACKLIGHT: Announcer = Announcer {
    program: "udevadm",
    args: &["monitor", "--kernel", "--subsystem-match=backlight"],
    announces: |line| line.starts_with("KERNEL["),
};

// what the OSD shows
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Osd {
    Volume(Volume),
    Brightness(u8),
    Keyboard(u8),
    Mode(Mode),
}

impl Osd {
    /*
     * a Transient on the focused output's Island, above everything as the answer to the user's own
     * key, that interrupts nothing, so an open Surface stays. One id per Kind, so a change while it
     * shows takes its place and starts its time again
     */
    fn activity(self) -> Activity {
        let detail = match self {
            Osd::Volume(volume) => Detail::Volume(volume),
            Osd::Brightness(percent) => Detail::Brightness(percent),
            Osd::Keyboard(percent) => Detail::Keyboard(percent),
            Osd::Mode(mode) => Detail::Mode(mode),
        };

        Activity::new(
            self.id(),
            Priority::Feedback,
            Lifetime::Transient(config::get().osd),
            Scope::FocusedOutput,
            Interrupt::None,
        )
        .expect("the config bounds osd above zero")
        .with_detail(detail)
    }

    fn kind(self) -> Kind {
        match self {
            Osd::Volume(_) => Kind::Volume,
            Osd::Brightness(_) | Osd::Keyboard(_) => Kind::Brightness,
            Osd::Mode(_) => Kind::Mode,
        }
    }

    fn id(self) -> Id {
        Id::new(self.kind(), KEY)
    }

    // the other Kinds' OSDs, which this one takes the place of
    fn others(self) -> impl Iterator<Item = Id> {
        KINDS
            .into_iter()
            .filter(move |&kind| kind != self.kind())
            .map(|kind| Id::new(kind, KEY))
    }
}

// every Kind the OSD shows
const KINDS: [Kind; 3] = [Kind::Volume, Kind::Brightness, Kind::Mode];

const KEY: &str = "osd";

// when a level last showed, so a volume read for an older ask gives way to it
static SHOWN: Mutex<Shown> = Mutex::new(Shown { last: None });

struct Shown {
    last: Option<Instant>,
}

impl Shown {
    // a change shows at once
    fn show(&mut self, now: Instant) -> bool {
        self.last = Some(now);
        true
    }

    /*
     * a level read for an ask made at `asked` shows, unless a level showed since: that one is newer
     * than the ask, so the latest change shows, not the slowest read
     */
    fn answer(&mut self, asked: Instant, now: Instant) -> bool {
        if self.last.is_some_and(|last| last > asked) {
            return false;
        }

        self.show(now)
    }
}

/*
 * posts the OSD to the Island, in place of the other Kinds', under the lock, so an answer checked
 * against it cannot slip between. Nothing shows, or counts as shown, while another session has the
 * seat, whose keys and radios are heard too
 */
fn post(osd: Osd, shows: impl FnOnce(&mut Shown, Instant) -> bool) {
    let mut shown = SHOWN.lock().unwrap_or_else(PoisonError::into_inner);
    let now = Instant::now();

    if seat::seated() && shows(&mut shown, now) {
        place(&mut IslandService::write(), osd, now);
    }
}

/*
 * reads a level and posts it under the same lock, so two threads reading it cannot post out of
 * order and leave the older level showing
 */
fn read_and_post(read: impl FnOnce() -> Option<Osd>) {
    let mut shown = SHOWN.lock().unwrap_or_else(PoisonError::into_inner);

    if let Some(osd) = read().filter(|_| seat::seated()) {
        let now = Instant::now();

        shown.show(now);
        place(&mut IslandService::write(), osd, now);
    }
}

// posted first, so the Island never goes without a primary between
fn place(island: &mut IslandService, osd: Osd, now: Instant) {
    island.post(osd.activity(), now);

    for other in osd.others() {
        island.withdraw(&other, now);
    }
}

// UPower, which drives the keyboard's backlight and announces each change to it
pub const UPOWER: &str = "org.freedesktop.UPower";
const KBD_BACKLIGHT: &str = "/org/freedesktop/UPower/KbdBacklight";
const KBD_INTERFACE: &str = "org.freedesktop.UPower.KbdBacklight";

// a level of the keyboard's backlight out of the highest, as a percent
fn keyboard_percent(level: f64, highest: f64) -> Option<u8> {
    (highest > 0.0).then(|| (level.clamp(0.0, highest) * 100.0 / highest).round() as u8)
}

/*
 * runs on its own thread for good, or until the system bus or the backlight is gone; waits on
 * UPower, so it never wakes on its own. Each change shows, from its key, which only the firmware
 * hears on many laptops, or from any program
 */
pub fn follow_keyboard() {
    let bus = Bus::system();

    // subscribed before the read, so no change falls between them
    let signals = bus.signals(KBD_INTERFACE, "BrightnessChanged");
    let highest = bus
        .call(
            UPOWER,
            KBD_BACKLIGHT,
            KBD_INTERFACE,
            "GetMaxBrightness",
            &[],
        )
        .number();

    if highest <= 0.0 {
        return;
    }

    for signal in signals.filter(|signal| signal.path() == KBD_BACKLIGHT) {
        let level = signal.arguments().first().map_or(-1.0, Value::number);

        if let Some(percent) = keyboard_percent(level, highest) {
            post(Osd::Keyboard(percent), Shown::show);
        }
    }

    supervise::stopped("kbd-backlight", String::from("the system bus closed"));
}

// a mode turned on or off, as `keys` and `radios` hear it
pub fn mode(mode: Mode) {
    post(Osd::Mode(mode), Shown::show);
}

// which level a key changes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Speaker,
    Brightness,
}

/*
 * a level's key was pressed or repeats: the level as it is shows at once, the change the keybind
 * makes follows as it lands, and at a limit, where nothing changes, it still shows. Nothing for a
 * level not read
 */
pub fn pressed(key: Key, reads: Reads) {
    read_and_post(|| match key {
        Key::Speaker if reads.audio => Some(Osd::Volume(speaker(&Audio::read()))),
        Key::Brightness if reads.brightness => brightness().map(Osd::Brightness),
        _ => None,
    });
}

// which Services this reads: those of the `audio` and `brightness` Modules that are on
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reads {
    pub audio: bool,
    pub brightness: bool,
}

impl Reads {
    pub fn any(self) -> bool {
        self.audio || self.brightness
    }

    // only the announcers of what it reads, so an off Module starts no helper process
    fn announcers(self) -> Vec<Announcer> {
        [(self.audio, PULSE), (self.brightness, BACKLIGHT)]
            .into_iter()
            .filter_map(|(reads, announcer)| reads.then_some(announcer))
            .collect()
    }
}

// every level the OSD can show, as the services last said; none for what is not read
#[derive(Debug, Clone, Copy, PartialEq)]
struct Levels {
    speaker: Option<Volume>,
    microphone: Option<Volume>,

    // none also without a backlight, like a desktop monitor
    brightness: Option<u8>,
}

impl Levels {
    fn read(reads: Reads) -> Levels {
        let (speaker, microphone) = if reads.audio {
            let audio = Audio::read();

            (Some(speaker(&audio)), Some(microphone(&audio)))
        } else {
            (None, None)
        };

        let brightness = reads.brightness.then(brightness).flatten();

        Levels {
            speaker,
            microphone,
            brightness,
        }
    }
}

fn speaker(audio: &Audio) -> Volume {
    Volume {
        device: Device::Speaker,
        percent: audio.volume(),
        muted: audio.muted(),
    }
}

fn microphone(audio: &Audio) -> Volume {
    Volume {
        device: Device::Microphone,
        percent: audio.microphone_volume(),
        muted: audio.microphone_muted(),
    }
}

// where `Brightness` finds the backlight
const BACKLIGHTS: &str = "/sys/class/backlight";

/*
 * the backlight as it is now, none without one. `Brightness` reads it again only every
 * 500 ms, which a change would wait on, so this reads the file it does, the first backlight the
 * kernel lists, as it reckons it, and brings that one up to date only when it lags, so a read that
 * finds nothing new redraws nothing
 */
fn brightness() -> Option<u8> {
    let (present, shared) = {
        let brightness = Brightness::read();

        (brightness.present(), brightness.percent())
    };

    if !present {
        return None;
    }

    let percent = backlight().unwrap_or(shared);

    if percent != shared {
        Brightness::write().update();
    }

    Some(percent)
}

// a sysfs read, cheap enough for the draw thread
fn backlight() -> Option<u8> {
    let first = fs::read_dir(BACKLIGHTS).ok()?.flatten().next()?.path();
    let read = |name| {
        fs::read_to_string(first.join(name))
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()
    };

    let highest = read("max_brightness").filter(|&highest| highest > 0)?;

    Some(brightness::percent_of(read("brightness")?, highest))
}

// which level `kanade osd` shows
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    Volume,
    Brightness,
}

impl Asked {
    pub fn parse(arguments: &[&str]) -> Option<Asked> {
        match arguments {
            ["volume"] => Some(Asked::Volume),
            ["brightness"] => Some(Asked::Brightness),
            _ => None,
        }
    }
}

/*
 * `kanade osd volume|brightness`: the OSD for the level as it is, on the focused output. Refused
 * while its Module is off, as nothing reads that level then, or without a backlight
 */
pub fn show(asked: Asked, reads: Reads) -> Result<(), String> {
    let level = match asked {
        Asked::Volume if !reads.audio => return Err(String::from("module audio is off")),
        Asked::Brightness if !reads.brightness => {
            return Err(String::from("module brightness is off"));
        }
        // shown by the volume thread, once it read it
        Asked::Volume => {
            return VOLUME
                .get()
                .ok_or_else(|| String::from("the volume thread is not running"))
                .and_then(|volume| ask(volume, Instant::now()));
        }
        // a keybind asks right after it set it, so read now
        Asked::Brightness => match brightness() {
            Some(percent) => Osd::Brightness(percent),
            None => return Err(String::from("there is no backlight")),
        },
    };

    post(level, Shown::show);
    Ok(())
}

/*
 * `osd volume` asks the volume thread to read it: `Audio` hears of a change from PulseAudio
 * a beat after it is made, and a keybind asks right after it made it. Reading waits on
 * PulseAudio, so not on the draw thread. Each ask carries when it was made
 */
static VOLUME: OnceLock<Sender<Instant>> = OnceLock::new();

// never waits: the channel is unbounded, and the thread drains it whole on each read
fn ask(volume: &Sender<Instant>, now: Instant) -> Result<(), String> {
    volume
        .send(now)
        .map_err(|_| String::from("the volume thread stopped"))
}

// the asks of `osd volume`, for the thread `answer_volume` runs on; once, while `audio` is on
pub fn volume_asks() -> Receiver<Instant> {
    let (sender, asks) = mpsc::channel();

    VOLUME
        .set(sender)
        .expect("the volume thread is started once");
    asks
}

// runs on its own thread for good; waits on asks, so it never wakes on its own
pub fn answer_volume(asks: &Receiver<Instant>) {
    serve(asks, fresh_speaker, |level, asked| {
        post(level, |shown, now| shown.answer(asked, now));
    });
}

/*
 * waiting asks fold into one read, answered for the latest. A read answers only asks made before it
 * started: one made during it may follow a change the read missed, so it reads again for that one
 */
fn serve(
    asks: &Receiver<Instant>,
    mut read: impl FnMut() -> Volume,
    mut show: impl FnMut(Osd, Instant),
) {
    while let Ok(first) = asks.recv() {
        let mut asked = asks.try_iter().last().unwrap_or(first);
        let mut volume = read();

        while let Some(later) = asks.try_iter().last() {
            asked = later;
            volume = read();
        }

        show(Osd::Volume(volume), asked);
    }
}

/*
 * a fresh Audio of its own, not the shared one: updating that one holds its write lock while
 * PulseAudio answers, which would stall every view that reads it. Its PulseAudio connection is
 * this thread's, kept for the next ask
 */
fn fresh_speaker() -> Volume {
    speaker(&Audio::new())
}

/*
 * what changed since the last read, in order. The first read only sets where the levels start, so
 * starting the shell shows nothing. The microphone shows only for a mute: apps tune its volume on
 * their own, like a call's gain control, which no one asked to see
 */
fn changes(before: Option<Levels>, now: Levels) -> Vec<Osd> {
    let Some(before) = before else {
        return Vec::new();
    };

    let mut changes = Vec::new();

    if let Some(speaker) = now.speaker
        && now.speaker != before.speaker
    {
        changes.push(Osd::Volume(speaker));
    }

    if let Some(microphone) = now.microphone
        && Some(microphone.muted) != before.microphone.map(|before| before.muted)
    {
        changes.push(Osd::Volume(microphone));
    }

    if let Some(percent) = now.brightness
        && now.brightness != before.brightness
    {
        changes.push(Osd::Brightness(percent));
    }

    changes
}

// runs on its own thread for good; only while it reads something
pub fn follow(reads: Reads) {
    let mut wakes = Wakes::new(PACE, reads.announcers());
    let mut last = None;

    supervise::run("osd", || {
        loop {
            let mut busy = false;

            read_and_post(|| {
                let levels = Levels::read(reads);

                // one OSD: what changed last takes its place
                let shown = changes(last, levels).last().copied();

                busy = last != Some(levels);
                last = Some(levels);
                shown
            });

            wakes.wait(busy);
        }
    });
}
