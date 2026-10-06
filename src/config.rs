//! The config (#39, #101, docs/design.md Config): TOML, read at start in layers, each over the
//! ones before it: the defaults, then every `*.toml` in `$XDG_CONFIG_HOME/kanade/` (else
//! `~/.config/kanade/`) in alphabetical order, then `$XDG_STATE_HOME/kanade/settings.toml` (else
//! `~/.local/state/kanade/settings.toml`), the layer the Settings app will own. Tables merge key by
//! key; any other value, a list too, replaces the one below. Kanade never writes these files.
//!
//! A file may name the layout it is written in with `schema_version`; without one it is the v0.1
//! layout, version 1. An older file is migrated in memory before it applies.
//!
//! Every key is optional, and each key is registered by the one Module that owns it. A file that
//! is not TOML is skipped whole; a key that is unknown or a value out of range is skipped alone,
//! keeping what the layers below gave it. Either says so on stderr with file and line, so a typo
//! never stops the shell. A reload while running (`crate::reload`) is stricter: any problem keeps
//! the config in effect whole.
//!
//! ```toml
//! reduced_motion = false
//! clock = "24h"   # or "12h"
//!
//! [timings]   # milliseconds
//! hover = 120
//! expand = 180
//! surface_change = 220
//! collapse = 180
//! grace = 250
//! osd = 1200
//!
//! [theme]
//! palette = "~/Pictures/wallpaper.jpg"
//!
//! [modules]   # every Module is on unless turned off here; `island` cannot be
//! media = false
//! ```

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
}

impl Config {
    pub fn off(&self, module: &str) -> bool {
        self.off.contains(&module)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            island: Timings::default(),
            osd: OSD,
            palette: None,
            clock: Hours::default(),
            off: Vec::new(),
        }
    }
}

// one key a Module owns, which the registry lists with the Module; others may read it too
pub struct Setting {
    // dotted: `timings.hover` is `hover` in `[timings]`
    pub key: &'static str,

    // stores the value, or says why it will not, keeping what was there; `home` expands a `~/`
    pub set: fn(config: &mut Config, value: &Value, home: Option<&str>) -> Result<(), String>,
}

pub const ISLAND: &[Setting] = &[
    Setting {
        key: "reduced_motion",
        set: |config, value, _| {
            let reduced = value.as_bool().ok_or("expected true or false")?;
            config.island.motion = if reduced { Mode::Reduced } else { Mode::Spring };
            Ok(())
        },
    },
    Setting {
        key: "clock",
        set: |config, value, _| {
            config.clock = match value.as_str() {
                Some("24h") => Hours::TwentyFour,
                Some("12h") => Hours::Twelve,
                Some(other) => {
                    return Err(format!("expected \"24h\" or \"12h\", found \"{other}\""));
                }
                None => return Err(String::from("expected \"24h\" or \"12h\"")),
            };
            Ok(())
        },
    },
    Setting {
        key: "timings.hover",
        set: |config, value, _| {
            config.island.hover = millis(value)?;
            Ok(())
        },
    },
    Setting {
        key: "timings.expand",
        set: |config, value, _| {
            config.island.expand = millis(value)?;
            Ok(())
        },
    },
    Setting {
        key: "timings.surface_change",
        set: |config, value, _| {
            config.island.surface_change = millis(value)?;
            Ok(())
        },
    },
    Setting {
        key: "timings.collapse",
        set: |config, value, _| {
            config.island.collapse = millis(value)?;
            Ok(())
        },
    },
    Setting {
        key: "timings.grace",
        set: |config, value, _| {
            config.island.grace = millis(value)?;
            Ok(())
        },
    },
    // the island's, since `workspace` reads it as well as `osd`
    Setting {
        key: "timings.osd",
        set: |config, value, _| {
            config.osd = millis(value)?;
            Ok(())
        },
    },
    Setting {
        key: "theme.palette",
        set: |config, value, home| {
            let path = value.as_str().ok_or("expected a \"path\"")?;
            config.palette = Some(expand(path, home));
            Ok(())
        },
    },
];

fn millis(value: &Value) -> Result<Duration, String> {
    let ms = value.as_integer().ok_or("expected milliseconds")?;

    u64::try_from(ms)
        .ok()
        .filter(|ms| (SHORTEST..=LONGEST).contains(ms))
        .map(Duration::from_millis)
        .ok_or_else(|| format!("{ms} is outside {SHORTEST}-{LONGEST} ms"))
}

// tests never read the user's files, so they see the defaults
static CURRENT: LazyLock<RwLock<Arc<Config>>> = LazyLock::new(|| {
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

    RwLock::new(Arc::new(config))
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
        .clone()
}

// a reload's config replaces the one in effect (`crate::reload`)
pub fn install(config: Config) {
    *CURRENT.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(config);
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
                Ok(()) => apply(config, &table, "", home, &mut problems),
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

// sets each key of a table at `prefix` that a Module owns, and names every other
fn apply(
    config: &mut Config,
    table: &Table,
    prefix: &str,
    home: Option<&str>,
    problems: &mut Vec<(usize, String)>,
) {
    for (key, entry) in table {
        let path = match prefix {
            "" => key.clone(),
            prefix => format!("{prefix}.{key}"),
        };

        let inside = |setting: &Setting| {
            setting
                .key
                .strip_prefix(path.as_str())
                .is_some_and(|rest| rest.starts_with('.'))
        };

        if path == "modules" {
            turn(config, entry, problems);
        } else if let Some(setting) = settings().find(|setting| setting.key == path) {
            if let Err(what) = (setting.set)(config, &entry.node.value(), home) {
                problems.push((entry.line, format!("{path}: {what}")));
            }
        } else if settings().any(inside) {
            match &entry.node {
                Node::Table(table) => apply(config, table, &path, home, problems),
                Node::Value(_) => problems.push((entry.line, format!("{path}: expected a table"))),
            }
        } else {
            problems.push((entry.line, format!("unknown key {path}")));
        }
    }
}

// `[modules]`: each Module in the registry on or off by name
fn turn(config: &mut Config, entry: &Entry, problems: &mut Vec<(usize, String)>) {
    let Node::Table(table) = &entry.node else {
        problems.push((entry.line, String::from("modules: expected a table")));
        return;
    };

    for (key, entry) in table {
        let module = modules::ALL.iter().find(|module| module.name == key);

        let problem = match (module, &entry.node) {
            (None, _) => format!("unknown module modules.{key}"),
            (Some(module), Node::Value(Value::Boolean(false))) if module.name == modules::CORE => {
                format!("modules.{key}: the core module cannot be turned off")
            }
            (Some(module), Node::Value(Value::Boolean(on))) => {
                config.off.retain(|&name| name != module.name);

                if !on {
                    config.off.push(module.name);
                }

                continue;
            }
            (Some(_), _) => format!("modules.{key}: expected true or false"),
        };

        problems.push((entry.line, problem));
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
            }
        );
        assert!(config.off("media"));
        assert!(!config.off("timer"));

        let readme = include_str!("../README.md");
        let start = readme.find("```toml\n").expect("README shows the config") + 8;
        let example = &readme[start..][..readme[start..].find("```").unwrap()];

        assert_eq!(one(example), (Config::default(), vec![]));
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

            assert!(!key.starts_with("modules.") && *key != "schema_version");
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
