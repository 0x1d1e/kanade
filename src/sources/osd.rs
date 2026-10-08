//! Volume, brightness and microphone changes (plan 3, 5.1, ADR 0007): a level that changes shows in
//! the OSD on the focused output, and a held key keeps showing it again, so it extends one OSD.
//! None of it shows on the island.
//!
//! Amane has no subscription between services, so this reads them again when PulseAudio or the
//! kernel announces a change (`wake`), and polls while a change settles or an announcer is down; a
//! read that finds the levels unchanged shows nothing, so an idle OSD never redraws. It reads only
//! what the `audio` and `brightness` Modules that are on allow.
//!
//! `kanade osd volume|brightness` shows a level as it is, for a keybind that changes it outside
//! Kanade.

use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use amane::{Audio, Brightness, Service};

use super::wake::{Announcer, Pace, Wakes};
use crate::island::activity::{Device, Volume};
use crate::osd::{Level, Osd};
use crate::supervise;

const PACE: Pace = Pace {
    // plan 3: 50-100 ms; a held key repeats every 40 ms or so, so the bar moves about every other
    // step
    poll: Duration::from_millis(80),

    // longer than Amane's Brightness takes to read the backlight again, every 500 ms
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

            (
                Some(speaker(&audio)),
                Some(Volume {
                    device: Device::Microphone,
                    percent: audio.microphone_volume(),
                    muted: audio.microphone_muted(),
                }),
            )
        } else {
            (None, None)
        };

        let brightness = reads
            .brightness
            .then(|| {
                let brightness = Brightness::read();

                brightness.present().then(|| brightness.percent())
            })
            .flatten();

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
        Asked::Brightness => {
            /*
             * Amane reads the backlight again only every 500 ms, and a keybind asks right after
             * it set it, so this reads it now; a sysfs read, cheap on the draw thread
             */
            let mut brightness = Brightness::write();
            brightness.update();

            if !brightness.present() {
                return Err(String::from("there is no backlight"));
            }

            Level::Brightness(brightness.percent())
        }
    };

    Osd::write().show(level, Instant::now());
    Ok(())
}

/*
 * `osd volume` asks the volume thread to read it: Amane's Audio hears of a change from PulseAudio
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
        // checked first under the read lock, as a write wakes every OSD window
        if !Osd::read().shown_since(asked) {
            Osd::write().answer(level, asked, Instant::now());
        }
    });
}

/*
 * waiting asks fold into one read, answered for the latest. A read answers only asks made before it
 * started: one made during it may follow a change the read missed, so it reads again for that one
 */
fn serve(
    asks: &Receiver<Instant>,
    mut read: impl FnMut() -> Volume,
    mut show: impl FnMut(Level, Instant),
) {
    while let Ok(first) = asks.recv() {
        let mut asked = asks.try_iter().last().unwrap_or(first);
        let mut volume = read();

        while let Some(later) = asks.try_iter().last() {
            asked = later;
            volume = read();
        }

        show(Level::Volume(volume), asked);
    }
}

/*
 * a fresh Audio of its own, not Amane's shared one: updating that one holds its write lock while
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
fn changes(before: Option<Levels>, now: Levels) -> Vec<Level> {
    let Some(before) = before else {
        return Vec::new();
    };

    let mut changes = Vec::new();

    if let Some(speaker) = now.speaker
        && now.speaker != before.speaker
    {
        changes.push(Level::Volume(speaker));
    }

    if let Some(microphone) = now.microphone
        && Some(microphone.muted) != before.microphone.map(|before| before.muted)
    {
        changes.push(Level::Volume(microphone));
    }

    if let Some(percent) = now.brightness
        && now.brightness != before.brightness
    {
        changes.push(Level::Brightness(percent));
    }

    changes
}

// runs on its own thread for good; only while it reads something
pub fn follow(reads: Reads) {
    let mut wakes = Wakes::new(PACE, reads.announcers());
    let mut last = None;

    supervise::run("osd", || {
        loop {
            let levels = Levels::read(reads);
            let changes = changes(last, levels);

            // one OSD: what changed last takes its place
            if let Some(&shown) = changes.last() {
                Osd::write().show(shown, Instant::now());
            }

            let busy = last != Some(levels);
            last = Some(levels);
            wakes.wait(busy);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    // as `pactl subscribe` and `udevadm monitor` print them
    #[test]
    fn only_level_changes_announce() {
        assert!((PULSE.announces)("Event 'change' on sink #55"));
        assert!((PULSE.announces)("Event 'change' on source #56"));
        assert!((PULSE.announces)("Event 'change' on server #-1"));
        assert!(!(PULSE.announces)("Event 'change' on client #223"));
        assert!(!(PULSE.announces)("Event 'new' on sink-input #12"));
        assert!(!(PULSE.announces)("Event 'change' on card #49"));

        let backlight =
            "KERNEL[2909.593314] change   /devices/backlight/nvidia_wmi_ec_backlight (backlight)";
        assert!((BACKLIGHT.announces)(backlight));
        assert!(!(BACKLIGHT.announces)(
            "monitor will print the received events for:"
        ));
        assert!(!(BACKLIGHT.announces)("KERNEL - the kernel uevent"));
    }

    fn levels(speaker: u8, muted: bool) -> Levels {
        Levels {
            speaker: Some(Volume {
                device: Device::Speaker,
                percent: speaker,
                muted,
            }),
            microphone: Some(Volume {
                device: Device::Microphone,
                percent: 80,
                muted: false,
            }),
            brightness: Some(60),
        }
    }

    fn microphone(levels: &mut Levels) -> &mut Volume {
        levels.microphone.as_mut().unwrap()
    }

    #[test]
    fn the_first_read_shows_nothing() {
        assert_eq!(changes(None, levels(40, false)), []);
    }

    #[test]
    fn unchanged_levels_show_nothing() {
        let idle = levels(40, false);

        assert_eq!(changes(Some(idle), idle), []);
    }

    #[test]
    fn volume_and_mute_show_the_speaker() {
        let before = levels(40, false);

        for now in [levels(45, false), levels(40, true)] {
            assert_eq!(
                changes(Some(before), now),
                [Level::Volume(now.speaker.unwrap())]
            );
        }
    }

    #[test]
    fn microphone_shows_for_a_mute_only() {
        let before = levels(40, false);

        let mut gain = before;
        microphone(&mut gain).percent = 30;
        assert_eq!(changes(Some(before), gain), []);

        let mut muted = before;
        microphone(&mut muted).muted = true;
        assert_eq!(
            changes(Some(before), muted),
            [Level::Volume(muted.microphone.unwrap())]
        );
    }

    #[test]
    fn brightness_shows_while_there_is_a_backlight() {
        let before = levels(40, false);

        let mut brighter = before;
        brighter.brightness = Some(70);
        assert_eq!(changes(Some(before), brighter), [Level::Brightness(70)]);

        let mut gone = before;
        gone.brightness = None;
        assert_eq!(changes(Some(before), gone), []);
    }

    // with `audio` or `brightness` off, its levels are never read, so never change
    #[test]
    fn what_is_not_read_never_shows() {
        let unread = Levels {
            speaker: None,
            microphone: None,
            brightness: None,
        };

        let mut brightness_only = unread;
        brightness_only.brightness = Some(50);

        assert_eq!(changes(Some(unread), unread), []);
        assert_eq!(
            changes(Some(unread), brightness_only),
            [Level::Brightness(50)]
        );
    }

    #[test]
    fn osd_asks_for_volume_or_brightness() {
        assert_eq!(Asked::parse(&["volume"]), Some(Asked::Volume));
        assert_eq!(Asked::parse(&["brightness"]), Some(Asked::Brightness));

        for words in [&[][..], &["microphone"], &["volume", "50"], &["Volume"]] {
            assert_eq!(Asked::parse(words), None, "{words:?}");
        }
    }

    // refused before any Service is read, so an off Module's stays cold
    #[test]
    fn osd_is_refused_for_a_module_that_is_off() {
        let off = Reads {
            audio: false,
            brightness: false,
        };

        assert_eq!(
            show(Asked::Volume, off),
            Err(String::from("module audio is off"))
        );
        assert_eq!(
            show(Asked::Brightness, off),
            Err(String::from("module brightness is off"))
        );
    }

    fn speaker_at(percent: u8) -> Volume {
        Volume {
            device: Device::Speaker,
            percent,
            muted: false,
        }
    }

    /*
     * a keybind sets the volume, then asks at once, when Amane's Audio may still say the old one:
     * what shows is read after the ask, and asks while one waits fold into it
     */
    #[test]
    fn osd_volume_shows_the_volume_read_after_the_ask() {
        let (volume, asks) = mpsc::channel();
        let first = Instant::now();
        let second = first + Duration::from_millis(100);
        let mut server = 40;

        server += 5;
        assert_eq!(ask(&volume, first), Ok(()));
        server += 5;
        assert_eq!(ask(&volume, second), Ok(()));
        drop(volume);

        let mut shown = Vec::new();
        serve(
            &asks,
            || speaker_at(server),
            |level, asked| shown.push((level, asked)),
        );

        assert_eq!(shown, [(Level::Volume(speaker_at(50)), second)]);
    }

    // the volume changes and asks again while the first ask is read: only the second read shows
    #[test]
    fn an_ask_during_a_read_reads_again_and_the_first_read_never_shows() {
        let (volume, asks) = mpsc::channel();
        let first = Instant::now();
        let second = first + Duration::from_millis(100);
        let mut volume = Some(volume);
        let mut server = 40;

        assert_eq!(ask(volume.as_ref().unwrap(), first), Ok(()));

        let mut shown = Vec::new();
        serve(
            &asks,
            || {
                let read = speaker_at(server);

                // the change and its ask land while this read waits on PulseAudio
                if let Some(volume) = volume.take() {
                    server += 5;
                    assert_eq!(ask(&volume, second), Ok(()));
                }

                read
            },
            |level, asked| shown.push((level, asked)),
        );

        assert_eq!(shown, [(Level::Volume(speaker_at(45)), second)]);
    }

    // brightness shows while the volume is still being read: the volume, read late, gives way
    #[test]
    fn a_slow_volume_read_does_not_cover_a_newer_level() {
        let (volume, asks) = mpsc::channel();
        let asked = Instant::now();
        let delay = Duration::from_millis(300);
        let osd = RefCell::new(Osd::new());

        assert_eq!(ask(&volume, asked), Ok(()));
        drop(volume);

        serve(
            &asks,
            || {
                osd.borrow_mut()
                    .show(Level::Brightness(80), asked + delay / 2);
                speaker_at(45)
            },
            |level, asked| osd.borrow_mut().answer(level, asked, asked + delay),
        );

        assert_eq!(osd.borrow().shown_on("eDP-1"), Some(Level::Brightness(80)));
    }

    #[test]
    fn osd_volume_without_its_thread_is_refused() {
        let (volume, asks) = mpsc::channel();
        drop(asks);

        assert_eq!(
            ask(&volume, Instant::now()),
            Err(String::from("the volume thread stopped"))
        );
    }

    #[test]
    fn only_the_announcers_of_what_is_read_start() {
        let programs = |audio, brightness| {
            Reads { audio, brightness }
                .announcers()
                .iter()
                .map(|announcer| announcer.program)
                .collect::<Vec<_>>()
        };

        assert_eq!(programs(true, true), ["pactl", "udevadm"]);
        assert_eq!(programs(true, false), ["pactl"]);
        assert_eq!(programs(false, true), ["udevadm"]);
        assert!(
            !Reads {
                audio: false,
                brightness: false
            }
            .any()
        );
    }
}
