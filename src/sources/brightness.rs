//! The screen's backlight, 0 to 100 (ADR 0027 step 1). Polled, because the kernel doesn't announce
//! a change to the backlight file; half a second keeps brightness keys feeling live. Set through
//! logind, since the file belongs to root.
//!
//! Without a backlight, or with `brightness.backend = "ddcci"` (the accepted values are `auto`,
//! `backlight` and `ddcci`; anything else is read as `auto`), the level is that of the external
//! monitor on the focused output, over DDC/CI (ADR 0082). With no external monitor focused there,
//! the one chosen before stands, its index clamped to the monitors now present. A monitor's level is
//! read once, on the `ddc` thread, and kept: the poll never touches the bus, and a change made on
//! the monitor's own buttons is not seen.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use kanade_runtime::service::Service;
use kanade_runtime::worker;

use crate::bus::{Argument, Bus};
use crate::config::{self, Backend};
use crate::sources::ddc::{self, Monitor, Reading};
use crate::sources::niri::Niri;

const BACKLIGHTS: &str = "/sys/class/backlight";

const LOGIND: &str = "org.freedesktop.login1";
const SESSION_PATH: &str = "/org/freedesktop/login1/session/auto";
const SESSION: &str = "org.freedesktop.login1.Session";

// the external monitors are looked for again this many polls apart, for one plugged in later
const RESCAN: u32 = 20;

#[derive(Default)]
pub struct Brightness {
    // none on a desktop monitor without a backlight
    path: Option<PathBuf>,

    // the external monitors, when DDC/CI is the backend, and the one on the focused output
    screens: Vec<Screen>,
    current: usize,
    polls: u32,

    // 0 to 100
    percent: u8,
}

struct Screen {
    monitor: Monitor,

    // none until the ddc thread read it
    reading: Option<Reading>,
    asked: bool,
}

impl Service for Brightness {
    fn new() -> Self {
        let backend = config::get().brightness_backend;
        let path = match backend {
            Backend::Ddc => None,
            Backend::Auto | Backend::Backlight => find(),
        };
        let mut brightness = Self {
            screens: match (&path, backend) {
                (None, Backend::Auto | Backend::Ddc) => screens(ddc::monitors(), &[]),
                _ => Vec::new(),
            },
            path,
            ..Self::default()
        };

        brightness.update();

        brightness
    }

    fn interval() -> Duration {
        Duration::from_millis(500)
    }

    fn update(&mut self) -> bool {
        let Some(path) = &self.path else {
            return self.update_screens();
        };

        let current = read(path, "brightness");
        let highest = read(path, "max_brightness");

        if highest == 0 {
            return false;
        }

        let before = self.percent;

        self.percent = percent_of(current, highest);

        self.percent != before
    }
}

impl Brightness {
    pub fn present(&self) -> bool {
        self.path.is_some() || self.screen().is_some_and(|screen| screen.reading.is_some())
    }

    // the level is a file the kernel keeps, not a monitor's remembered one
    pub fn backlight(&self) -> bool {
        self.path.is_some()
    }

    fn screen(&self) -> Option<&Screen> {
        self.screens.get(self.current)
    }

    // the monitor on the focused output, else the one before; asks the ddc thread for its level once
    fn update_screens(&mut self) -> bool {
        let before = (self.current, self.percent);

        self.polls += 1;

        if self.polls.is_multiple_of(RESCAN) {
            let known = std::mem::take(&mut self.screens);

            self.screens = screens(ddc::monitors(), &known);
        }

        if let Some(focused) = Niri::read().seen.focused_output.as_deref()
            && let Some(at) = self
                .screens
                .iter()
                .position(|screen| screen.monitor.connector == focused)
        {
            self.current = at;
        }

        self.current = self.current.min(self.screens.len().saturating_sub(1));

        if let Some(screen) = self.screens.get_mut(self.current)
            && screen.reading.is_none()
            && !screen.asked
        {
            let bus = screen.monitor.bus;

            screen.asked = true;
            ddc::ask(bus, move |reading| {
                let mut brightness = Brightness::write();

                brightness.learn(bus, reading);
            });
        }

        self.percent = self
            .screen()
            .and_then(|screen| screen.reading)
            .map_or(0, ddc::percent_of);

        before != (self.current, self.percent)
    }

    // the ddc thread's answer; a failed one is asked again when the monitors are looked for again
    fn learn(&mut self, bus: u32, reading: Option<Reading>) {
        if let Some(screen) = self
            .screens
            .iter_mut()
            .find(|screen| screen.monitor.bus == bus)
        {
            screen.reading = reading;
        }

        self.update_screens();
    }

    pub fn percent(&self) -> u8 {
        self.percent
    }

    // logind writes the backlight for whoever sits at the machine, with no password
    pub fn set(percent: u8) {
        let mut write = Self::write();
        let brightness = &mut *write;

        // a monitor's level is remembered at once, and written by the ddc thread, newest only
        if brightness.path.is_none() {
            if let Some(screen) = brightness.screens.get_mut(brightness.current)
                && let Some(reading) = &mut screen.reading
            {
                reading.current = ddc::value_of(percent, reading.max);
                ddc::set(screen.monitor.bus, percent, reading.max);
            }

            brightness.update_screens();
            return;
        }

        write.quiet();
        drop(write);

        worker::run(move || write_level(percent));
    }
}

// the monitors found, with what is already known of those that were there
fn screens(monitors: Vec<Monitor>, known: &[Screen]) -> Vec<Screen> {
    monitors
        .into_iter()
        .map(|monitor| {
            let before = known.iter().find(|screen| screen.monitor == monitor);

            Screen {
                reading: before.and_then(|screen| screen.reading),
                asked: false,
                monitor,
            }
        })
        .collect()
}

/// What `kanade brightness` asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Set(u8),
    Up,
    Down,
    Status,
}

const STEP: u8 = 5;

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        match arguments {
            ["set", percent] => percent
                .parse()
                .ok()
                .filter(|percent| (1..=100).contains(percent))
                .map(Request::Set),
            ["up"] => Some(Request::Up),
            ["down"] => Some(Request::Down),
            ["status"] => Some(Request::Status),
            _ => None,
        }
    }
}

// `kanade brightness`: the level of the backlight, or of the monitor on the focused output
pub fn request(request: Request) -> Result<String, String> {
    let (present, now) = {
        let brightness = Brightness::read();

        (brightness.present(), brightness.percent())
    };

    if !present {
        return Err(String::from("there is no brightness to set"));
    }

    let want = match request {
        Request::Status => return Ok(format!("{now}%")),
        Request::Set(percent) => percent,
        Request::Up => now.saturating_add(STEP).min(100),
        Request::Down => now.saturating_sub(STEP).max(1),
    };

    Brightness::set(want);

    Ok(format!("{want}%"))
}

fn write_level(percent: u8) {
    let Some(path) = Brightness::read().path.clone() else {
        return;
    };

    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };

    let highest = read(&path, "max_brightness");

    // never 0, a black screen is hard to undo without seeing it
    let percent = u64::from(percent.clamp(1, 100));

    let level = level_of(percent, highest) as u32;

    let arguments = [
        Argument::from("backlight"),
        Argument::from(name),
        Argument::from(level),
    ];

    Bus::system().call(LOGIND, SESSION_PATH, SESSION, "SetBrightness", &arguments);

    let mut brightness = Brightness::write();

    if !brightness.update() {
        brightness.quiet();
    }
}

/*
 * `current` of `highest` as a percent, rounded as `write_level` rounds the other way, so a percent
 * set reads back as itself, not one lower, whatever the backlight's range
 */
fn level_of(percent: u64, highest: u64) -> u64 {
    // and never level 0, on a backlight of few steps too
    ((highest * percent + 50) / 100).max(1)
}

pub fn percent_of(current: u64, highest: u64) -> u8 {
    ((current * 100 + highest / 2) / highest).min(100) as u8
}

// the first backlight the kernel lists, a laptop usually has exactly one
fn find() -> Option<PathBuf> {
    let mut entries = fs::read_dir(BACKLIGHTS).ok()?.flatten();

    let first = entries.next()?;

    Some(first.path())
}

fn read(folder: &Path, name: &str) -> u64 {
    let text = fs::read_to_string(folder.join(name)).unwrap_or_default();

    text.trim().parse().unwrap_or(0)
}
