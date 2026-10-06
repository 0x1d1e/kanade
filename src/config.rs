//! The config file (#39): `$XDG_CONFIG_HOME/kanade/config.toml`, else `~/.config/kanade/config.toml`.
//! Read once at start, every key optional. A missing file is the defaults; a line that does not
//! parse or a value out of range keeps its default and says so on stderr, so a typo never stops
//! the shell. `amane dev` builds against `amane` and std only, so this reads a TOML subset itself:
//! `[section]`, `key = value` with integers, booleans and "strings", and `#` comments.
//!
//! ```toml
//! reduced_motion = false
//!
//! [timings]   # milliseconds
//! hover = 120
//! expand = 180
//! surface_change = 220
//! collapse = 180
//! grace = 250
//! osd = 1200
//! toast = 5000
//!
//! [theme]
//! palette = "~/Pictures/wallpaper.jpg"
//! ```

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use crate::island::motion::Mode;
use crate::island::service::Timings;

// plan 5.2: a level change shows for 1000-1400 ms, a toast for 4000-6000 ms
const OSD: Duration = Duration::from_millis(1200);
const TOAST: Duration = Duration::from_millis(5000);

// a timing outside this keeps its default: a spring needs some time, and a minute is no glance
const SHORTEST: u64 = 1;
const LONGEST: u64 = 60_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub island: Timings,

    // how long a Volume, Brightness or workspace Transient shows
    pub osd: Duration,

    // how long a notification shows as a Transient
    pub toast: Duration,

    // the image the theme is taken from, usually the wallpaper; none keeps the near-black body
    pub palette: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            island: Timings::default(),
            osd: OSD,
            toast: TOAST,
            palette: None,
        }
    }
}

// tests never read the user's file, so they see the defaults
static CONFIG: LazyLock<Config> = LazyLock::new(|| {
    if cfg!(test) {
        Config::default()
    } else {
        load()
    }
});

pub fn get() -> &'static Config {
    &CONFIG
}

fn load() -> Config {
    let home = env::var("HOME").ok();
    let path = env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.as_ref()
                .map(|home| PathBuf::from(home).join(".config"))
        })
        .map(|dir| dir.join("kanade/config.toml"));

    let mut config = path.map_or_else(Config::default, |path| {
        let (config, problems) = parse(&read(&path), home.as_deref());

        for problem in problems {
            eprintln!("kanade: {}:{problem}", path.display());
        }

        config
    });

    if let Some(motion) = motion(env::var_os("KANADE_REDUCED_MOTION").as_deref()) {
        config.island.motion = motion;
    }

    config
}

// no file is no config; one that will not read says so
fn read(path: &Path) -> String {
    match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            eprintln!(
                "kanade: {} unreadable, using the defaults: {error}",
                path.display()
            );
            String::new()
        }
    }
}

// KANADE_REDUCED_MOTION set and not empty wins over the file: 0 is off, anything else on
fn motion(reduced: Option<&OsStr>) -> Option<Mode> {
    match reduced {
        Some(value) if value.is_empty() => None,
        Some(value) if value == "0" => Some(Mode::Spring),
        Some(_) => Some(Mode::Reduced),
        None => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Integer(u64),
    Boolean(bool),
    String(String),
}

// the config and each problem as `line: what`, a line that has one changes nothing
fn parse(text: &str, home: Option<&str>) -> (Config, Vec<String>) {
    let mut config = Config::default();
    let mut problems = Vec::new();
    let mut section = String::new();

    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let mut problem = |what: String| problems.push(format!("{number}: {what}"));

        let line = match uncomment(line) {
            Ok(line) => line.trim(),
            Err(what) => {
                problem(what);
                continue;
            }
        };

        if line.is_empty() {
            continue;
        }

        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = name.trim().to_owned();

            if !matches!(section.as_str(), "timings" | "theme") {
                problem(format!("unknown section [{section}]"));
            }

            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            problem(format!("expected `key = value`, found `{line}`"));
            continue;
        };

        let key = key.trim();
        let value = match value_of(value.trim()) {
            Ok(value) => value,
            Err(what) => {
                problem(format!("{key}: {what}"));
                continue;
            }
        };

        if let Err(what) = set(&mut config, &section, key, value, home) {
            problem(what);
        }
    }

    (config, problems)
}

// the line up to a `#` that is not inside a string
fn uncomment(line: &str) -> Result<&str, String> {
    let mut quoted = false;
    let mut escaped = false;

    for (at, char) in line.char_indices() {
        match char {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '#' if !quoted => return Ok(&line[..at]),
            _ => {}
        }
    }

    if quoted {
        return Err(String::from("unterminated string"));
    }

    Ok(line)
}

fn value_of(text: &str) -> Result<Value, String> {
    match text {
        "true" => return Ok(Value::Boolean(true)),
        "false" => return Ok(Value::Boolean(false)),
        _ => {}
    }

    if let Some(inner) = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        && text.len() >= 2
    {
        return unescape(inner).map(Value::String);
    }

    // TOML allows `_` between digits
    let digits = text.replace('_', "");

    if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return digits
            .parse()
            .map(Value::Integer)
            .map_err(|_| format!("{text} is too large"));
    }

    Err(format!(
        "expected a number, true, false or a \"string\", found `{text}`"
    ))
}

fn unescape(inner: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = inner.chars();

    while let Some(char) = chars.next() {
        match char {
            '\\' => match chars.next() {
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                other => {
                    let shown = other.map_or(String::new(), String::from);

                    return Err(format!("unsupported escape \\{shown}"));
                }
            },
            '"' => return Err(String::from("unescaped \" inside a string")),
            _ => out.push(char),
        }
    }

    Ok(out)
}

fn set(
    config: &mut Config,
    section: &str,
    key: &str,
    value: Value,
    home: Option<&str>,
) -> Result<(), String> {
    let island = &mut config.island;
    let place = |section: &str| match section {
        "" => key.to_owned(),
        section => format!("{section}.{key}"),
    };

    match (section, key, value) {
        ("", "reduced_motion", Value::Boolean(reduced)) => {
            island.motion = if reduced { Mode::Reduced } else { Mode::Spring };
        }
        ("timings", key, Value::Integer(ms)) => {
            let slot = match key {
                "hover" => &mut island.hover,
                "expand" => &mut island.expand,
                "surface_change" => &mut island.surface_change,
                "collapse" => &mut island.collapse,
                "grace" => &mut island.grace,
                "osd" => &mut config.osd,
                "toast" => &mut config.toast,
                _ => return Err(format!("unknown key {}", place(section))),
            };

            if !(SHORTEST..=LONGEST).contains(&ms) {
                return Err(format!(
                    "{}: {ms} is outside {SHORTEST}-{LONGEST} ms",
                    place(section)
                ));
            }

            *slot = Duration::from_millis(ms);
        }
        ("theme", "palette", Value::String(path)) => {
            config.palette = Some(expand(&path, home));
        }
        ("", "reduced_motion", _) => {
            return Err(String::from("reduced_motion: expected true or false"));
        }
        ("timings", _, _) => return Err(format!("{}: expected milliseconds", place(section))),
        ("theme", "palette", _) => {
            return Err(String::from("theme.palette: expected a \"path\""));
        }
        _ => return Err(format!("unknown key {}", place(section))),
    }

    Ok(())
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

    #[test]
    fn an_empty_file_is_the_defaults() {
        assert_eq!(parse("", None), (Config::default(), vec![]));
        assert_eq!(parse("\n  # nothing\n", None), (Config::default(), vec![]));
    }

    #[test]
    fn every_key_sets_its_value() {
        let text = r#"
            reduced_motion = true   # trailing comment

            [timings]
            hover = 100
            expand = 200
            surface_change = 260
            collapse = 160
            grace = 300
            osd = 1_000
            toast = 6000

            [theme]
            palette = "~/Pictures/wall # 1.jpg"
        "#;

        let (config, problems) = parse(text, Some("/home/you"));

        assert_eq!(problems, Vec::<String>::new());
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
                toast: ms(6000),
                palette: Some(String::from("/home/you/Pictures/wall # 1.jpg")),
            }
        );
    }

    // a bad line keeps its default and names itself, the rest still applies
    #[test]
    fn a_bad_line_keeps_its_default_and_says_where() {
        let text = r#"
[timings]
hover = 0
expand = fast
grace = 300
toast = "5000"
speed = 3
[colors]
osd = 900
nonsense
[theme]
palette = 1
palette = "a\nb"
"#;

        let (config, problems) = parse(text, None);

        assert_eq!(config.island.hover, Timings::default().hover);
        assert_eq!(config.island.expand, Timings::default().expand);
        assert_eq!(config.island.grace, ms(300));
        assert_eq!(config.toast, TOAST);
        assert_eq!(config.osd, OSD);
        assert_eq!(config.palette, None);
        assert_eq!(
            problems,
            [
                "3: timings.hover: 0 is outside 1-60000 ms",
                "4: expand: expected a number, true, false or a \"string\", found `fast`",
                "6: timings.toast: expected milliseconds",
                "7: unknown key timings.speed",
                "8: unknown section [colors]",
                "9: unknown key colors.osd",
                "10: expected `key = value`, found `nonsense`",
                "12: theme.palette: expected a \"path\"",
                "13: palette: unsupported escape \\n",
            ]
        );
    }

    #[test]
    fn strings_keep_escaped_quotes_and_flag_open_ones() {
        let (config, problems) = parse("[theme]\npalette = \"a \\\"b\\\" \\\\c\"", None);
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(config.palette.as_deref(), Some("a \"b\" \\c"));

        let (_, problems) = parse("[theme]\npalette = \"open", None);
        assert_eq!(problems, ["2: unterminated string"]);
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
