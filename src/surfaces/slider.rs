//! A level a Surface sets by a press, a drag along its bar, or the wheel: the speaker's volume on
//! Media and Controls, and the screen's brightness, the microphone's volume and an app's on
//! Controls.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::sources::brightness::Brightness;
use crate::sources::pulse::Audio;
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{Center, Color, Cursor, Rectangle, Stack, Start, children, request_frame};
use kanade_runtime::{MutexExt, Widget, service::Service};

use crate::modules;
use crate::sources::audio::{self, Mixer, Node};
use crate::theme;
use crate::theme::space::TARGET;
use crate::view::bar;

// percent per wheel line
const WHEEL: f32 = 5.0;

// the thumb's width, and how long it stays after the last drag or wheel line
const THUMB: f32 = 14.0;
const THUMB_STAYS: Duration = Duration::from_millis(700);

// how long an asked level stands for the device's own
const ASKED_FOR: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slider {
    Speaker,
    Brightness,
    Microphone,

    // an app playing or recording sound
    Stream(Node),
}

impl Slider {
    // the Module without which nothing reads or sets its device
    fn module(self) -> &'static str {
        match self {
            Slider::Speaker | Slider::Microphone | Slider::Stream(_) => "audio",
            Slider::Brightness => "brightness",
        }
    }

    // 0 to 100, as the device last reported it; past 100 for an app amplified
    pub fn percent(self) -> u8 {
        match self {
            Slider::Speaker => Audio::read().volume(),
            Slider::Brightness => Brightness::read().percent(),
            Slider::Microphone => Audio::read().microphone_volume(),
            Slider::Stream(node) => Mixer::read()
                .streams
                .iter()
                .find(|stream| stream.node == node)
                .map_or(0, |stream| stream.level.volume),
        }
    }

    // its bar `width` wide and `fraction` filled, pressed or dragged along to set it
    pub fn bar(self, width: f32, fraction: f32, tone: Color) -> Rectangle {
        self.bar_ringed(width, fraction, tone, false)
    }

    /*
     * the same, with a thumb at the end of the fill while the ring is on it or it was just set by
     * a drag or the wheel, so the handle shows when it is being used and the track is quiet when
     * it is not
     */
    pub fn bar_ringed(self, width: f32, fraction: f32, tone: Color, ring: bool) -> Rectangle {
        let held = ASKED
            .locked()
            .as_ref()
            .is_some_and(|asked| asked.slider == self && asked.at.elapsed() < THUMB_STAYS);

        // the thumb goes a moment after the last move, so one more frame draws it away
        if held {
            request_frame();
        }

        let track: Box<dyn Widget> = if ring || held {
            let at = (width * fraction.clamp(0.0, 1.0) - THUMB / 2.0)
                .clamp(0.0, (width - THUMB).max(0.0));

            Box::new(
                Stack::new(children![
                    Rectangle::new()
                        .width(width)
                        .height(TARGET)
                        .align_child(Start, Center)
                        .child(bar(width, fraction, tone)),
                    Rectangle::new()
                        .width(THUMB)
                        .height(THUMB)
                        .radius(THUMB / 2.0)
                        .fill(theme::island().on_surface)
                        .border(1.5, theme::island().surface)
                        .translate(at, (TARGET - THUMB) / 2.0),
                ])
                .width(width)
                .height(TARGET),
            )
        } else {
            Box::new(bar(width, fraction, tone))
        };

        Rectangle::new()
            .width(width)
            .height(TARGET)
            .align_child(Start, Center)
            .cursor(Cursor::Pointer)
            .semantics(|| {
                let name = match self {
                    Slider::Speaker => "volume",
                    Slider::Brightness => "brightness",
                    Slider::Microphone => "microphone",
                    Slider::Stream(_) => "stream",
                };

                Semantics::new(Role::Slider, name).level(fraction)
            })
            .on_drag(move |point| {
                crate::view::claim();
                self.set((point.x / width * 100.0).round().clamp(0.0, 100.0) as u8);
            })
            .child(track)
    }

    // down is lower
    pub fn wheel(self, lines: f32) {
        if !modules::on(self.module()) {
            return;
        }

        let mut asked = ASKED.locked();
        let now = Instant::now();

        let (from, carry) = match asked.as_ref() {
            Some(asked) if asked.slider == self && now.duration_since(asked.at) < ASKED_FOR => {
                (asked.percent, asked.carry)
            }
            _ => (self.percent(), 0.0),
        };

        let steps = carry - lines * WHEEL;
        let whole = steps.trunc();
        let percent = (f32::from(from) + whole).clamp(0.0, 100.0) as u8;

        *asked = Some(Asked {
            slider: self,
            percent,
            at: now,
            carry: steps - whole,
        });
        drop(asked);

        if percent != from {
            self.send(percent);
        }
    }

    fn set(self, percent: u8) {
        if !modules::on(self.module()) {
            return;
        }

        *ASKED.locked() = Some(Asked {
            slider: self,
            percent,
            at: Instant::now(),
            carry: 0.0,
        });

        self.send(percent);
    }

    fn send(self, percent: u8) {
        match self {
            Slider::Speaker => Audio::set_volume(percent),
            Slider::Brightness => Brightness::set(percent),
            Slider::Microphone => Audio::set_microphone_volume(percent),
            Slider::Stream(node) => audio::ask_volume(node, percent),
        }
    }
}

/*
 * the level last asked for and when, since a device reports it only after a moment: a wheel spun
 * faster than that steps on from what it asked, not from the old level. And the part of a line
 * too small to move a percent, which a touchpad scrolls in, kept for the next one
 */
struct Asked {
    slider: Slider,
    percent: u8,
    at: Instant,
    carry: f32,
}

static ASKED: Mutex<Option<Asked>> = Mutex::new(None);
