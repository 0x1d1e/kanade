//! Matugen palettes and configured application templates, kept off the draw and reload threads.

use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::thread::Builder;
use std::time::Duration;

use kanade_runtime::{Color, MutexExt, service::Service};
use serde_json::Value as Json;
use toml::Value as Toml;

use super::adapters::Scheme;
use super::child::{self, Interrupt, Ran};
use crate::look::Tone;
use crate::theme::{self, ThemeRoles, tint};

const HELPER_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq)]
struct Palettes {
    dark: ThemeRoles,
    light: ThemeRoles,

    // the Island's tinted ramp, built once from the source color rather than every frame
    island: ThemeRoles,
}

#[derive(Debug, Default)]
struct Generated {
    palettes: Option<Palettes>,
}

impl Service for Generated {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {}
}

#[derive(Debug)]
struct Job {
    serial: u64,
    path: PathBuf,
    tone: Tone,
}

type Run<T> = Ran<Job, T>;

#[derive(Debug, Default)]
struct Requests {
    current: Option<PathBuf>,
    serial: u64,
}

impl Requests {
    fn wallpaper(&mut self, path: PathBuf, tone: impl FnOnce() -> Tone) -> Job {
        self.current = Some(path.clone());
        self.job(path, tone())
    }

    fn seed_wallpaper(&mut self, path: PathBuf, tone: impl FnOnce() -> Tone) -> Option<Job> {
        if self.current.is_some() {
            return None;
        }

        Some(self.wallpaper(path, tone))
    }

    fn shown_wallpaper(&mut self, path: PathBuf, tone: impl FnOnce() -> Tone) -> Option<Job> {
        if self.current.as_ref() == Some(&path) {
            return None;
        }

        Some(self.wallpaper(path, tone))
    }

    fn tone_changed(&mut self, tone: Tone) -> Option<Job> {
        let path = self.current.clone()?;
        Some(self.job(path, tone))
    }

    fn job(&mut self, path: PathBuf, tone: Tone) -> Job {
        self.serial += 1;
        Job {
            serial: self.serial,
            path,
            tone,
        }
    }
}

static REQUESTS: Mutex<Requests> = Mutex::new(Requests {
    current: None,
    serial: 0,
});
static TEMP: AtomicU64 = AtomicU64::new(0);
static WORKER: OnceLock<Option<Sender<Job>>> = OnceLock::new();
static FALLBACK: LazyLock<Toml> = LazyLock::new(empty_config);

pub fn roles(tone: Tone) -> ThemeRoles {
    let generated = Generated::read();

    generated
        .palettes
        .map(|palettes| palettes.for_tone(tone))
        .unwrap_or_else(|| match tone {
            Tone::Dark => theme::ISLAND_DARK,
            Tone::Light => theme::ISLAND_LIGHT,
        })
}

// the tinted solid ramp of the wallpaper's source color; slate while there is none (ADR 0050)
pub fn island_roles() -> ThemeRoles {
    static SLATE: LazyLock<ThemeRoles> = LazyLock::new(|| tint::roles(tint::NEUTRAL));

    Generated::read()
        .palettes
        .map_or(*SLATE, |palettes| palettes.island)
}

pub fn tone_changed(tone: Tone) {
    let mut requests = REQUESTS.locked();
    if let Some(job) = requests.tone_changed(tone) {
        submit(job);
    }
}

pub fn wallpaper(path: PathBuf) {
    let mut requests = REQUESTS.locked();
    let job = requests.wallpaper(path, || crate::config::get().appearance.tone);
    submit(job);
}

pub fn seed_wallpaper(path: PathBuf) {
    let mut requests = REQUESTS.locked();
    if let Some(job) = requests.seed_wallpaper(path, || crate::config::get().appearance.tone) {
        submit(job);
    }
}

// a wallpaper something outside Kanade showed, replacing the palette unless it is the same image
pub fn shown_wallpaper(path: PathBuf) {
    let mut requests = REQUESTS.locked();
    if let Some(job) = requests.shown_wallpaper(path, || crate::config::get().appearance.tone) {
        submit(job);
    }
}

fn submit(job: Job) {
    let worker = WORKER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        match Builder::new()
            .name(String::from("kanade-matugen"))
            .spawn(move || follow(receiver))
        {
            Ok(_) => Some(sender),
            Err(error) => {
                eprintln!("kanade: could not start Matugen worker: {error}");
                None
            }
        }
    });

    if let Some(sender) = worker
        && let Err(error) = sender.send(job)
    {
        eprintln!("kanade: could not queue Matugen update: {error}");
    }
}

fn follow(receiver: Receiver<Job>) {
    follow_with(receiver, generate, |serial, result| match result {
        Ok(generation) => {
            if install(serial, Some(generation.palettes)) {
                super::adapters::colors(generation.scheme);
            }
        }
        Err(error) => failed(serial, error),
    });
}

fn follow_with<T>(
    receiver: Receiver<Job>,
    mut generate: impl FnMut(&Job, &Receiver<Job>) -> Run<T>,
    mut finish: impl FnMut(u64, Result<T, String>),
) {
    let mut pending = None;

    loop {
        let Some(job) = pending.take().or_else(|| receiver.recv().ok()) else {
            return;
        };
        let job = newest(job, &receiver);

        match generate(&job, &receiver) {
            Run::Done(result) => finish(job.serial, result),
            Run::Superseded(newer) => pending = Some(newer),
        }
    }
}

fn newest(mut job: Job, receiver: &Receiver<Job>) -> Job {
    while let Ok(candidate) = receiver.try_recv() {
        if candidate.serial > job.serial {
            job = candidate;
        }
    }

    job
}

fn failed(serial: u64, error: String) {
    let current = REQUESTS.locked().serial;
    if current == serial {
        eprintln!("kanade: could not derive the wallpaper palette with Matugen: {error}");
        install(serial, None);
    }
}

// whether `serial` is still the latest request, its palettes then the shell's
fn install(serial: u64, palettes: Option<Palettes>) -> bool {
    let requests = REQUESTS.locked();
    if requests.serial != serial {
        return false;
    }

    if Generated::read().palettes != palettes {
        Generated::write().palettes = palettes;
    }

    true
}

// the shell's palettes and every role Matugen gave, for Kanade's own application targets
struct Generation {
    palettes: Palettes,
    scheme: Scheme,
}

fn generate(job: &Job, receiver: &Receiver<Job>) -> Run<Generation> {
    let path = &job.path;
    if !path.is_file() {
        return Run::Done(Err(format!("no wallpaper image at {}", path.display())));
    }

    let config = match safe_config() {
        Ok(config) => config,
        Err(error) => return Run::Done(Err(error)),
    };

    if let Some(warning) = config.warning.as_deref() {
        eprintln!("kanade: {warning}");
    }

    let mut command = Command::new(super::MATUGEN);
    command
        .arg("image")
        .arg(path)
        .args([
            "--mode",
            job.tone.name(),
            "--source-color-index",
            "0",
            "--json",
            "hex",
            "--quiet",
            "--config",
        ])
        .arg(&config.file.0)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    match run(command, HELPER_TIMEOUT, job.serial, receiver) {
        Run::Done(Ok(output)) => Run::Done(parse(&output).and_then(|palettes| {
            Ok(Generation {
                palettes,
                scheme: parse_scheme(&output, path)?,
            })
        })),
        Run::Done(Err(error)) => Run::Done(Err(error)),
        Run::Superseded(newer) => Run::Superseded(newer),
    }
}

struct SafeConfig {
    file: TempConfig,
    warning: Option<String>,
}

struct TempConfig(PathBuf);

impl Drop for TempConfig {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn safe_config() -> Result<SafeConfig, String> {
    let path = config_path();
    let (mut document, base, mut warning) = match path {
        Some(path) => match fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Toml>(&text) {
                Ok(document) => {
                    let base = fs::canonicalize(&path)
                        .ok()
                        .and_then(|path| path.parent().map(Path::to_path_buf))
                        .or_else(|| path.parent().map(Path::to_path_buf));
                    (document, base, None)
                }
                Err(error) => (
                    FALLBACK.clone(),
                    None,
                    Some(format!(
                        "Matugen config is invalid; using its default palette without templates: {error}"
                    )),
                ),
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => (FALLBACK.clone(), None, None),
            Err(error) => (
                FALLBACK.clone(),
                None,
                Some(format!(
                    "cannot read Matugen config; using its default palette without templates: {error}"
                )),
            ),
        },
        None => (FALLBACK.clone(), None, None),
    };

    let targets = if let Some(base) = base {
        match sanitize(&mut document, &base) {
            Ok(targets) => targets,
            Err(error) => {
                warning = Some(format!(
                    "cannot safely use Matugen templates; using its default palette: {error}"
                ));
                document = FALLBACK.clone();
                false
            }
        }
    } else {
        false
    };

    terminal_colors(&mut document);

    let contents = toml::to_string(&document)
        .map_err(|error| format!("cannot serialize safe Matugen config: {error}"))?;
    let file = temporary_config(&contents)
        .map_err(|error| format!("cannot create safe Matugen config: {error}"))?;

    if targets && warning.is_some() {
        warning = None;
    }

    Ok(SafeConfig { file, warning })
}

fn config_path() -> Option<PathBuf> {
    let config = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;

    Some(config.join("matugen/config.toml"))
}

fn sanitize(document: &mut Toml, base: &Path) -> Result<bool, String> {
    let root = document
        .as_table_mut()
        .ok_or_else(|| String::from("Matugen config root is not a table"))?;

    if !root.contains_key("templates") {
        root.insert(String::from("templates"), Toml::Table(Default::default()));
    }

    let templates = root
        .get_mut("templates")
        .and_then(Toml::as_table_mut)
        .ok_or_else(|| String::from("Matugen templates is not a table"))?;
    let mut has_targets = false;

    for (_, template) in templates.iter_mut() {
        let template = template
            .as_table_mut()
            .ok_or_else(|| String::from("Matugen template is not a table"))?;
        template.remove("pre_hook");
        template.remove("post_hook");

        for key in ["input_path", "output_path"] {
            if let Some(path) = template.get_mut(key) {
                rebase_paths(path, base)?;
            }
        }

        if let Some(modes) = template
            .get_mut("input_path_modes")
            .and_then(Toml::as_table_mut)
        {
            for (_, path) in modes.iter_mut() {
                rebase_paths(path, base)?;
            }
        }

        if template.get("enabled").and_then(Toml::as_bool) != Some(false)
            && template.contains_key("output_path")
        {
            has_targets = true;
        }
    }

    let config = root
        .entry(String::from("config"))
        .or_insert_with(|| Toml::Table(Default::default()))
        .as_table_mut()
        .ok_or_else(|| String::from("Matugen config section is not a table"))?;

    if let Some(imports) = config.get_mut("import_json_files") {
        rebase_paths(imports, base)?;
    }

    config.insert(String::from("version_check"), Toml::Boolean(false));
    config.insert(String::from("caching"), Toml::Boolean(false));
    config.insert(
        String::from("wallpaper"),
        Toml::Table(
            [
                (String::from("set"), Toml::Boolean(false)),
                (String::from("command"), Toml::String(String::from("true"))),
            ]
            .into_iter()
            .collect(),
        ),
    );

    Ok(has_targets)
}

// the ANSI hues, as Matugen custom colors harmonized toward the wallpaper, each with a tone for
// dark and light (`kanade_red`, `on_kanade_red_container`, ...), for the terminal targets; a
// custom_colors that is not a table is left alone and the terminals keep their colors
fn terminal_colors(document: &mut Toml) {
    let Some(colors) = document
        .as_table_mut()
        .and_then(|root| root.get_mut("config"))
        .and_then(Toml::as_table_mut)
        .map(|config| {
            config
                .entry(String::from("custom_colors"))
                .or_insert_with(|| Toml::Table(Default::default()))
        })
        .and_then(Toml::as_table_mut)
    else {
        return;
    };

    for (name, color) in theme::TERMINAL_HUES {
        let custom = [
            (String::from("color"), Toml::String(color.to_owned())),
            (String::from("blend"), Toml::Boolean(true)),
        ];
        colors.insert(
            format!("kanade_{name}"),
            Toml::Table(custom.into_iter().collect()),
        );
    }
}

fn rebase_paths(value: &mut Toml, base: &Path) -> Result<(), String> {
    if let Some(path) = value.as_str() {
        *value = Toml::String(absolute(path, base)?);
    } else if let Some(paths) = value.as_array_mut() {
        for path in paths {
            rebase_paths(path, base)?;
        }
    }

    Ok(())
}

fn absolute(path: &str, base: &Path) -> Result<String, String> {
    let path = if path == "~" {
        env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
            String::from("HOME is unset, so a Matugen path beginning with ~ cannot be resolved")
        })?
    } else if let Some(path) = path.strip_prefix("~/") {
        env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| {
                String::from(
                    "HOME is unset, so a Matugen path beginning with ~/ cannot be resolved",
                )
            })?
            .join(path)
    } else {
        PathBuf::from(path)
    };

    let path = if path.is_absolute() {
        path
    } else {
        base.join(path)
    };

    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| String::from("a Matugen path is not valid UTF-8"))
}

fn temporary_config(contents: &str) -> io::Result<TempConfig> {
    for _ in 0..32 {
        let serial = TEMP.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "kanade-matugen-{}-{serial}.toml",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);

        match options.open(&path) {
            Ok(mut file) => {
                if let Err(error) = file.write_all(contents.as_bytes()) {
                    let _ = fs::remove_file(&path);
                    return Err(error);
                }
                return Ok(TempConfig(path));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not choose a unique temporary config path",
    ))
}

fn empty_config() -> Toml {
    toml::from_str(
        "templates = {}\n[config]\nversion_check = false\ncaching = false\n[config.wallpaper]\nset = false\ncommand = \"true\"\n",
    )
    .expect("the built-in Matugen config is valid TOML")
}

fn run(command: Command, timeout: Duration, serial: u64, requests: &Receiver<Job>) -> Run<Vec<u8>> {
    child::run(command, "matugen", timeout, || match requests.try_recv() {
        Ok(newer) if newer.serial > serial => Some(Interrupt::Superseded(newer)),
        Err(TryRecvError::Disconnected) => {
            Some(Interrupt::Abort(String::from("Matugen worker stopped")))
        }
        Ok(_) | Err(TryRecvError::Empty) => None,
    })
}

fn parse(output: &[u8]) -> Result<Palettes, String> {
    let json: Json = serde_json::from_slice(output)
        .map_err(|error| format!("Matugen returned invalid JSON: {error}"))?;

    let source = json
        .get("colors")
        .and_then(|colors| colors.get("source_color"))
        .and_then(|role| mode_color(role, "dark"))
        .and_then(hex)
        .map_or(tint::NEUTRAL, tint::Source::of);

    Ok(Palettes {
        dark: parse_roles(&json, Tone::Dark)?,
        light: parse_roles(&json, Tone::Light)?,
        island: tint::roles(source),
    })
}

// a role's color in one mode, as Matugen writes it: a string or an object with `color`
fn mode_color<'a>(role: &'a Json, mode: &str) -> Option<&'a str> {
    let value = role.get(mode)?;
    value
        .as_str()
        .or_else(|| value.get("color").and_then(Json::as_str))
}

fn parse_scheme(output: &[u8], wallpaper: &Path) -> Result<Scheme, String> {
    let json: Json = serde_json::from_slice(output)
        .map_err(|error| format!("Matugen returned invalid JSON: {error}"))?;
    let colors = json
        .get("colors")
        .and_then(Json::as_object)
        .ok_or_else(|| String::from("Matugen did not provide colors"))?;
    let mut scheme = Scheme {
        wallpaper: wallpaper.to_owned(),
        dark: BTreeMap::new(),
        light: BTreeMap::new(),
    };

    for (role, value) in colors {
        for (mode, roles) in [("dark", &mut scheme.dark), ("light", &mut scheme.light)] {
            if let Some(color) = mode_color(value, mode).filter(|color| hex(color).is_some()) {
                roles.insert(role.clone(), color.to_ascii_lowercase());
            }
        }
    }

    Ok(scheme)
}

fn parse_roles(json: &Json, tone: Tone) -> Result<ThemeRoles, String> {
    let mode = tone.name();
    let color = |role: &str| {
        json.get("colors")
            .and_then(|colors| colors.get(role))
            .and_then(|role| mode_color(role, mode))
            .and_then(hex)
            .ok_or_else(|| format!("Matugen did not provide colors.{role}.{mode}"))
    };

    let alpha = |color: Color, alpha| Color::rgba(color.red(), color.green(), color.blue(), alpha);
    let (container, high, outline) = match tone {
        Tone::Dark => (20, 36, 64),
        Tone::Light => (14, 26, 46),
    };

    Ok(ThemeRoles {
        primary: color("primary")?,
        on_primary: color("on_primary")?,
        surface: color("surface")?,
        on_surface: color("on_surface")?,
        on_surface_variant: color("on_surface_variant")?,
        surface_container: alpha(color("surface_container")?, container),
        surface_container_high: alpha(color("surface_container_high")?, high),
        outline: alpha(color("outline")?, outline),
    })
}

fn hex(text: &str) -> Option<Color> {
    let digits = text.strip_prefix('#')?;
    if digits.len() != 6 || !digits.is_ascii() {
        return None;
    }

    let channel = |at| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some(Color::rgb(channel(0)?, channel(2)?, channel(4)?))
}

impl Palettes {
    fn for_tone(self, tone: Tone) -> ThemeRoles {
        match tone {
            Tone::Dark => self.dark,
            Tone::Light => self.light,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex as StdMutex};
    use std::thread;
    use std::time::Instant;

    use super::*;

    fn job(serial: u64, tone: Tone) -> Job {
        Job {
            serial,
            path: PathBuf::from("wallpaper.png"),
            tone,
        }
    }

    fn palette_json(primary_dark: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "colors": {
                "primary": { "dark": primary_dark, "light": "#445566" },
                "on_primary": { "dark": "#abcdef", "light": "#fedcba" },
                "surface": { "dark": "#010203", "light": "#040506" },
                "on_surface": { "dark": "#070809", "light": "#0a0b0c" },
                "on_surface_variant": { "dark": "#0d0e0f", "light": "#101112" },
                "surface_container": { "dark": "#131415", "light": "#161718" },
                "surface_container_high": { "dark": "#191a1b", "light": "#1c1d1e" },
                "outline": { "dark": "#1f2021", "light": "#222324" }
            }
        }))
        .unwrap()
    }

    #[test]
    fn sanitizes_matugen_side_effects_and_keeps_configured_targets() {
        let base = env::temp_dir().join(format!(
            "kanade-matugen-config-test-{}-{}",
            std::process::id(),
            TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&base).unwrap();
        let input = base.join("input.css");
        let output = base.join("themes/app.css");
        let json = base.join("colors.json");
        let mut document: Toml = toml::from_str(
            r#"
[config]
import_json_files = ["./colors.json"]
[config.wallpaper]
set = true
command = "dangerous wallpaper command"
pre_hook = "run wallpaper hook"

[templates.app]
input_path = "./input.css"
output_path = "./themes/app.css"
pre_hook = "run app hook"
post_hook = "run another hook"
input_path_modes = { light = "./input.css", dark = "./input.css" }
"#,
        )
        .unwrap();

        assert!(sanitize(&mut document, &base).unwrap());

        let root = document.as_table().unwrap();
        let config = root["config"].as_table().unwrap();
        assert_eq!(config["version_check"].as_bool(), Some(false));
        assert_eq!(config["caching"].as_bool(), Some(false));
        assert_eq!(config["wallpaper"]["set"].as_bool(), Some(false));
        assert_eq!(config["wallpaper"]["command"].as_str(), Some("true"));
        assert!(config["wallpaper"].get("pre_hook").is_none());
        assert_eq!(
            Path::new(config["import_json_files"][0].as_str().unwrap()),
            json.as_path()
        );

        let template = root["templates"]["app"].as_table().unwrap();
        assert_eq!(
            Path::new(template["input_path"].as_str().unwrap()),
            input.as_path()
        );
        assert_eq!(
            Path::new(template["output_path"].as_str().unwrap()),
            output.as_path()
        );
        assert!(template.get("pre_hook").is_none());
        assert!(template.get("post_hook").is_none());
        assert_eq!(
            Path::new(template["input_path_modes"]["light"].as_str().unwrap()),
            input.as_path()
        );
        assert_eq!(
            Path::new(template["input_path_modes"]["dark"].as_str().unwrap()),
            input.as_path()
        );

        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn disabled_or_outputless_templates_are_not_targets() {
        let base = env::temp_dir();
        let mut document: Toml = toml::from_str(
            r#"
[templates.disabled]
enabled = false
input_path = "unused"
output_path = "unused"

[templates.import_only]
input_path = "include"
"#,
        )
        .unwrap();

        assert!(!sanitize(&mut document, &base).unwrap());
    }

    #[test]
    fn startup_wallpaper_seeds_matugen_but_cannot_replace_a_later_kanade_set() {
        let mut requests = Requests::default();

        let seeded = requests
            .seed_wallpaper(PathBuf::from("shown.png"), || Tone::Dark)
            .unwrap();
        assert_eq!(seeded.serial, 1);
        assert_eq!(seeded.path, PathBuf::from("shown.png"));

        let selected = requests.wallpaper(PathBuf::from("selected.png"), || Tone::Dark);
        assert_eq!(selected.serial, 2);
        assert!(
            requests
                .seed_wallpaper(PathBuf::from("stale.png"), || Tone::Light)
                .is_none()
        );

        let changed = requests.tone_changed(Tone::Light).unwrap();
        assert_eq!(changed.serial, 3);
        assert_eq!(changed.path, PathBuf::from("selected.png"));
        assert_eq!(changed.tone, Tone::Light);
    }

    #[test]
    fn concurrent_wallpaper_and_tone_changes_render_the_latest_tone() {
        let requests = Arc::new(StdMutex::new(Requests::default()));
        let (sender, receiver) = mpsc::channel();
        let (captured, captured_rx) = mpsc::channel();
        let (resume, resume_rx) = mpsc::channel();
        let (tone_started, tone_started_rx) = mpsc::channel();
        let (finished, finished_rx) = mpsc::channel();
        let rendered = Arc::new(StdMutex::new(None));
        let rendered_by_worker = Arc::clone(&rendered);

        let worker = thread::spawn(move || {
            follow_with(
                receiver,
                move |job, _| {
                    *rendered_by_worker.lock().unwrap() = Some(job.tone);
                    Run::Done(Ok(Palettes {
                        dark: theme::ISLAND_DARK,
                        light: theme::ISLAND_LIGHT,
                        island: theme::ISLAND_DARK,
                    }))
                },
                move |serial, result| finished.send((serial, result.is_ok())).unwrap(),
            );
        });

        let wallpaper_requests = Arc::clone(&requests);
        let wallpaper_sender = sender.clone();
        let wallpaper = thread::spawn(move || {
            let mut requests = wallpaper_requests.lock().unwrap();
            let job = requests.wallpaper(PathBuf::from("wallpaper.png"), || {
                captured.send(()).unwrap();
                resume_rx.recv().unwrap();
                Tone::Dark
            });
            wallpaper_sender.send(job).unwrap();
        });

        captured_rx.recv().unwrap();
        let tone_requests = Arc::clone(&requests);
        let tone_sender = sender.clone();
        let tone_change = thread::spawn(move || {
            tone_started.send(()).unwrap();
            let mut requests = tone_requests.lock().unwrap();
            tone_sender
                .send(requests.tone_changed(Tone::Light).unwrap())
                .unwrap();
        });
        tone_started_rx.recv().unwrap();
        resume.send(()).unwrap();

        wallpaper.join().unwrap();
        tone_change.join().unwrap();

        while finished_rx.recv().unwrap().0 != 2 {}
        drop(sender);
        worker.join().unwrap();

        assert_eq!(*rendered.lock().unwrap(), Some(Tone::Light));
    }

    #[test]
    fn coalescing_keeps_the_highest_serial() {
        let (sender, receiver) = mpsc::channel();
        sender.send(job(3, Tone::Light)).unwrap();
        sender.send(job(2, Tone::Dark)).unwrap();

        let newest = newest(job(1, Tone::Dark), &receiver);
        assert_eq!(newest.serial, 3);
        assert_eq!(newest.tone, Tone::Light);
    }

    #[test]
    fn malformed_imported_color_does_not_stop_later_palette_updates() {
        let (sender, receiver) = mpsc::channel();
        let (finished, finished_rx) = mpsc::channel();
        let malformed = palette_json("#aéaaa");
        let valid = palette_json("#112233");
        let worker = thread::spawn(move || {
            follow_with(
                receiver,
                move |job, _| {
                    let output = if job.serial == 1 { &malformed } else { &valid };
                    Run::Done(parse(output))
                },
                move |serial, result| finished.send((serial, result.is_ok())).unwrap(),
            );
        });

        sender.send(job(1, Tone::Dark)).unwrap();
        assert_eq!(finished_rx.recv().unwrap(), (1, false));
        sender.send(job(2, Tone::Light)).unwrap();
        assert_eq!(finished_rx.recv().unwrap(), (2, true));

        drop(sender);
        worker.join().unwrap();
    }

    #[test]
    fn parses_both_modes_from_matugen_color_objects_or_strings() {
        let json = serde_json::json!({
            "colors": {
                "primary": { "dark": { "color": "#112233" }, "light": "#445566" },
                "on_primary": { "dark": "#abcdef", "light": { "color": "#fedcba" } },
                "surface": { "dark": "#010203", "light": "#040506" },
                "on_surface": { "dark": "#070809", "light": "#0a0b0c" },
                "on_surface_variant": { "dark": "#0d0e0f", "light": "#101112" },
                "surface_container": { "dark": "#131415", "light": "#161718" },
                "surface_container_high": { "dark": "#191a1b", "light": "#1c1d1e" },
                "outline": { "dark": "#1f2021", "light": "#222324" }
            }
        });

        let palettes = parse(&serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(palettes.dark.primary, Color::rgb(0x11, 0x22, 0x33));
        assert_eq!(palettes.light.primary, Color::rgb(0x44, 0x55, 0x66));
        assert_eq!(palettes.dark.surface_container.alpha(), 20);
        assert_eq!(palettes.light.surface_container.alpha(), 14);
        assert_eq!(palettes.dark.outline.alpha(), 64);
        assert_eq!(palettes.light.outline.alpha(), 46);
    }

    // the Island's tint comes from the wallpaper's source color, whatever the tone's palettes say
    #[test]
    fn the_island_is_tinted_from_the_source_color() {
        let role = |dark: &str, light: &str| serde_json::json!({ "dark": dark, "light": light });
        let mut colors = serde_json::Map::new();

        for name in [
            "primary",
            "on_primary",
            "surface",
            "on_surface",
            "on_surface_variant",
            "surface_container",
            "surface_container_high",
            "outline",
        ] {
            colors.insert(name.to_string(), role("#101010", "#f0f0f0"));
        }

        let parse_with = |source: Option<&str>| {
            let mut colors = colors.clone();

            if let Some(source) = source {
                colors.insert("source_color".into(), role(source, "#ffffff"));
            }

            parse(&serde_json::to_vec(&serde_json::json!({ "colors": colors })).unwrap()).unwrap()
        };
        let blue = parse_with(Some("#2040e0")).island;

        assert_ne!(blue, parse_with(None).island);
        assert_ne!(blue, parse_with(Some("#e04020")).island);
        assert_eq!(parse_with(None).island, tint::roles(tint::NEUTRAL));
    }

    #[test]
    fn rejects_incomplete_or_non_hex_matugen_palettes() {
        assert!(parse(br#"{"colors":{}}"#).is_err());
        assert!(hex("#12ab").is_none());
        assert!(hex("#aéaaa").is_none());
        assert!(hex("not-a-color").is_none());
    }

    #[test]
    fn a_stalled_matugen_helper_is_killed_and_reaped() {
        let (_sender, receiver) = mpsc::channel();
        let mut command = Command::new("/bin/sleep");
        command.arg("10");
        let started = Instant::now();

        let Run::Done(Err(error)) = run(command, Duration::from_millis(60), 1, &receiver) else {
            panic!("expected timeout");
        };

        assert!(error.contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_new_wallpaper_cancels_the_current_matugen_helper() {
        let (sender, receiver) = mpsc::channel();
        let newer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(60));
            sender
                .send(Job {
                    serial: 2,
                    path: PathBuf::from("new-wallpaper.png"),
                    tone: Tone::Light,
                })
                .unwrap();
        });
        let mut command = Command::new("/bin/sleep");
        command.arg("10");

        let Run::Superseded(job) = run(command, HELPER_TIMEOUT, 1, &receiver) else {
            panic!("expected the older helper to be canceled");
        };
        newer.join().unwrap();

        assert_eq!(job.serial, 2);
        assert_eq!(job.path, PathBuf::from("new-wallpaper.png"));
    }
}
