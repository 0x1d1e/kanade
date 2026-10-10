//! The screen's backlight, 0 to 100 (ADR 0027 step 1). Polled, because the kernel doesn't announce
//! a change to the backlight file; half a second keeps brightness keys feeling live. Set through
//! logind, since the file belongs to root.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use kanade_runtime::service::Service;
use kanade_runtime::worker;

use crate::bus::{Argument, Bus};

const BACKLIGHTS: &str = "/sys/class/backlight";

const LOGIND: &str = "org.freedesktop.login1";
const SESSION_PATH: &str = "/org/freedesktop/login1/session/auto";
const SESSION: &str = "org.freedesktop.login1.Session";

#[derive(Default)]
pub struct Brightness {
    // none on a desktop monitor without a backlight
    path: Option<PathBuf>,

    // 0 to 100
    percent: u8,
}

impl Service for Brightness {
    fn new() -> Self {
        let mut brightness = Self {
            path: find(),
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
            return false;
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
        self.path.is_some()
    }

    pub fn percent(&self) -> u8 {
        self.percent
    }

    // logind writes the backlight for whoever sits at the machine, with no password
    pub fn set(percent: u8) {
        worker::run(move || write_level(percent));
    }
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
