//! A level a Surface sets by a press, a drag along its bar, or the wheel: the speaker's volume on
//! Media and Controls, and the screen's brightness on Controls.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use amane::{Audio, Brightness, Center, Color, Cursor, Rectangle, Service, Start};

use crate::theme::space::TARGET;
use crate::view::bar;

// percent per wheel line
const WHEEL: f32 = 5.0;

// how long an asked level stands for the device's own
const ASKED_FOR: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slider {
    Speaker,
    Brightness,
}

impl Slider {
    // 0 to 100, as the device last reported it
    pub fn percent(self) -> u8 {
        match self {
            Slider::Speaker => Audio::read().volume(),
            Slider::Brightness => Brightness::read().percent(),
        }
    }

    // its bar `width` wide and `fraction` filled, pressed or dragged along to set it
    pub fn bar(self, width: f32, fraction: f32, tone: Color) -> Rectangle {
        Rectangle::new()
            .width(width)
            .height(TARGET)
            .align_child(Start, Center)
            .cursor(Cursor::Pointer)
            .on_drag(move |point| {
                crate::view::claim();
                self.set((point.x / width * 100.0).round().clamp(0.0, 100.0) as u8);
            })
            .child(bar(width, fraction, tone))
    }

    // down is lower
    pub fn wheel(self, lines: f32) {
        let mut asked = ASKED.lock().unwrap_or_else(PoisonError::into_inner);
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
        *ASKED.lock().unwrap_or_else(PoisonError::into_inner) = Some(Asked {
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
