//! The config (docs/design.md Config): TOML, read at start in layers, each over the
//! ones before it: the defaults, then every `*.toml` in `$XDG_CONFIG_HOME/kanade/` (else
//! `~/.config/kanade/`) in alphabetical order, then `$XDG_STATE_HOME/kanade/settings.toml` (else
//! `~/.local/state/kanade/settings.toml`), the layer the Settings window owns (`crate::settings`),
//! which `kanade module` writes through too.
//! Tables merge key by key; any other value, a list too, replaces the one below. Kanade writes only
//! the settings file, never the config directory.
//!
//! A key the registry marks per output may also be set in `[output."<name>"]`, for the monitor of
//! that name only. An output's value wins over the global one from any layer, as the more
//! specific; output values layer like global ones. `config::on` gives the config in effect on one
//! output, resolved once per config, so a view never builds one while drawing.
//!
//! A file may name the layout it is written in with `schema_version`; without one it is the v0.1
//! layout, version 1. An older file is migrated in memory before it applies.
//!
//! Every key is optional, and each key is registered by the one Module that owns it, as a
//! `Setting`: its kind, default, help and whether it takes a restart are defined there once, and
//! reading, checking, `kanade config defaults` and the wiki's Configuration defaults follow from it. A file that
//! is not TOML is skipped whole; a key that is unknown or a value out of range is skipped alone,
//! keeping what the layers below gave it. Either says so on stderr with file and line, so a typo
//! never stops the shell. A reload while running (`crate::reload`) is stricter: any problem keeps
//! the config in effect whole.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::Duration;

use kanade_runtime::{MutexExt, RwLockExt, service::Service};
use serde::Deserialize;
use toml::Value;
use toml::de::{DeTable, DeValue, ValueDeserializer};

use crate::clock::Hours;
use crate::gesture;
use crate::hooks::{self, Commands};
use crate::idle::schedule::Stages;
use crate::island::geometry::{self, Shape};
use crate::island::motion::{Mode, REDUCED_FADE};
use crate::island::service::Timings;
use crate::look::{
    Along, Anchor, Bounce, DockSize, Edge, Entrance, Finish, LockBackdrop, LyricsSource, Magnify,
    Merge, Placement, ShellMode, Tone, UiDescribe, Visualizer,
};
use crate::modules;
use crate::sources::accessibility;
use crate::sources::notifications::rules::Rule;
use crate::sources::weather::Units;
use crate::tiles::{self, Placed};

// a level change shows for 1000-1400 ms
const OSD_TIME: Duration = Duration::from_millis(1200);

// a timing outside this keeps its default: a spring needs some time, and a minute is no glance
pub const SHORTEST: u64 = 1;
pub const LONGEST: u64 = 60_000;
pub const ANIMATION_SPEED: Range = Range {
    least: 25,
    most: 400,
};

// `appearance.scale`, whole percent: 0.8 to 2.0 times each output's own scale
pub const UI_SCALE: Range = Range {
    least: 80,
    most: 200,
};

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub island: Timings,

    // how long the OSD and a workspace switch on the island show
    pub osd: Duration,

    // legacy `theme.palette`, accepted for old configs but no longer used
    pub palette: Option<String>,

    // how the time reads at Rest
    pub clock: Hours,

    // the Modules turned off, each once
    pub off: Vec<&'static str>,

    // `windows.apps`: an app id to the `.desktop` file id its windows belong to (ADR 0014)
    pub apps: BTreeMap<String, String>,

    // `gestures`: gesture name to the Kanade verb it runs (ADR 0106)
    pub gestures: BTreeMap<String, String>,

    // `dock.pinned`: the `.desktop` file ids the Dock keeps, in its order
    pub pinned: Vec<String>,

    // `wallpaper.directory`: where the Launcher finds wallpapers, else ~/Pictures/Wallpapers
    pub wallpapers: Option<String>,

    // `wallpaper.transition` and `wallpaper.transition_duration`: how awww moves to a new wallpaper
    pub transition: Transition,

    // a wallpaper picked in the shell is set on the monitor it was picked on, or on all of them
    pub wallpaper_per_monitor: bool,

    // `calendar.paths`: the iCalendar files and directories the calendar reads, else the default
    // directory (ADR 0015)
    pub calendars: Vec<String>,

    // `calendar.reminder`: minutes before an event without alarms of its own that it is reminded
    // of, 0 for none (ADR 0043)
    pub reminder: u32,

    // `calendar.subscriptions`: the https:// addresses of public iCalendar feeds, `webcal://` read
    // as `https://` (ADR 0062). One may hold a secret: never logged, nor shown in `status`
    pub subscriptions: Vec<String>,

    // `calendar.subscription_interval`: minutes between fetches of each feed
    pub subscription_interval: u32,

    // `weather.location`: where the weather is for; none fetches nothing (ADR 0017)
    pub location: Option<Location>,

    // `weather.place`: the location's name, as the Weather Surface titles it
    pub place: Option<String>,

    // `weather.units`: how the Weather Surface reads temperatures and speeds
    pub units: Units,

    // `brightness.backend`: what sets the screen's brightness (ADR 0082)
    pub brightness_backend: Backend,

    // `appearance.*`: what the bodies are made of, and how they move
    pub appearance: Appearance,

    // `animation.speed`: a global multiplier for animation durations
    pub animation_speed: u32,

    // `island.*`: where the Island sits, and how large it grows
    pub island_place: Placement,
    pub largest: Largest,

    // `dock.*`: where the Dock sits, and how its icons grow under the pointer
    pub dock_place: Placement,
    pub magnify: Magnify,
    pub dock_size: DockSize,
    pub merge: Merge,

    // `shell.*`: the combined shell's placement, or the separate Island and Dock
    pub shell: ShellConfig,

    // `rest.*`: what an idle Island shows
    pub rest: Rest,

    // `media.visualizer`
    pub visualizer: Visualizer,

    // `media.lyrics`
    pub lyrics: LyricsSource,

    // `ui.describe`
    pub ui_describe: UiDescribe,

    // `lock.backdrop`
    pub lock_backdrop: LockBackdrop,

    // `lock.pam_service`, none being the default service (`lock::PAM`)
    pub lock_pam: Option<String>,

    // `lock.grace`, whole seconds; 0 is none
    pub lock_grace: u32,

    // `privacy.indicators`
    pub privacy_indicators: bool,

    // `island.wheel`
    pub island_wheel: bool,

    // `island.device_events`
    pub device_events: bool,

    // `launcher.actions`: the apps' Desktop Actions are rows of the Launcher
    pub launcher_actions: bool,

    // `launcher.commands`: Kanade's own verbs, like Toggle Caffeine, are rows of the Launcher (ADR 0112)
    pub launcher_commands: bool,

    // `settings.health_notice`: the one-time first-start Peek of checks that need attention (ADR 0088)
    pub health_notice: bool,

    // `idle.*`: after how long idle each stage fires (ADR 0047)
    pub idle: Stages,

    // `banners.entrance`
    pub entrance: Entrance,

    // `notifications.sound`: a notification arriving plays a sound
    pub notification_sound: bool,

    // `notifications.rules`: what a notification does, by the first rule matching it
    pub notification_rules: Vec<Rule>,

    // `osd.layout`: a keyboard layout switch shows as an OSD
    pub osd_layout: bool,
    // `live.stale`: how long a live Activity lasts without a word from its program (ADR 0042)
    pub live_stale: Duration,
    // `battery.*`: when a draining battery shows, and what a critical one does
    pub battery: Battery,

    // `night-light.*`: when and how warm the screen turns (ADR 0046)
    pub night_light: NightLight,

    // `focus.*`: what Focus turns on
    pub focus: Focus,

    // `dictate.*`: where a dictation's text goes (ADR 0111)
    pub dictate: Dictate,

    // `controls.tiles`: which switches the Controls Surface shows, in order and size (ADR 0067)
    pub tiles: Vec<Placed>,
    // `timer.pomodoro.*`: what `kanade timer pomodoro` runs when not told
    pub pomodoro: Pomodoro,

    // `hooks.*`: the commands run on an event (ADR 0055)
    pub hooks: Commands,

    // `output."<name>"`: each output's overrides of the keys marked per output, by output name
    pub outputs: BTreeMap<String, Output>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Appearance {
    // Settings' and apps' tone; the Island, Dock and Banners follow `finish` whatever it is
    pub tone: Tone,

    // what the Island, Dock and Banners are made of
    pub finish: Finish,

    // share the tone with supported desktop, GTK, Qt and Zen appearance adapters
    pub sync: bool,

    // the family every label is set in, else Inter, else the system's sans-serif
    pub font: Option<String>,

    // follow the desktop's reduced motion and higher contrast (ADR 0080)
    pub accessibility: bool,

    // Liquid Glass bends what lies under its edge, with the shell's own drawing taken out of the
    // screen copy (experimental, ADR 0100)
    pub refraction: bool,

    // the UI scale in percent, on top of each output's own scale (ADR 0091)
    pub scale: u32,
}

impl Appearance {
    // the multiplier the runtime scales the whole UI by
    pub fn ui_scale(&self) -> f32 {
        self.scale.clamp(UI_SCALE.least, UI_SCALE.most) as f32 / 100.0
    }
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            tone: Tone::default(),
            finish: Finish::default(),
            sync: false,
            font: None,
            accessibility: true,
            refraction: false,
            scale: 100,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellConfig {
    pub mode: ShellMode,
    pub anchor: Anchor,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            mode: ShellMode::Separate,
            anchor: Anchor::default(),
        }
    }
}

// `island.width` and `island.height`: the largest body, which the Surfaces with a list grow to
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Largest {
    pub width: u32,
    pub height: u32,
}

impl Largest {
    pub const WIDTH: Range = Range {
        least: 520,
        most: 800,
    };
    pub const HEIGHT: Range = Range {
        least: 330,
        most: 600,
    };

    pub fn shape(self) -> Shape {
        geometry::largest(self.width as f32, self.height as f32)
    }
}

impl Default for Largest {
    fn default() -> Self {
        Largest {
            width: geometry::DEFAULT_LARGEST.width as u32,
            height: geometry::DEFAULT_LARGEST.height as u32,
        }
    }
}

// an inclusive numeric range used by bounded settings
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub least: u32,
    pub most: u32,
}

// what an idle Island shows, and what its Peek shows
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rest {
    // the time
    pub clock: bool,

    // the battery's percent after the time
    pub battery: bool,

    // the Peek of the time: the date always, these if set
    pub peek_battery: bool,
    pub weather: bool,
    pub agenda: bool,
}

impl Default for Rest {
    fn default() -> Self {
        Rest {
            clock: true,
            battery: false,
            peek_battery: true,
            weather: true,
            agenda: true,
        }
    }
}

impl Rest {
    // nothing to show at Rest: the Island then hides as `island.autohide` does
    pub fn bare(self, battery: bool) -> bool {
        !self.clock && !(self.battery && battery)
    }
}

impl Config {
    pub fn island_placement(&self) -> Placement {
        let mut place = self.island_place;
        if self.shell.mode == ShellMode::Combined {
            place.anchor = self.shell.anchor;
        }
        place
    }

    pub fn dock_placement(&self) -> Placement {
        let mut place = self.dock_place;
        if self.shell.mode == ShellMode::Combined {
            place.anchor = self.shell.anchor;
        }
        place
    }

    pub fn animation_duration(&self, duration: Duration) -> Duration {
        scale_animation(duration, self.animation_speed)
    }

    // only transitions scale; the pointer's hover and leave delays keep their meaning
    pub fn animation_timings(&self) -> Timings {
        let mut timings = self.island;
        timings.expand = self.animation_duration(timings.expand);
        timings.surface_change = self.animation_duration(timings.surface_change);
        timings.collapse = self.animation_duration(timings.collapse);
        timings.reduced_fade = self.animation_duration(REDUCED_FADE);
        timings
    }

    pub fn off(&self, module: &str) -> bool {
        self.off.contains(&module)
    }

    // what Rest and its Peek show on each of these outputs, the global setting while none is known
    pub fn rests<'a>(&self, outputs: impl IntoIterator<Item = &'a str>) -> Vec<Rest> {
        let mut rests: Vec<Rest> = outputs
            .into_iter()
            .map(|output| self.on(output).rest)
            .collect();

        if rests.is_empty() {
            rests.push(self.rest);
        }

        rests
    }

    // the config of one output: the global one with that output's overrides over it
    pub fn on(&self, output: &str) -> Cow<'_, Config> {
        let Some(over) = self.outputs.get(output) else {
            return Cow::Borrowed(self);
        };

        let mut config = self.clone();
        for setting in settings().filter(|setting| over.keys.contains(&setting.key)) {
            setting.kind.copy(&over.values, &mut config);
        }

        Cow::Owned(config)
    }
}

// a whole number in a range, in a unit the docs and Settings name
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Count {
    pub range: Range,

    // after the number, like `min`, and what a value must be, like `whole minutes`
    pub unit: &'static str,
    pub expected: &'static str,
}

// a pomodoro's focus, break and long break in minutes, and the focus rounds before a long break
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pomodoro {
    pub work: u32,
    pub short: u32,
    pub long: u32,
    pub rounds: u32,
}

impl Pomodoro {
    const MINUTES: Count = Count {
        range: Range {
            least: 1,
            most: 180,
        },
        unit: "min",
        expected: "whole minutes",
    };
    pub const ROUNDS: Count = Count {
        range: Range { least: 1, most: 12 },
        unit: "rounds",
        expected: "whole rounds",
    };
}

impl Default for Pomodoro {
    fn default() -> Self {
        Pomodoro {
            work: 25,
            short: 5,
            long: 15,
            rounds: 4,
        }
    }
}

/*
 * how awww moves to a new wallpaper: one of its transitions, those a Settings row has room for,
 * and how long it takes, which awww's own "simple" fade ignores
 */
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transition {
    pub kind: &'static str,
    pub duration: Duration,
}

impl Transition {
    pub const KINDS: &[&str] = &["simple", "fade", "wipe", "grow", "outer", "random"];

    // those that grow or shrink a circle, from where the wallpaper was chosen; awww's "random" may
    // pick one, and ignores the point when it picks another
    pub fn circle(self) -> bool {
        matches!(self.kind, "grow" | "outer" | "random")
    }
}

impl Default for Transition {
    fn default() -> Self {
        Transition {
            kind: "simple",
            duration: Duration::from_millis(1000),
        }
    }
}

// one output's overrides: the keys it sets, and their values in a config of their own
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Output {
    keys: Vec<&'static str>,
    values: Box<Config>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            island: Timings::default(),
            osd: OSD_TIME,
            palette: None,
            clock: Hours::default(),
            // idle locking is hypridle's until this Module is turned on (ADR 0047); dictation sends
            // audio to Google, so it is the user's choice (ADR 0111)
            off: vec!["dictation", "idle"],
            apps: BTreeMap::new(),
            gestures: BTreeMap::new(),
            pinned: Vec::new(),
            wallpapers: None,
            transition: Transition::default(),
            wallpaper_per_monitor: true,
            calendars: Vec::new(),
            reminder: 0,
            subscriptions: Vec::new(),
            subscription_interval: 60,
            location: None,
            place: None,
            units: Units::default(),
            brightness_backend: Backend::default(),
            appearance: Appearance::default(),
            animation_speed: 100,
            island_place: Placement {
                anchor: Anchor::default(),
                autohide: false,
                reserve: false,
            },
            largest: Largest::default(),
            dock_place: Placement {
                anchor: Anchor::BOTTOM,
                autohide: false,
                reserve: true,
            },
            magnify: Magnify::default(),
            dock_size: DockSize::default(),
            merge: Merge::default(),
            shell: ShellConfig::default(),
            rest: Rest::default(),
            visualizer: Visualizer::default(),
            lyrics: LyricsSource::default(),
            ui_describe: UiDescribe::default(),
            lock_backdrop: LockBackdrop::default(),
            lock_pam: None,
            lock_grace: 0,
            privacy_indicators: true,
            island_wheel: true,
            device_events: true,
            launcher_actions: true,
            launcher_commands: true,
            health_notice: true,
            idle: Stages {
                dim: Some(Duration::from_secs(270)),
                lock: Some(Duration::from_secs(300)),
                screen_off: Some(Duration::from_secs(330)),
                suspend: None,
            },
            entrance: Entrance::default(),
            notification_sound: false,
            notification_rules: Vec::new(),
            osd_layout: true,
            live_stale: Duration::from_secs(30 * 60),
            battery: Battery::default(),
            night_light: NightLight::default(),
            focus: Focus::default(),
            dictate: Dictate::default(),
            tiles: tiles::DEFAULT.to_vec(),
            pomodoro: Pomodoro::default(),
            hooks: Commands::default(),
            outputs: BTreeMap::new(),
        }
    }
}

// `battery.*`, in percent of a full battery
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    // at or below, a draining battery shows
    pub low: u32,

    // at or below, it preempts and `critical_action` counts down; never above `low`
    pub critical: u32,

    // what a critical battery asks of logind after a countdown, none for nothing
    pub critical_action: CriticalAction,
}

impl Battery {
    pub const LOW: Range = Range { least: 5, most: 50 };
    pub const CRITICAL: Range = Range { least: 2, most: 30 };

    // `critical` as it counts: one above `low` is `low`, so a battery is never critical yet not low
    pub fn critical(self) -> u32 {
        self.critical.min(self.low)
    }
}

impl Default for Battery {
    fn default() -> Self {
        Battery {
            low: 20,
            critical: 10,
            critical_action: CriticalAction::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CriticalAction {
    #[default]
    None,
    Suspend,
    Hibernate,
    PowerOff,
}

impl CriticalAction {
    pub const NAMES: &[&str] = &["none", "suspend", "hibernate", "poweroff"];

    pub fn name(self) -> &'static str {
        match self {
            CriticalAction::None => "none",
            CriticalAction::Suspend => "suspend",
            CriticalAction::Hibernate => "hibernate",
            CriticalAction::PowerOff => "poweroff",
        }
    }

    pub fn named(name: &str) -> CriticalAction {
        match name {
            "suspend" => CriticalAction::Suspend,
            "hibernate" => CriticalAction::Hibernate,
            "poweroff" => CriticalAction::PowerOff,
            _ => CriticalAction::None,
        }
    }
}

// `focus.*`: the pieces `kanade focus on` turns on, each restored by `off`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Focus {
    pub dnd: bool,
    pub caffeine: bool,
    pub night_light: bool,

    // `focus.auto` has `cast`: a screen cast turns Focus on, and off again when it ends (ADR 0078)
    pub cast: bool,
}

impl Default for Focus {
    fn default() -> Self {
        Focus {
            dnd: true,
            caffeine: true,
            night_light: false,
            cast: false,
        }
    }
}

// `brightness.backend`: the backlight, an external monitor over DDC/CI, or the first there is
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    #[default]
    Auto,
    Backlight,
    Ddc,
}

impl Backend {
    pub const NAMES: &[&str] = &["auto", "backlight", "ddcci"];

    pub fn name(self) -> &'static str {
        match self {
            Backend::Auto => "auto",
            Backend::Backlight => "backlight",
            Backend::Ddc => "ddcci",
        }
    }

    pub fn named(name: &str) -> Backend {
        match name {
            "backlight" => Backend::Backlight,
            "ddcci" => Backend::Ddc,
            _ => Backend::Auto,
        }
    }
}

// `dictate.*`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Dictate {
    pub insert: Insertion,

    // app ids whose windows only ever get the text on the clipboard
    pub never_in: Vec<String>,
}

// how a dictation's text reaches the window in front
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Insertion {
    // on the clipboard, for the user to paste
    #[default]
    Copy,

    // on the clipboard, then the paste chord (Ctrl+V) sent to the focused window
    Paste,

    // typed into the focused window, key by key
    Type,
}

impl Insertion {
    pub const NAMES: &[&str] = &["copy", "paste", "type"];

    pub fn name(self) -> &'static str {
        match self {
            Insertion::Copy => "copy",
            Insertion::Paste => "paste",
            Insertion::Type => "type",
        }
    }

    pub fn named(name: &str) -> Insertion {
        match name {
            "paste" => Insertion::Paste,
            "type" => Insertion::Type,
            _ => Insertion::Copy,
        }
    }
}

// `night-light.*`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NightLight {
    pub mode: NightMode,

    // how warm the screen turns, in kelvin
    pub temperature: u32,

    // when it warms and cools again, both set or the location's sun is the schedule
    pub sunset: Option<DayTime>,
    pub sunrise: Option<DayTime>,
}

impl NightLight {
    pub const TEMPERATURE: Range = Range {
        least: 1000,
        most: 6500,
    };
}

impl Default for NightLight {
    fn default() -> Self {
        NightLight {
            mode: NightMode::Off,
            temperature: 4000,
            sunset: None,
            sunrise: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NightMode {
    #[default]
    Off,

    // warm all day
    On,

    // warm from sunset to sunrise
    Schedule,
}

impl NightMode {
    pub const NAMES: &[&str] = &["off", "on", "schedule"];

    pub fn name(self) -> &'static str {
        match self {
            NightMode::Off => "off",
            NightMode::On => "on",
            NightMode::Schedule => "schedule",
        }
    }

    pub fn named(name: &str) -> NightMode {
        match name {
            "on" => NightMode::On,
            "schedule" => NightMode::Schedule,
            _ => NightMode::Off,
        }
    }
}

// a time of day, `HH:MM` on a 24 hour clock
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DayTime {
    pub hour: u8,
    pub minute: u8,
}

impl DayTime {
    const EXPECTED: &str = "expected a time as \"HH:MM\", 00:00 to 23:59";

    pub fn parse(text: &str) -> Result<DayTime, String> {
        let number = |text: &str| {
            (text.len() == 2 && text.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| text.parse::<u8>().ok())
                .flatten()
        };

        text.trim()
            .split_once(':')
            .and_then(|(hour, minute)| Some((number(hour)?, number(minute)?)))
            .filter(|&(hour, minute)| hour < 24 && minute < 60)
            .map(|(hour, minute)| DayTime { hour, minute })
            .ok_or_else(|| String::from(DayTime::EXPECTED))
    }
}

impl std::fmt::Display for DayTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02}:{:02}", self.hour, self.minute)
    }
}

// a point on the Earth, in degrees, north and east positive
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Location {
    pub latitude: f64,
    pub longitude: f64,
}

impl Location {
    const EXPECTED: &str = "expected [latitude, longitude] in degrees";

    // `[latitude, longitude]`, each in range
    fn read(value: &Value) -> Result<Location, String> {
        let degrees = |value: &Value| {
            value
                .as_float()
                .or(value.as_integer().map(|int| int as f64))
        };

        match value.as_array().map(Vec::as_slice) {
            Some([latitude, longitude]) => match (degrees(latitude), degrees(longitude)) {
                (Some(latitude), Some(longitude)) => Location::new(latitude, longitude),
                _ => Err(String::from(Location::EXPECTED)),
            },
            _ => Err(String::from(Location::EXPECTED)),
        }
    }

    // `latitude, longitude` as typed into Settings, as a TOML value
    pub fn parse(text: &str) -> Result<Value, String> {
        let degrees = |text: &str| text.trim().parse::<f64>().ok();

        let (latitude, longitude) = text
            .split_once(',')
            .and_then(|(latitude, longitude)| Some((degrees(latitude)?, degrees(longitude)?)))
            .ok_or("expected latitude, longitude in degrees")?;

        Location::new(latitude, longitude).map(|location| location.value())
    }

    fn new(latitude: f64, longitude: f64) -> Result<Location, String> {
        if !(-90.0..=90.0).contains(&latitude) {
            return Err(format!("latitude {latitude} is outside -90 to 90"));
        }
        if !(-180.0..=180.0).contains(&longitude) {
            return Err(format!("longitude {longitude} is outside -180 to 180"));
        }

        Ok(Location {
            latitude,
            longitude,
        })
    }

    fn value(self) -> Value {
        Value::Array(vec![
            Value::Float(self.latitude),
            Value::Float(self.longitude),
        ])
    }
}

/*
 * one key a Module owns, which the registry lists with the Module; others may read it too. It is the
 * key's whole definition: reading a file, checking a value, the default, the docs (`defaults`),
 * a reload's pending restart and the Settings app all follow from it
 */
pub struct Setting {
    // dotted: `timings.hover` is `hover` in `[timings]`
    pub key: &'static str,

    // what Settings calls it
    pub label: &'static str,

    // what it does, a line for the docs and `kanade config defaults`
    pub help: &'static str,

    // a shorter line for Settings when `help` is written for the docs; none shows `help`
    pub hint: Option<&'static str>,

    // what it takes, which says how it is checked and shown, and where in `Config` it goes
    pub kind: Kind,

    // a TOML value worth writing, for a key whose default sets nothing
    pub example: Option<&'static str>,

    // read only at start: a reload keeps the running value and reports the new one pending
    pub restart: bool,

    // an output may override it in `[output."<name>"]`; never a table or a key that takes a restart
    pub per_output: bool,
}

// where in `Config` a key's value goes, as the one type its Kind reads
pub struct Field<T> {
    pub get: fn(&Config) -> T,
    pub set: fn(&mut Config, T),
}

impl<T> Field<T> {
    fn copy(&self, from: &Config, to: &mut Config) {
        (self.set)(to, (self.get)(from));
    }
}

// the strings a Choice takes, and the label the settings window shows for those that need one
pub struct Choices {
    pub names: &'static [&'static str],
    labels: &'static [(&'static str, &'static str)],
}

impl Choices {
    pub const fn named(names: &'static [&'static str]) -> Self {
        Self { names, labels: &[] }
    }

    pub const fn labeled(
        names: &'static [&'static str],
        labels: &'static [(&'static str, &'static str)],
    ) -> Self {
        Self { names, labels }
    }

    pub fn label(&self, name: &str) -> Option<&'static str> {
        self.labels
            .iter()
            .find_map(|&(named, label)| (named == name).then_some(label))
    }
}

// what a key takes; each checks a value the one way, with one wording
pub enum Kind {
    // true or false
    Switch(Field<bool>),

    // whole milliseconds, SHORTEST to LONGEST
    Millis(Field<Duration>),

    // whole logical pixels in a range
    Pixels(Range, Field<u32>),

    // whole percent in a range
    Percent(Range, Field<u32>),

    // whole seconds in a range, 0 being never
    Seconds(Range, Field<u32>),
    // whole minutes in a range
    Minutes(Range, Field<Duration>),
    // a color temperature, whole kelvin in a range
    Kelvin(Range, Field<u32>),
    // a whole number of something else in a range
    Count(Count, Field<u32>),

    // one of these strings
    Choice(Choices, Field<&'static str>),

    // a file or directory, a leading `~/` the home directory; unset is none
    Path(Field<Option<String>>),

    // a line of text, not empty; unset is none
    Text(Field<Option<String>>),

    // `[latitude, longitude]` in degrees; unset is none
    Location(Field<Option<Location>>),

    // a time of day as "HH:MM"; unset is none
    DayTime(Field<Option<DayTime>>),

    // a list of `.desktop` file ids, the `.desktop` optional, each kept once; a layer replaces the
    // list below
    DesktopIds(Field<Vec<String>>),

    // a list of files or directories, a leading `~/` the home directory, each kept once; a layer
    // replaces the list below
    Paths(Field<Vec<String>>),

    // a list of words as given, like app ids, none empty, each kept once; a layer replaces the list below
    Words(Field<Vec<String>>),

    // a list of public calendar feeds, `https://` or `webcal://` addresses (`feed`), each kept once;
    // a layer replaces the list below. An error never repeats an address, which may hold a secret
    Urls(Field<Vec<String>>),

    // a table of app id = `.desktop` file id, merged over the layers below by app id
    AppIds(Field<BTreeMap<String, String>>),

    // a table of gesture name = a Kanade verb (`gesture::check`), merged over the layers below by name
    Gestures(Field<BTreeMap<String, String>>),

    // a table of Module name = true or false, merged by name; the Field holds the ones off
    Modules(Field<Vec<&'static str>>),

    // a list of notification rules, each a table; a layer replaces the list below
    Rules(Field<Vec<Rule>>),

    // a command as a list of strings, the program first, run without a shell (`hooks`); an empty
    // list is none, and a layer replaces the list below
    Argv(Field<Vec<String>>),

    // the Controls tiles, `name` or `name:size` each, at least one; a layer replaces the list below
    Tiles(Field<Vec<Placed>>),

    // a list of these names, each kept once, in the order given; a layer replaces the list below
    Names(&'static [&'static str], Field<Vec<&'static str>>),
}

impl Kind {
    // stores a value, or says why it will not, keeping what was there; `home` expands a `~/`
    fn set(&self, config: &mut Config, value: &Value, home: Option<&str>) -> Result<(), String> {
        match self {
            Kind::Switch(field) => {
                (field.set)(config, value.as_bool().ok_or("expected true or false")?);
            }
            Kind::Millis(field) => (field.set)(config, millis(value)?),
            Kind::Pixels(range, field) => (field.set)(config, pixels(value, *range)?),
            Kind::Percent(range, field) => (field.set)(config, percent(value, *range)?),
            Kind::Seconds(range, field) => (field.set)(config, seconds(value, *range)?),
            Kind::Minutes(range, field) => (field.set)(config, minutes(value, *range)?),
            Kind::Kelvin(range, field) => (field.set)(config, kelvin(value, *range)?),
            Kind::Count(count, field) => (field.set)(config, counted(value, *count)?),
            Kind::Choice(options, field) => {
                let expected = format!(
                    "expected {}",
                    options
                        .names
                        .iter()
                        .map(|option| format!("\"{option}\""))
                        .collect::<Vec<_>>()
                        .join(" or ")
                );
                let found = value.as_str().ok_or_else(|| expected.clone())?;
                let option = options
                    .names
                    .iter()
                    .find(|&&option| option == found)
                    .ok_or_else(|| format!("{expected}, found \"{found}\""))?;

                (field.set)(config, option);
            }
            Kind::Path(field) => {
                let path = value.as_str().ok_or("expected a \"path\"")?;
                (field.set)(config, Some(expand(path, home)));
            }
            Kind::Text(field) => {
                let text = value
                    .as_str()
                    .filter(|text| !text.trim().is_empty() && !text.contains('\n'))
                    .ok_or("expected a line of text")?;
                (field.set)(config, Some(text.trim().to_owned()));
            }
            Kind::Location(field) => (field.set)(config, Some(Location::read(value)?)),
            Kind::DayTime(field) => {
                let text = value.as_str().ok_or(DayTime::EXPECTED)?;
                (field.set)(config, Some(DayTime::parse(text)?));
            }
            Kind::DesktopIds(field) => {
                let list = value
                    .as_array()
                    .ok_or("expected a list of \"desktop file id\"s")?;
                let mut ids: Vec<String> = Vec::new();

                for id in list {
                    let id = id
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .map(desktop)
                        .ok_or_else(|| format!("expected a \"desktop file id\", found {id}"))?;

                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }

                (field.set)(config, ids);
            }
            Kind::Words(field) => {
                let list = value.as_array().ok_or("expected a list of \"app id\"s")?;
                let mut words: Vec<String> = Vec::new();

                for word in list {
                    let word = word
                        .as_str()
                        .map(str::trim)
                        .filter(|word| !word.is_empty())
                        .ok_or_else(|| format!("expected an \"app id\", found {word}"))?;

                    if !words.iter().any(|kept| kept == word) {
                        words.push(word.to_owned());
                    }
                }

                (field.set)(config, words);
            }
            Kind::Paths(field) => {
                let list = value.as_array().ok_or("expected a list of \"path\"s")?;
                let mut paths: Vec<String> = Vec::new();

                for path in list {
                    let path = path
                        .as_str()
                        .filter(|path| !path.is_empty())
                        .map(|path| expand(path, home))
                        .ok_or_else(|| format!("expected a \"path\", found {path}"))?;

                    if !paths.contains(&path) {
                        paths.push(path);
                    }
                }

                (field.set)(config, paths);
            }
            Kind::Urls(field) => {
                let list = value.as_array().ok_or(FEEDS_EXPECTED)?;
                let mut urls: Vec<String> = Vec::new();

                for (at, url) in list.iter().enumerate() {
                    let url = url
                        .as_str()
                        .ok_or(FEEDS_EXPECTED)
                        .and_then(feed)
                        .map_err(|why| format!("entry {}: {why}", at + 1))?;

                    if !urls.contains(&url) {
                        urls.push(url);
                    }
                }

                if urls.len() > FEEDS {
                    return Err(format!("at most {FEEDS} addresses"));
                }

                (field.set)(config, urls);
            }
            Kind::AppIds(field) => {
                let table = value
                    .as_table()
                    .ok_or("expected a table of app id = \"desktop file id\"")?;
                let mut apps = (field.get)(config);

                // checked whole first, so a bad one keeps all of its file's below
                let mut given = BTreeMap::new();
                for (app_id, id) in table {
                    let id = id
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .ok_or_else(|| format!("{app_id}: expected a \"desktop file id\""))?;

                    given.insert(app_id.clone(), desktop(id));
                }

                apps.extend(given);
                (field.set)(config, apps);
            }
            Kind::Gestures(field) => {
                let table = value
                    .as_table()
                    .ok_or("expected a table of name = \"kanade verb\"")?;
                let mut gestures = (field.get)(config);

                // checked whole first, so a bad one keeps all of its file's below
                let mut given = BTreeMap::new();
                for (name, verb) in table {
                    if !gesture::valid_name(name) {
                        return Err(format!("{name}: a name is {}", gesture::NAME_RULE));
                    }

                    let verb = verb
                        .as_str()
                        .ok_or_else(|| format!("{name}: expected a \"kanade verb\""))?;
                    let words = gesture::check(verb).map_err(|why| format!("{name}: {why}"))?;

                    given.insert(name.clone(), words.join(" "));
                }

                gestures.extend(given);
                (field.set)(config, gestures);
            }
            Kind::Modules(field) => {
                let table = value.as_table().ok_or("expected a table")?;
                let mut off = (field.get)(config);

                for (name, value) in table {
                    turn(&mut off, name, value)?;
                }

                (field.set)(config, off);
            }
            Kind::Rules(field) => (field.set)(config, Rule::read_all(value)?),
            Kind::Tiles(field) => {
                let list = value
                    .as_array()
                    .ok_or("expected a list of \"tile\"s or \"tile:size\"s")?
                    .iter()
                    .map(|tile| {
                        tile.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| format!("expected a \"tile\", found {tile}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                (field.set)(config, tiles::parse(&list)?);
            }
            Kind::Names(known, field) => {
                let expected = || format!("expected a list of {}", quoted(known));
                let mut names: Vec<&'static str> = Vec::new();

                for name in value.as_array().ok_or_else(expected)? {
                    let name = name.as_str().ok_or_else(expected)?;
                    let name = known
                        .iter()
                        .find(|&&known| known == name)
                        .ok_or_else(|| format!("unknown name {name:?}, {}", expected()))?;

                    if !names.contains(name) {
                        names.push(name);
                    }
                }

                (field.set)(config, names);
            }
            Kind::Argv(field) => {
                let mut argv = hooks::argv(value)?;

                // no shell is run, so only the program's own `~/` is expanded
                if let Some(program) = argv.first_mut() {
                    *program = expand(program, home);
                }

                (field.set)(config, argv);
            }
        }

        Ok(())
    }

    // the value as TOML, none for one that sets nothing, like an empty list
    pub fn value(&self, config: &Config) -> Option<Value> {
        let value = match self {
            Kind::Switch(field) => Value::Boolean((field.get)(config)),
            Kind::Millis(field) => {
                Value::Integer(i64::try_from((field.get)(config).as_millis()).ok()?)
            }
            Kind::Pixels(_, field)
            | Kind::Percent(_, field)
            | Kind::Seconds(_, field)
            | Kind::Kelvin(_, field)
            | Kind::Count(_, field) => Value::Integer(i64::from((field.get)(config))),
            Kind::Minutes(_, field) => {
                Value::Integer(i64::try_from((field.get)(config).as_secs() / 60).ok()?)
            }
            Kind::Choice(_, field) => Value::String((field.get)(config).to_owned()),
            Kind::Path(field) | Kind::Text(field) => Value::String((field.get)(config)?),
            Kind::Location(field) => (field.get)(config)?.value(),
            Kind::DayTime(field) => Value::String((field.get)(config)?.to_string()),
            Kind::DesktopIds(field)
            | Kind::Paths(field)
            | Kind::Urls(field)
            | Kind::Words(field)
            | Kind::Argv(field) => {
                Value::Array((field.get)(config).into_iter().map(Value::String).collect())
            }
            Kind::AppIds(field) | Kind::Gestures(field) => Value::Table(
                (field.get)(config)
                    .into_iter()
                    .map(|(app_id, id)| (app_id, Value::String(id)))
                    .collect(),
            ),
            Kind::Modules(field) => Value::Table(
                (field.get)(config)
                    .into_iter()
                    .map(|name| (name.to_owned(), Value::Boolean(false)))
                    .collect(),
            ),
            Kind::Rules(field) => {
                Value::Array((field.get)(config).iter().map(Rule::value).collect())
            }
            Kind::Names(_, field) => Value::Array(
                (field.get)(config)
                    .into_iter()
                    .map(|name| Value::String(name.to_owned()))
                    .collect(),
            ),
            Kind::Tiles(field) => Value::Array(
                (field.get)(config)
                    .iter()
                    .map(|placed| Value::String(placed.to_string()))
                    .collect(),
            ),
        };

        match &value {
            Value::Array(list) if list.is_empty() => None,
            Value::Table(table) if table.is_empty() => None,
            _ => Some(value),
        }
    }

    // `from`'s value into `to`
    fn copy(&self, from: &Config, to: &mut Config) {
        match self {
            Kind::Switch(field) => field.copy(from, to),
            Kind::Millis(field) => field.copy(from, to),
            Kind::Pixels(_, field)
            | Kind::Percent(_, field)
            | Kind::Seconds(_, field)
            | Kind::Kelvin(_, field)
            | Kind::Count(_, field) => field.copy(from, to),
            Kind::Minutes(_, field) => field.copy(from, to),
            Kind::Choice(_, field) => field.copy(from, to),
            Kind::Path(field) | Kind::Text(field) => field.copy(from, to),
            Kind::Location(field) => field.copy(from, to),
            Kind::DayTime(field) => field.copy(from, to),
            Kind::DesktopIds(field)
            | Kind::Paths(field)
            | Kind::Urls(field)
            | Kind::Words(field) => field.copy(from, to),
            Kind::AppIds(field) | Kind::Gestures(field) => field.copy(from, to),
            Kind::Modules(field) => field.copy(from, to),
            Kind::Rules(field) => field.copy(from, to),
            Kind::Argv(field) => field.copy(from, to),
            Kind::Names(_, field) => field.copy(from, to),
            Kind::Tiles(field) => field.copy(from, to),
        }
    }

    // a table of its own keys, which a file writes as `[key]`
    pub fn table(&self) -> bool {
        matches!(self, Kind::AppIds(_) | Kind::Gestures(_) | Kind::Modules(_))
    }
}

pub const ISLAND: &[Setting] = &[
    Setting {
        key: "reduced_motion",
        label: "Reduce motion",
        help: "the island snaps to its new shape and fades its content; the fade is 80 ms at 100% animation speed; KANADE_REDUCED_MOTION=1 or 0 overrides it",
        hint: Some("Snap to new shapes instead of morphing"),
        kind: Kind::Switch(Field {
            get: |config| config.island.motion == Mode::Reduced,
            set: |config, reduced| {
                config.island.motion = if reduced { Mode::Reduced } else { Mode::Spring };
            },
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "clock",
        label: "Clock format",
        help: "the time an idle island shows: \"24h\" (14:05) or \"12h\" (2:05 PM)",
        hint: Some("How an idle Island writes the time"),
        kind: Kind::Choice(
            Choices::labeled(&["24h", "12h"], &[("24h", "14:05"), ("12h", "2:05 PM")]),
            Field {
                get: |config| match config.clock {
                    Hours::TwentyFour => "24h",
                    Hours::Twelve => "12h",
                },
                set: |config, hours| {
                    config.clock = match hours {
                        "12h" => Hours::Twelve,
                        _ => Hours::TwentyFour,
                    };
                },
            },
        ),
        example: None,
        restart: false,
        per_output: true,
    },
    Setting {
        key: "timings.hover",
        label: "Peek delay",
        help: "pointer resting on a Compact island before it peeks",
        hint: Some("How long the pointer rests before a Peek"),
        kind: Kind::Millis(Field {
            get: |config| config.island.hover,
            set: |config, hover| config.island.hover = hover,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "timings.expand",
        label: "Expansion",
        help: "morph to a larger form",
        hint: Some("How long growing to a larger form takes"),
        kind: Kind::Millis(Field {
            get: |config| config.island.expand,
            set: |config, expand| config.island.expand = expand,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "timings.surface_change",
        label: "Content change",
        help: "one Surface replacing another, and a new track dissolving in",
        hint: Some("How long one Surface takes to replace another"),
        kind: Kind::Millis(Field {
            get: |config| config.island.surface_change,
            set: |config, change| config.island.surface_change = change,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "timings.collapse",
        label: "Collapse",
        help: "morph to a smaller form",
        hint: Some("How long shrinking to a smaller form takes"),
        kind: Kind::Millis(Field {
            get: |config| config.island.collapse,
            set: |config, collapse| config.island.collapse = collapse,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "timings.grace",
        label: "Leave delay",
        help: "pointer out before a Peek or Surface collapses",
        hint: Some("How long the Island waits after the pointer leaves"),
        kind: Kind::Millis(Field {
            get: |config| config.island.grace,
            set: |config, grace| config.island.grace = grace,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    // the island's, since `workspace` reads it as well as `osd`
    Setting {
        key: "timings.osd",
        label: "Feedback duration",
        help: "the OSD, and a workspace switch on the island",
        hint: Some("How long volume, brightness and workspace changes show"),
        kind: Kind::Millis(Field {
            get: |config| config.osd,
            set: |config, osd| config.osd = osd,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "theme.palette",
        label: "Legacy palette image",
        help: "accepted from older config files; no longer changes the shell theme",
        hint: None,
        kind: Kind::Path(Field {
            get: |config| config.palette.clone(),
            set: |config, palette| config.palette = palette,
        }),
        example: Some("\"~/Pictures/wallpaper.jpg\""),
        restart: false,
        per_output: false,
    },
    Setting {
        key: "shell.mode",
        label: "Arrangement",
        help: "separate keeps the Island and Dock in their own positions; combined puts both in one shell, with apps on either side",
        hint: Some("Keep the Island and Dock apart, or join them in one shell"),
        kind: Kind::Choice(
            Choices::named(ShellMode::NAMES),
            Field {
                get: |config| config.shell.mode.name(),
                set: |config, name| config.shell.mode = ShellMode::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "shell.edge",
        label: "Combined edge",
        help: "which top or bottom edge the combined shell hangs from; stored while Separate is active",
        hint: Some("Top or bottom edge the joined shell hangs from"),
        kind: Kind::Choice(
            Choices::named(&["top", "bottom"]),
            Field {
                get: |config| config.shell.anchor.edge.name(),
                set: |config, name| config.shell.anchor.edge = Edge::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "shell.align",
        label: "Combined position",
        help: "where along that edge the combined shell sits; stored while Separate is active",
        hint: Some("Where along that edge the joined shell sits"),
        kind: Kind::Choice(
            Choices::named(Along::NAMES),
            Field {
                get: |config| config.shell.anchor.along.name(),
                set: |config, name| config.shell.anchor.along = Along::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "appearance.tone",
        label: "Color tone",
        help: "selects light or dark for Settings and apps; the Island, Dock and Banners follow the finish. Matugen derives both palettes from the wallpaper awww shows at startup or Kanade sets",
        hint: Some("Light or dark for Settings and apps"),
        kind: Kind::Choice(
            Choices::named(Tone::NAMES),
            Field {
                get: |config| config.appearance.tone.name(),
                set: |config, name| config.appearance.tone = Tone::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "appearance.finish",
        label: "Finish",
        help: "what the Island, Dock and Banners are made of, tinted toward the wallpaper's hue in each: \"tinted-solid\", dark with light text, \"paper\", light with dark ink, \"liquid-glass\", clear glass over the screen, its edge reflecting what lies around, or \"blur\", the tinted solid translucent over what the compositor blurs behind it, paper while `appearance.tone` is \"light\" (needs niri 26.04)",
        hint: Some("What the Island, Dock and Banners are made of"),
        kind: Kind::Choice(
            Choices::named(Finish::NAMES),
            Field {
                get: |config| config.appearance.finish.name(),
                set: |config, name| config.appearance.finish = Finish::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "appearance.refraction",
        label: "Refraction (experimental)",
        help: "Liquid Glass only: bend what lies under its edge, taking the shell's own drawing out of the copy of the screen, where that can be told apart, else the edge reflects what lies around as before; costs a copy of each frame of the window",
        hint: Some("Experimental: Liquid Glass bends what is under its edge"),
        kind: Kind::Switch(Field {
            get: |config| config.appearance.refraction,
            set: |config, refraction| config.appearance.refraction = refraction,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "appearance.sync",
        label: "Sync desktop appearance",
        help: "sets the desktop color preference and paired GTK theme; writes the wallpaper colors for GTK, qt5ct/qt6ct, KDE apps, niri, kitty and Pywalfox. Kvantum gives way to Darkly or Fusion; GTK and Qt apps need restart. Other apps are unchanged",
        hint: Some("Match GTK, Qt, KDE, niri and kitty to the wallpaper"),
        kind: Kind::Switch(Field {
            get: |config| config.appearance.sync,
            set: |config, sync| config.appearance.sync = sync,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "appearance.accessibility",
        label: "Follow desktop accessibility",
        help: "follow the desktop's reduced motion and higher contrast preferences (the settings portal): the first turns reduced_motion on, the second makes the Island, Dock and Banners a solid finish, with nothing written to your files; reduced_motion = true or KANADE_REDUCED_MOTION=1 win over a desktop that does not ask, KANADE_REDUCED_MOTION=0 over one that does; turning it on takes a restart",
        hint: Some("Reduce motion and raise contrast when the desktop asks"),
        kind: Kind::Switch(Field {
            get: |config| config.appearance.accessibility,
            set: |config, follow| config.appearance.accessibility = follow,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "animation.speed",
        label: "Animation speed",
        help: "100% is unchanged; higher values are faster. Interaction delays and countdowns do not change",
        hint: Some("How fast the shell animates; 100% is the default"),
        kind: Kind::Percent(
            ANIMATION_SPEED,
            Field {
                get: |config| config.animation_speed,
                set: |config, speed| config.animation_speed = speed,
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "appearance.motion",
        label: "Spring character",
        help: "how the Island and Dock settle: \"bouncy\" like macOS, \"smooth\" with no overshoot, \"snappy\", or \"playful\"",
        hint: Some("How the Island and Dock settle into place"),
        kind: Kind::Choice(
            Choices::named(Bounce::NAMES),
            Field {
                get: |config| Bounce::of(config.island.damping).name(),
                set: |config, name| config.island.damping = Bounce::named(name).damping(),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "appearance.font",
        label: "Font",
        help: "installed family for shell labels; unset uses Inter, then sans-serif. A family without a static SemiBold face renders regular weights",
        hint: Some("Typeface for the shell's labels"),
        kind: Kind::Text(Field {
            get: |config| config.appearance.font.clone(),
            set: |config, font| config.appearance.font = font,
        }),
        example: Some("\"Inter\""),

        // the runtime takes the font as the app starts
        restart: true,
        per_output: false,
    },
    Setting {
        key: "appearance.scale",
        label: "Interface size",
        help: "the size of everything Kanade draws, on top of each output's own scale: 100% is unchanged, 125% a quarter larger. Windows already open keep their size; the lock screen is not scaled",
        hint: Some("Make the whole shell smaller or larger; 100% is the default"),
        kind: Kind::Percent(
            UI_SCALE,
            Field {
                get: |config| config.appearance.scale,
                set: |config, scale| config.appearance.scale = scale,
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "island.edge",
        label: "Edge",
        help: "the edge of the screen the Island hangs from",
        hint: None,
        kind: Kind::Choice(
            Choices::named(Edge::NAMES),
            Field {
                get: |config| config.island_place.anchor.edge.name(),
                set: |config, name| config.island_place.anchor.edge = Edge::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "island.align",
        label: "Side",
        help: "where along its edge the Island sits",
        hint: None,
        kind: Kind::Choice(
            Choices::named(Along::NAMES),
            Field {
                get: |config| config.island_place.anchor.along.name(),
                set: |config, name| config.island_place.anchor.along = Along::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "island.width",
        label: "Largest width",
        help: "how wide the Surfaces with a list grow, as their content asks",
        hint: Some("How wide a list Surface can grow"),
        kind: Kind::Pixels(
            Largest::WIDTH,
            Field {
                get: |config| config.largest.width,
                set: |config, px| config.largest.width = px,
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "island.height",
        label: "Largest height",
        help: "how tall the Surfaces with a list grow, as their content asks",
        hint: Some("How tall a list Surface can grow"),
        kind: Kind::Pixels(
            Largest::HEIGHT,
            Field {
                get: |config| config.largest.height,
                set: |config, px| config.largest.height = px,
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "island.autohide",
        label: "Hide at Rest",
        help: "an idle Island slides past its edge, back as the pointer touches the edge under it or anything shows",
        hint: Some("Slide away while idle, back at the screen edge"),
        kind: Kind::Switch(Field {
            get: |config| config.island_place.autohide,
            set: |config, on| config.island_place.autohide = on,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "island.reserve",
        label: "Keep windows clear",
        help: "windows keep off the Island's strip of the screen, unless it autohides; off, it floats over them",
        hint: Some("Keep windows out from under the Island"),
        kind: Kind::Switch(Field {
            get: |config| config.island_place.reserve,
            set: |config, on| config.island_place.reserve = on,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "island.wheel",
        label: "Wheel sets the volume",
        help: "the wheel over the Island sets the volume while it shows Media or the volume; over anything else it does nothing",
        hint: Some("Scroll over the Island to set the volume"),
        kind: Kind::Switch(Field {
            get: |config| config.island_wheel,
            set: |config, on| config.island_wheel = on,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "island.device_events",
        label: "Device events",
        help: "a Bluetooth device connecting or going, the charger plugged in or out, a new sound output and a Wi-Fi network joined show on the Island for a moment, each while its Module is on",
        hint: Some("Show a device connecting or a charger plugged in"),
        kind: Kind::Switch(Field {
            get: |config| config.device_events,
            set: |config, on| config.device_events = on,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "rest.clock",
        label: "Clock at Rest",
        help: "an idle Island shows the time; with it and the battery off, the Island hides as it does with autohide",
        hint: Some("Show the time while idle"),
        kind: Kind::Switch(Field {
            get: |config| config.rest.clock,
            set: |config, on| config.rest.clock = on,
        }),
        example: None,
        restart: false,
        per_output: true,
    },
    Setting {
        key: "rest.battery",
        label: "Battery at Rest",
        help: "an idle Island shows the battery's percent after the time",
        hint: Some("Show the battery after the time"),
        kind: Kind::Switch(Field {
            get: |config| config.rest.battery,
            set: |config, on| config.rest.battery = on,
        }),
        example: None,
        restart: false,
        per_output: true,
    },
    Setting {
        key: "rest.peek.battery",
        label: "Battery in the clock's Peek",
        help: "the time's Peek shows the battery's percent beside the time, where an autohidden Island is seen",
        hint: Some("Show the battery in the time's Peek"),
        kind: Kind::Switch(Field {
            get: |config| config.rest.peek_battery,
            set: |config, on| config.rest.peek_battery = on,
        }),
        example: None,
        restart: false,
        per_output: true,
    },
    Setting {
        key: "rest.peek.weather",
        label: "Weather in the clock's Peek",
        help: "the time's Peek shows the weather beside the date",
        hint: Some("Show the weather beside the date"),
        kind: Kind::Switch(Field {
            get: |config| config.rest.weather,
            set: |config, on| config.rest.weather = on,
        }),
        example: None,
        restart: false,
        per_output: true,
    },
    Setting {
        key: "rest.peek.agenda",
        label: "Agenda in the clock's Peek",
        help: "the time's Peek shows the next event beside the date",
        hint: Some("Show the next event beside the date"),
        kind: Kind::Switch(Field {
            get: |config| config.rest.agenda,
            set: |config, on| config.rest.agenda = on,
        }),
        example: None,
        restart: false,
        per_output: true,
    },
    Setting {
        key: "privacy.indicators",
        label: "Capture indicators",
        help: "dots at the Island's trailing end while a microphone, camera or screen cast is in use, and the apps named in Controls, with whether a cast captures a window or a screen; not shown over a fullscreen window",
        hint: Some("Mark a microphone, camera or screen cast in use"),
        kind: Kind::Switch(Field {
            get: |config| config.privacy_indicators,
            set: |config, on| config.privacy_indicators = on,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "banners.entrance",
        label: "Banner entrance",
        help: "how a Banner comes in: \"morph\" out from under the Island, \"drop\" from the edge, or \"fade\"",
        hint: Some("How a Banner arrives"),
        kind: Kind::Choice(
            Choices::named(Entrance::NAMES),
            Field {
                get: |config| config.entrance.name(),
                set: |config, name| config.entrance = Entrance::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    // the island's, since it cannot be turned off
    Setting {
        key: "modules",
        label: "Modules",
        help: "every Module is on unless set to false here; idle and dictation are off unless set to true",
        hint: None,
        kind: Kind::Modules(Field {
            get: |config| config.off.clone(),
            set: |config, off| config.off = off,
        }),
        example: None,

        // the runtime registers windows only at start
        restart: true,
        per_output: false,
    },
];

pub const NOTIFICATIONS: &[Setting] = &[
    Setting {
        key: "notifications.sound",
        label: "Notification sounds",
        help: "a notification arriving plays the sound it names, else a default, through pw-play or canberra-gtk-play; never under Do Not Disturb",
        hint: Some("Play a sound for each notification"),
        kind: Kind::Switch(Field {
            get: |config| config.notification_sound,
            set: |config, on| config.notification_sound = on,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "notifications.rules",
        label: "Notification rules",
        help: "per app: the first rule whose app, desktop_entry, category and summary piece all match, ignoring case, sets banner, history, sound, bypass_dnd, timeout (ms) and urgency; file only",
        hint: None,
        kind: Kind::Rules(Field {
            get: |config| config.notification_rules.clone(),
            set: |config, rules| config.notification_rules = rules,
        }),
        example: Some(
            "[{ app = \"Slack\", banner = false }, { summary = \"battery\", bypass_dnd = true }]",
        ),
        restart: false,
        per_output: false,
    },
];

pub const OSD: &[Setting] = &[Setting {
    key: "osd.layout",
    label: "Keyboard layout switch",
    help: "a keyboard layout switch shows as an OSD, with two or more layouts",
    hint: Some("Show a keyboard layout switch"),
    kind: Kind::Switch(Field {
        get: |config| config.osd_layout,
        set: |config, on| config.osd_layout = on,
    }),
    example: None,
    restart: false,
    per_output: false,
}];

pub const WINDOWS: &[Setting] = &[Setting {
    key: "windows.apps",
    label: "App matching",
    help: "app id = the .desktop file it belongs to, where Kanade's guess is wrong",
    hint: Some("Name the .desktop file of an app Kanade mismatches"),
    kind: Kind::AppIds(Field {
        get: |config| config.apps.clone(),
        set: |config, apps| config.apps = apps,
    }),
    example: Some("{ jetbrains-idea = \"intellij-idea-ultimate-edition\" }"),
    restart: false,
    per_output: false,
}];

pub const GESTURES: &[Setting] = &[Setting {
    key: "gestures",
    label: "Gestures",
    help: "gesture name = the Kanade verb it runs, like \"island collapse\"; record a gesture with `kanade gesture record <name>`; never a program, which a hook or a verb of yours runs; file only",
    hint: None,
    kind: Kind::Gestures(Field {
        get: |config| config.gestures.clone(),
        set: |config, gestures| config.gestures = gestures,
    }),
    example: Some("{ circle = \"launcher open\", check = \"island collapse\" }"),
    restart: false,
    per_output: false,
}];

pub const DOCK: &[Setting] = &[
    Setting {
        key: "dock.pinned",
        label: "Pinned apps",
        help: ".desktop file ids, in the Dock's order",
        hint: None,
        kind: Kind::DesktopIds(Field {
            get: |config| config.pinned.clone(),
            set: |config, pinned| config.pinned = pinned,
        }),
        example: Some("[\"firefox\", \"kitty\"]"),
        restart: false,
        per_output: false,
    },
    Setting {
        key: "dock.edge",
        label: "Edge",
        help: "the edge of the screen the Dock sits on; beside the Island, on its edge and side or either centered, it sits further in than the Island; wherever the Island grows over it, as an open Surface, it steps aside and fades back as the Island closes",
        hint: Some("Which screen edge the Dock sits on"),
        kind: Kind::Choice(
            Choices::named(Edge::NAMES),
            Field {
                get: |config| config.dock_place.anchor.edge.name(),
                set: |config, name| config.dock_place.anchor.edge = Edge::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "dock.align",
        label: "Side",
        help: "where along its edge the Dock sits",
        hint: Some("Where along its edge the Dock sits"),
        kind: Kind::Choice(
            Choices::named(Along::NAMES),
            Field {
                get: |config| config.dock_place.anchor.along.name(),
                set: |config, name| config.dock_place.anchor.along = Along::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "dock.autohide",
        label: "Hide until needed",
        help: "the Dock hides until the pointer touches the edge under it; beside the Island it always shows, as the pointer could not reach past the Island; merged with it, it folds into the Island as `merge = \"fold\"` does",
        hint: Some("Hide until the pointer touches the edge"),
        kind: Kind::Switch(Field {
            get: |config| config.dock_place.autohide,
            set: |config, on| config.dock_place.autohide = on,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "dock.reserve",
        label: "Keep windows clear",
        help: "windows keep off the Dock's strip of the screen, unless it autohides; off, it floats over them",
        hint: Some("Keep windows out from under the Dock"),
        kind: Kind::Switch(Field {
            get: |config| config.dock_place.reserve,
            set: |config, on| config.dock_place.reserve = on,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "dock.magnification",
        label: "Magnification",
        help: "how much an icon grows under the pointer, its neighbors less",
        hint: Some("How much an icon grows under the pointer"),
        kind: Kind::Choice(
            Choices::named(Magnify::NAMES),
            Field {
                get: |config| config.magnify.name(),
                set: |config, name| config.magnify = Magnify::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "dock.size",
        label: "Icon size",
        help: "how big the icons are at rest; the gaps between and how far the swell under the pointer reaches grow with them",
        hint: Some("Icon size at rest"),
        kind: Kind::Choice(
            Choices::named(DockSize::NAMES),
            Field {
                get: |config| config.dock_size.name(),
                set: |config, name| config.dock_size = DockSize::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "dock.merge",
        label: "Join the Island",
        help: "how the Dock and the Island join where they share an edge and a side, along a top or bottom edge: crown, the Island rising from the Dock's middle; keystone, the Island the Dock's middle piece; fold, the Dock unfolding out of the Island under the pointer; off, the Dock beside the Island, further in",
        hint: Some("How the Dock joins the Island on a shared edge"),
        kind: Kind::Choice(
            Choices::named(Merge::NAMES),
            Field {
                get: |config| config.merge.name(),
                set: |config, name| config.merge = Merge::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
];

pub const WALLPAPER: &[Setting] = &[
    Setting {
        key: "wallpaper.directory",
        label: "Wallpaper folder",
        help: "the images the Wallpapers Surface shows, else ~/Pictures/Wallpapers",
        hint: Some("Images the Wallpapers Surface shows"),
        kind: Kind::Path(Field {
            get: |config| config.wallpapers.clone(),
            set: |config, wallpapers| config.wallpapers = wallpapers,
        }),
        example: Some("\"~/Pictures/Wallpapers\""),
        restart: false,
        per_output: false,
    },
    Setting {
        key: "wallpaper.per_monitor",
        label: "Wallpaper per monitor",
        help: "on, a wallpaper picked in the shell (the Wallpapers Surface, `kanade wallpaper shuffle`) is set on that monitor alone; off, on every monitor. `kanade wallpaper set --output` names one either way",
        hint: Some("Pick a wallpaper for each monitor, or one for all"),
        kind: Kind::Switch(Field {
            get: |config| config.wallpaper_per_monitor,
            set: |config, on| config.wallpaper_per_monitor = on,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "wallpaper.transition",
        label: "Transition",
        help: "how awww moves to a new wallpaper: \"simple\" fades at its own pace, \"fade\" fades, \"wipe\" sweeps across, \"grow\" opens a circle and \"outer\" closes one, from the tile chosen on the Wallpapers Surface, else the middle, and \"random\" lets awww pick one each time",
        hint: Some("How the wallpaper changes to a new image"),
        kind: Kind::Choice(
            Choices::named(Transition::KINDS),
            Field {
                get: |config| config.transition.kind,
                set: |config, kind| config.transition.kind = kind,
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "wallpaper.transition_duration",
        label: "Transition length",
        help: "how long a transition other than \"simple\" takes",
        hint: Some("How long a transition takes"),
        kind: Kind::Millis(Field {
            get: |config| config.transition.duration,
            set: |config, duration| config.transition.duration = duration,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
];

pub const TIMER: &[Setting] = &[
    Setting {
        key: "timer.pomodoro.work",
        label: "Focus",
        help: "how long a pomodoro's focus lasts, unless `kanade timer pomodoro` says",
        hint: Some("Length of a focus round"),
        kind: Kind::Count(
            Pomodoro::MINUTES,
            Field {
                get: |config| config.pomodoro.work,
                set: |config, minutes| config.pomodoro.work = minutes,
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "timer.pomodoro.break",
        label: "Break",
        help: "how long the break after a focus lasts",
        hint: Some("Break after a focus round"),
        kind: Kind::Count(
            Pomodoro::MINUTES,
            Field {
                get: |config| config.pomodoro.short,
                set: |config, minutes| config.pomodoro.short = minutes,
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "timer.pomodoro.long_break",
        label: "Long break",
        help: "how long the break after the last round's focus lasts",
        hint: Some("Break after the last round"),
        kind: Kind::Count(
            Pomodoro::MINUTES,
            Field {
                get: |config| config.pomodoro.long,
                set: |config, minutes| config.pomodoro.long = minutes,
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "timer.pomodoro.rounds",
        label: "Rounds",
        help: "how many focus rounds come before a long break",
        hint: Some("Focus rounds before the long break"),
        kind: Kind::Count(
            Pomodoro::ROUNDS,
            Field {
                get: |config| config.pomodoro.rounds,
                set: |config, rounds| config.pomodoro.rounds = rounds,
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
];

pub const CALENDAR: &[Setting] = &[Setting {
    key: "calendar.paths",
    label: "Calendar files",
    help: "iCalendar (.ics) files and directories of them, read recursively, else \
           $XDG_DATA_HOME/calendars",
    hint: Some("Calendar files and folders to read"),
    kind: Kind::Paths(Field {
        get: |config| config.calendars.clone(),
        set: |config, calendars| config.calendars = calendars,
    }),
    example: Some("[\"~/.local/share/calendars\", \"~/Documents/holidays.ics\"]"),
    restart: false,
    per_output: false,
}];

// the longest lead `calendar.reminder` takes, a week
pub const REMINDER: Range = Range {
    least: 0,
    most: 7 * 24 * 60,
};

pub const REMINDERS: &[Setting] = &[Setting {
    key: "calendar.reminder",
    label: "Default reminder",
    help: "minutes before a timed event with no reminders of its own to remind of it, 0 for none",
    hint: Some("Minutes ahead to remind of an event; 0 for none"),
    kind: Kind::Minutes(
        REMINDER,
        Field {
            get: |config| Duration::from_secs(u64::from(config.reminder) * 60),
            set: |config, lead| {
                config.reminder = u32::try_from(lead.as_secs() / 60).unwrap_or(u32::MAX)
            },
        },
    ),
    example: None,
    restart: false,
    per_output: false,
}];

// how many feeds `calendar.subscriptions` takes
pub const FEEDS: usize = 16;

const FEEDS_EXPECTED: &str = "expected a list of https:// or webcal:// addresses";

// how often a feed is fetched: from half an hour to a day
pub const SUBSCRIPTION_INTERVAL: Range = Range {
    least: 30,
    most: 24 * 60,
};

pub const SUBSCRIPTIONS: &[Setting] = &[
    Setting {
        key: "calendar.subscriptions",
        label: "Subscriptions",
        help: "https:// (or webcal://) addresses of public iCalendar feeds, fetched into read-only \
               calendars; an address may hold a secret, so none is logged or shown",
        hint: Some("Add a calendar feed address, then Enter"),
        kind: Kind::Urls(Field {
            get: |config| config.subscriptions.clone(),
            set: |config, urls| config.subscriptions = urls,
        }),
        example: Some("[\"https://example.org/holidays.ics\", \"webcal://example.org/team.ics\"]"),
        restart: false,
        per_output: false,
    },
    Setting {
        key: "calendar.subscription_interval",
        label: "Subscription interval",
        help: "minutes between fetches of each subscription, at least 30",
        hint: Some("Minutes between fetches of a feed"),
        kind: Kind::Minutes(
            SUBSCRIPTION_INTERVAL,
            Field {
                get: |config| Duration::from_secs(u64::from(config.subscription_interval) * 60),
                set: |config, every| {
                    config.subscription_interval =
                        u32::try_from(every.as_secs() / 60).unwrap_or(u32::MAX)
                },
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
];

pub const WEATHER: &[Setting] = &[
    Setting {
        key: "weather.location",
        label: "Location",
        help: "where the weather is for, as [latitude, longitude] in degrees, north and east \
               positive; unset fetches nothing",
        hint: Some("Latitude and longitude the weather is for"),
        kind: Kind::Location(Field {
            get: |config| config.location,
            set: |config, location| config.location = location,
        }),
        example: Some("[52.52, 13.41]"),
        restart: false,
        per_output: false,
    },
    Setting {
        key: "weather.place",
        label: "Place name",
        help: "the location's name, which the Weather Surface shows",
        hint: Some("Name the Weather Surface shows"),
        kind: Kind::Text(Field {
            get: |config| config.place.clone(),
            set: |config, place| config.place = place,
        }),
        example: Some("\"Berlin\""),
        restart: false,
        per_output: false,
    },
    Setting {
        key: "weather.units",
        label: "Units",
        help: "°C and km/h, or °F and mph",
        hint: Some("Metric or imperial"),
        kind: Kind::Choice(
            Choices::labeled(
                &["metric", "imperial"],
                &[
                    ("metric", "\u{00b0}C \u{00b7} km/h"),
                    ("imperial", "\u{00b0}F \u{00b7} mph"),
                ],
            ),
            Field {
                get: |config| match config.units {
                    Units::Metric => "metric",
                    Units::Imperial => "imperial",
                },
                set: |config, units| {
                    config.units = match units {
                        "imperial" => Units::Imperial,
                        _ => Units::Metric,
                    };
                },
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
];

pub const MEDIA: &[Setting] = &[
    Setting {
        key: "media.visualizer",
        label: "Media visualizer",
        help: "what moves beside a playing track",
        hint: Some("What moves beside a playing track"),
        kind: Kind::Choice(
            Choices::named(Visualizer::NAMES),
            Field {
                get: |config| config.visualizer.name(),
                set: |config, name| config.visualizer = Visualizer::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "media.lyrics",
        label: "Synced lyrics",
        help: "the current line of a playing track's synced lyrics on the Media Surface, from a .lrc file beside a local track, or also from LRCLIB, which is told the track's title, artist, album and length",
        hint: Some("Show the current lyric line; lrclib asks the internet"),
        kind: Kind::Choice(
            Choices::named(LyricsSource::NAMES),
            Field {
                get: |config| config.lyrics.name(),
                set: |config, name| config.lyrics = LyricsSource::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
];

pub const UI: &[Setting] = &[Setting {
    key: "ui.describe",
    label: "UI tree",
    help: "what `kanade ui describe` says of the windows, and so what an agent or script can read: \"structure\" is roles, names, states and places with every label and value `<redacted>`; \"content\" adds what notifications, the clipboard, the calendar and lyrics show (never a password), for debugging on this machine; \"off\" refuses `kanade ui` entirely",
    hint: Some("What the UI tree says; content shows notification and clipboard text"),
    kind: Kind::Choice(
        Choices::named(UiDescribe::NAMES),
        Field {
            get: |config| config.ui_describe.name(),
            set: |config, name| config.ui_describe = UiDescribe::named(name),
        },
    ),
    example: None,
    restart: false,
    per_output: false,
}];

pub const LAUNCHER: &[Setting] = &[
    Setting {
        key: "launcher.actions",
        label: "App actions",
        help: "an app's own actions, like a browser's New Private Window, are rows of the Launcher once two letters are typed",
        hint: Some("List an app's own actions in the Launcher"),
        kind: Kind::Switch(Field {
            get: |config| config.launcher_actions,
            set: |config, on| config.launcher_actions = on,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "launcher.commands",
        label: "Commands",
        help: "Kanade's own commands, like Toggle Caffeine or Screenshot Area, are rows of the Launcher once two letters of their name are typed; pressing one runs it",
        hint: Some("List Kanade's own commands in the Launcher"),
        kind: Kind::Switch(Field {
            get: |config| config.launcher_commands,
            set: |config, on| config.launcher_commands = on,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
];

pub const SETTINGS: &[Setting] = &[Setting {
    key: "settings.health_notice",
    label: "Health notice",
    help: "tell once, at the first start with checks that need attention, to open Settings' Health page",
    hint: Some("Tell once when checks need attention"),
    kind: Kind::Switch(Field {
        get: |config| config.health_notice,
        set: |config, on| config.health_notice = on,
    }),
    example: None,
    restart: false,
    per_output: false,
}];

pub const LIVE_STALE: Range = Range {
    least: 1,
    most: 1440,
};

pub const LIVE: &[Setting] = &[Setting {
    key: "live.stale",
    label: "Withdraw after",
    help: "how long a live Activity from `kanade activity` shows without a word from its program",
    hint: Some("How long to keep a live Activity its program stopped updating"),
    kind: Kind::Minutes(
        LIVE_STALE,
        Field {
            get: |config| config.live_stale,
            set: |config, stale| config.live_stale = stale,
        },
    ),
    example: None,
    restart: false,
    per_output: false,
}];

// the longest `lock.grace`, so a lock the idle Module asked for is never open for long (ADR 0066)
pub const LOCK_GRACE: Range = Range { least: 0, most: 30 };

pub const LOCK: &[Setting] = &[
    Setting {
        key: "lock.backdrop",
        label: "Backdrop",
        help: "what the lock screen shows behind the clock: the wallpaper awww shows on that output \"blurred\", sharp as \"wallpaper\", \"dimmed\", or \"solid\"; solid also stands in for a wallpaper that is not png, jpeg, webp, gif or svg",
        hint: Some("What shows behind the lock screen clock"),
        kind: Kind::Choice(
            Choices::named(LockBackdrop::NAMES),
            Field {
                get: |config| config.lock_backdrop.name(),
                set: |config, name| config.lock_backdrop = LockBackdrop::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "lock.pam_service",
        label: "PAM service",
        help: "the PAM service that checks the password, a file of that name in /etc/pam.d; unset uses login. Letters, digits, dots, dashes and underscores. Only the first hidden prompt gets the password; an echoed or later one fails the check, and PAM's own messages show under the field. A name with no file refuses every lock",
        hint: Some("PAM service that checks the lock screen password"),
        kind: Kind::Text(Field {
            get: |config| config.lock_pam.clone(),
            set: |config, service| config.lock_pam = service,
        }),
        example: Some("\"login\""),
        restart: false,
        per_output: false,
    },
    Setting {
        key: "lock.grace",
        label: "Grace",
        help: "seconds after the idle Module locks the session in which any key, click, scroll or pointer movement unlocks it without a password; never for `kanade lock`, a lock before sleep or a lock asked again after a failure; 0 is none",
        hint: Some("Seconds after an idle lock when input unlocks without a password; 0 none"),
        kind: Kind::Seconds(
            LOCK_GRACE,
            Field {
                get: |config| config.lock_grace,
                set: |config, seconds| config.lock_grace = seconds,
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
];

// whole seconds each idle stage waits, up to a day, 0 being never
pub const IDLE_SECONDS: Range = Range {
    least: 0,
    most: 86_400,
};

fn stage(after: Option<Duration>) -> u32 {
    after.map_or(0, |after| {
        u32::try_from(after.as_secs()).unwrap_or(u32::MAX)
    })
}

fn after(seconds: u32) -> Option<Duration> {
    (seconds > 0).then(|| Duration::from_secs(u64::from(seconds)))
}

// what the idle Module does, and when (ADR 0047)
pub const IDLE: &[Setting] = &[
    Setting {
        key: "idle.dim",
        label: "Dim after",
        help: "seconds idle before the screens fade toward dark, until the lock; any input brings them back; 0 is never",
        hint: Some("Idle time before the screens fade toward dark; 0 never"),
        kind: Kind::Seconds(
            IDLE_SECONDS,
            Field {
                get: |config| stage(config.idle.dim),
                set: |config, seconds| config.idle.dim = after(seconds),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "idle.lock",
        label: "Lock after",
        help: "seconds idle before the session locks, as `kanade lock` does; 0 is never",
        hint: Some("Idle time before the session locks; 0 never"),
        kind: Kind::Seconds(
            IDLE_SECONDS,
            Field {
                get: |config| stage(config.idle.lock),
                set: |config, seconds| config.idle.lock = after(seconds),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "idle.screen_off",
        label: "Screens off after",
        help: "seconds idle before niri powers the monitors off, back on at any input; never before the lock holds; 0 is never",
        hint: Some("Idle time before the monitors switch off; 0 never"),
        kind: Kind::Seconds(
            IDLE_SECONDS,
            Field {
                get: |config| stage(config.idle.screen_off),
                set: |config, seconds| config.idle.screen_off = after(seconds),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "idle.suspend",
        label: "Suspend after",
        help: "seconds idle before the machine suspends, through logind; never before the lock holds; 0 is never",
        hint: Some("Idle time before the machine suspends; 0 never"),
        kind: Kind::Seconds(
            IDLE_SECONDS,
            Field {
                get: |config| stage(config.idle.suspend),
                set: |config, seconds| config.idle.suspend = after(seconds),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
];

pub const BATTERY: &[Setting] = &[
    Setting {
        key: "battery.low",
        label: "Low battery",
        help: "at or below, a draining battery shows on the island",
        hint: Some("Show the battery on the Island at or below this"),
        kind: Kind::Percent(
            Battery::LOW,
            Field {
                get: |config| config.battery.low,
                set: |config, low| config.battery.low = low,
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "battery.critical",
        label: "Critical battery",
        help: "at or below, the battery interrupts and battery.critical_action counts down; one above battery.low counts as battery.low",
        hint: Some("Count down to the critical action at or below this"),
        kind: Kind::Percent(
            Battery::CRITICAL,
            Field {
                get: |config| config.battery.critical,
                set: |config, critical| config.battery.critical = critical,
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "battery.critical_action",
        label: "When critical",
        help: "what a draining critical battery does after a 60 s countdown that Cancel or a charger stops: \"none\", \"suspend\", \"hibernate\" or \"poweroff\"",
        hint: Some("What a critical battery does after a 60 s countdown"),
        kind: Kind::Choice(
            Choices::labeled(
                CriticalAction::NAMES,
                &[
                    ("none", "Nothing"),
                    ("suspend", "Sleep"),
                    ("hibernate", "Hibernate"),
                    ("poweroff", "Power off"),
                ],
            ),
            Field {
                get: |config| config.battery.critical_action.name(),
                set: |config, name| config.battery.critical_action = CriticalAction::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
];

pub const BRIGHTNESS: &[Setting] = &[Setting {
    key: "brightness.backend",
    label: "Brightness backend",
    help: "what sets the brightness: \"backlight\", an external monitor over \"ddcci\" (DDC/CI, needs the i2c-dev module and access to /dev/i2c-*), or \"auto\" for the backlight, else a monitor",
    hint: Some("Backlight or an external monitor over DDC/CI"),
    kind: Kind::Choice(
        Choices::labeled(
            Backend::NAMES,
            &[
                ("auto", "Automatic"),
                ("backlight", "Backlight"),
                ("ddcci", "Monitor (DDC/CI)"),
            ],
        ),
        Field {
            get: |config| config.brightness_backend.name(),
            set: |config, name| config.brightness_backend = Backend::named(name),
        },
    ),
    example: None,
    restart: true,
    per_output: false,
}];

pub const DICTATE: &[Setting] = &[
    Setting {
        key: "dictate.insert",
        label: "Insert dictation",
        help: "where a dictation's text goes: \"copy\" puts it on the clipboard, \"paste\" puts it there and sends Ctrl+V to the focused window, \"type\" types it with wtype. Either of the last two falls back to the clipboard, marked sensitive, when the window in front is not the one you started in, is in dictate.never_in, or wtype fails",
        hint: Some("What to do with the text"),
        kind: Kind::Choice(
            Choices::labeled(
                Insertion::NAMES,
                &[("copy", "Copy"), ("paste", "Paste"), ("type", "Type")],
            ),
            Field {
                get: |config| config.dictate.insert.name(),
                set: |config, name| config.dictate.insert = Insertion::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "dictate.never_in",
        label: "Never insert into",
        help: "app ids, as niri names them (`niri msg windows`), whose windows only ever get the clipboard, like a password manager",
        hint: None,
        kind: Kind::Words(Field {
            get: |config| config.dictate.never_in.clone(),
            set: |config, apps| config.dictate.never_in = apps,
        }),
        example: Some("[\"org.keepassxc.KeePassXC\"]"),
        restart: false,
        per_output: false,
    },
];

pub const NIGHT_LIGHT: &[Setting] = &[
    Setting {
        key: "night-light.mode",
        label: "Night light",
        help: "warms the screen through wlsunset: \"off\", \"on\" all day, or on a \"schedule\" from sunset to sunrise",
        hint: Some("Warm the screen at night"),
        kind: Kind::Choice(
            Choices::labeled(
                NightMode::NAMES,
                &[("off", "Off"), ("on", "Always"), ("schedule", "Schedule")],
            ),
            Field {
                get: |config| config.night_light.mode.name(),
                set: |config, name| config.night_light.mode = NightMode::named(name),
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "night-light.temperature",
        label: "Warmth",
        help: "the screen's color temperature while warm; lower is warmer, 6500 is neutral",
        hint: Some("Lower is warmer; 6500 is neutral"),
        kind: Kind::Kelvin(
            NightLight::TEMPERATURE,
            Field {
                get: |config| config.night_light.temperature,
                set: |config, kelvin| config.night_light.temperature = kelvin,
            },
        ),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "night-light.sunset",
        label: "Warm from",
        help: "when the schedule warms the screen, as \"HH:MM\"; with night-light.sunrise it is the schedule, else the sun at weather.location is",
        hint: Some("When the screen turns warm"),
        kind: Kind::DayTime(Field {
            get: |config| config.night_light.sunset,
            set: |config, time| config.night_light.sunset = time,
        }),
        example: Some("\"20:00\""),
        restart: false,
        per_output: false,
    },
    Setting {
        key: "night-light.sunrise",
        label: "Neutral from",
        help: "when the schedule turns the screen neutral again, as \"HH:MM\"",
        hint: Some("When the screen turns neutral again"),
        kind: Kind::DayTime(Field {
            get: |config| config.night_light.sunrise,
            set: |config, time| config.night_light.sunrise = time,
        }),
        example: Some("\"07:00\""),
        restart: false,
        per_output: false,
    },
];

// the names as `"a", "b"`, for what a key expects
fn quoted(names: &[&str]) -> String {
    names
        .iter()
        .map(|name| format!("{name:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

// what `focus.auto` takes
const FOCUS_AUTO: &[&str] = &["cast"];

pub const FOCUS: &[Setting] = &[
    Setting {
        key: "focus.dnd",
        label: "Do Not Disturb",
        help: "Focus turns Do Not Disturb on, and off again if it was off before",
        hint: Some("Focus quiets the Banners"),
        kind: Kind::Switch(Field {
            get: |config| config.focus.dnd,
            set: |config, on| config.focus.dnd = on,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "focus.caffeine",
        label: "Caffeine",
        help: "Focus keeps the session awake, and releases it again if it was not held before",
        hint: Some("Focus keeps the session awake"),
        kind: Kind::Switch(Field {
            get: |config| config.focus.caffeine,
            set: |config, on| config.focus.caffeine = on,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "focus.night_light",
        label: "Night light",
        help: "Focus warms the screen, and turns the night light off again if it was off before",
        hint: Some("Focus warms the screen"),
        kind: Kind::Switch(Field {
            get: |config| config.focus.night_light,
            set: |config, on| config.focus.night_light = on,
        }),
        example: None,
        restart: false,
        per_output: false,
    },
    Setting {
        key: "focus.auto",
        label: "Automatic Focus",
        help: "what turns Focus on by itself, and off again when it ends, only if nobody turned Focus on or off meanwhile: \"cast\" is a screen cast running (needs the privacy Module); empty is never",
        hint: None,
        kind: Kind::Names(
            FOCUS_AUTO,
            Field {
                get: |config| {
                    if config.focus.cast {
                        vec!["cast"]
                    } else {
                        Vec::new()
                    }
                },
                set: |config, names| config.focus.cast = names.contains(&"cast"),
            },
        ),
        example: Some("[\"cast\"]"),
        restart: false,
        per_output: false,
    },
];

pub const CONTROLS: &[Setting] = &[Setting {
    key: "controls.tiles",
    label: "Controls tiles",
    help: "the switches the Controls Surface shows, in order, each \"name\" or \"name:size\" (wifi, bluetooth, caffeine, dnd, night-light, focus, charge-limit, power-profile; half, full or icon, where wifi and bluetooth have no icon); at most three rows, a tile this machine cannot use, like the charge limit without a battery that has one, is left out; the pencil in Controls edits them",
    hint: Some("Their order and size; add one, like focus or wifi:full, then Enter"),
    kind: Kind::Tiles(Field {
        get: |config| config.tiles.clone(),
        set: |config, tiles| config.tiles = tiles,
    }),
    example: None,
    restart: false,
    per_output: false,
}];

// a desktop file id, given with or without its `.desktop`
fn desktop(id: &str) -> String {
    match id.strip_suffix(".desktop") {
        Some(_) => id.to_owned(),
        None => format!("{id}.desktop"),
    }
}

fn millis(value: &Value) -> Result<Duration, String> {
    let ms = value.as_integer().ok_or("expected milliseconds")?;

    u64::try_from(ms)
        .ok()
        .filter(|ms| (SHORTEST..=LONGEST).contains(ms))
        .map(Duration::from_millis)
        .ok_or_else(|| format!("{ms} is outside {SHORTEST}-{LONGEST} ms"))
}

fn pixels(value: &Value, range: Range) -> Result<u32, String> {
    let px = value.as_integer().ok_or("expected whole pixels")?;

    u32::try_from(px)
        .ok()
        .filter(|px| (range.least..=range.most).contains(px))
        .ok_or_else(|| format!("{px} is outside {}-{} px", range.least, range.most))
}

pub fn scale_animation(duration: Duration, speed: u32) -> Duration {
    duration.mul_f64(100.0 / f64::from(speed.clamp(ANIMATION_SPEED.least, ANIMATION_SPEED.most)))
}

fn seconds(value: &Value, range: Range) -> Result<u32, String> {
    let seconds = value.as_integer().ok_or("expected whole seconds")?;

    u32::try_from(seconds)
        .ok()
        .filter(|seconds| (range.least..=range.most).contains(seconds))
        .ok_or_else(|| format!("{seconds} is outside {}-{} s", range.least, range.most))
}

fn minutes(value: &Value, range: Range) -> Result<Duration, String> {
    let minutes = value.as_integer().ok_or("expected whole minutes")?;

    u32::try_from(minutes)
        .ok()
        .filter(|minutes| (range.least..=range.most).contains(minutes))
        .map(|minutes| Duration::from_secs(u64::from(minutes) * 60))
        .ok_or_else(|| format!("{minutes} is outside {}-{} min", range.least, range.most))
}

/*
 * a feed address as the calendar fetches it: `webcal://` is `https://`, `http://` is refused, and
 * nothing else is taken. The reasons never repeat the address, which may hold a secret
 */
pub fn feed(text: &str) -> Result<String, &'static str> {
    const LONGEST: usize = 2048;

    let text = text.trim();
    let scheme = |name: &str| {
        text.get(..name.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(name))
    };

    let rest = if scheme("https://") {
        &text["https://".len()..]
    } else if scheme("webcal://") {
        &text["webcal://".len()..]
    } else if scheme("http://") {
        return Err("http:// is refused, as the feed would be read in the clear; use https://");
    } else {
        return Err("expected an https:// or webcal:// address");
    };

    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();

    if text.len() > LONGEST {
        return Err("the address is over 2048 bytes");
    }
    if host.is_empty() || host.starts_with(':') {
        return Err("the address has no host");
    }

    let url = format!("https://{rest}");
    if url
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte == b' ')
        || url.parse::<ureq::http::Uri>().is_err()
    {
        return Err("not a valid address");
    }

    Ok(url)
}

fn kelvin(value: &Value, range: Range) -> Result<u32, String> {
    let kelvin = value.as_integer().ok_or("expected whole kelvin")?;

    u32::try_from(kelvin)
        .ok()
        .filter(|kelvin| (range.least..=range.most).contains(kelvin))
        .ok_or_else(|| format!("{kelvin} is outside {}-{} K", range.least, range.most))
}

fn percent(value: &Value, range: Range) -> Result<u32, String> {
    let percent = value.as_integer().ok_or("expected whole percent")?;

    u32::try_from(percent)
        .ok()
        .filter(|percent| (range.least..=range.most).contains(percent))
        .ok_or_else(|| format!("{percent} is outside {}-{} %", range.least, range.most))
}

fn counted(value: &Value, count: Count) -> Result<u32, String> {
    let number = value
        .as_integer()
        .ok_or_else(|| format!("expected {}", count.expected))?;
    let Range { least, most } = count.range;

    u32::try_from(number)
        .ok()
        .filter(|number| (least..=most).contains(number))
        .ok_or_else(|| format!("{number} is outside {least}-{most} {}", count.unit))
}

// one `[modules]` entry over the Modules off: a Module in the registry on or off by name
fn turn(off: &mut Vec<&'static str>, name: &str, value: &Value) -> Result<(), String> {
    let module = modules::ALL
        .iter()
        .find(|module| module.name == name)
        .ok_or_else(|| format!("unknown module modules.{name}"))?;

    match value {
        Value::Boolean(false) if module.name == modules::CORE => Err(format!(
            "modules.{name}: the core module cannot be turned off"
        )),
        Value::Boolean(on) => {
            off.retain(|&other| other != module.name);

            if !on {
                off.push(module.name);
            }

            Ok(())
        }
        _ => Err(format!("modules.{name}: expected true or false")),
    }
}

/*
 * the keys a restart would change between the running config and a read one; a key of each
 * Module for `modules`
 */
pub fn pending(running: &Config, read: &Config) -> Vec<String> {
    let mut pending = Vec::new();

    for setting in settings().filter(|setting| setting.restart) {
        let (was, will) = (setting.kind.value(running), setting.kind.value(read));

        match &setting.kind {
            Kind::Modules(_) => pending.extend(
                modules::ALL
                    .iter()
                    .filter(|module| running.off(module.name) != read.off(module.name))
                    .map(|module| format!("modules.{}", module.name)),
            ),
            _ if was != will => pending.push(setting.key.to_owned()),
            _ => {}
        }
    }

    pending
}

// a read config with the running value of each key that needs a restart
pub fn restarted(running: &Config, read: Config) -> Config {
    let mut next = read;

    for setting in settings().filter(|setting| setting.restart) {
        setting.kind.copy(running, &mut next);
    }

    next
}

/*
 * the defaults as a config file, every key with what it does, one that sets nothing commented
 * with an example: what `kanade config defaults` prints and the wiki's Configuration page shows
 */
pub fn defaults() -> String {
    let config = Config::default();
    let mut sections: Vec<(&str, Vec<String>)> = vec![("", Vec::new())];

    for setting in settings() {
        let (section, key) = match setting.key.rsplit_once('.') {
            _ if setting.kind.table() => (setting.key, ""),
            Some((section, key)) => (section, key),
            None => ("", setting.key),
        };

        let mut lines = Vec::new();
        let mut help = String::from(setting.help);

        match setting.kind {
            Kind::Millis(_) => help.push_str(&format!(", ms {SHORTEST}-{LONGEST}")),
            Kind::Pixels(range, _) => {
                help.push_str(&format!(", px {}-{}", range.least, range.most))
            }
            Kind::Percent(range, _) => {
                help.push_str(&format!(", % {}-{}", range.least, range.most))
            }
            Kind::Seconds(range, _) => {
                help.push_str(&format!(", s {}-{}", range.least, range.most))
            }
            Kind::Minutes(range, _) => {
                help.push_str(&format!(", min {}-{}", range.least, range.most))
            }
            Kind::Kelvin(range, _) => help.push_str(&format!(", K {}-{}", range.least, range.most)),
            Kind::Count(count, _) => help.push_str(&format!(
                ", {} {}-{}",
                count.unit, count.range.least, count.range.most
            )),
            _ => {}
        }
        if setting.restart {
            help.push_str(", takes a restart");
        }
        if setting.per_output {
            help.push_str(", per output");
        }
        lines.push(format!("# {help}"));

        // a key that sets nothing by default shows its example, commented
        let (comment, value) = match setting.kind.value(&config) {
            Some(value) => ("", Some(value)),
            None => (
                "# ",
                setting.example.map(|example| {
                    example
                        .parse::<Value>()
                        .expect("a setting's example is a TOML value")
                }),
            ),
        };

        match value {
            Some(Value::Table(table)) if setting.kind.table() => lines.extend(
                table
                    .iter()
                    .map(|(key, value)| format!("{comment}{key} = {value}")),
            ),
            Some(value) => lines.push(format!("{comment}{key} = {value}")),
            None => {}
        }

        match sections.iter_mut().find(|(name, _)| *name == section) {
            Some((_, entries)) => entries.push(lines.join("\n")),
            None => sections.push((section, vec![lines.join("\n")])),
        }
    }

    // an output's own keys, each with its default, commented
    let mut output = vec![String::from(
        "# for the output of this name only, over the keys above from any file",
    )];
    for setting in settings().filter(|setting| setting.per_output) {
        if let Some(value) = setting.kind.value(&config) {
            output.push(format!("# {} = {value}", setting.key));
        }
    }
    sections.push(("output.\"eDP-1\"", vec![output.join("\n")]));

    sections
        .into_iter()
        .filter(|(_, entries)| !entries.is_empty())
        .map(|(section, entries)| match section {
            "" => entries.join("\n\n"),
            section => format!("[{section}]\n{}", entries.join("\n")),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
        + "\n"
}

/*
 * a config in effect, with each overridden output's config resolved once, so a view drawing a
 * frame only looks one up
 */
struct Effect {
    config: Arc<Config>,
    outputs: BTreeMap<String, Arc<Config>>,
}

impl Effect {
    fn new(config: Config) -> Self {
        let outputs = config
            .outputs
            .keys()
            .map(|name| (name.clone(), Arc::new(config.on(name).into_owned())))
            .collect();

        Effect {
            config: Arc::new(config),
            outputs,
        }
    }
}

// tests never read the user's files, so they see the defaults
static CURRENT: LazyLock<RwLock<Effect>> = LazyLock::new(|| {
    let config = if cfg!(test) {
        Config::default()
    } else {
        let (config, problems) = read();

        for problem in &problems {
            eprintln!("kanade: {problem}");
        }

        *SKIPPED.locked() = problems;
        config
    };

    RwLock::new(Effect::new(config))
});

// what the config read at start skipped; a config a reload applies skipped nothing
static SKIPPED: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn skipped() -> Vec<String> {
    LazyLock::force(&CURRENT);

    SKIPPED.locked().clone()
}

/*
 * a reload, which every window that read the config draws again for: reading the config through
 * `get` or `on` reads this too. Its guards are taken and dropped at once, never held while taking another lock
 */
#[derive(Default)]
pub struct Reloads;

impl Service for Reloads {
    fn new() -> Self {
        Self
    }

    // only `install` changes it
    fn listen() {}
}

// the config in effect; a reload swaps it whole, so one read never mixes two
pub fn get() -> Arc<Config> {
    // subscribes the window being drawn, if any, to reloads
    drop(Reloads::read());

    CURRENT.reading().config.clone()
}

// the config in effect on one output, its overrides over the global one, as `Config::on` gives it
pub fn on(output: &str) -> Arc<Config> {
    drop(Reloads::read());

    let effect = CURRENT.reading();

    effect.outputs.get(output).unwrap_or(&effect.config).clone()
}

// a reload's config replaces the one in effect (`crate::reload`)
pub fn install(config: Config) {
    *CURRENT.writing() = Effect::new(config);
}

// draws again every window that read the config, once a reload has applied all of it
pub fn announce() {
    drop(Reloads::write());
}

/*
 * every layer read over the defaults, and what did not apply as `file:line: what`, in layer order;
 * at start each problem is skipped alone, a reload refuses the lot
 */
pub fn read() -> (Config, Vec<String>) {
    let (mut config, problems) = layers(true);

    if config.appearance.accessibility {
        let desktop = accessibility::current();

        if desktop.reduced_motion {
            config.island.motion = Mode::Reduced;
        }

        if desktop.higher_contrast {
            config.appearance.finish = config.appearance.finish.solid(config.appearance.tone);
        }
    }

    if let Some(motion) = motion(env::var_os("KANADE_REDUCED_MOTION").as_deref()) {
        config.island.motion = motion;
    }

    (config, problems)
}

/*
 * the layers as `read` gives them but without KANADE_REDUCED_MOTION and the desktop's accessibility
 * preferences (ADR 0080), which no file sets, and only
 * with the settings file for `settings`: without it is what a value there is compared with
 */
pub fn layers(settings: bool) -> (Config, Vec<String>) {
    let home = env::var("HOME").ok();
    let mut config = Config::default();
    let mut problems = Vec::new();

    for path in files(home.as_deref(), settings, &mut problems) {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                problems.push(format!("{} unreadable, skipped: {error}", path.display()));
                continue;
            }
        };

        for (line, problem) in layer(&mut config, &text, MIGRATIONS, home.as_deref()) {
            problems.push(format!("{}:{line}: {problem}", path.display()));
        }
    }

    (config, problems)
}

// the settings file, the one layer Kanade writes
pub fn settings_file() -> Option<PathBuf> {
    settings_path(env::var("HOME").ok().as_deref())
}

fn settings_path(home: Option<&str>) -> Option<PathBuf> {
    base("XDG_STATE_HOME", ".local/state", home).map(|dir| dir.join("kanade/settings.toml"))
}

// where a layer's file may appear, so `crate::reload` can watch for one
pub enum Place {
    // the config directory, any `*.toml` in it
    Directory(PathBuf),

    // the settings file
    File(PathBuf),
}

pub fn places() -> Vec<Place> {
    let home = env::var("HOME").ok();
    let home = home.as_deref();

    base("XDG_CONFIG_HOME", ".config", home)
        .map(|dir| Place::Directory(dir.join("kanade")))
        .into_iter()
        .chain(settings_path(home).map(Place::File))
        .collect()
}

// a file in the config directory that is a layer: `*.toml`, not hidden
pub fn layer_name(name: &OsStr) -> bool {
    !name.as_encoded_bytes().starts_with(b".")
        && Path::new(name).extension() == Some(OsStr::new("toml"))
}

// every layer's file, in the order they apply; the settings file, the last, only for `settings`
fn files(home: Option<&str>, settings: bool, problems: &mut Vec<String>) -> Vec<PathBuf> {
    let mut files = base("XDG_CONFIG_HOME", ".config", home)
        .map(|dir| tomls(&dir.join("kanade"), problems))
        .unwrap_or_default();

    if settings {
        files.extend(settings_path(home));
    }

    files
}

// an XDG base directory: the variable if set to an absolute path, else its default under home
pub fn base(variable: &str, default: &str, home: Option<&str>) -> Option<PathBuf> {
    env::var_os(variable)
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| home.map(|home| Path::new(home).join(default)))
}

// the `*.toml` files in a directory by name, leaving out hidden ones; no directory is no files
fn tomls(dir: &Path, problems: &mut Vec<String>) -> Vec<PathBuf> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            problems.push(format!("{} unreadable, skipped: {error}", dir.display()));
            return Vec::new();
        }
    };

    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.file_name().is_some_and(layer_name) && path.is_file())
        .collect();

    files.sort();
    files
}

// KANADE_REDUCED_MOTION set and not empty wins over the files: 0 is off, anything else on
fn motion(reduced: Option<&OsStr>) -> Option<Mode> {
    match reduced {
        Some(value) if value.is_empty() => None,
        Some(value) if value == "0" => Some(Mode::Spring),
        Some(_) => Some(Mode::Reduced),
        None => None,
    }
}

// a parsed file, each key with the line it is set on
type Table = BTreeMap<String, Entry>;

#[derive(Debug, Clone, PartialEq)]
struct Entry {
    line: usize,
    node: Node,
}

// a table merges with the one below it; anything else replaces what is below
#[derive(Debug, Clone, PartialEq)]
enum Node {
    Table(Table),
    Value(Value),
}

impl Node {
    fn value(&self) -> Value {
        match self {
            Node::Table(table) => Value::Table(plain(table)),
            Node::Value(value) => value.clone(),
        }
    }
}

// a parsed file as plain TOML, without its lines
fn plain(table: &Table) -> toml::Table {
    table
        .iter()
        .map(|(key, entry)| (key.clone(), entry.node.value()))
        .collect()
}

// plain TOML as a parsed file, each key on no line
fn entries(table: &toml::Table) -> Table {
    table
        .iter()
        .map(|(key, value)| {
            let node = match value {
                Value::Table(table) => Node::Table(entries(table)),
                value => Node::Value(value.clone()),
            };

            (key.clone(), Entry { line: 0, node })
        })
        .collect()
}

/*
 * the settings file's text in the current layout, for the Settings window to change a key of and
 * write back whole; one it cannot read whole is refused, so a write never drops what it held
 */
pub fn settings_table(text: &str) -> Result<toml::Table, String> {
    let parsed = DeTable::parse(text).map_err(|error| {
        let line = error.span().map_or(1, |span| line(text, span.start));

        format!("{line}: not valid TOML: {}", error.message().trim_end())
    })?;

    let mut problems = Vec::new();
    let mut table = tree(text, parsed.into_inner(), &mut problems);

    if let Some((line, problem)) = problems.into_iter().next() {
        return Err(format!("{line}: {problem}"));
    }

    migrate(&mut table, MIGRATIONS).map_err(|(line, problem)| format!("{line}: {problem}"))?;

    Ok(plain(&table))
}

// sets each key of a table as a layer would, and says what did not apply
pub fn apply_table(config: &mut Config, table: &toml::Table) -> Vec<String> {
    let home = env::var("HOME").ok();
    let mut problems = Vec::new();

    apply(
        config,
        &entries(table),
        "",
        home.as_deref(),
        &mut Scope::Global,
        &mut problems,
    );

    problems.into_iter().map(|(_, problem)| problem).collect()
}

// takes a file from the layout at its index plus one to the next, before it applies
type Migration = fn(&mut Table);

// the current layout is the last one migrated to
const MIGRATIONS: &[Migration] = &[
    // 2: notifications show as Banners, with no `timings.toast`
    |table| {
        if let Some(Node::Table(timings)) = table.get_mut("timings").map(|entry| &mut entry.node) {
            timings.remove("toast");
        }
    },
    // 3: privacy dots have no `privacy.style` choice, only `privacy.indicators` (ADR 0033)
    |table| {
        if let Some(Node::Table(privacy)) = table.get_mut("privacy").map(|entry| &mut entry.node) {
            privacy.remove("style");
        }
    },
    // 4: the old material names select Frosted Glass; 5 drops the key
    |_| {},
    // 5: the Island, Dock and Banners are solid black, with no `appearance.material` or
    // `appearance.highlight` (ADR 0049)
    |table| {
        if let Some(Node::Table(appearance)) =
            table.get_mut("appearance").map(|entry| &mut entry.node)
        {
            appearance.remove("material");
            appearance.remove("highlight");
        }
    },
];

// the `schema_version` this build writes and reads up to
pub const SCHEMA_VERSION: usize = MIGRATIONS.len() + 1;

/*
 * applies one file over the config and returns what in it did not apply, as `(line, what)` in line
 * order: parse, migrate, then set each key, so a later file's key replaces this one's
 */
fn layer(
    config: &mut Config,
    text: &str,
    migrations: &[Migration],
    home: Option<&str>,
) -> Vec<(usize, String)> {
    let mut problems = Vec::new();

    match DeTable::parse(text) {
        Ok(table) => {
            let mut table = tree(text, table.into_inner(), &mut problems);

            match migrate(&mut table, migrations) {
                Ok(()) => apply(config, &table, "", home, &mut Scope::Global, &mut problems),
                Err(problem) => problems = vec![problem],
            }
        }
        Err(error) => {
            let line = error.span().map_or(1, |span| line(text, span.start));
            let message = error.message().trim_end();

            problems.push((
                line,
                format!("not valid TOML, skipped this file: {message}"),
            ));
        }
    }

    problems.sort_by_key(|&(line, _)| line);
    problems
}

fn line(text: &str, offset: usize) -> usize {
    text.get(..offset)
        .map_or(0, |before| before.matches('\n').count())
        + 1
}

fn tree(text: &str, table: DeTable, problems: &mut Vec<(usize, String)>) -> Table {
    let mut out = Table::new();

    for (key, value) in table {
        let line = line(text, key.span().start);
        let span = value.span();

        let node = match value.into_inner() {
            DeValue::Table(table) => Node::Table(tree(text, table, problems)),
            other => {
                let value = ValueDeserializer::from(toml::Spanned::new(span, other));

                match Value::deserialize(value) {
                    Ok(value) => Node::Value(value),
                    Err(error) => {
                        problems.push((line, format!("{key}: {}", error.message().trim_end())));
                        continue;
                    }
                }
            }
        };

        out.insert(key.into_inner().into_owned(), Entry { line, node });
    }

    out
}

/*
 * brings a file to the current layout, from its `schema_version` or else version 1; a version this
 * build has no migration from says how to read none of the file, so the whole file is skipped
 */
fn migrate(table: &mut Table, migrations: &[Migration]) -> Result<(), (usize, String)> {
    let current = migrations.len() + 1;

    let from = match table.remove("schema_version") {
        None => 1,
        Some(entry) => {
            let version = match &entry.node {
                Node::Value(Value::Integer(version)) => usize::try_from(*version).ok(),
                _ => None,
            };

            match version {
                Some(version) if (1..=current).contains(&version) => version,
                Some(version) if version > current => {
                    return Err((
                        entry.line,
                        format!(
                            "schema_version {version} is newer than this Kanade reads \
                             ({current}), skipped this file"
                        ),
                    ));
                }
                _ => {
                    return Err((
                        entry.line,
                        format!(
                            "schema_version: expected 1 to {current}, found {}, skipped this file",
                            entry.node.value()
                        ),
                    ));
                }
            }
        }
    };

    for migration in &migrations[from - 1..] {
        migration(table);
    }

    Ok(())
}

pub fn settings() -> impl Iterator<Item = &'static Setting> {
    modules::ALL.iter().flat_map(|module| module.settings)
}

// the setting a dotted key names, or why none does
pub fn setting(key: &str) -> Result<&'static Setting, String> {
    settings()
        .find(|setting| setting.key == key)
        .ok_or_else(|| unknown(key, key, false))
}

// `unknown key`, with the nearest key or section when one is close, as `shown` names it
fn unknown(shown: &str, path: &str, per_output: bool) -> String {
    let known = settings().filter(|setting| !per_output || setting.per_output);
    let candidates = known.flat_map(|setting| {
        setting
            .key
            .match_indices('.')
            .map(|(at, _)| &setting.key[..at])
            .chain([setting.key])
    });
    let limit = (path.len() / 4).clamp(1, 2);

    match candidates
        .map(|candidate| (distance(path, candidate), candidate))
        .filter(|&(distance, _)| (1..=limit).contains(&distance))
        .min_by_key(|&(distance, _)| distance)
    {
        Some((_, near)) => format!("unknown key {shown}, did you mean {near}?"),
        None => format!("unknown key {shown}"),
    }
}

// how many single-character edits turn `from` into `to`
fn distance(from: &str, to: &str) -> usize {
    let to: Vec<char> = to.chars().collect();
    let mut row: Vec<usize> = (0..=to.len()).collect();

    for (at, from) in from.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = at + 1;

        for (to_at, &to) in to.iter().enumerate() {
            let above = row[to_at + 1];

            row[to_at + 1] = (diagonal + usize::from(from != to))
                .min(above + 1)
                .min(row[to_at] + 1);
            diagonal = above;
        }
    }

    row[to.len()]
}

/*
 * the files that set a key and apply, in layer order, the last of which gives its value; none when
 * the defaults do. A file that does not read whole is `layers`' to report
 */
pub fn origin(setting: &Setting) -> Vec<PathBuf> {
    let home = env::var("HOME").ok();
    let mut config = Config::default();
    let mut origin = Vec::new();

    for path in files(home.as_deref(), true, &mut Vec::new()) {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };

        if let Ok(table) = settings_table(&text)
            && let Some(value) = setting
                .key
                .split('.')
                .try_fold(&Value::Table(table), |value, key| value.get(key))
        {
            let nested = setting
                .key
                .split('.')
                .rev()
                .fold(value.clone(), |inner, key| {
                    Value::Table(toml::Table::from_iter([(key.to_owned(), inner)]))
                });

            if let Value::Table(nested) = nested
                && apply_table(&mut config.clone(), &nested).is_empty()
            {
                origin.push(path.clone());
            }
        }

        layer(&mut config, &text, MIGRATIONS, home.as_deref());
    }

    origin
}

// what a file alone would not apply, as `(line, what)`; the file is no layer, so it is read over the defaults
pub fn check(text: &str) -> Vec<(usize, String)> {
    let home = env::var("HOME").ok();

    layer(&mut Config::default(), text, MIGRATIONS, home.as_deref())
}

// the keys a table may set: any, or for an output the keys marked per output, noting each it sets
enum Scope<'a> {
    Global,
    Output {
        name: &'a str,
        keys: &'a mut Vec<&'static str>,
    },
}

impl Scope<'_> {
    // a key as the file names it
    fn shown(&self, path: &str) -> String {
        match self {
            Scope::Global => path.to_owned(),
            Scope::Output { name, .. } => format!("output.{name}.{path}"),
        }
    }
}

// sets each key of a table at `prefix` that a Module owns, and names every other
fn apply(
    config: &mut Config,
    table: &Table,
    prefix: &str,
    home: Option<&str>,
    scope: &mut Scope,
    problems: &mut Vec<(usize, String)>,
) {
    for (key, entry) in table {
        let path = match prefix {
            "" => key.clone(),
            prefix => format!("{prefix}.{key}"),
        };
        let shown = scope.shown(&path);

        if matches!(scope, Scope::Global) && path == "output" {
            outputs(config, entry, home, problems);
            continue;
        }

        let inside = |setting: &Setting| {
            setting
                .key
                .strip_prefix(path.as_str())
                .is_some_and(|rest| rest.starts_with('.'))
        };

        match settings().find(|setting| setting.key == path) {
            Some(setting) if matches!(scope, Scope::Output { .. }) && !setting.per_output => {
                problems.push((entry.line, format!("{shown}: cannot be set per output")));
            }
            // each Module on its own line, so one bad entry keeps the others
            Some(Setting {
                kind: Kind::Modules(field),
                ..
            }) => match &entry.node {
                Node::Table(table) => {
                    let mut off = (field.get)(config);

                    for (name, entry) in table {
                        if let Err(what) = turn(&mut off, name, &entry.node.value()) {
                            problems.push((entry.line, what));
                        }
                    }

                    (field.set)(config, off);
                }
                Node::Value(_) => problems.push((entry.line, format!("{path}: expected a table"))),
            },
            Some(setting) => match setting.kind.set(config, &entry.node.value(), home) {
                Ok(()) => {
                    if let Scope::Output { keys, .. } = scope
                        && !keys.contains(&setting.key)
                    {
                        keys.push(setting.key);
                    }
                }
                Err(what) => problems.push((entry.line, format!("{shown}: {what}"))),
            },
            None if settings().any(inside) => match &entry.node {
                Node::Table(table) => apply(config, table, &path, home, scope, problems),
                Node::Value(_) => {
                    problems.push((entry.line, format!("{shown}: expected a table")));
                }
            },
            None => problems.push((
                entry.line,
                unknown(&shown, &path, matches!(scope, Scope::Output { .. })),
            )),
        }
    }
}

// `[output."<name>"]` tables, each over what the layers below gave that output
fn outputs(
    config: &mut Config,
    entry: &Entry,
    home: Option<&str>,
    problems: &mut Vec<(usize, String)>,
) {
    let Node::Table(table) = &entry.node else {
        problems.push((
            entry.line,
            String::from("output: expected a table of outputs"),
        ));
        return;
    };

    for (name, entry) in table {
        let Node::Table(keys) = &entry.node else {
            problems.push((entry.line, format!("output.{name}: expected a table")));
            continue;
        };

        let mut output = config.outputs.remove(name).unwrap_or_default();
        let mut scope = Scope::Output {
            name,
            keys: &mut output.keys,
        };

        apply(&mut output.values, keys, "", home, &mut scope, problems);

        // none set keeps none, so an empty table changes nothing
        if !output.keys.is_empty() {
            config.outputs.insert(name.clone(), output);
        }
    }
}

// a leading `~/` is the home directory, as a shell would have it
fn expand(path: &str, home: Option<&str>) -> String {
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => format!("{}/{rest}", home.trim_end_matches('/')),
        _ => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    // each text a file over the last, as `load` applies them; the problems by file
    fn layers(texts: &[&str], migrations: &[Migration]) -> (Config, Vec<Vec<(usize, String)>>) {
        let mut config = Config::default();
        let problems = texts
            .iter()
            .map(|text| layer(&mut config, text, migrations, Some("/home/you")))
            .collect();

        (config, problems)
    }

    fn one(text: &str) -> (Config, Vec<(usize, String)>) {
        let (config, mut problems) = layers(&[text], MIGRATIONS);

        (config, problems.remove(0))
    }

    fn said(list: &[(usize, &str)]) -> Vec<(usize, String)> {
        list.iter()
            .map(|&(line, what)| (line, what.to_owned()))
            .collect()
    }

    #[test]
    fn an_empty_file_is_the_defaults() {
        assert_eq!(one(""), (Config::default(), vec![]));
        assert_eq!(one("\n  # nothing\n"), (Config::default(), vec![]));
    }

    #[test]
    fn removed_material_and_highlight_keys_are_dropped_silently() {
        for version in [None, Some(SCHEMA_VERSION - 1)] {
            let version = version.map_or(String::new(), |version| {
                format!("schema_version = {version}\n")
            });
            let text = format!(
                "{version}[appearance]\nmaterial = \"liquid-glass\"\nhighlight = \"bright\"\ntone = \"light\""
            );
            let (config, problems) = one(&text);

            assert!(problems.is_empty(), "{problems:?}");
            assert_eq!(config.appearance.tone, Tone::Light);
        }
    }

    #[test]
    fn animation_speed_scales_transitions_not_interaction_delays() {
        let (config, problems) = one(
            "[animation]\nspeed = 200\n\n[timings]\nhover = 120\nexpand = 200\nsurface_change = 240\ncollapse = 160\ngrace = 250\nosd = 1200",
        );

        assert!(problems.is_empty());
        assert_eq!(config.animation_duration(ms(220)), ms(110));
        assert_eq!(
            config.animation_timings(),
            Timings {
                hover: ms(120),
                expand: ms(100),
                surface_change: ms(120),
                collapse: ms(80),
                reduced_fade: ms(40),
                grace: ms(250),
                ..Timings::default()
            }
        );
        assert_eq!(config.osd, ms(1200));
        assert_eq!(scale_animation(ms(200), 50), ms(400));
    }

    // the README's and this module's examples, as v0.1 read them
    #[test]
    fn a_v0_1_file_loads_unchanged() {
        let text = r#"
            reduced_motion = true   # trailing comment
            clock = "12h"

            [timings]   # milliseconds
            hover = 100
            expand = 200
            surface_change = 260
            collapse = 160
            grace = 300
            osd = 1_000

            [ theme ]
            palette = "~/Pictures/wall # 1.jpg"

            [modules]   # every Module is on unless turned off here; `island` cannot be
            media = false
            timer = true
            island = true
        "#;

        let (config, problems) = one(text);

        assert_eq!(problems, vec![]);
        assert_eq!(
            config,
            Config {
                island: Timings {
                    motion: Mode::Reduced,
                    hover: ms(100),
                    expand: ms(200),
                    surface_change: ms(260),
                    collapse: ms(160),
                    grace: ms(300),
                    ..Timings::default()
                },
                osd: ms(1000),
                palette: Some(String::from("/home/you/Pictures/wall # 1.jpg")),
                clock: Hours::Twelve,
                off: vec!["dictation", "idle", "media"],
                apps: BTreeMap::new(),
                gestures: BTreeMap::new(),
                pinned: Vec::new(),
                wallpapers: None,
                calendars: Vec::new(),
                location: None,
                place: None,
                units: Units::Metric,
                outputs: BTreeMap::new(),
                ..Config::default()
            }
        );
        assert!(config.off("media"));
        assert!(!config.off("timer"));

        assert_eq!(one(&defaults()), (Config::default(), vec![]));
    }

    // the wiki's Configuration page shows what the registry says, so the docs cannot drift from the keys
    #[test]
    fn the_wiki_shows_the_defaults() {
        let page = include_str!("../docs/wiki/Configuration.md");
        let start = page
            .find("```toml\n")
            .expect("Configuration page shows the config")
            + 8;
        let shown = &page[start..][..page[start..].find("```").unwrap()];

        assert_eq!(
            shown,
            defaults(),
            "paste `cargo run -- config defaults` into docs/wiki/Configuration.md"
        );
    }

    // every key v0.1 and the Modules since read is in the registry, so none became unknown
    #[test]
    fn every_key_is_in_the_registry() {
        let keys: Vec<&str> = settings().map(|setting| setting.key).collect();

        assert_eq!(
            keys,
            [
                "reduced_motion",
                "clock",
                "timings.hover",
                "timings.expand",
                "timings.surface_change",
                "timings.collapse",
                "timings.grace",
                "timings.osd",
                "theme.palette",
                "shell.mode",
                "shell.edge",
                "shell.align",
                "appearance.tone",
                "appearance.finish",
                "appearance.refraction",
                "appearance.sync",
                "appearance.accessibility",
                "animation.speed",
                "appearance.motion",
                "appearance.font",
                "appearance.scale",
                "island.edge",
                "island.align",
                "island.width",
                "island.height",
                "island.autohide",
                "island.reserve",
                "island.wheel",
                "island.device_events",
                "rest.clock",
                "rest.battery",
                "rest.peek.battery",
                "rest.peek.weather",
                "rest.peek.agenda",
                "privacy.indicators",
                "banners.entrance",
                "modules",
                "windows.apps",
                "dock.pinned",
                "dock.edge",
                "dock.align",
                "dock.autohide",
                "dock.reserve",
                "dock.magnification",
                "dock.size",
                "dock.merge",
                "battery.low",
                "battery.critical",
                "battery.critical_action",
                "media.visualizer",
                "media.lyrics",
                "timer.pomodoro.work",
                "timer.pomodoro.break",
                "timer.pomodoro.long_break",
                "timer.pomodoro.rounds",
                "live.stale",
                "brightness.backend",
                "osd.layout",
                "notifications.sound",
                "notifications.rules",
                "controls.tiles",
                "wallpaper.directory",
                "wallpaper.per_monitor",
                "wallpaper.transition",
                "wallpaper.transition_duration",
                "launcher.actions",
                "launcher.commands",
                "calendar.paths",
                "calendar.reminder",
                "calendar.subscriptions",
                "calendar.subscription_interval",
                "weather.location",
                "weather.place",
                "weather.units",
                "gestures",
                "dictate.insert",
                "dictate.never_in",
                "night-light.mode",
                "night-light.temperature",
                "night-light.sunset",
                "night-light.sunrise",
                "focus.dnd",
                "focus.caffeine",
                "focus.night_light",
                "focus.auto",
                "lock.backdrop",
                "lock.pam_service",
                "lock.grace",
                "idle.dim",
                "idle.lock",
                "idle.screen_off",
                "idle.suspend",
                "settings.health_notice",
                "ui.describe",
                "hooks.lock",
                "hooks.unlock",
                "hooks.wallpaper",
                "hooks.tone",
                "hooks.charger",
                "hooks.suspend",
                "hooks.resume",
                "hooks.started",
                "hooks.dnd",
                "hooks.power_profile",
                "hooks.logout",
                "hooks.reboot",
                "hooks.poweroff",
                "hooks.timeout",
            ]
        );
        assert_eq!(
            settings()
                .filter(|setting| setting.restart)
                .map(|setting| setting.key)
                .collect::<Vec<_>>(),
            ["appearance.font", "modules", "brightness.backend"]
        );
        assert_eq!(
            settings()
                .filter(|setting| setting.per_output)
                .map(|setting| setting.key)
                .collect::<Vec<_>>(),
            [
                "clock",
                "rest.clock",
                "rest.battery",
                "rest.peek.battery",
                "rest.peek.weather",
                "rest.peek.agenda"
            ]
        );

        // `Config::on` copies an output's value over whole, and `pending` reads only the global one
        for setting in settings().filter(|setting| setting.per_output) {
            assert!(
                !setting.kind.table() && !setting.restart,
                "{} cannot be per output",
                setting.key
            );
        }
    }

    #[test]
    fn combined_shell_uses_its_own_anchor_without_changing_separate_placements() {
        let config = Config {
            island_place: Placement {
                anchor: Anchor {
                    edge: Edge::Left,
                    along: Along::Right,
                },
                autohide: true,
                reserve: true,
            },
            dock_place: Placement {
                anchor: Anchor {
                    edge: Edge::Bottom,
                    along: Along::Left,
                },
                autohide: true,
                reserve: false,
            },
            shell: ShellConfig {
                mode: ShellMode::Combined,
                anchor: Anchor {
                    edge: Edge::Bottom,
                    along: Along::Right,
                },
            },
            ..Config::default()
        };

        let island = config.island_placement();
        assert_eq!(island.anchor, config.shell.anchor);
        assert!(island.autohide && island.reserve);
        let dock = config.dock_placement();
        assert_eq!(dock.anchor, config.shell.anchor);
        assert!(dock.autohide);
        assert!(!dock.reserve);
        assert_eq!(config.island_place.anchor.edge, Edge::Left);
        assert_eq!(config.island_place.anchor.along, Along::Right);
        assert_eq!(config.dock_place.anchor.edge, Edge::Bottom);
        assert_eq!(config.dock_place.anchor.along, Along::Left);
    }

    #[test]
    fn separate_shell_uses_its_own_saved_placements() {
        let mut config = Config::default();
        config.island_place.anchor.edge = Edge::Right;
        config.dock_place.anchor.along = Along::Left;
        config.shell.anchor = Anchor {
            edge: Edge::Bottom,
            along: Along::Right,
        };

        assert_eq!(config.island_placement(), config.island_place);
        assert_eq!(config.dock_placement(), config.dock_place);
    }

    // what a key reads it gives back, so its default and its docs are what the config holds
    #[test]
    fn each_kind_reads_back_what_it_gives() {
        let config = Config::default();

        for setting in settings() {
            let shown = setting.kind.value(&config);
            let example = setting
                .example
                .map(|example| example.parse::<Value>().unwrap());

            assert!(
                shown.is_some() != example.is_some(),
                "{}: a default or an example, not both",
                setting.key
            );

            // given back as read, like an id with its `.desktop`, which reads the same again
            let mut read = Config::default();
            let given = shown.or(example).unwrap();
            assert_eq!(
                setting.kind.set(&mut read, &given, None),
                Ok(()),
                "{}",
                setting.key
            );

            let value = setting.kind.value(&read).expect(setting.key);
            let mut again = Config::default();
            assert_eq!(
                setting.kind.set(&mut again, &value, None),
                Ok(()),
                "{}",
                setting.key
            );
            assert_eq!(again, read, "{}", setting.key);
        }
    }

    // a key read only at start keeps its running value, and a change to it is pending
    #[test]
    fn a_restart_key_keeps_its_running_value() {
        let running = one("[modules]\nmedia = false\ntimer = false").0;
        let read = one("clock = \"12h\"\n[modules]\ntimer = false\nbattery = false").0;

        assert_eq!(
            pending(&running, &read),
            ["modules.battery", "modules.media"]
        );
        assert_eq!(pending(&running, &running), Vec::<String>::new());

        let next = restarted(&running, read);
        assert_eq!(next.clock, Hours::Twelve);
        assert_eq!(next.off, ["dictation", "idle", "media", "timer"]);
    }

    /*
     * an output's value wins over the global one from any layer, and output values layer like
     * global ones; another output, or one not named, reads the global value
     */
    #[test]
    fn an_output_overrides_the_global_value_from_any_layer() {
        let first = "clock = \"12h\"\n[output.eDP-1]\nclock = \"24h\"";
        let second = "clock = \"12h\"\n[output.\"HDMI-A-1\"]\nclock = \"24h\"";
        let third = "clock = \"24h\"\noutput.HDMI-A-1.clock = \"12h\"\n[output.DP-2]";

        let (config, problems) = layers(&[first, second, third], MIGRATIONS);

        assert_eq!(problems, [vec![], vec![], vec![]]);
        assert_eq!(config.clock, Hours::TwentyFour);
        assert_eq!(config.on("eDP-1").clock, Hours::TwentyFour);
        assert_eq!(config.on("HDMI-A-1").clock, Hours::Twelve);
        assert_eq!(config.on("DP-1").clock, Hours::TwentyFour);

        // an empty table sets nothing, so that output reads the global config itself
        assert!(matches!(config.on("DP-2"), Cow::Borrowed(_)));
        assert_eq!(config.outputs.len(), 2);

        // only the overridden key differs
        let edp = one("clock = \"12h\"\ntimings.hover = 90\noutput.eDP-1.clock = \"24h\"").0;
        assert_eq!(
            *edp.on("eDP-1"),
            Config {
                clock: Hours::TwentyFour,
                ..edp.clone()
            }
        );
    }

    // an override on every connected output hides the global Rest, and one on an absent output shows nowhere
    #[test]
    fn rests_follow_the_outputs_there_are() {
        let (config, _) = one(
            "rest.clock = false\noutput.eDP-1.rest.clock = true\noutput.DP-9.rest.clock = true",
        );

        assert!(config.rests(["eDP-1"]).iter().all(|rest| rest.clock));
        assert!(
            config
                .rests(["eDP-1", "HDMI-A-1"])
                .iter()
                .any(|rest| !rest.clock)
        );
        assert!(config.rests(["HDMI-A-1"]).iter().all(|rest| !rest.clock));
        assert_eq!(config.rests([]), [config.rest]);
    }

    // a key no output may set, or a bad value, keeps what the layers below gave that output
    #[test]
    fn a_bad_output_key_keeps_what_is_below_and_says_where() {
        let first = "[output.eDP-1]\nclock = \"12h\"";
        let second = r#"
[output.eDP-1]
clock = "13h"
reduced_motion = true
timings.hover = 90
teleport = 1
modules.media = false
[output.DP-1]
theme = 1
[output]
HDMI-A-1 = "12h"
"#;

        let (config, problems) = layers(&[first, second], MIGRATIONS);

        assert_eq!(config.on("eDP-1").clock, Hours::Twelve);
        assert_eq!(
            *config.on("eDP-1"),
            Config {
                clock: Hours::Twelve,
                ..config.clone()
            }
        );
        assert_eq!(config.on("DP-1").clock, Hours::TwentyFour);
        assert_eq!(
            problems[1],
            said(&[
                (
                    3,
                    "output.eDP-1.clock: expected \"24h\" or \"12h\", found \"13h\""
                ),
                (4, "output.eDP-1.reduced_motion: cannot be set per output"),
                (5, "output.eDP-1.timings.hover: cannot be set per output"),
                (6, "unknown key output.eDP-1.teleport"),
                (7, "output.eDP-1.modules: cannot be set per output"),
                (9, "output.DP-1.theme: expected a table"),
                (11, "output.HDMI-A-1: expected a table"),
            ])
        );

        assert_eq!(
            one("output = 1").1,
            said(&[(1, "output: expected a table of outputs")])
        );
    }

    // each overridden output's config is resolved once, as `Config::on` gives it
    #[test]
    fn the_config_in_effect_resolves_each_output_once() {
        let config = one("clock = \"12h\"\noutput.eDP-1.clock = \"24h\"").0;
        let effect = Effect::new(config.clone());

        assert_eq!(*effect.outputs["eDP-1"], *config.on("eDP-1"));
        assert_eq!(effect.outputs.len(), 1);
        assert_eq!(*effect.config, config);
    }

    #[test]
    fn real_toml_reads_dotted_keys_inline_tables_and_escapes() {
        let text = r#"
            timings.hover = 90
            theme = { palette = "a \"b\" \\c\t" }
            modules.media = false
        "#;

        let (config, problems) = one(text);

        assert_eq!(problems, vec![]);
        assert_eq!(config.island.hover, ms(90));
        assert_eq!(config.palette.as_deref(), Some("a \"b\" \\c\t"));
        assert_eq!(config.off, ["dictation", "idle", "media"]);
    }

    // a later file wins key by key; a table it leaves out keeps what is below
    #[test]
    fn later_layers_win_and_tables_merge() {
        let first = "clock = \"12h\"\n[timings]\nhover = 100\nexpand = 200\n[modules]\nmedia = false\ntimer = false";
        let second = "[timings]\nexpand = 300\n[modules]\ntimer = true\nbattery = false";
        let third = "[theme]\npalette = \"/a.png\"";

        let (config, problems) = layers(&[first, second, third], MIGRATIONS);

        assert_eq!(problems, [vec![], vec![], vec![]]);
        assert_eq!(config.clock, Hours::Twelve);
        assert_eq!(config.island.hover, ms(100));
        assert_eq!(config.island.expand, ms(300));
        assert_eq!(config.palette.as_deref(), Some("/a.png"));
        assert_eq!(config.off, ["dictation", "idle", "media", "battery"]);
    }

    // what a bad key would have set stays as the layers below left it
    #[test]
    fn a_bad_key_keeps_what_is_below_and_says_where() {
        let first = "[timings]\nhover = 100\nosd = 1000";
        let second = r#"
clock = "13h"
[timings]
hover = 0
expand = -5
grace = 300
osd = "900"
speed = 3
[colors]
osd = 900
[theme]
palette = 1
[modules]
island = false
teleport = false
media = 0
battery = false
"#;

        let (config, problems) = layers(&[first, second], MIGRATIONS);

        assert_eq!(config.island.hover, ms(100));
        assert_eq!(config.island.expand, Timings::default().expand);
        assert_eq!(config.island.grace, ms(300));
        assert_eq!(config.osd, ms(1000));
        assert_eq!(config.palette, None);
        assert_eq!(config.clock, Hours::TwentyFour);
        assert_eq!(config.off, ["dictation", "idle", "battery"]);
        assert_eq!(
            problems[1],
            said(&[
                (2, "clock: expected \"24h\" or \"12h\", found \"13h\""),
                (4, "timings.hover: 0 is outside 1-60000 ms"),
                (5, "timings.expand: -5 is outside 1-60000 ms"),
                (7, "timings.osd: expected milliseconds"),
                (8, "unknown key timings.speed"),
                (9, "unknown key colors"),
                (12, "theme.palette: expected a \"path\""),
                (14, "modules.island: the core module cannot be turned off"),
                (15, "unknown module modules.teleport"),
                (16, "modules.media: expected true or false"),
            ])
        );
    }

    // a typo names the key it is close to, in a section or per output; one far from any names none
    #[test]
    fn an_unknown_key_names_the_nearest() {
        let text = "clok = \"12h\"\n[timings]\nhovr = 300\n[output.\"eDP-1\"]\nclok = \"12h\"\nteleport = 1";
        let (_, problems) = one(text);

        assert_eq!(
            problems,
            said(&[
                (1, "unknown key clok, did you mean clock?"),
                (3, "unknown key timings.hovr, did you mean timings.hover?"),
                (5, "unknown key output.eDP-1.clok, did you mean clock?"),
                (6, "unknown key output.eDP-1.teleport"),
            ])
        );
    }

    // a later file overrides an app id below it, and keeps the others
    #[test]
    fn app_overrides_merge_by_app_id() {
        let first = "[windows.apps]\ncode = \"code-oss\"\nsteam_app_1 = \"game.desktop\"";
        let second = "windows.apps.code = \"com.visualstudio.code.desktop\"\nwindows.apps.zed = \"dev.zed.Zed\"";
        let third = "[windows.apps]\nok = \"ok\"\nbad = 2";

        let (config, problems) = layers(&[first, second, third], MIGRATIONS);

        assert_eq!(problems[..2], [vec![], vec![]]);
        assert_eq!(
            problems[2],
            said(&[(1, "windows.apps: bad: expected a \"desktop file id\"")])
        );
        assert_eq!(
            config.apps,
            BTreeMap::from([
                (
                    String::from("code"),
                    String::from("com.visualstudio.code.desktop")
                ),
                (String::from("steam_app_1"), String::from("game.desktop")),
                (String::from("zed"), String::from("dev.zed.Zed.desktop")),
            ])
        );

        // a bad one keeps all of its file's below
        assert!(!config.apps.contains_key("ok"));

        assert_eq!(
            one("windows.apps = 1").1,
            said(&[(
                1,
                "windows.apps: expected a table of app id = \"desktop file id\""
            )])
        );
    }

    // a later file's list replaces the one below
    #[test]
    fn pinned_apps_are_a_list_a_later_file_replaces() {
        let first = "[dock]\npinned = [\"firefox\", \"kitty.desktop\"]";
        let second = "dock.pinned = [\"org.kde.dolphin\", \"zed\", \"zed.desktop\"]";

        let (config, problems) = layers(&[first, second], MIGRATIONS);

        assert_eq!(problems, [vec![], vec![]]);
        assert_eq!(config.pinned, ["org.kde.dolphin.desktop", "zed.desktop"]);

        let (config, _) = layers(&[first], MIGRATIONS);
        assert_eq!(config.pinned, ["firefox.desktop", "kitty.desktop"]);

        // a bad one keeps the list below
        let (config, problems) = layers(&[first, "dock.pinned = [\"a\", 2]"], MIGRATIONS);
        assert_eq!(config.pinned, ["firefox.desktop", "kitty.desktop"]);
        assert_eq!(
            problems[1],
            said(&[(1, "dock.pinned: expected a \"desktop file id\", found 2")])
        );

        assert_eq!(
            one("dock.pinned = \"firefox\"").1,
            said(&[(1, "dock.pinned: expected a list of \"desktop file id\"s")])
        );
    }

    // `[[notifications.rules]]` reads as a list a later file replaces; a bad rule keeps the list
    // below and says where
    #[test]
    fn notification_rules_are_a_list_a_later_file_replaces() {
        let first = "[[notifications.rules]]\napp = \"Slack\"\nbanner = false\n\n[[notifications.rules]]\nsummary = \"battery\"\nbypass_dnd = true\n";

        let (config, problems) = one(first);
        assert_eq!(problems, vec![]);
        assert_eq!(config.notification_rules.len(), 2);
        assert_eq!(config.notification_rules[0].app.as_deref(), Some("Slack"));
        assert_eq!(config.notification_rules[1].bypass_dnd, Some(true));

        let (config, problems) = layers(
            &[
                first,
                "\n[[notifications.rules]]\napp = \"Mail\"\nsumary = \"x\"\n",
            ],
            MIGRATIONS,
        );
        assert_eq!(config.notification_rules.len(), 2);
        assert_eq!(problems[1].len(), 1);
        assert_eq!(problems[1][0].0, 2);
        assert!(
            problems[1][0]
                .1
                .starts_with("notifications.rules: rule 1: unknown key sumary"),
            "{:?}",
            problems[1]
        );

        let (config, problems) = layers(
            &[first, "[[notifications.rules]]\ncategory = \"email\"\n"],
            MIGRATIONS,
        );
        assert_eq!(problems, [vec![], vec![]]);
        assert_eq!(config.notification_rules.len(), 1);
        assert_eq!(
            config.notification_rules[0].category.as_deref(),
            Some("email")
        );
    }

    // a path list expands `~/`, keeps each once, and a later file's replaces the one below
    #[test]
    fn calendar_paths_are_a_list_of_paths() {
        let first = "calendar.paths = [\"~/cal\", \"/srv/a.ics\", \"/home/you/cal\"]";
        let (config, problems) = layers(&[first], MIGRATIONS);

        assert_eq!(problems, [vec![]]);
        assert_eq!(config.calendars, ["/home/you/cal", "/srv/a.ics"]);

        let (config, _) = layers(&[first, "[calendar]\npaths = [\"/b.ics\"]"], MIGRATIONS);
        assert_eq!(config.calendars, ["/b.ics"]);

        let (config, problems) = layers(&[first, "calendar.paths = [\"\"]"], MIGRATIONS);
        assert_eq!(config.calendars, ["/home/you/cal", "/srv/a.ics"]);
        assert_eq!(
            problems[1],
            said(&[(1, "calendar.paths: expected a \"path\", found \"\"")])
        );
        assert_eq!(
            one("calendar.paths = \"~/cal\"").1,
            said(&[(1, "calendar.paths: expected a list of \"path\"s")])
        );
    }

    // a feed is https, `webcal://` read as it, and a refusal never repeats the address, which may
    // hold a secret (ADR 0062)
    #[test]
    fn a_subscription_is_https_and_a_refusal_names_no_address() {
        let (config, problems) = one(
            "calendar.subscriptions = [\"webcal://example.org/a.ics?token=s3cret\", \"HTTPS://example.org/b.ics\", \"https://example.org/b.ics\"]\ncalendar.subscription_interval = 45",
        );

        assert_eq!(problems, vec![]);
        assert_eq!(
            config.subscriptions,
            [
                "https://example.org/a.ics?token=s3cret",
                "https://example.org/b.ics"
            ]
        );
        assert_eq!(config.subscription_interval, 45);

        for bad in [
            "http://example.org/s3cret.ics",
            "ftp://example.org/s3cret.ics",
            "https:///s3cret",
            "https://exa mple.org/s3cret",
            "s3cret",
        ] {
            let (config, problems) = one(&format!("calendar.subscriptions = [\"{bad}\"]"));

            assert!(config.subscriptions.is_empty(), "{bad}");
            assert_eq!(problems.len(), 1, "{bad}");
            assert!(!problems[0].1.contains("s3cret"), "{}", problems[0].1);
        }

        assert_eq!(
            one("calendar.subscription_interval = 10").1,
            said(&[(
                1,
                "calendar.subscription_interval: 10 is outside 30-1440 min"
            )])
        );
    }

    // a time is "HH:MM" on a 24 hour clock and a temperature whole kelvin in range; anything else
    // keeps what was below
    #[test]
    fn the_night_light_takes_times_of_day_and_kelvin_in_range() {
        let first = "[night-light]\nmode = \"schedule\"\ntemperature = 3400\nsunset = \"21:30\"\nsunrise = \" 06:05 \"";
        let (config, problems) = layers(&[first], MIGRATIONS);
        let evening = DayTime {
            hour: 21,
            minute: 30,
        };

        assert_eq!(problems, [vec![]]);
        assert_eq!(config.night_light.mode, NightMode::Schedule);
        assert_eq!(config.night_light.temperature, 3400);
        assert_eq!(config.night_light.sunset, Some(evening));
        assert_eq!(
            config.night_light.sunrise,
            Some(DayTime { hour: 6, minute: 5 })
        );
        assert_eq!(evening.to_string(), "21:30");

        for bad in [
            "\"24:00\"",
            "\"23:60\"",
            "\"7:00\"",
            "\"07:0\"",
            "\"0700\"",
            "\"-1:00\"",
            "\"+1:00\"",
            "\"\"",
            "2100",
        ] {
            let (config, problems) =
                layers(&[first, &format!("night-light.sunset = {bad}")], MIGRATIONS);

            assert_eq!(config.night_light.sunset, Some(evening), "{bad}");
            assert_eq!(
                problems[1],
                said(&[(1, &format!("night-light.sunset: {}", DayTime::EXPECTED))]),
                "{bad}"
            );
        }

        for (bad, why) in [
            ("999", "999 is outside 1000-6500 K"),
            ("6501", "6501 is outside 1000-6500 K"),
            ("-4000", "-4000 is outside 1000-6500 K"),
            ("4000.5", "expected whole kelvin"),
            ("\"4000\"", "expected whole kelvin"),
        ] {
            let (config, problems) = layers(
                &[first, &format!("night-light.temperature = {bad}")],
                MIGRATIONS,
            );

            assert_eq!(config.night_light.temperature, 3400, "{bad}");
            assert_eq!(
                problems[1],
                said(&[(1, &format!("night-light.temperature: {why}"))]),
                "{bad}"
            );
        }

        for edge in ["1000", "6500"] {
            let (_, problems) = layers(
                &[first, &format!("night-light.temperature = {edge}")],
                MIGRATIONS,
            );

            assert_eq!(problems[1], vec![], "{edge}");
        }
    }

    // a location is two numbers in range, integers too; anything else keeps what was below
    #[test]
    fn a_location_is_latitude_and_longitude_in_range() {
        let berlin = Location {
            latitude: 52.52,
            longitude: 13.0,
        };
        let first = "[weather]\nlocation = [52.52, 13]\nplace = \" Berlin \"\nunits = \"imperial\"";
        let (config, problems) = layers(&[first], MIGRATIONS);

        assert_eq!(problems, [vec![]]);
        assert_eq!(config.location, Some(berlin));
        assert_eq!(config.place.as_deref(), Some("Berlin"));
        assert_eq!(config.units, Units::Imperial);

        for (bad, why) in [
            ("[91, 0]", "latitude 91 is outside -90 to 90"),
            ("[0, -180.5]", "longitude -180.5 is outside -180 to 180"),
            ("[nan, 0]", "latitude NaN is outside -90 to 90"),
            ("[52.52]", Location::EXPECTED),
            ("\"52.52, 13.41\"", Location::EXPECTED),
        ] {
            let (config, problems) =
                layers(&[first, &format!("weather.location = {bad}")], MIGRATIONS);

            assert_eq!(config.location, Some(berlin), "{bad}");
            assert_eq!(
                problems[1],
                said(&[(1, &format!("weather.location: {why}"))]),
                "{bad}"
            );
        }

        for bad in ["\"  \"", "\"a\\nb\""] {
            let (config, problems) =
                layers(&[first, &format!("weather.place = {bad}")], MIGRATIONS);

            assert_eq!(config.place.as_deref(), Some("Berlin"), "{bad}");
            assert_eq!(
                problems[1],
                said(&[(1, "weather.place: expected a line of text")])
            );
        }

        // as typed into Settings
        assert_eq!(
            Location::parse(" 48.85 ,2.35 "),
            Ok(toml::Value::Array(vec![
                toml::Value::Float(48.85),
                toml::Value::Float(2.35)
            ]))
        );
        assert!(Location::parse("52,52, 13,41").is_err());
        assert_eq!(
            Location::parse("-91, 0"),
            Err(String::from("latitude -91 is outside -90 to 90"))
        );
    }

    #[test]
    fn a_table_key_given_a_value_says_so() {
        let (_, problems) = one("timings = 5\nclock = { a = 1 }\nmodules = true");

        assert_eq!(
            problems,
            said(&[
                (1, "timings: expected a table"),
                (2, "clock: expected \"24h\" or \"12h\""),
                (3, "modules: expected a table"),
            ])
        );
    }

    // TOML is all or nothing per file, so a broken one is skipped and the others still apply
    #[test]
    fn a_file_that_is_not_toml_is_skipped() {
        let broken = "[timings]\nexpand = 200\nhover = fast\n";

        let (config, problems) = layers(&["clock = \"12h\"", broken], MIGRATIONS);

        assert_eq!(config.clock, Hours::Twelve);
        assert_eq!(config.island.expand, Timings::default().expand);
        assert_eq!(problems[1].len(), 1);
        assert_eq!(problems[1][0].0, 3);
        assert!(
            problems[1][0]
                .1
                .starts_with("not valid TOML, skipped this file: ")
        );

        let (_, problems) = one("[modules]\nmedia = false\nmedia = true");
        assert_eq!(problems[0].0, 3);
    }

    // a fake history: version 1 called hover `peek`, version 2 kept timings under `motion`
    #[test]
    fn older_files_migrate_before_they_apply() {
        let migrations: &[Migration] = &[
            |table| {
                if let Some(Node::Table(timings)) = table.get_mut("timings").map(|e| &mut e.node)
                    && let Some(peek) = timings.remove("peek")
                {
                    timings.insert(String::from("hover"), peek);
                }
            },
            |table| {
                if let Some(motion) = table.remove("motion") {
                    table.insert(String::from("timings"), motion);
                }
            },
        ];

        let v1 = "[timings]\npeek = 90";
        let v2 = "schema_version = 2\n[motion]\nexpand = 200\ncollapse = 0";
        let v3 = "schema_version = 3\n[timings]\ngrace = 300";

        let (config, problems) = layers(&[v1, v2, v3], migrations);

        assert_eq!(config.island.hover, ms(90));
        assert_eq!(config.island.expand, ms(200));
        assert_eq!(config.island.grace, ms(300));

        // a migrated key keeps the line it was written on
        assert_eq!(
            problems,
            [
                vec![],
                said(&[(4, "timings.collapse: 0 is outside 1-60000 ms")]),
                vec![],
            ]
        );
    }

    /*
     * a version this build cannot migrate from skips the file, so a newer build's settings.toml
     * never applies over the config below it after a downgrade
     */
    #[test]
    fn an_unusable_schema_version_skips_the_file() {
        let current = MIGRATIONS.len() + 1;
        let newer = format!("schema_version = {}\nclock = \"24h\"", current + 1);

        for (upper, problem) in [
            (
                newer.as_str(),
                format!(
                    "schema_version {} is newer than this Kanade reads ({current}), skipped this file",
                    current + 1
                ),
            ),
            (
                "clock = \"24h\"\nschema_version = \"two\"\ntimings.hover = 0",
                format!(
                    "schema_version: expected 1 to {current}, found \"two\", skipped this file"
                ),
            ),
            (
                "schema_version = 0\nclock = \"24h\"",
                format!("schema_version: expected 1 to {current}, found 0, skipped this file"),
            ),
            (
                "schema_version = -1\nclock = \"24h\"",
                format!("schema_version: expected 1 to {current}, found -1, skipped this file"),
            ),
        ] {
            let (config, problems) = layers(&["clock = \"12h\"", upper], MIGRATIONS);
            let line = upper
                .lines()
                .position(|line| line.starts_with("schema"))
                .unwrap()
                + 1;

            assert_eq!(config.clock, Hours::Twelve, "{upper}");
            assert_eq!(problems, [vec![], vec![(line, problem)]]);
        }
    }

    // a valid v1 file with the key `timings.toast` dropped still applies, so live reload keeps working
    #[test]
    fn a_v1_toast_timing_migrates_away() {
        for text in [
            "schema_version = 1\n[timings]\ntoast = 5000\nhover = 100",
            "[timings]\ntoast = 5000\nhover = 100",
            "timings.toast = 5000\ntimings.hover = 100",
        ] {
            let (config, problems) = one(text);

            assert_eq!(problems, vec![], "{text}");
            assert_eq!(config.island.hover, ms(100), "{text}");
        }

        let (_, problems) = one("schema_version = 2\n[timings]\ntoast = 5000");
        assert_eq!(problems, said(&[(3, "unknown key timings.toast")]));
    }

    // `privacy.style` went with the dots (ADR 0033): an older file with it applies without a complaint
    #[test]
    fn an_older_privacy_style_migrates_away() {
        for text in [
            "privacy.style = \"dots\"\nprivacy.indicators = false",
            "schema_version = 2\n[privacy]\nstyle = \"dots\"\nindicators = false",
        ] {
            let (config, problems) = one(text);

            assert_eq!(problems, vec![], "{text}");
            assert!(!config.privacy_indicators, "{text}");
        }

        let (_, problems) = one("schema_version = 3\nprivacy.style = \"dots\"");
        assert_eq!(problems, said(&[(2, "unknown key privacy.style")]));
    }

    #[test]
    fn this_build_reads_its_own_version() {
        let (_, problems) = one(&format!("schema_version = {SCHEMA_VERSION}"));

        assert_eq!(problems, vec![]);
    }

    // every key a Module owns, once, and no two where one is a table of the other
    #[test]
    fn the_registry_names_each_setting_once() {
        let keys: Vec<&str> = settings().map(|setting| setting.key).collect();

        for (at, key) in keys.iter().enumerate() {
            for other in &keys[..at] {
                assert_ne!(key, other, "{key} twice");
                assert!(
                    !key.starts_with(&format!("{other}."))
                        && !other.starts_with(&format!("{key}.")),
                    "{key} and {other} overlap"
                );
            }

            assert!(
                !key.starts_with("modules.")
                    && *key != "schema_version"
                    && *key != "output"
                    && !key.starts_with("output.")
            );
        }
    }

    #[test]
    fn the_config_directory_reads_in_name_order() {
        let dir = env::temp_dir().join(format!("kanade-config-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        fs::create_dir_all(dir.join("nested.toml")).unwrap();
        for name in ["b.toml", "a.toml", "10.toml", ".hidden.toml", "notes.txt"] {
            fs::write(dir.join(name), "").unwrap();
        }

        let names: Vec<_> = tomls(&dir, &mut Vec::new())
            .iter()
            .map(|path| path.file_name().unwrap().to_str().unwrap().to_owned())
            .collect();

        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(names, ["10.toml", "a.toml", "b.toml"]);
        assert_eq!(tomls(&dir, &mut Vec::new()), Vec::<PathBuf>::new());
    }

    // without a home, `~` stays as written; a path that is not under `~/` is left alone
    #[test]
    fn only_a_leading_tilde_slash_is_home() {
        assert_eq!(expand("~/a.png", Some("/home/you/")), "/home/you/a.png");
        assert_eq!(expand("~/a.png", None), "~/a.png");
        assert_eq!(expand("/srv/~/a.png", Some("/home/you")), "/srv/~/a.png");
        assert_eq!(expand("~a.png", Some("/home/you")), "~a.png");
    }

    #[test]
    fn the_environment_overrides_reduced_motion_both_ways() {
        assert_eq!(motion(None), None);
        assert_eq!(motion(Some(OsStr::new(""))), None);
        assert_eq!(motion(Some(OsStr::new("0"))), Some(Mode::Spring));
        assert_eq!(motion(Some(OsStr::new("1"))), Some(Mode::Reduced));
        assert_eq!(motion(Some(OsStr::new("yes"))), Some(Mode::Reduced));
    }
}
