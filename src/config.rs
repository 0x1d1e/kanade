//! The config (#39, #101, docs/design.md Config): TOML, read at start in layers, each over the
//! ones before it: the defaults, then every `*.toml` in `$XDG_CONFIG_HOME/kanade/` (else
//! `~/.config/kanade/`) in alphabetical order, then `$XDG_STATE_HOME/kanade/settings.toml` (else
//! `~/.local/state/kanade/settings.toml`), the layer the Settings app will own. Tables merge key by
//! key; any other value, a list too, replaces the one below. Kanade never writes these files.
//!
//! A key the registry marks per output may also be set in `[output."<name>"]`, for the monitor of
//! that name only (#147). An output's value wins over the global one from any layer, as the more
//! specific; output values layer like global ones. `config::on` gives the config in effect on one
//! output, resolved once per config, so a view never builds one while drawing.
//!
//! A file may name the layout it is written in with `schema_version`; without one it is the v0.1
//! layout, version 1. An older file is migrated in memory before it applies.
//!
//! Every key is optional, and each key is registered by the one Module that owns it, as a
//! `Setting`: its kind, default, help and whether it takes a restart are defined there once, and
//! reading, checking, `kanade config defaults` and the README's defaults follow from it. A file that
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
use std::sync::{Arc, LazyLock, Mutex, PoisonError, RwLock};
use std::time::Duration;

use serde::Deserialize;
use toml::Value;
use toml::de::{DeTable, DeValue, ValueDeserializer};

use crate::clock::Hours;
use crate::island::motion::Mode;
use crate::island::service::Timings;
use crate::modules;

// plan 5.2: a level change shows for 1000-1400 ms
const OSD: Duration = Duration::from_millis(1200);

// a timing outside this keeps its default: a spring needs some time, and a minute is no glance
const SHORTEST: u64 = 1;
const LONGEST: u64 = 60_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub island: Timings,

    // how long the OSD and a workspace switch on the island show
    pub osd: Duration,

    // the image the theme roles are taken from, usually the wallpaper; the Island stays black and white
    pub palette: Option<String>,

    // how the time reads at Rest
    pub clock: Hours,

    // the Modules turned off, each once
    pub off: Vec<&'static str>,

    // `windows.apps`: an app id to the `.desktop` file id its windows belong to (ADR 0014)
    pub apps: BTreeMap<String, String>,

    // `dock.pinned`: the `.desktop` file ids the Dock keeps, in its order
    pub pinned: Vec<String>,

    // `wallpaper.directory`: where the Launcher finds wallpapers, else ~/Pictures/Wallpapers
    pub wallpapers: Option<String>,

    // `output."<name>"`: each output's overrides of the keys marked per output, by output name
    pub outputs: BTreeMap<String, Output>,
}

impl Config {
    pub fn off(&self, module: &str) -> bool {
        self.off.contains(&module)
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
            osd: OSD,
            palette: None,
            clock: Hours::default(),
            off: Vec::new(),
            apps: BTreeMap::new(),
            pinned: Vec::new(),
            wallpapers: None,
            outputs: BTreeMap::new(),
        }
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

    // what it does, a line for the docs and Settings
    pub help: &'static str,

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

// what a key takes; each checks a value the one way, with one wording
pub enum Kind {
    // true or false
    Switch(Field<bool>),

    // whole milliseconds, SHORTEST to LONGEST
    Millis(Field<Duration>),

    // one of these strings
    Choice(&'static [&'static str], Field<&'static str>),

    // a file or directory, a leading `~/` the home directory; unset is none
    Path(Field<Option<String>>),

    // a list of `.desktop` file ids, the `.desktop` optional, each kept once; a layer replaces the
    // list below
    DesktopIds(Field<Vec<String>>),

    // a table of app id = `.desktop` file id, merged over the layers below by app id
    AppIds(Field<BTreeMap<String, String>>),

    // a table of Module name = true or false, merged by name; the Field holds the ones off
    Modules(Field<Vec<&'static str>>),
}

impl Kind {
    // stores a value, or says why it will not, keeping what was there; `home` expands a `~/`
    fn set(&self, config: &mut Config, value: &Value, home: Option<&str>) -> Result<(), String> {
        match self {
            Kind::Switch(field) => {
                (field.set)(config, value.as_bool().ok_or("expected true or false")?);
            }
            Kind::Millis(field) => (field.set)(config, millis(value)?),
            Kind::Choice(options, field) => {
                let expected = format!(
                    "expected {}",
                    options
                        .iter()
                        .map(|option| format!("\"{option}\""))
                        .collect::<Vec<_>>()
                        .join(" or ")
                );
                let found = value.as_str().ok_or_else(|| expected.clone())?;
                let option = options
                    .iter()
                    .find(|&&option| option == found)
                    .ok_or_else(|| format!("{expected}, found \"{found}\""))?;

                (field.set)(config, option);
            }
            Kind::Path(field) => {
                let path = value.as_str().ok_or("expected a \"path\"")?;
                (field.set)(config, Some(expand(path, home)));
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
            Kind::Modules(field) => {
                let table = value.as_table().ok_or("expected a table")?;
                let mut off = (field.get)(config);

                for (name, value) in table {
                    turn(&mut off, name, value)?;
                }

                (field.set)(config, off);
            }
        }

        Ok(())
    }

    // the value as TOML, none for one that sets nothing, like an empty list
    fn value(&self, config: &Config) -> Option<Value> {
        let value = match self {
            Kind::Switch(field) => Value::Boolean((field.get)(config)),
            Kind::Millis(field) => {
                Value::Integer(i64::try_from((field.get)(config).as_millis()).ok()?)
            }
            Kind::Choice(_, field) => Value::String((field.get)(config).to_owned()),
            Kind::Path(field) => Value::String((field.get)(config)?),
            Kind::DesktopIds(field) => {
                Value::Array((field.get)(config).into_iter().map(Value::String).collect())
            }
            Kind::AppIds(field) => Value::Table(
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
            Kind::Choice(_, field) => field.copy(from, to),
            Kind::Path(field) => field.copy(from, to),
            Kind::DesktopIds(field) => field.copy(from, to),
            Kind::AppIds(field) => field.copy(from, to),
            Kind::Modules(field) => field.copy(from, to),
        }
    }

    // a table of its own keys, which a file writes as `[key]`
    fn table(&self) -> bool {
        matches!(self, Kind::AppIds(_) | Kind::Modules(_))
    }
}

pub const ISLAND: &[Setting] = &[
    Setting {
        key: "reduced_motion",
        help: "the island snaps to its new shape and only fades its content, over 80 ms; \
               KANADE_REDUCED_MOTION=1 or 0 overrides it",
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
        help: "the time an idle island shows: \"24h\" (14:05) or \"12h\" (2:05 PM)",
        kind: Kind::Choice(
            &["24h", "12h"],
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
        help: "pointer resting on a Compact island before it peeks",
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
        help: "morph to a larger form",
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
        help: "one Surface replacing another, and a new track dissolving in",
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
        help: "morph to a smaller form",
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
        help: "pointer out before a Peek or Surface collapses",
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
        help: "the OSD, and a workspace switch on the island",
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
        help: "the image the theme roles beside the island take their tone from",
        kind: Kind::Path(Field {
            get: |config| config.palette.clone(),
            set: |config, palette| config.palette = palette,
        }),
        example: Some("\"~/Pictures/wallpaper.jpg\""),
        restart: false,
        per_output: false,
    },
    // the island's, since it cannot be turned off
    Setting {
        key: "modules",
        help: "every Module is on unless set to false here",
        kind: Kind::Modules(Field {
            get: |config| config.off.clone(),
            set: |config, off| config.off = off,
        }),
        example: Some("{ media = false }"),

        // Amane registers windows only at start
        restart: true,
        per_output: false,
    },
];

pub const WINDOWS: &[Setting] = &[Setting {
    key: "windows.apps",
    help: "app id = the .desktop file it belongs to, where Kanade's guess is wrong",
    kind: Kind::AppIds(Field {
        get: |config| config.apps.clone(),
        set: |config, apps| config.apps = apps,
    }),
    example: Some("{ jetbrains-idea = \"intellij-idea-ultimate-edition\" }"),
    restart: false,
    per_output: false,
}];

pub const DOCK: &[Setting] = &[Setting {
    key: "dock.pinned",
    help: ".desktop file ids, in the Dock's order",
    kind: Kind::DesktopIds(Field {
        get: |config| config.pinned.clone(),
        set: |config, pinned| config.pinned = pinned,
    }),
    example: Some("[\"firefox\", \"kitty\"]"),
    restart: false,
    per_output: false,
}];

pub const WALLPAPER: &[Setting] = &[Setting {
    key: "wallpaper.directory",
    help: "the images the Launcher offers after `@`, else ~/Pictures/Wallpapers",
    kind: Kind::Path(Field {
        get: |config| config.wallpapers.clone(),
        set: |config, wallpapers| config.wallpapers = wallpapers,
    }),
    example: Some("\"~/Pictures/Wallpapers\""),
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
 * with an example: what `kanade config defaults` prints and the README shows
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

        if let Kind::Millis(_) = setting.kind {
            help.push_str(&format!(", ms {SHORTEST}-{LONGEST}"));
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

        *SKIPPED.lock().unwrap_or_else(PoisonError::into_inner) = problems;
        config
    };

    RwLock::new(Effect::new(config))
});

// what the config read at start skipped; a config a reload applies skipped nothing
static SKIPPED: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn skipped() -> Vec<String> {
    LazyLock::force(&CURRENT);

    SKIPPED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

// the config in effect; a reload swaps it whole, so one read never mixes two
pub fn get() -> Arc<Config> {
    CURRENT
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .config
        .clone()
}

// the config in effect on one output, its overrides over the global one, as `Config::on` gives it
pub fn on(output: &str) -> Arc<Config> {
    let effect = CURRENT.read().unwrap_or_else(PoisonError::into_inner);

    effect.outputs.get(output).unwrap_or(&effect.config).clone()
}

// a reload's config replaces the one in effect (`crate::reload`)
pub fn install(config: Config) {
    *CURRENT.write().unwrap_or_else(PoisonError::into_inner) = Effect::new(config);
}

/*
 * every layer read over the defaults, and what did not apply as `file:line: what`, in layer order;
 * at start each problem is skipped alone, a reload refuses the lot
 */
pub fn read() -> (Config, Vec<String>) {
    let home = env::var("HOME").ok();
    let mut config = Config::default();
    let mut problems = Vec::new();

    for path in files(home.as_deref(), &mut problems) {
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

    if let Some(motion) = motion(env::var_os("KANADE_REDUCED_MOTION").as_deref()) {
        config.island.motion = motion;
    }

    (config, problems)
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
        .chain(
            base("XDG_STATE_HOME", ".local/state", home)
                .map(|dir| Place::File(dir.join("kanade/settings.toml"))),
        )
        .collect()
}

// a file in the config directory that is a layer: `*.toml`, not hidden
pub fn layer_name(name: &OsStr) -> bool {
    !name.as_encoded_bytes().starts_with(b".")
        && Path::new(name).extension() == Some(OsStr::new("toml"))
}

// every layer's file, in the order they apply
fn files(home: Option<&str>, problems: &mut Vec<String>) -> Vec<PathBuf> {
    let mut files = base("XDG_CONFIG_HOME", ".config", home)
        .map(|dir| tomls(&dir.join("kanade"), problems))
        .unwrap_or_default();

    files.extend(
        base("XDG_STATE_HOME", ".local/state", home).map(|dir| dir.join("kanade/settings.toml")),
    );

    files
}

// an XDG base directory: the variable if set to an absolute path, else its default under home
fn base(variable: &str, default: &str, home: Option<&str>) -> Option<PathBuf> {
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
            Node::Table(table) => Value::Table(
                table
                    .iter()
                    .map(|(key, entry)| (key.clone(), entry.node.value()))
                    .collect(),
            ),
            Node::Value(value) => value.clone(),
        }
    }
}

// takes a file from the layout at its index plus one to the next, before it applies
type Migration = fn(&mut Table);

// the current layout is the last one migrated to
const MIGRATIONS: &[Migration] = &[
    // 2: notifications show as Banners, with no `timings.toast` (#109)
    |table| {
        if let Some(Node::Table(timings)) = table.get_mut("timings").map(|entry| &mut entry.node) {
            timings.remove("toast");
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

fn settings() -> impl Iterator<Item = &'static Setting> {
    modules::ALL.iter().flat_map(|module| module.settings)
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
            None => problems.push((entry.line, format!("unknown key {shown}"))),
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
                },
                osd: ms(1000),
                palette: Some(String::from("/home/you/Pictures/wall # 1.jpg")),
                clock: Hours::Twelve,
                off: vec!["media"],
                apps: BTreeMap::new(),
                pinned: Vec::new(),
                wallpapers: None,
                outputs: BTreeMap::new(),
            }
        );
        assert!(config.off("media"));
        assert!(!config.off("timer"));

        assert_eq!(one(&defaults()), (Config::default(), vec![]));
    }

    // the README shows what the registry says, so the docs cannot drift from the keys
    #[test]
    fn the_readme_shows_the_defaults() {
        let readme = include_str!("../README.md");
        let start = readme.find("```toml\n").expect("README shows the config") + 8;
        let shown = &readme[start..][..readme[start..].find("```").unwrap()];

        assert_eq!(
            shown,
            defaults(),
            "paste `cargo run -- config defaults` into the README"
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
                "modules",
                "windows.apps",
                "dock.pinned",
                "wallpaper.directory",
            ]
        );
        assert_eq!(
            settings()
                .filter(|setting| setting.restart)
                .map(|setting| setting.key)
                .collect::<Vec<_>>(),
            ["modules"]
        );
        assert_eq!(
            settings()
                .filter(|setting| setting.per_output)
                .map(|setting| setting.key)
                .collect::<Vec<_>>(),
            ["clock"]
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
        assert_eq!(next.off, ["media", "timer"]);
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

    // a key no output may set, or a bad value, keeps what the layers below gave that output
    #[test]
    fn a_bad_output_key_keeps_what_is_below_and_says_where() {
        let first = "[output.eDP-1]\nclock = \"12h\"";
        let second = r#"
[output.eDP-1]
clock = "13h"
reduced_motion = true
timings.hover = 90
weather = 1
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
                (6, "unknown key output.eDP-1.weather"),
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
        assert_eq!(config.off, ["media"]);
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
        assert_eq!(config.off, ["media", "battery"]);
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
weather = false
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
        assert_eq!(config.off, ["battery"]);
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
                (15, "unknown module modules.weather"),
                (16, "modules.media: expected true or false"),
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

    // a valid v1 file with the key #109 dropped still applies, so live reload keeps working
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
