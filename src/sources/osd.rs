//! Volume, brightness and microphone changes (plan 3, 5.1, ADR 0007): a level that changes shows in
//! the OSD on the focused output, and a held key keeps showing it again, so it extends one OSD.
//! Until #109 each change also shows as a Transient on the focused island.
//!
//! Amane has no subscription between services, so this reads them again when PulseAudio or the
//! kernel announces a change (`wake`), and polls while a change settles or an announcer is down; a
//! read that finds the levels unchanged shows nothing, so an idle OSD never redraws. It reads only
//! what the `audio` and `brightness` Modules that are on allow.

use std::time::{Duration, Instant};

use amane::{Audio, Brightness, Service};

use super::wake::{Announcer, Pace, Wakes};
use crate::island::activity::{
    Activity, Detail, Device, Id, Interrupt, Kind, Lifetime, Priority, Scope, Volume,
};
use crate::island::service::IslandService;
use crate::osd::{Level, Osd};
use crate::{config, supervise};

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
                Some(Volume {
                    device: Device::Speaker,
                    percent: audio.volume(),
                    muted: audio.muted(),
                }),
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

/*
 * until #109, the Transient that shows a change over the primary on the island the user is
 * looking at; one Activity per device, so the speaker and the microphone each extend their own
 */
fn transient(level: Level) -> Activity {
    let (id, detail) = match level {
        Level::Volume(volume) => {
            let key = match volume.device {
                Device::Speaker => "speaker",
                Device::Microphone => "microphone",
            };

            (Id::new(Kind::Volume, key), Detail::Volume(volume))
        }
        Level::Brightness(percent) => (
            Id::new(Kind::Brightness, "backlight"),
            Detail::Brightness(percent),
        ),
    };

    Activity::new(
        id,
        Priority::Osd,
        Lifetime::Transient(config::get().osd),
        Scope::FocusedOutput,
        Interrupt::Transient,
    )
    .expect("the config bounds osd above zero")
    .with_detail(detail)
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
                let now = Instant::now();

                Osd::write().show(shown, now);

                let mut island = IslandService::write();

                for level in changes {
                    island.post(transient(level), now);
                }
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

    // every step of a held key is the same Transient, so the Arbiter extends one, until #109
    #[test]
    fn repeated_steps_share_one_transient() {
        let steps = [40, 45, 50].map(|percent| levels(percent, false));

        let ids: Vec<Id> = steps
            .windows(2)
            .flat_map(|pair| changes(Some(pair[0]), pair[1]))
            .map(|level| transient(level).id().clone())
            .collect();

        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], ids[1]);

        let shown = transient(Level::Brightness(70));
        assert_eq!(shown.kind(), Kind::Brightness);
        assert_eq!(shown.detail(), &Detail::Brightness(70));
        assert_eq!(shown.lifetime(), Lifetime::Transient(config::get().osd));
    }
}
