//! Volume and Brightness Transients (plan 3, 5.1): a level that changes shows on the focused island
//! for OSD, and a held key keeps reposting the same Activity, so it extends one Transient.
//!
//! Amane has no subscription between services, so this polls their reads; a read that finds the
//! levels unchanged posts nothing, so an idle island never redraws.

use std::thread;
use std::time::{Duration, Instant};

use amane::{Audio, Brightness, Service};

use crate::island::activity::{Activity, Detail, Device, Id, Kind, Priority, Volume};
use crate::island::service::IslandService;

// plan 3: 50-100 ms; a held key repeats every 40 ms or so, so the bar moves about every other step
const POLL: Duration = Duration::from_millis(80);

// plan 5.2: 1000-1400 ms from the last change
const OSD: Duration = Duration::from_millis(1200);

// every level the island can show, as the services last said
#[derive(Debug, Clone, Copy, PartialEq)]
struct Levels {
    speaker: Volume,
    microphone: Volume,

    // none without a backlight, like a desktop monitor
    brightness: Option<u8>,
}

impl Levels {
    fn read() -> Levels {
        let audio = Audio::read();

        let speaker = Volume {
            device: Device::Speaker,
            percent: audio.volume(),
            muted: audio.muted(),
        };

        let microphone = Volume {
            device: Device::Microphone,
            percent: audio.microphone_volume(),
            muted: audio.microphone_muted(),
        };

        drop(audio);

        let brightness = Brightness::read();

        Levels {
            speaker,
            microphone,
            brightness: brightness.present().then(|| brightness.percent()),
        }
    }
}

/*
 * what changed since the last read, as the Transients that show it. The first read only sets where
 * the levels start, so starting the shell shows nothing. The microphone shows only for a mute:
 * apps tune its volume on their own, like a call's gain control, which no one asked to see
 */
fn changes(before: Option<Levels>, now: Levels) -> Vec<Activity> {
    let Some(before) = before else {
        return Vec::new();
    };

    let mut changes = Vec::new();

    if now.speaker != before.speaker {
        changes.push(volume(now.speaker));
    }

    if now.microphone.muted != before.microphone.muted {
        changes.push(volume(now.microphone));
    }

    if let Some(percent) = now.brightness
        && now.brightness != before.brightness
    {
        changes.push(
            Activity::transient(Id::new(Kind::Brightness, "backlight"), Priority::Osd, OSD)
                .with_detail(Detail::Brightness(percent)),
        );
    }

    changes
}

// one Activity per device, so the speaker and the microphone each extend their own
fn volume(volume: Volume) -> Activity {
    let key = match volume.device {
        Device::Speaker => "speaker",
        Device::Microphone => "microphone",
    };

    Activity::transient(Id::new(Kind::Volume, key), Priority::Osd, OSD)
        .with_detail(Detail::Volume(volume))
}

// runs on its own thread for good
pub fn follow() {
    let mut last = None;

    loop {
        let levels = Levels::read();
        let changes = changes(last, levels);

        if !changes.is_empty() {
            let now = Instant::now();
            let mut island = IslandService::write();

            for activity in changes {
                island.post(activity, now);
            }
        }

        last = Some(levels);
        thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::activity::Lifetime;

    fn levels(speaker: u8, muted: bool) -> Levels {
        Levels {
            speaker: Volume {
                device: Device::Speaker,
                percent: speaker,
                muted,
            },
            microphone: Volume {
                device: Device::Microphone,
                percent: 80,
                muted: false,
            },
            brightness: Some(60),
        }
    }

    fn shown(changes: &[Activity]) -> Vec<(&str, &Detail)> {
        changes
            .iter()
            .map(|activity| (activity.id().key(), activity.detail()))
            .collect()
    }

    #[test]
    fn the_first_read_shows_nothing() {
        assert_eq!(changes(None, levels(40, false)), []);
    }

    #[test]
    fn unchanged_levels_post_nothing() {
        let idle = levels(40, false);

        assert_eq!(changes(Some(idle), idle), []);
    }

    #[test]
    fn volume_and_mute_show_the_speaker() {
        let before = levels(40, false);

        for now in [levels(45, false), levels(40, true)] {
            let changes = changes(Some(before), now);

            assert_eq!(shown(&changes), [("speaker", &Detail::Volume(now.speaker))]);
            assert_eq!(changes[0].kind(), Kind::Volume);
            assert_eq!(changes[0].lifetime(), Lifetime::Transient(OSD));
        }
    }

    // every step of a held key is the same Activity, so the Arbiter extends one Transient
    #[test]
    fn repeated_steps_share_one_id() {
        let steps = [40, 45, 50].map(|percent| levels(percent, false));

        let ids: Vec<Id> = steps
            .windows(2)
            .flat_map(|pair| changes(Some(pair[0]), pair[1]))
            .map(|activity| activity.id().clone())
            .collect();

        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], ids[1]);
    }

    #[test]
    fn microphone_shows_for_a_mute_only() {
        let before = levels(40, false);

        let mut gain = before;
        gain.microphone.percent = 30;
        assert_eq!(changes(Some(before), gain), []);

        let mut muted = before;
        muted.microphone.muted = true;
        assert_eq!(
            shown(&changes(Some(before), muted)),
            [("microphone", &Detail::Volume(muted.microphone))]
        );
    }

    #[test]
    fn brightness_shows_while_there_is_a_backlight() {
        let before = levels(40, false);

        let mut brighter = before;
        brighter.brightness = Some(70);
        assert_eq!(
            shown(&changes(Some(before), brighter)),
            [("backlight", &Detail::Brightness(70))]
        );

        let mut gone = before;
        gone.brightness = None;
        assert_eq!(changes(Some(before), gone), []);
    }
}
