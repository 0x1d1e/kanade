//! Opt-in desktop appearance adapters. Each target has its own file or preference contract.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, Builder};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::look::Tone;

mod colors;

pub use colors::Scheme;

const HELPER_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const ZEN_BEGIN: &str = "// Kanade appearance sync: begin";
const ZEN_END: &str = "// Kanade appearance sync: end";
const ZEN_PREFIX_NEWLINE: &str = "// Kanade appearance sync: prefix ended with newline";
const ZEN_PREFIX_NO_NEWLINE: &str = "// Kanade appearance sync: prefix had no final newline";
// the toolbar follows the system, which Zen reads live from the desktop portal
const ZEN_PREFS: [(&str, &str); 1] = [("browser.theme.toolbar-theme", "2")];
// once written by Kanade, they kept Zen from following the portal; restored when Zen is closed
const ZEN_RETIRED: [&str; 1] = ["ui.systemUsesDarkTheme"];
const HELPER_RETRY: Duration = Duration::from_secs(2);
const INTERFACE_SCHEMA: &str = "org.gnome.desktop.interface";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq)]
enum Request {
    Set(Tone),
    Disable,
    Colors(Box<Scheme>),
}

enum Helper<T> {
    Done(Result<T, String>),
    Superseded(Request),
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct SyncState {
    desktop: BTreeMap<String, SavedValue>,
    gtk3: Option<SavedGtkConfig>,
    qt5ct: Option<SavedQtConfig>,
    qt6ct: Option<SavedQtConfig>,
    zen: BTreeMap<String, SavedZenProfile>,
    zen_restore_pending: bool,
    zen_retired: BTreeMap<String, SavedZenProfile>,
    gtk_css: BTreeMap<String, colors::SavedGtkCss>,
    // kdeglobals' [UiSettings] ColorScheme, set to Kanade's KDE scheme
    kde_scheme: Option<SavedValue>,
    // the Pywalfox host manifest Kanade's replaced, put back when sync goes off
    pywalfox_manifest: Option<String>,
    // the last wallpaper scheme, so a restart keeps the applied colors until Matugen answers
    colors: Option<Scheme>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SavedValue {
    before: Option<String>,
    applied: String,
    #[serde(default)]
    pending: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct SavedZenProfile {
    preferences: BTreeMap<String, SavedZenPreference>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct SavedZenPreference {
    before: Option<String>,
    applied: BTreeSet<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct SavedGtkConfig {
    theme: Option<SavedValue>,
    prefer_dark: SavedValue,
    created: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct SavedQtConfig {
    scheme: SavedValue,
    custom_palette: SavedValue,
    style: Option<SavedValue>,
}

#[derive(Debug)]
struct StateLoadError {
    kind: io::ErrorKind,
    message: String,
}

impl StateLoadError {
    fn is_retryable(&self) -> bool {
        matches!(
            self.kind,
            io::ErrorKind::Interrupted
                | io::ErrorKind::WouldBlock
                | io::ErrorKind::TimedOut
                | io::ErrorKind::Other
        )
    }
}

static WORKER: OnceLock<Option<Sender<Request>>> = OnceLock::new();

pub fn sync(config: &Config) {
    request(if config.appearance.sync {
        Request::Set(config.appearance.tone)
    } else {
        Request::Disable
    });
}

pub fn colors(scheme: Scheme) {
    request(Request::Colors(Box::new(scheme)));
}

fn request(request: Request) {
    let worker = WORKER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        match Builder::new()
            .name(String::from("kanade-appearance"))
            .spawn(move || follow(receiver))
        {
            Ok(_) => Some(sender),
            Err(error) => {
                eprintln!("kanade: could not start appearance sync worker: {error}");
                None
            }
        }
    });

    if let Some(sender) = worker
        && let Err(error) = sender.send(request)
    {
        eprintln!("kanade: could not queue appearance sync: {error}");
    }
}

fn follow(receiver: Receiver<Request>) {
    let mut wanted = None;
    let mut scheme = None;
    let mut pending = None;
    let mut rerun = false;
    let mut retry_zen_restore = false;

    loop {
        let next = match pending.take() {
            Some(request) => Some(request),
            None if retry_zen_restore => match receiver.recv_timeout(HELPER_RETRY) {
                Ok(request) => Some(request),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            },
            None => match receiver.recv() {
                Ok(request) => Some(request),
                Err(_) => return,
            },
        };
        let Some(request) = next else {
            retry_zen_restore = try_restore_zen_preferences();
            continue;
        };

        let mut run = std::mem::take(&mut rerun) | fold(request, &mut wanted, &mut scheme);
        while let Ok(newer) = receiver.try_recv() {
            run |= fold(newer, &mut wanted, &mut scheme);
        }

        let result = match wanted {
            Some(Request::Set(tone)) if run => apply(tone, scheme.as_deref(), &receiver),
            Some(Request::Disable) if run => disable(&receiver),
            _ => continue,
        };

        match result {
            Helper::Done(Ok(())) => retry_zen_restore = zen_restore_retry_state(),
            Helper::Done(Err(error)) => {
                eprintln!("kanade: appearance sync failed: {error}");
                retry_zen_restore = zen_restore_retry_state();
            }
            Helper::Superseded(newer) => {
                pending = Some(newer);
                rerun = true;
            }
        }
    }
}

// keeps the latest sync request and scheme; whether the targets need applying again
fn fold(request: Request, wanted: &mut Option<Request>, scheme: &mut Option<Box<Scheme>>) -> bool {
    match request {
        Request::Colors(colors) => {
            *scheme = Some(colors);
            matches!(wanted, Some(Request::Set(_)))
        }
        request => {
            *wanted = Some(request);
            true
        }
    }
}

fn try_restore_zen_preferences() -> bool {
    try_restore_zen_preferences_with(load_state)
}

fn try_restore_zen_preferences_with(
    load: impl FnOnce() -> Result<SyncState, StateLoadError>,
) -> bool {
    match load() {
        Ok(mut state) if state.zen_restore_pending || !state.zen_retired.is_empty() => {
            let result = restore_zen_preferences(&mut state);
            if let Err(error) = &result {
                eprintln!("kanade: could not restore Zen Browser preferences: {error}");
            }
            should_retry_zen_restore(
                &result,
                state.zen_restore_pending || !state.zen_retired.is_empty(),
            )
        }
        Ok(_) => false,
        Err(error) => {
            eprintln!(
                "kanade: could not read appearance sync state: {}",
                error.message
            );
            error.is_retryable()
        }
    }
}

fn should_retry_zen_restore(result: &io::Result<()>, pending: bool) -> bool {
    result.is_err() || pending
}

fn zen_restore_retry_state() -> bool {
    zen_restore_retry_state_with(load_state)
}

fn zen_restore_retry_state_with(load: impl FnOnce() -> Result<SyncState, StateLoadError>) -> bool {
    match load() {
        Ok(state) => state.zen_restore_pending || !state.zen_retired.is_empty(),
        Err(error) => error.is_retryable(),
    }
}

fn apply(tone: Tone, scheme: Option<&Scheme>, requests: &Receiver<Request>) -> Helper<()> {
    let mut state = match load_state() {
        Ok(state) => state,
        Err(error) => return Helper::Done(Err(error.message)),
    };
    if let Some(scheme) = scheme
        && state.colors.as_ref() != Some(scheme)
    {
        state.colors = Some(scheme.clone());
        if let Err(error) = save_state(&state) {
            return Helper::Done(Err(error));
        }
    }
    let scheme = state.colors.clone();

    let current_scheme = match read_setting("color-scheme", requests) {
        Helper::Done(Ok(value)) => value,
        Helper::Done(Err(error)) => {
            eprintln!("kanade: could not read desktop color-scheme: {error}");
            None
        }
        Helper::Superseded(newer) => return Helper::Superseded(newer),
    };

    let current_theme = match read_setting("gtk-theme", requests) {
        Helper::Done(Ok(value)) => value,
        Helper::Done(Err(error)) => {
            eprintln!("kanade: could not read GTK theme: {error}");
            None
        }
        Helper::Superseded(newer) => return Helper::Superseded(newer),
    };

    if let Some(current) = current_scheme {
        match set_desktop(
            "color-scheme",
            preference(tone),
            Some(current),
            &mut state,
            requests,
        ) {
            Helper::Done(Ok(())) => {}
            Helper::Done(Err(error)) => {
                eprintln!("kanade: could not apply desktop preference: {error}")
            }
            Helper::Superseded(newer) => return Helper::Superseded(newer),
        }
    }

    let gtk_theme = current_theme
        .as_deref()
        .and_then(|current| paired_theme(current, tone, &gtk_theme_names()));
    if let (Some(current), Some(target)) = (current_theme.as_deref(), gtk_theme.as_deref()) {
        match set_desktop(
            "gtk-theme",
            target,
            Some(current.to_owned()),
            &mut state,
            requests,
        ) {
            Helper::Done(Ok(())) => {}
            Helper::Done(Err(error)) => eprintln!("kanade: could not apply GTK theme: {error}"),
            Helper::Superseded(newer) => return Helper::Superseded(newer),
        }
    }

    if let Err(error) = apply_gtk3_config(tone, gtk_theme.as_deref(), &mut state) {
        eprintln!("kanade: could not update GTK3 settings: {error}");
    }

    if let Some(scheme) = &scheme
        && let Err(error) = colors::apply_gtk(scheme, tone, &mut state)
    {
        eprintln!("kanade: could not write GTK colors: {error}");
    }

    for major in [5, 6] {
        if let Some(newer) = take_pending(requests) {
            return Helper::Superseded(newer);
        }
        if !config_exists(&format!("qt{major}ct/qt{major}ct.conf")) {
            continue;
        }

        let generated = scheme.as_ref().and_then(|scheme| {
            colors::apply_qt(major, scheme, tone)
                .inspect_err(|error| {
                    eprintln!("kanade: could not write qt{major}ct colors: {error}")
                })
                .ok()
        });
        let style = generated.is_some().then(|| colors::palette_style(major));
        match generated.or_else(|| qt_palette(major, tone)) {
            Some(palette) => {
                if let Err(error) = apply_qt_config(major, tone, &palette, style, &mut state) {
                    eprintln!("kanade: could not update qt{major}ct: {error}");
                }
            }
            None => eprintln!("kanade: no qt{major}ct palette for {} mode", tone.name()),
        }
    }

    if let Some(scheme) = &scheme {
        match colors::apply_kde(scheme, tone) {
            Ok(()) => {
                if let Err(error) = apply_kde_config(&mut state) {
                    eprintln!("kanade: could not select the KDE color scheme: {error}");
                }
            }
            Err(error) => eprintln!("kanade: could not write KDE colors: {error}"),
        }
    }

    if let Some(scheme) = &scheme {
        if let Err(error) = colors::apply_niri(scheme, tone) {
            eprintln!("kanade: could not write niri colors: {error}");
        }
        if let Err(error) = colors::apply_kitty(scheme, tone) {
            eprintln!("kanade: could not write kitty colors: {error}");
        }
    }

    if let Some(newer) = take_pending(requests) {
        return Helper::Superseded(newer);
    }

    if let Err(error) = update_zen_profiles(true, &mut state) {
        eprintln!("kanade: could not update Zen Browser appearance: {error}");
    }

    if let Some(scheme) = &scheme
        && let Err(error) = colors::apply_pywalfox(scheme, tone, &mut state)
    {
        eprintln!("kanade: could not write Pywalfox colors: {error}");
    }

    Helper::Done(Ok(()))
}

fn disable(requests: &Receiver<Request>) -> Helper<()> {
    let mut state = match load_state() {
        Ok(state) => state,
        Err(error) => return Helper::Done(Err(error.message)),
    };

    let keys: Vec<String> = state.desktop.keys().cloned().collect();
    for key in keys {
        if let Some(newer) = take_pending(requests) {
            return Helper::Superseded(newer);
        }

        let saved = state.desktop.get(&key).expect("key came from state");
        let current = match read_setting(&key, requests) {
            Helper::Done(Ok(value)) => value,
            Helper::Done(Err(error)) => {
                eprintln!(
                    "kanade: could not read desktop setting {key} while disabling sync: {error}"
                );
                continue;
            }
            Helper::Superseded(newer) => return Helper::Superseded(newer),
        };

        if let Some(before) = restore_value(current.as_deref(), saved) {
            match restore_desktop(&key, before.as_deref(), requests) {
                Helper::Done(Ok(())) => {}
                Helper::Done(Err(error)) => {
                    eprintln!("kanade: could not restore desktop setting {key}: {error}");
                    continue;
                }
                Helper::Superseded(newer) => return Helper::Superseded(newer),
            }
        }
        state.desktop.remove(&key);
        if let Err(error) = save_state(&state) {
            return Helper::Done(Err(error));
        }
    }

    if let Some(newer) = take_pending(requests) {
        return Helper::Superseded(newer);
    }
    if let Err(error) = restore_gtk3_config(&mut state) {
        eprintln!("kanade: could not restore GTK3 settings: {error}");
    }
    if let Err(error) = colors::restore_gtk(&mut state) {
        eprintln!("kanade: could not remove GTK colors: {error}");
    }

    for major in [5, 6] {
        if let Some(newer) = take_pending(requests) {
            return Helper::Superseded(newer);
        }
        if let Err(error) = restore_qt_config(major, &mut state) {
            eprintln!("kanade: could not restore qt{major}ct: {error}");
        } else if let Err(error) = colors::restore_qt(major) {
            eprintln!("kanade: could not remove qt{major}ct colors: {error}");
        }
    }

    if let Err(error) = restore_kde_config(&mut state) {
        eprintln!("kanade: could not restore the KDE color scheme: {error}");
    } else if let Err(error) = colors::restore_kde() {
        eprintln!("kanade: could not remove KDE colors: {error}");
    }

    if let Err(error) = colors::restore_niri() {
        eprintln!("kanade: could not remove niri colors: {error}");
    }
    if let Err(error) = colors::restore_kitty() {
        eprintln!("kanade: could not remove kitty colors: {error}");
    }

    if let Err(error) = update_zen_profiles(false, &mut state) {
        eprintln!("kanade: could not remove Zen Browser appearance override: {error}");
    }

    if let Err(error) = colors::restore_pywalfox(&mut state) {
        eprintln!("kanade: could not remove Pywalfox colors: {error}");
    }
    state.colors = None;

    if let Err(error) = save_state(&state) {
        return Helper::Done(Err(error));
    }
    Helper::Done(Ok(()))
}

fn read_setting(key: &str, requests: &Receiver<Request>) -> Helper<Option<String>> {
    let mut command = Command::new("gsettings");
    command.args(["get", INTERFACE_SCHEMA, key]);

    match run(command, HELPER_TIMEOUT, requests) {
        Helper::Done(Ok(output)) => match parse_gsettings_string(&output.stdout) {
            Some(value) => Helper::Done(Ok(Some(value))),
            None => Helper::Done(Err(format!(
                "gsettings returned an invalid string for {key}"
            ))),
        },
        Helper::Done(Err(error)) => Helper::Done(Err(error)),
        Helper::Superseded(newer) => Helper::Superseded(newer),
    }
}

fn set_desktop(
    key: &str,
    value: &str,
    current: Option<String>,
    state: &mut SyncState,
    requests: &Receiver<Request>,
) -> Helper<()> {
    let saved = state
        .desktop
        .entry(key.to_owned())
        .or_insert_with(|| SavedValue {
            before: current.clone(),
            applied: current.clone().unwrap_or_else(|| value.to_owned()),
            pending: None,
        });
    begin_saved_update(saved, current, value);

    if let Err(error) = save_state(state) {
        return Helper::Done(Err(error));
    }

    let mut command = Command::new("gsettings");
    command.args(["set", INTERFACE_SCHEMA, key, value]);
    match run(command, HELPER_TIMEOUT, requests) {
        Helper::Done(Ok(_)) => {
            let saved = state.desktop.get_mut(key).expect("setting was saved");
            commit_saved_update(saved, value);
            match save_state(state) {
                Ok(()) => Helper::Done(Ok(())),
                Err(error) => Helper::Done(Err(error)),
            }
        }
        Helper::Done(Err(error)) => Helper::Done(Err(format!("{key}: {error}"))),
        Helper::Superseded(newer) => Helper::Superseded(newer),
    }
}

fn restore_desktop(key: &str, value: Option<&str>, requests: &Receiver<Request>) -> Helper<()> {
    let mut command = Command::new("gsettings");
    if let Some(value) = value {
        command.args(["set", INTERFACE_SCHEMA, key, value]);
    } else {
        command.args(["reset", INTERFACE_SCHEMA, key]);
    }

    match run(command, HELPER_TIMEOUT, requests) {
        Helper::Done(Ok(_)) => Helper::Done(Ok(())),
        Helper::Done(Err(error)) => Helper::Done(Err(error)),
        Helper::Superseded(newer) => Helper::Superseded(newer),
    }
}

fn run(mut command: Command, timeout: Duration, requests: &Receiver<Request>) -> Helper<Output> {
    let mut child = match command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return Helper::Done(Err(format!("could not run helper: {error}"))),
    };
    let deadline = Instant::now() + timeout;

    loop {
        match requests.try_recv() {
            Ok(newer) => {
                kill_and_reap(child);
                return Helper::Superseded(newer);
            }
            Err(TryRecvError::Disconnected) => {
                kill_and_reap(child);
                return Helper::Done(Err(String::from("appearance sync worker stopped")));
            }
            Err(TryRecvError::Empty) => {}
        }

        match child.try_wait() {
            Ok(Some(_)) => return Helper::Done(output(child)),
            Ok(None) => {}
            Err(error) => {
                kill_and_reap(child);
                return Helper::Done(Err(format!("could not wait for helper: {error}")));
            }
        }

        if Instant::now() >= deadline {
            kill_and_reap(child);
            return Helper::Done(Err(format!(
                "helper timed out after {} ms",
                timeout.as_millis()
            )));
        }

        thread::sleep(POLL_INTERVAL);
    }
}

fn output(child: Child) -> Result<Output, String> {
    let output = child
        .wait_with_output()
        .map_err(|error| format!("could not collect helper output: {error}"))?;

    if output.status.success() {
        return Ok(output);
    }

    let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if error.is_empty() {
        Err(format!("helper exited with {}", output.status))
    } else {
        Err(error)
    }
}

fn kill_and_reap(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait_with_output();
}

fn preference(tone: Tone) -> &'static str {
    match tone {
        Tone::Dark => "prefer-dark",
        Tone::Light => "prefer-light",
    }
}

fn gtk_dark_preference(tone: Tone) -> &'static str {
    match tone {
        Tone::Dark => "1",
        Tone::Light => "0",
    }
}

fn parse_gsettings_string(output: &[u8]) -> Option<String> {
    let value = std::str::from_utf8(output).ok()?.trim();
    let value = value.strip_prefix('\'')?.strip_suffix('\'')?;
    Some(value.replace("\\'", "'").replace("\\\\", "\\"))
}

fn observe_saved_value(saved: &mut SavedValue, current: Option<String>) {
    if saved
        .pending
        .as_deref()
        .is_some_and(|pending| current.as_deref() == Some(pending))
    {
        saved.applied = saved.pending.take().expect("pending value matched");
    } else {
        saved.pending = None;
    }
    if current.as_deref() != Some(saved.applied.as_str()) && current != saved.before {
        saved.before = current;
    }
}

fn begin_saved_update(saved: &mut SavedValue, current: Option<String>, target: &str) {
    observe_saved_value(saved, current);
    saved.pending = Some(target.to_owned());
}

fn commit_saved_update(saved: &mut SavedValue, target: &str) {
    saved.applied = target.to_owned();
    saved.pending = None;
}

fn restore_value(current: Option<&str>, saved: &SavedValue) -> Option<Option<String>> {
    (current == Some(saved.applied.as_str())
        || saved
            .pending
            .as_deref()
            .is_some_and(|pending| current == Some(pending)))
    .then(|| saved.before.clone())
}

fn paired_theme(current: &str, tone: Tone, installed: &[String]) -> Option<String> {
    let base = current
        .strip_suffix("-dark")
        .or_else(|| current.strip_suffix("-Dark"))
        .unwrap_or(current);
    if !installed.iter().any(|name| name == base) {
        return None;
    }

    match tone {
        Tone::Light => Some(base.to_owned()),
        Tone::Dark => installed
            .iter()
            .find(|name| {
                name.strip_prefix(base)
                    .is_some_and(|suffix| suffix.eq_ignore_ascii_case("-dark"))
            })
            .cloned(),
    }
}

// with Kanade's palette, `palette_style` replaces a Kvantum style, which draws its own colors;
// without it, a dark-only style gives way to Fusion in light mode
fn qt_style_for_tone(current: &str, tone: Tone, palette_style: Option<&str>) -> String {
    let style = current.to_ascii_lowercase();
    match palette_style {
        Some(palette_style) if style.starts_with("kvantum") => palette_style.to_owned(),
        None if matches!(style.as_str(), "kvantum-dark" | "darkly") && tone == Tone::Light => {
            String::from("Fusion")
        }
        _ => current.to_owned(),
    }
}

fn gtk_theme_names() -> Vec<String> {
    let mut names = Vec::new();
    for root in data_dirs()
        .into_iter()
        .map(|path| path.join("themes"))
        .chain(home_dir().map(|home| home.join(".themes")))
    {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir()
                && let Some(name) = entry.file_name().to_str()
            {
                names.push(name.to_owned());
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

fn config_exists(relative: &str) -> bool {
    config_home().join(relative).is_file()
}

fn gtk3_config_path() -> PathBuf {
    config_home().join("gtk-3.0/settings.ini")
}

fn apply_gtk3_config(
    tone: Tone,
    desktop_theme: Option<&str>,
    state: &mut SyncState,
) -> io::Result<()> {
    let path = gtk3_config_path();
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is not a regular config file", path.display()),
                ));
            }
            Some(metadata)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let created = metadata.is_none();
    let original = match metadata {
        Some(_) => fs::read_to_string(&path)?,
        None => String::new(),
    };
    let current_theme = gtk_ini_value(&original, "Settings", "gtk-theme-name");
    let paired = desktop_theme
        .and_then(|theme| paired_theme(theme, tone, &gtk_theme_names()))
        .or_else(|| {
            current_theme
                .as_deref()
                .and_then(|theme| paired_theme(theme, tone, &gtk_theme_names()))
        });
    let dark = gtk_dark_preference(tone);
    let mut updated = original.clone();
    let mut saved = state.gtk3.take().unwrap_or_else(|| SavedGtkConfig {
        theme: current_theme.as_ref().map(|theme| SavedValue {
            before: Some(theme.clone()),
            applied: theme.clone(),
            pending: None,
        }),
        prefer_dark: SavedValue {
            before: gtk_ini_value(&original, "Settings", "gtk-application-prefer-dark-theme"),
            applied: gtk_ini_value(&original, "Settings", "gtk-application-prefer-dark-theme")
                .unwrap_or_else(|| dark.to_owned()),
            pending: None,
        },
        created,
    });
    if let Some(theme) = paired.as_deref() {
        let mut saved_theme = saved.theme.take().unwrap_or_else(|| SavedValue {
            before: current_theme.clone(),
            applied: current_theme.clone().unwrap_or_else(|| theme.to_owned()),
            pending: None,
        });
        begin_saved_update(&mut saved_theme, current_theme, theme);
        updated = gtk_ini_set(&updated, "Settings", "gtk-theme-name", theme);
        saved.theme = Some(saved_theme);
    }
    let current_prefer_dark =
        gtk_ini_value(&original, "Settings", "gtk-application-prefer-dark-theme");
    begin_saved_update(&mut saved.prefer_dark, current_prefer_dark, dark);
    updated = gtk_ini_set(
        &updated,
        "Settings",
        "gtk-application-prefer-dark-theme",
        dark,
    );

    state.gtk3 = Some(saved);
    save_state(state).map_err(io::Error::other)?;
    if updated != original {
        atomic_write(&path, updated.as_bytes())?;
    }
    let saved = state.gtk3.as_mut().expect("GTK state was saved");
    if let (Some(target), Some(theme)) = (paired.as_deref(), saved.theme.as_mut()) {
        commit_saved_update(theme, target);
    }
    commit_saved_update(&mut saved.prefer_dark, dark);
    save_state(state).map_err(io::Error::other)
}

fn restore_gtk3_config(state: &mut SyncState) -> io::Result<()> {
    let Some(saved) = state.gtk3.as_ref() else {
        return Ok(());
    };
    let path = gtk3_config_path();
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{} is not a regular config file", path.display()),
                ));
            }
            Some(metadata)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let Some(_) = metadata else {
        state.gtk3 = None;
        return save_state(state).map_err(io::Error::other);
    };

    let original = fs::read_to_string(&path)?;
    let mut updated = original.clone();
    if let Some(theme) = &saved.theme {
        restore_gtk_ini_value(&mut updated, "Settings", "gtk-theme-name", theme);
    }
    restore_gtk_ini_value(
        &mut updated,
        "Settings",
        "gtk-application-prefer-dark-theme",
        &saved.prefer_dark,
    );
    if updated != original {
        if saved.created
            && updated
                .lines()
                .all(|line| line.trim().is_empty() || line.trim() == "[Settings]")
        {
            fs::remove_file(&path)?;
        } else {
            atomic_write(&path, updated.as_bytes())?;
        }
    }
    state.gtk3 = None;
    save_state(state).map_err(io::Error::other)
}

fn qt_palette(major: u8, tone: Tone) -> Option<PathBuf> {
    let file = match tone {
        Tone::Dark => "darker.conf",
        Tone::Light => "simple.conf",
    };
    let name = format!("qt{major}ct/colors/{file}");
    data_dirs()
        .into_iter()
        .map(|root| root.join(name.as_str()))
        .find(|path| path.is_file())
}

fn apply_qt_config(
    major: u8,
    tone: Tone,
    palette: &Path,
    palette_style: Option<&str>,
    state: &mut SyncState,
) -> io::Result<()> {
    let path = config_home().join(format!("qt{major}ct/qt{major}ct.conf"));
    let metadata = fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a regular config file", path.display()),
        ));
    }

    let original = fs::read_to_string(&path)?;
    let mut updated = original.clone();
    let scheme_path = palette.to_string_lossy().into_owned();
    let scheme = ini_value(&original, "Appearance", "color_scheme_path");
    let custom = ini_value(&original, "Appearance", "custom_palette");
    let style = ini_value(&original, "Appearance", "style");
    let mut saved = saved_qt_clone(state, major).unwrap_or_else(|| SavedQtConfig {
        scheme: SavedValue {
            before: scheme.clone(),
            applied: scheme.clone().unwrap_or_else(|| scheme_path.clone()),
            pending: None,
        },
        custom_palette: SavedValue {
            before: custom.clone(),
            applied: custom.clone().unwrap_or_else(|| String::from("true")),
            pending: None,
        },
        style: None,
    });
    begin_saved_update(&mut saved.scheme, scheme, &scheme_path);
    begin_saved_update(&mut saved.custom_palette, custom, "true");

    if let Some(current) = style {
        let mut saved_style = saved.style.take().unwrap_or_else(|| SavedValue {
            before: Some(current.clone()),
            applied: current.clone(),
            pending: None,
        });
        observe_saved_value(&mut saved_style, Some(current.clone()));
        let base = saved_style.before.as_deref().unwrap_or(current.as_str());
        let target = qt_style_for_tone(base, tone, palette_style);
        saved_style.pending = Some(target.clone());
        updated = ini_set(&updated, "Appearance", "style", &target);
        saved.style = Some(saved_style);
    }

    updated = ini_set(&updated, "Appearance", "color_scheme_path", &scheme_path);
    updated = ini_set(&updated, "Appearance", "custom_palette", "true");
    set_qt_state(state, major, Some(saved));
    save_state(state).map_err(io::Error::other)?;

    if updated != original {
        atomic_write(&path, updated.as_bytes())?;
    }
    let saved = match major {
        5 => state.qt5ct.as_mut(),
        6 => state.qt6ct.as_mut(),
        _ => None,
    }
    .expect("Qt state was saved");
    commit_saved_update(&mut saved.scheme, &scheme_path);
    commit_saved_update(&mut saved.custom_palette, "true");
    if let Some(style) = saved.style.as_mut()
        && let Some(target) = ini_value(&updated, "Appearance", "style")
    {
        commit_saved_update(style, &target);
    }
    save_state(state).map_err(io::Error::other)
}

fn saved_qt_clone(state: &SyncState, major: u8) -> Option<SavedQtConfig> {
    qt_state(state, major).map(|saved| SavedQtConfig {
        scheme: SavedValue {
            before: saved.scheme.before.clone(),
            applied: saved.scheme.applied.clone(),
            pending: saved.scheme.pending.clone(),
        },
        custom_palette: SavedValue {
            before: saved.custom_palette.before.clone(),
            applied: saved.custom_palette.applied.clone(),
            pending: saved.custom_palette.pending.clone(),
        },
        style: saved.style.as_ref().map(|style| SavedValue {
            before: style.before.clone(),
            applied: style.applied.clone(),
            pending: style.pending.clone(),
        }),
    })
}

fn qt_state(state: &SyncState, major: u8) -> Option<&SavedQtConfig> {
    match major {
        5 => state.qt5ct.as_ref(),
        6 => state.qt6ct.as_ref(),
        _ => None,
    }
}

fn set_qt_state(state: &mut SyncState, major: u8, saved: Option<SavedQtConfig>) {
    match major {
        5 => state.qt5ct = saved,
        6 => state.qt6ct = saved,
        _ => {}
    }
}

fn restore_qt_config(major: u8, state: &mut SyncState) -> io::Result<()> {
    let Some(saved) = saved_qt_clone(state, major) else {
        return Ok(());
    };
    let path = config_home().join(format!("qt{major}ct/qt{major}ct.conf"));
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            clear_qt_state(state, major);
            return save_state(state).map_err(io::Error::other);
        }
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is not a regular config file", path.display()),
        ));
    }

    let original = fs::read_to_string(&path)?;
    let mut updated = original.clone();
    restore_ini_value(
        &mut updated,
        "Appearance",
        "color_scheme_path",
        &saved.scheme,
    );
    restore_ini_value(
        &mut updated,
        "Appearance",
        "custom_palette",
        &saved.custom_palette,
    );
    if let Some(style) = &saved.style {
        restore_ini_value(&mut updated, "Appearance", "style", style);
    }
    if updated != original {
        atomic_write(&path, updated.as_bytes())?;
    }
    clear_qt_state(state, major);
    save_state(state).map_err(io::Error::other)
}

// KDE apps read [UiSettings] ColorScheme from their own config, falling back to kdeglobals
fn kdeglobals() -> PathBuf {
    config_home().join("kdeglobals")
}

fn apply_kde_config(state: &mut SyncState) -> io::Result<()> {
    let path = kdeglobals();
    let original = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a regular config file", path.display()),
            ));
        }
        Ok(_) => fs::read_to_string(&path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let current = ini_value(&original, "UiSettings", "ColorScheme");
    let mut saved = state.kde_scheme.take().unwrap_or_else(|| SavedValue {
        before: current.clone(),
        applied: colors::KDE_SCHEME.to_owned(),
        pending: None,
    });
    begin_saved_update(&mut saved, current, colors::KDE_SCHEME);
    state.kde_scheme = Some(saved);
    save_state(state).map_err(io::Error::other)?;

    let updated = ini_set(&original, "UiSettings", "ColorScheme", colors::KDE_SCHEME);
    if updated != original {
        atomic_write(&path, updated.as_bytes())?;
    }
    let saved = state.kde_scheme.as_mut().expect("KDE state was saved");
    commit_saved_update(saved, colors::KDE_SCHEME);
    save_state(state).map_err(io::Error::other)
}

fn restore_kde_config(state: &mut SyncState) -> io::Result<()> {
    let Some(saved) = &state.kde_scheme else {
        return Ok(());
    };
    let path = kdeglobals();
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a regular config file", path.display()),
            ));
        }
        Ok(_) => {
            let original = fs::read_to_string(&path)?;
            let mut updated = original.clone();
            restore_ini_value(&mut updated, "UiSettings", "ColorScheme", saved);
            if updated != original {
                atomic_write(&path, updated.as_bytes())?;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    state.kde_scheme = None;
    save_state(state).map_err(io::Error::other)
}

fn restore_ini_value(text: &mut String, section: &str, key: &str, saved: &SavedValue) {
    restore_ini_value_with_sections(text, section, key, saved, IniSectionSemantics::Qt);
}

fn restore_gtk_ini_value(text: &mut String, section: &str, key: &str, saved: &SavedValue) {
    restore_ini_value_with_sections(text, section, key, saved, IniSectionSemantics::Gtk);
}

fn restore_ini_value_with_sections(
    text: &mut String,
    section: &str,
    key: &str,
    saved: &SavedValue,
    semantics: IniSectionSemantics,
) {
    let current = ini_value_with_sections(text, section, key, semantics);
    let applied = current.as_deref() == Some(saved.applied.as_str());
    let pending = saved
        .pending
        .as_deref()
        .is_some_and(|pending| current.as_deref() == Some(pending));
    if !applied && !pending {
        return;
    }
    *text = match &saved.before {
        Some(value) => ini_set_with_sections(text, section, key, value, semantics),
        None => ini_remove_with_sections(text, section, key, semantics),
    };
}

fn clear_qt_state(state: &mut SyncState, major: u8) {
    match major {
        5 => state.qt5ct = None,
        6 => state.qt6ct = None,
        _ => {}
    }
}

#[derive(Clone, Copy)]
enum IniSectionSemantics {
    Gtk,
    Qt,
}

impl IniSectionSemantics {
    fn matches(self, name: &str, section: &str) -> bool {
        match self {
            Self::Gtk => name == section,
            Self::Qt => name.trim() == section,
        }
    }
}

fn gtk_ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    ini_value_with_sections(text, section, key, IniSectionSemantics::Gtk)
}

fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    ini_value_with_sections(text, section, key, IniSectionSemantics::Qt)
}

fn ini_value_with_sections(
    text: &str,
    section: &str,
    key: &str,
    semantics: IniSectionSemantics,
) -> Option<String> {
    let mut in_section = false;
    let mut value = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            in_section = semantics.matches(name, section);
            continue;
        }
        if !in_section || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        let Some((name, current)) = trimmed.split_once('=') else {
            continue;
        };
        if name.trim() == key {
            value = Some(current.trim().to_owned());
        }
    }
    value
}

fn gtk_ini_set(text: &str, section: &str, key: &str, value: &str) -> String {
    ini_set_with_sections(text, section, key, value, IniSectionSemantics::Gtk)
}

fn ini_set(text: &str, section: &str, key: &str, value: &str) -> String {
    ini_set_with_sections(text, section, key, value, IniSectionSemantics::Qt)
}

fn ini_set_with_sections(
    text: &str,
    section: &str,
    key: &str,
    value: &str,
    semantics: IniSectionSemantics,
) -> String {
    let mut lines = Vec::new();
    let mut in_section = false;
    let mut found_section = false;
    let mut found_key = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            if in_section && !found_key {
                lines.push(format!("{key}={value}"));
                found_key = true;
            }
            in_section = semantics.matches(name, section);
            found_section |= in_section;
            lines.push(line.to_owned());
            continue;
        }

        if in_section
            && !trimmed.starts_with('#')
            && !trimmed.starts_with(';')
            && trimmed
                .split_once('=')
                .is_some_and(|(name, _)| name.trim() == key)
        {
            if !found_key {
                lines.push(format!("{key}={value}"));
                found_key = true;
            }
        } else {
            lines.push(line.to_owned());
        }
    }

    if in_section && !found_key {
        lines.push(format!("{key}={value}"));
    }
    if !found_section {
        if lines.last().is_some_and(|line: &String| !line.is_empty()) {
            lines.push(String::new());
        }
        lines.push(format!("[{section}]"));
        lines.push(format!("{key}={value}"));
    }
    let mut result = lines.join("\n");
    if !result.is_empty() {
        result.push('\n');
    }
    result
}

fn ini_remove_with_sections(
    text: &str,
    section: &str,
    key: &str,
    semantics: IniSectionSemantics,
) -> String {
    let mut lines = Vec::new();
    let mut in_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            in_section = semantics.matches(name, section);
            lines.push(line.to_owned());
            continue;
        }
        if in_section
            && !trimmed.starts_with('#')
            && !trimmed.starts_with(';')
            && trimmed
                .split_once('=')
                .is_some_and(|(name, _)| name.trim() == key)
        {
            continue;
        }
        lines.push(line.to_owned());
    }
    let mut result = lines.join("\n");
    if !result.is_empty() {
        result.push('\n');
    }
    result
}

fn update_zen_profiles(enabled: bool, state: &mut SyncState) -> io::Result<()> {
    state.zen_restore_pending = !enabled && !state.zen.is_empty();
    if enabled {
        retire_zen_preferences(state);
    }
    save_state(state).map_err(io::Error::other)?;
    for root in zen_roots() {
        let profiles = root.join("profiles.ini");
        for profile in parse_profiles(&profiles)? {
            let user_js = profile.join("user.js");
            if user_js.is_symlink() {
                eprintln!(
                    "kanade: left Zen profile file alone because it is a symlink: {}",
                    user_js.display()
                );
                continue;
            }
            let original = match fs::read_to_string(&user_js) {
                Ok(text) => text,
                Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
                Err(error) => return Err(error),
            };
            let updated = managed_zen_prefs(&original, enabled)
                .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;

            if enabled {
                let unowned = managed_zen_prefs(&original, false)
                    .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
                let prefs_path = profile.join("prefs.js");
                let prefs = read_zen_prefs(&prefs_path)?;
                let profile_state = state
                    .zen
                    .entry(profile.to_string_lossy().into_owned())
                    .or_default();
                for (key, target) in ZEN_PREFS {
                    let current =
                        zen_pref_value(&unowned, key).or_else(|| zen_pref_value(&prefs, key));
                    let saved = profile_state.preferences.entry(key.to_owned()).or_default();
                    remember_zen_preference(saved, current, target.to_owned());
                }
                state.zen_restore_pending = false;
                save_state(state).map_err(io::Error::other)?;
            }

            if updated != original {
                if !enabled && updated.is_empty() {
                    fs::remove_file(&user_js)?;
                } else {
                    atomic_write(&user_js, updated.as_bytes())?;
                }
            }
        }
    }

    if !enabled {
        state.zen_restore_pending = !state.zen.is_empty();
        save_state(state).map_err(io::Error::other)?;
    }
    restore_zen_preferences(state)
}

// moves the saved values of retired preferences aside, to be restored while sync stays on
fn retire_zen_preferences(state: &mut SyncState) {
    for (profile, saved) in &mut state.zen {
        for key in ZEN_RETIRED {
            if let Some(preference) = saved.preferences.remove(key) {
                state
                    .zen_retired
                    .entry(profile.clone())
                    .or_default()
                    .preferences
                    .insert(key.to_owned(), preference);
            }
        }
    }
}

fn remember_zen_preference(
    saved: &mut SavedZenPreference,
    current: Option<String>,
    target: String,
) {
    let current_is_latest = saved.applied.len() == 1
        && current
            .as_ref()
            .is_some_and(|value| saved.applied.contains(value));
    if !current_is_latest && current != saved.before {
        saved.before = current;
    }
    saved.applied.clear();
    saved.applied.insert(target);
}

fn read_zen_prefs(path: &Path) -> io::Result<String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{} is not a regular Zen preferences file", path.display()),
            ))
        }
        Ok(_) => fs::read_to_string(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error),
    }
}

fn zen_pref_value(text: &str, key: &str) -> Option<String> {
    text.lines()
        .filter_map(parse_zen_pref_line)
        .filter(|(name, _)| name == key)
        .map(|(_, value)| value)
        .next_back()
}

fn parse_zen_pref_line(line: &str) -> Option<(String, String)> {
    let line = line.trim().strip_prefix("user_pref(")?.strip_suffix(");")?;
    let (key, value) = line.split_once(',')?;
    let key = serde_json::from_str::<String>(key.trim()).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(value.trim()).ok()?;
    Some((key, value.to_string()))
}

fn restore_zen_preferences(state: &mut SyncState) -> io::Result<()> {
    restore_zen_preferences_with_save(state, save_state)
}

fn restore_zen_preferences_with_save(
    state: &mut SyncState,
    save: impl FnOnce(&SyncState) -> Result<(), String>,
) -> io::Result<()> {
    if !state.zen_restore_pending && state.zen_retired.is_empty() {
        return Ok(());
    }
    let retired = restore_saved_zen_map(&mut state.zen_retired, false);
    let restored = if state.zen_restore_pending {
        restore_saved_zen_profiles(state)
    } else {
        Ok(())
    };
    let saved = save(state).map_err(io::Error::other);
    if let Err(error) = &saved {
        eprintln!("kanade: could not save Zen restoration progress: {error}");
    }
    retired.and(restored).and(saved)
}

fn restore_saved_zen_profiles(state: &mut SyncState) -> io::Result<()> {
    let restored = restore_saved_zen_map(&mut state.zen, true);
    state.zen_restore_pending = !state.zen.is_empty();
    restored
}

// restores each closed profile's saved preferences and forgets it; `remove_override` also takes
// Kanade's block out of its user.js
fn restore_saved_zen_map(
    profiles: &mut BTreeMap<String, SavedZenProfile>,
    remove_override: bool,
) -> io::Result<()> {
    let paths: Vec<String> = profiles.keys().cloned().collect();
    let mut first_error = None;
    for stored_path in paths {
        let profile = PathBuf::from(&stored_path);
        match fs::metadata(&profile) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                if first_error.is_none() {
                    first_error = Some(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{} is not a Zen profile directory", profile.display()),
                    ));
                }
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                profiles.remove(&stored_path);
                continue;
            }
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(io::Error::new(
                        error.kind(),
                        format!(
                            "could not inspect Zen profile {}: {error}",
                            profile.display()
                        ),
                    ));
                }
                continue;
            }
        }

        let saved = profiles.get(&stored_path).expect("path came from state");
        match restore_zen_profile(&profile, saved, remove_override) {
            Ok(true) => {
                profiles.remove(&stored_path);
            }
            Ok(false) => {}
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn restore_zen_profile(
    profile: &Path,
    saved: &SavedZenProfile,
    remove_override: bool,
) -> io::Result<bool> {
    if remove_override {
        remove_zen_override(profile)?;
    }
    if zen_profile_running(profile) {
        return Ok(false);
    }

    let path = profile.join("prefs.js");
    let original = read_zen_prefs(&path)?;
    let updated = restore_zen_pref_values(&original, &saved.preferences);
    if updated != original {
        atomic_write(&path, updated.as_bytes())?;
    }
    Ok(true)
}

fn remove_zen_override(profile: &Path) -> io::Result<()> {
    let path = profile.join("user.js");
    let original = read_zen_prefs(&path)?;
    let updated = managed_zen_prefs(&original, false)
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidData, message))?;
    if updated != original {
        if updated.is_empty() {
            fs::remove_file(path)?;
        } else {
            atomic_write(&path, updated.as_bytes())?;
        }
    }
    Ok(())
}

fn restore_zen_pref_values(
    text: &str,
    preferences: &BTreeMap<String, SavedZenPreference>,
) -> String {
    let mut updated = text.to_owned();
    for (key, preference) in preferences {
        let values: Vec<String> = text
            .lines()
            .filter_map(parse_zen_pref_line)
            .filter_map(|(name, value)| (name == *key).then_some(value))
            .collect();
        let Some(applied) = (preference.applied.len() == 1)
            .then(|| preference.applied.iter().next())
            .flatten()
        else {
            continue;
        };
        let Some(current) = values.last() else {
            continue;
        };
        if current != applied
            || values.iter().any(|value| {
                value != applied && preference.before.as_deref() != Some(value.as_str())
            })
        {
            continue;
        }
        updated = set_zen_pref(&updated, key, preference.before.as_deref());
    }
    updated
}

fn set_zen_pref(text: &str, key: &str, value: Option<&str>) -> String {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut result = String::new();
    let mut inserted = false;
    for raw_line in text.split_inclusive('\n') {
        let line = raw_line
            .strip_suffix('\n')
            .unwrap_or(raw_line)
            .strip_suffix('\r')
            .unwrap_or(raw_line.strip_suffix('\n').unwrap_or(raw_line));
        if parse_zen_pref_line(line).is_some_and(|(name, _)| name == key) {
            if let Some(value) = value
                && !inserted
            {
                result.push_str(&format!("user_pref(\"{key}\", {value});"));
                result.push_str(if raw_line.ends_with("\r\n") {
                    "\r\n"
                } else if raw_line.ends_with('\n') {
                    "\n"
                } else {
                    ""
                });
                inserted = true;
            }
        } else {
            result.push_str(raw_line);
        }
    }
    if let Some(value) = value
        && !inserted
    {
        if !result.is_empty() && !result.ends_with('\n') {
            result.push_str(newline);
        }
        result.push_str(&format!("user_pref(\"{key}\", {value});"));
        if text.ends_with('\n') {
            result.push_str(newline);
        }
    }
    result
}

fn zen_profile_running(profile: &Path) -> bool {
    let lock = profile.join("lock");
    match fs::symlink_metadata(&lock) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let Ok(target) = fs::read_link(lock) else {
                return true;
            };
            let target = target.to_string_lossy();
            let Some((_, pid)) = target.rsplit_once('+') else {
                return true;
            };
            pid.parse::<u32>()
                .is_ok_and(|pid| Path::new("/proc").join(pid.to_string()).exists())
        }
        Ok(_) => true,
        // Gecko may leave `.parentlock` behind after the process exits. A live profile
        // advertises its PID through the `lock` symlink above.
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

fn zen_roots() -> Vec<PathBuf> {
    let mut roots = vec![config_home().join("zen")];
    if let Some(home) = home_dir() {
        roots.push(home.join(".zen"));
    }
    roots.sort();
    roots.dedup();
    roots
}

fn parse_profiles(path: &Path) -> io::Result<Vec<PathBuf>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    let mut section = String::new();
    let mut profile_path: Option<String> = None;
    let mut relative = true;
    let mut profiles = Vec::new();

    let finish = |section: &str,
                  profile_path: &mut Option<String>,
                  relative: bool,
                  profiles: &mut Vec<PathBuf>| {
        if !section.starts_with("Profile") {
            profile_path.take();
            return;
        }
        if let Some(path) = profile_path.take() {
            let profile = PathBuf::from(path);
            let profile = if relative {
                root.join(profile)
            } else {
                profile
            };
            profiles.push(profile);
        }
    };

    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            finish(&section, &mut profile_path, relative, &mut profiles);
            section = name.trim().to_owned();
            relative = true;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == "Path" {
            profile_path = Some(value.trim().to_owned());
        } else if key.trim() == "IsRelative" {
            relative = value.trim() != "0";
        }
    }
    finish(&section, &mut profile_path, relative, &mut profiles);

    profiles.retain(|profile| profile.is_dir());
    profiles.sort();
    profiles.dedup();
    Ok(profiles)
}

fn managed_zen_prefs(text: &str, enabled: bool) -> Result<String, &'static str> {
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    let has_marker = lines.iter().any(|line| {
        matches!(
            line.trim_end_matches(['\r', '\n']),
            ZEN_BEGIN | ZEN_END | ZEN_PREFIX_NEWLINE | ZEN_PREFIX_NO_NEWLINE
        )
    });
    if !enabled && !has_marker {
        return Ok(text.to_owned());
    }

    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut kept = String::new();
    let mut skipping = false;
    let mut after_block = false;
    for raw_line in lines {
        let line = raw_line
            .strip_suffix('\n')
            .unwrap_or(raw_line)
            .strip_suffix('\r')
            .unwrap_or_else(|| raw_line.strip_suffix('\n').unwrap_or(raw_line));
        match line {
            ZEN_PREFIX_NEWLINE => {}
            ZEN_PREFIX_NO_NEWLINE => {
                if kept.ends_with("\r\n") {
                    kept.truncate(kept.len() - 2);
                } else if kept.ends_with('\n') {
                    kept.pop();
                }
            }
            ZEN_BEGIN => {
                if skipping {
                    return Err("nested managed block in Zen user.js");
                }
                skipping = true;
            }
            ZEN_END => {
                if !skipping {
                    return Err("unmatched managed block marker in Zen user.js");
                }
                skipping = false;
                after_block = true;
            }
            _ if !skipping => {
                if after_block && !kept.is_empty() && !kept.ends_with('\n') {
                    kept.push_str(newline);
                }
                kept.push_str(raw_line);
            }
            _ => {}
        }
    }
    if skipping {
        return Err("unterminated managed block in Zen user.js");
    }

    if !enabled {
        return Ok(kept);
    }
    let prefix_newline = kept.ends_with('\n');
    if !kept.is_empty() {
        if !prefix_newline {
            kept.push_str(newline);
        }
        kept.push_str(if prefix_newline {
            ZEN_PREFIX_NEWLINE
        } else {
            ZEN_PREFIX_NO_NEWLINE
        });
        kept.push_str(newline);
    }
    kept.push_str(ZEN_BEGIN);
    kept.push_str(newline);
    for (key, value) in ZEN_PREFS {
        kept.push_str(&format!("user_pref(\"{key}\", {value});"));
        kept.push_str(newline);
    }
    kept.push_str(ZEN_END);
    if prefix_newline {
        kept.push_str(newline);
    }
    Ok(kept)
}

fn config_home() -> PathBuf {
    env_path("XDG_CONFIG_HOME")
        .or_else(|| home_dir().map(|home| home.join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
}

fn state_path() -> PathBuf {
    state_dir().join("appearance-sync.json")
}

pub(super) fn state_dir() -> PathBuf {
    env_path("XDG_STATE_HOME")
        .or_else(|| home_dir().map(|home| home.join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".local/state"))
        .join("kanade")
}

pub(super) fn data_home() -> PathBuf {
    env_path("XDG_DATA_HOME")
        .or_else(|| home_dir().map(|home| home.join(".local/share")))
        .unwrap_or_else(|| PathBuf::from(".local/share"))
}

fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![data_home()];
    let configured = env::var_os("XDG_DATA_DIRS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/usr/local/share:/usr/share"));
    if env::var_os("XDG_DATA_DIRS").is_some() {
        dirs.extend(env::split_paths(&configured));
    } else {
        dirs.extend([
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ]);
    }
    unique_paths(dirs)
}

fn unique_paths(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    paths
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub(super) fn home_dir() -> Option<PathBuf> {
    env_path("HOME")
}

fn load_state() -> Result<SyncState, StateLoadError> {
    load_state_at(&state_path())
}

fn load_state_at(path: &Path) -> Result<SyncState, StateLoadError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(SyncState::default()),
        Err(error) => {
            return Err(StateLoadError {
                kind: error.kind(),
                message: format!("could not inspect {}: {error}", path.display()),
            });
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(StateLoadError {
            kind: io::ErrorKind::InvalidInput,
            message: format!("{} is not a regular state file", path.display()),
        });
    }
    let bytes = fs::read(path).map_err(|error| StateLoadError {
        kind: error.kind(),
        message: format!("could not read {}: {error}", path.display()),
    })?;
    serde_json::from_slice(&bytes).map_err(|error| StateLoadError {
        kind: io::ErrorKind::InvalidData,
        message: format!("could not parse {}: {error}", path.display()),
    })
}

fn save_state(state: &SyncState) -> Result<(), String> {
    save_state_at(&state_path(), state)
}

fn save_state_at(path: &Path, state: &SyncState) -> Result<(), String> {
    if state.desktop.is_empty()
        && state.gtk3.is_none()
        && state.qt5ct.is_none()
        && state.qt6ct.is_none()
        && state.zen.is_empty()
        && !state.zen_restore_pending
        && state.zen_retired.is_empty()
        && state.gtk_css.is_empty()
        && state.kde_scheme.is_none()
        && state.pywalfox_manifest.is_none()
        && state.colors.is_none()
    {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!("refusing to remove symlink {}", path.display()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("could not inspect {}: {error}", path.display())),
        }
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("could not remove {}: {error}", path.display())),
        }
        return Ok(());
    }
    let bytes = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("could not encode appearance sync state: {error}"))?;
    atomic_write(path, &bytes)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let existing = match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "refusing to replace a symlink",
                ));
            }
            Some(metadata)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    let mut temp = None;
    let mut file = None;
    for _ in 0..8 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{file_name}.kanade-{}-{sequence}",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(
            existing
                .as_ref()
                .map(|metadata| metadata.permissions().mode() & 0o777)
                .unwrap_or(0o600),
        );
        match options.open(&candidate) {
            Ok(opened) => {
                temp = Some(candidate);
                file = Some(opened);
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    let temp = temp.ok_or_else(|| io::Error::other("could not allocate temporary file"))?;
    let result = (|| {
        let mut file = file.expect("temporary file allocated with its path");
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn take_pending(requests: &Receiver<Request>) -> Option<Request> {
    let mut pending = None;
    while let Ok(request) = requests.try_recv() {
        pending = Some(request);
    }
    pending
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_tone_to_the_desktop_preference() {
        assert_eq!(preference(Tone::Dark), "prefer-dark");
        assert_eq!(preference(Tone::Light), "prefer-light");
    }

    #[test]
    fn maps_tone_to_the_gtk3_dark_preference() {
        assert_eq!(gtk_dark_preference(Tone::Dark), "1");
        assert_eq!(gtk_dark_preference(Tone::Light), "0");
    }

    #[test]
    fn parses_gsettings_strings_without_shell_quoting() {
        assert_eq!(
            parse_gsettings_string(b"'adw-gtk3-dark'\n"),
            Some(String::from("adw-gtk3-dark"))
        );
        assert_eq!(parse_gsettings_string(b"prefer-dark\n"), None);
    }

    #[test]
    fn chooses_the_installed_gtk_theme_pair_for_each_tone() {
        let themes = [String::from("adw-gtk3"), String::from("adw-gtk3-dark")];
        assert_eq!(
            paired_theme("adw-gtk3", Tone::Dark, &themes),
            Some(String::from("adw-gtk3-dark"))
        );
        assert_eq!(
            paired_theme("adw-gtk3-dark", Tone::Light, &themes),
            Some(String::from("adw-gtk3"))
        );
        assert_eq!(
            paired_theme("adw-gtk3", Tone::Light, &themes),
            Some(String::from("adw-gtk3"))
        );
        assert_eq!(paired_theme("unknown", Tone::Dark, &themes), None);
    }

    #[test]
    fn gtk_theme_pair_supports_adwaita_dark_suffix_casing() {
        let themes = [String::from("Adwaita"), String::from("Adwaita-dark")];
        assert_eq!(
            paired_theme("Adwaita-dark", Tone::Light, &themes),
            Some(String::from("Adwaita"))
        );
        assert_eq!(
            paired_theme("Adwaita", Tone::Dark, &themes),
            Some(String::from("Adwaita-dark"))
        );
    }

    #[test]
    fn qt_styles_that_ignore_the_palette_give_way_to_one_that_draws_it() {
        assert_eq!(
            qt_style_for_tone("kvantum-dark", Tone::Light, None),
            "Fusion"
        );
        assert_eq!(
            qt_style_for_tone("kvantum-dark", Tone::Dark, None),
            "kvantum-dark"
        );
        assert_eq!(qt_style_for_tone("Darkly", Tone::Light, None), "Fusion");
        assert_eq!(qt_style_for_tone("Fusion", Tone::Light, None), "Fusion");

        for tone in [Tone::Dark, Tone::Light] {
            assert_eq!(
                qt_style_for_tone("kvantum-dark", tone, Some("Darkly")),
                "Darkly"
            );
            assert_eq!(qt_style_for_tone("kvantum", tone, Some("Fusion")), "Fusion");
            assert_eq!(qt_style_for_tone("Darkly", tone, Some("Darkly")), "Darkly");
            assert_eq!(qt_style_for_tone("Breeze", tone, Some("Darkly")), "Breeze");
        }
    }

    #[test]
    fn updates_only_the_requested_qt_ini_keys_and_restores_original_values() {
        let original = "[Appearance]\nstyle=kvantum-dark\ncustom_palette=false\nicon_theme=Papirus\n\n[SettingsWindow]\ngeometry=x\n";
        let mut updated = ini_set(
            original,
            "Appearance",
            "color_scheme_path",
            "/tmp/simple.conf",
        );
        updated = ini_set(&updated, "Appearance", "custom_palette", "true");
        updated = ini_set(&updated, "Appearance", "style", "Fusion");
        assert!(updated.contains("color_scheme_path=/tmp/simple.conf"));
        assert!(updated.contains("style=Fusion"));
        assert!(updated.contains("icon_theme=Papirus"));
        assert!(updated.contains("[SettingsWindow]\ngeometry=x"));

        let mut restored = updated.clone();
        restore_ini_value(
            &mut restored,
            "Appearance",
            "color_scheme_path",
            &SavedValue {
                before: None,
                applied: String::from("/tmp/simple.conf"),
                pending: None,
            },
        );
        restore_ini_value(
            &mut restored,
            "Appearance",
            "custom_palette",
            &SavedValue {
                before: Some(String::from("false")),
                applied: String::from("true"),
                pending: None,
            },
        );
        restore_ini_value(
            &mut restored,
            "Appearance",
            "style",
            &SavedValue {
                before: Some(String::from("kvantum-dark")),
                applied: String::from("Fusion"),
                pending: None,
            },
        );
        assert_eq!(
            ini_value(&restored, "Appearance", "color_scheme_path"),
            None
        );
        assert_eq!(
            ini_value(&restored, "Appearance", "custom_palette"),
            Some(String::from("false"))
        );
        assert!(restored.contains("style=kvantum-dark"));
    }

    #[test]
    fn gtk3_theme_and_dark_preference_restore_without_touching_other_settings() {
        let original = "[Settings]\ngtk-theme-name=adw-gtk3-dark\ngtk-application-prefer-dark-theme=1\ngtk-font-name=Inter 11\n";
        let light = gtk_ini_set(
            &gtk_ini_set(original, "Settings", "gtk-theme-name", "adw-gtk3"),
            "Settings",
            "gtk-application-prefer-dark-theme",
            "0",
        );
        assert_eq!(
            gtk_ini_value(&light, "Settings", "gtk-theme-name").as_deref(),
            Some("adw-gtk3")
        );
        assert_eq!(
            gtk_ini_value(&light, "Settings", "gtk-application-prefer-dark-theme").as_deref(),
            Some("0")
        );

        let mut restored = light;
        restore_gtk_ini_value(
            &mut restored,
            "Settings",
            "gtk-theme-name",
            &SavedValue {
                before: Some(String::from("adw-gtk3-dark")),
                applied: String::from("adw-gtk3"),
                pending: None,
            },
        );
        restore_gtk_ini_value(
            &mut restored,
            "Settings",
            "gtk-application-prefer-dark-theme",
            &SavedValue {
                before: Some(String::from("1")),
                applied: String::from("0"),
                pending: None,
            },
        );
        assert_eq!(restored, original);
    }

    #[test]
    fn old_sync_state_without_gtk3_fields_still_loads() {
        let state: SyncState =
            serde_json::from_str(r#"{"desktop":{},"qt5ct":null,"qt6ct":null}"#).unwrap();
        assert!(state.gtk3.is_none());
        assert!(state.zen.is_empty());
        assert!(!state.zen_restore_pending);
    }

    #[test]
    fn ini_lookup_and_restore_follow_the_last_duplicate_key() {
        let text = "[Appearance]\nstyle=managed\nstyle=UserChanged\n";
        assert_eq!(
            ini_value(text, "Appearance", "style").as_deref(),
            Some("UserChanged")
        );

        let mut restored = text.to_owned();
        restore_ini_value(
            &mut restored,
            "Appearance",
            "style",
            &SavedValue {
                before: Some(String::from("original")),
                applied: String::from("managed"),
                pending: None,
            },
        );
        assert_eq!(restored, text);
    }

    #[test]
    fn gtk_sections_keep_internal_whitespace_distinct_during_restore() {
        let text = "[Settings]\ngtk-theme-name=UserChanged\n[ Settings ]\ngtk-theme-name=managed\n";
        assert_eq!(
            gtk_ini_value(text, "Settings", "gtk-theme-name").as_deref(),
            Some("UserChanged")
        );
        assert_eq!(
            ini_value(text, "Settings", "gtk-theme-name").as_deref(),
            Some("managed")
        );

        let mut restored = text.to_owned();
        restore_gtk_ini_value(
            &mut restored,
            "Settings",
            "gtk-theme-name",
            &SavedValue {
                before: Some(String::from("original")),
                applied: String::from("managed"),
                pending: None,
            },
        );
        assert_eq!(restored, text);

        let mut exact_section = gtk_ini_set(
            "[Settings]\ngtk-theme-name=old\n[ Settings ]\ngtk-theme-name=keep\n",
            "Settings",
            "gtk-theme-name",
            "managed",
        );
        assert_eq!(
            exact_section,
            "[Settings]\ngtk-theme-name=managed\n[ Settings ]\ngtk-theme-name=keep\n"
        );
        exact_section =
            String::from("[Settings]\ngtk-theme-name=managed\n[ Settings ]\ngtk-theme-name=keep\n");
        restore_gtk_ini_value(
            &mut exact_section,
            "Settings",
            "gtk-theme-name",
            &SavedValue {
                before: None,
                applied: String::from("managed"),
                pending: None,
            },
        );
        assert_eq!(
            exact_section,
            "[Settings]\n[ Settings ]\ngtk-theme-name=keep\n"
        );
    }

    #[test]
    fn qt_restore_keeps_values_changed_by_the_user_after_sync() {
        let mut text = String::from(
            "[Appearance]\ncolor_scheme_path=/home/user/custom.conf\ncustom_palette=false\n",
        );
        restore_ini_value(
            &mut text,
            "Appearance",
            "color_scheme_path",
            &SavedValue {
                before: Some(String::from("/home/user/original.conf")),
                applied: String::from("/tmp/simple.conf"),
                pending: None,
            },
        );
        assert_eq!(
            ini_value(&text, "Appearance", "color_scheme_path"),
            Some(String::from("/home/user/custom.conf"))
        );
    }

    #[test]
    fn updates_and_removes_only_the_managed_zen_preferences() {
        let original = "user_pref(\"other.setting\", true);\n";
        let managed = managed_zen_prefs(original, true).unwrap();
        assert!(managed.contains("other.setting"));
        assert!(managed.contains("browser.theme.toolbar-theme\", 2"));
        assert!(!managed.contains("ui.systemUsesDarkTheme"));
        assert_eq!(managed_zen_prefs(&managed, true).unwrap(), managed);
        assert_eq!(managed_zen_prefs(&managed, false).unwrap(), original);
    }

    #[test]
    fn zen_block_keeps_preferences_appended_after_an_unterminated_prefix() {
        let original = "// personal comment";
        let dark = managed_zen_prefs(original, true).unwrap();
        let appended = format!("{dark}\nuser_pref(\"other.setting\", true);\n");
        let expected = "// personal comment\nuser_pref(\"other.setting\", true);\n";
        assert_eq!(managed_zen_prefs(&appended, false).unwrap(), expected);

        let again = managed_zen_prefs(&appended, true).unwrap();
        assert_eq!(again.matches(ZEN_BEGIN).count(), 1);
        assert!(again.contains("other.setting"));
        assert_eq!(managed_zen_prefs(&again, false).unwrap(), expected);
    }

    #[test]
    fn zen_user_js_keeps_unowned_crlf_bytes_when_disabled() {
        let original = "user_pref(\"other.setting\", true);\r\n";
        assert_eq!(managed_zen_prefs(original, false).unwrap(), original);
        let dark = managed_zen_prefs(original, true).unwrap();
        assert!(dark.contains("\r\n"));
        assert_eq!(managed_zen_prefs(&dark, false).unwrap(), original);

        let without_final_newline = "user_pref(\"other.setting\", true);";
        let dark = managed_zen_prefs(without_final_newline, true).unwrap();
        assert_eq!(
            managed_zen_prefs(&dark, false).unwrap(),
            without_final_newline
        );
    }

    #[test]
    fn restores_persisted_zen_prefs_only_while_kanade_owns_them() {
        let mut preferences = BTreeMap::new();
        preferences.insert(
            "browser.theme.toolbar-theme".to_owned(),
            SavedZenPreference {
                before: Some(String::from("0")),
                applied: [String::from("2")].into_iter().collect(),
            },
        );
        preferences.insert(
            "ui.systemUsesDarkTheme".to_owned(),
            SavedZenPreference {
                before: None,
                applied: [String::from("1")].into_iter().collect(),
            },
        );
        let prefs = concat!(
            "user_pref(\"browser.theme.toolbar-theme\", 2);\n",
            "user_pref(\"ui.systemUsesDarkTheme\", 1);\n",
            "user_pref(\"other.setting\", true);\n"
        );
        let restored = restore_zen_pref_values(prefs, &preferences);
        assert!(restored.contains("user_pref(\"browser.theme.toolbar-theme\", 0);"));
        assert!(!restored.contains("ui.systemUsesDarkTheme"));
        assert!(restored.contains("user_pref(\"other.setting\", true);"));

        let user_changed = prefs.replace(
            "browser.theme.toolbar-theme\", 2",
            "browser.theme.toolbar-theme\", 3",
        );
        let restored = restore_zen_pref_values(&user_changed, &preferences);
        assert!(restored.contains("browser.theme.toolbar-theme\", 3"));
    }

    #[test]
    fn zen_restoration_preserves_a_user_change_to_a_previous_tone() {
        let mut preference = SavedZenPreference {
            before: Some(String::from("0")),
            applied: [String::from("2")].into_iter().collect(),
        };
        remember_zen_preference(&mut preference, Some(String::from("2")), String::from("1"));
        assert_eq!(
            preference.applied,
            [String::from("1")].into_iter().collect()
        );

        let preferences =
            BTreeMap::from([(String::from("browser.theme.toolbar-theme"), preference)]);
        let user_changed_to_previous_tone = "user_pref(\"browser.theme.toolbar-theme\", 2);\n";
        assert_eq!(
            restore_zen_pref_values(user_changed_to_previous_tone, &preferences),
            user_changed_to_previous_tone
        );
    }

    #[test]
    fn ambiguous_legacy_zen_values_are_not_restored() {
        let preferences = BTreeMap::from([(
            String::from("browser.theme.toolbar-theme"),
            SavedZenPreference {
                before: Some(String::from("0")),
                applied: [String::from("1"), String::from("2")].into_iter().collect(),
            },
        )]);
        let previous_tone = "user_pref(\"browser.theme.toolbar-theme\", 2);\n";
        assert_eq!(
            restore_zen_pref_values(previous_tone, &preferences),
            previous_tone
        );
    }

    #[test]
    fn cleans_saved_zen_profiles_after_they_are_unregistered() {
        let root = env::temp_dir().join(format!(
            "kanade-zen-unregistered-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let profile = root.join("profile");
        fs::create_dir_all(&profile).unwrap();
        fs::write(
            root.join("profiles.ini"),
            "[Profile0]\nName=other\nIsRelative=1\nPath=other\n",
        )
        .unwrap();
        let original_user_js = "user_pref(\"other.setting\", true);\n";
        fs::write(
            profile.join("user.js"),
            managed_zen_prefs(original_user_js, true).unwrap(),
        )
        .unwrap();
        fs::write(
            profile.join("prefs.js"),
            "user_pref(\"browser.theme.toolbar-theme\", 2);\n",
        )
        .unwrap();

        let preferences = BTreeMap::from([(
            String::from("browser.theme.toolbar-theme"),
            SavedZenPreference {
                before: Some(String::from("0")),
                applied: [String::from("2")].into_iter().collect(),
            },
        )]);
        let mut state = SyncState {
            zen: BTreeMap::from([(
                profile.to_string_lossy().into_owned(),
                SavedZenProfile { preferences },
            )]),
            zen_restore_pending: true,
            ..SyncState::default()
        };

        restore_saved_zen_profiles(&mut state).unwrap();

        assert_eq!(
            fs::read_to_string(profile.join("user.js")).unwrap(),
            original_user_js
        );
        assert_eq!(
            fs::read_to_string(profile.join("prefs.js")).unwrap(),
            "user_pref(\"browser.theme.toolbar-theme\", 0);\n"
        );
        assert!(state.zen.is_empty());
        assert!(!state.zen_restore_pending);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_retired_zen_preference_is_restored_while_sync_keeps_the_rest() {
        let root = env::temp_dir().join(format!(
            "kanade-zen-retired-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let profile = root.join("profile");
        fs::create_dir_all(&profile).unwrap();
        let user_js = managed_zen_prefs("", true).unwrap();
        fs::write(profile.join("user.js"), &user_js).unwrap();
        fs::write(
            profile.join("prefs.js"),
            "user_pref(\"browser.theme.toolbar-theme\", 2);\nuser_pref(\"ui.systemUsesDarkTheme\", 1);\n",
        )
        .unwrap();

        let saved = |before: Option<&str>, applied: &str| SavedZenPreference {
            before: before.map(String::from),
            applied: [String::from(applied)].into_iter().collect(),
        };
        let preferences = BTreeMap::from([
            (
                String::from("browser.theme.toolbar-theme"),
                saved(Some("0"), "2"),
            ),
            (String::from("ui.systemUsesDarkTheme"), saved(None, "1")),
        ]);
        let mut state = SyncState {
            zen: BTreeMap::from([(
                profile.to_string_lossy().into_owned(),
                SavedZenProfile { preferences },
            )]),
            ..SyncState::default()
        };

        retire_zen_preferences(&mut state);
        restore_zen_preferences_with_save(&mut state, |_| Ok(())).unwrap();

        assert_eq!(
            fs::read_to_string(profile.join("user.js")).unwrap(),
            user_js
        );
        assert_eq!(
            fs::read_to_string(profile.join("prefs.js")).unwrap(),
            "user_pref(\"browser.theme.toolbar-theme\", 2);\n"
        );
        let kept = &state.zen[profile.to_string_lossy().as_ref()];
        assert_eq!(
            kept.preferences.keys().collect::<Vec<_>>(),
            ["browser.theme.toolbar-theme"]
        );
        assert!(state.zen_retired.is_empty());
        assert!(!state.zen_restore_pending);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_appearance_state_does_not_schedule_automatic_retries() {
        let root = env::temp_dir().join(format!(
            "kanade-invalid-appearance-state-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("appearance-sync.json");
        fs::write(&path, b"not json").unwrap();

        let retry = try_restore_zen_preferences_with(|| load_state_at(&path));

        assert!(!retry);
        assert!(!zen_restore_retry_state_with(|| load_state_at(&path)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transient_state_read_errors_keep_retrying() {
        let error = StateLoadError {
            kind: io::ErrorKind::WouldBlock,
            message: String::from("temporarily busy"),
        };
        assert!(error.is_retryable());
        let denied = StateLoadError {
            kind: io::ErrorKind::PermissionDenied,
            message: String::from("permission denied"),
        };
        assert!(!denied.is_retryable());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_appearance_state_does_not_schedule_automatic_retries() {
        let root = env::temp_dir().join(format!(
            "kanade-symlink-appearance-state-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let target = root.join("target.json");
        let path = root.join("appearance-sync.json");
        fs::write(&target, b"{}").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();

        let retry = try_restore_zen_preferences_with(|| load_state_at(&path));

        assert!(!retry);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_final_zen_state_save_keeps_retrying_until_ownership_is_cleared() {
        use std::os::unix::fs::PermissionsExt;

        let root = env::temp_dir().join(format!(
            "kanade-zen-state-save-failure-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let profile = root.join("profile");
        let state_path = root.join("state/appearance-sync.json");
        fs::create_dir_all(&profile).unwrap();
        let original_user_js = "user_pref(\"other.setting\", true);\n";
        fs::write(
            profile.join("user.js"),
            managed_zen_prefs(original_user_js, true).unwrap(),
        )
        .unwrap();
        fs::write(
            profile.join("prefs.js"),
            "user_pref(\"browser.theme.toolbar-theme\", 2);\n",
        )
        .unwrap();
        let preferences = BTreeMap::from([(
            String::from("browser.theme.toolbar-theme"),
            SavedZenPreference {
                before: Some(String::from("0")),
                applied: [String::from("2")].into_iter().collect(),
            },
        )]);
        let initial = SyncState {
            zen: BTreeMap::from([(
                profile.to_string_lossy().into_owned(),
                SavedZenProfile { preferences },
            )]),
            zen_restore_pending: true,
            ..SyncState::default()
        };
        save_state_at(&state_path, &initial).unwrap();
        let mut state = load_state_at(&state_path).unwrap();
        let state_dir = state_path.parent().unwrap();
        fs::set_permissions(state_dir, fs::Permissions::from_mode(0o500)).unwrap();
        let can_write = fs::write(state_dir.join("write-probe"), b"probe").is_ok();

        if can_write {
            fs::remove_file(state_dir.join("write-probe")).unwrap();
            fs::set_permissions(state_dir, fs::Permissions::from_mode(0o700)).unwrap();
            fs::remove_dir_all(root).unwrap();
            return;
        }

        let first_result = restore_zen_preferences_with_save(&mut state, |state| {
            save_state_at(&state_path, state)
        });
        let memory_pending = state.zen_restore_pending;
        let should_retry = should_retry_zen_restore(&first_result, memory_pending);
        let persisted = load_state_at(&state_path);
        let worker_should_retry = zen_restore_retry_state_with(|| load_state_at(&state_path));
        fs::set_permissions(state_dir, fs::Permissions::from_mode(0o700)).unwrap();

        assert!(
            first_result
                .unwrap_err()
                .to_string()
                .contains("Permission denied")
        );
        assert!(!memory_pending);
        assert!(should_retry);
        assert!(worker_should_retry);
        let persisted = persisted.unwrap();
        assert!(persisted.zen_restore_pending);
        assert_eq!(persisted.zen.len(), 1);

        let mut retry_state = load_state_at(&state_path).unwrap();
        let retry_result = restore_zen_preferences_with_save(&mut retry_state, |state| {
            save_state_at(&state_path, state)
        });
        assert!(retry_result.is_ok());
        assert!(!should_retry_zen_restore(
            &retry_result,
            retry_state.zen_restore_pending
        ));
        assert!(!state_path.exists());
        assert_eq!(
            fs::read_to_string(profile.join("user.js")).unwrap(),
            original_user_js
        );
        assert_eq!(
            fs::read_to_string(profile.join("prefs.js")).unwrap(),
            "user_pref(\"browser.theme.toolbar-theme\", 0);\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_zen_override_cleanup_keeps_restoration_pending_without_blocking_other_profiles() {
        let root = env::temp_dir().join(format!(
            "kanade-zen-cleanup-failure-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let first = root.join("a-first");
        let healthy = root.join("z-healthy");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&healthy).unwrap();
        let target = root.join("user.js-target");
        fs::write(&target, "managed target\n").unwrap();
        std::os::unix::fs::symlink(&target, first.join("user.js")).unwrap();
        let original_user_js = "user_pref(\"other.setting\", true);\n";
        fs::write(
            healthy.join("user.js"),
            managed_zen_prefs(original_user_js, true).unwrap(),
        )
        .unwrap();
        fs::write(
            healthy.join("prefs.js"),
            "user_pref(\"browser.theme.toolbar-theme\", 2);\n",
        )
        .unwrap();
        let preferences = BTreeMap::from([(
            String::from("browser.theme.toolbar-theme"),
            SavedZenPreference {
                before: Some(String::from("0")),
                applied: [String::from("2")].into_iter().collect(),
            },
        )]);
        let mut state = SyncState {
            zen: BTreeMap::from([
                (
                    first.to_string_lossy().into_owned(),
                    SavedZenProfile::default(),
                ),
                (
                    healthy.to_string_lossy().into_owned(),
                    SavedZenProfile {
                        preferences: preferences.clone(),
                    },
                ),
            ]),
            zen_restore_pending: true,
            ..SyncState::default()
        };

        assert!(restore_saved_zen_profiles(&mut state).is_err());

        assert_eq!(state.zen.len(), 1);
        assert!(state.zen.contains_key(first.to_string_lossy().as_ref()));
        assert!(state.zen_restore_pending);
        assert!(first.join("user.js").is_symlink());
        assert_eq!(fs::read_to_string(&target).unwrap(), "managed target\n");
        assert_eq!(
            fs::read_to_string(healthy.join("user.js")).unwrap(),
            original_user_js
        );
        assert_eq!(
            fs::read_to_string(healthy.join("prefs.js")).unwrap(),
            "user_pref(\"browser.theme.toolbar-theme\", 0);\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn inaccessible_zen_profile_retains_state_until_restoration_succeeds() {
        use std::os::unix::fs::PermissionsExt;

        let root = env::temp_dir().join(format!(
            "kanade-zen-inaccessible-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let parent = root.join("parent");
        let profile = parent.join("profile");
        fs::create_dir_all(&profile).unwrap();
        let original_user_js = "user_pref(\"other.setting\", true);\n";
        fs::write(
            profile.join("user.js"),
            managed_zen_prefs(original_user_js, true).unwrap(),
        )
        .unwrap();
        fs::write(
            profile.join("prefs.js"),
            "user_pref(\"browser.theme.toolbar-theme\", 2);\n",
        )
        .unwrap();
        let preferences = BTreeMap::from([(
            String::from("browser.theme.toolbar-theme"),
            SavedZenPreference {
                before: Some(String::from("0")),
                applied: [String::from("2")].into_iter().collect(),
            },
        )]);
        let mut state = SyncState {
            zen: BTreeMap::from([(
                profile.to_string_lossy().into_owned(),
                SavedZenProfile { preferences },
            )]),
            zen_restore_pending: true,
            ..SyncState::default()
        };

        fs::set_permissions(&parent, fs::Permissions::from_mode(0o0)).unwrap();
        let inspection_error = fs::metadata(&profile).err();
        let restoration = restore_saved_zen_profiles(&mut state);
        let remaining_profiles = state.zen.len();
        let pending = state.zen_restore_pending;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();

        if inspection_error.is_none() {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        assert_eq!(
            inspection_error.unwrap().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(restoration.is_err());
        assert_eq!(remaining_profiles, 1);
        assert!(pending);

        restore_saved_zen_profiles(&mut state).unwrap();
        assert!(state.zen.is_empty());
        assert!(!state.zen_restore_pending);
        assert_eq!(
            fs::read_to_string(profile.join("user.js")).unwrap(),
            original_user_js
        );
        assert_eq!(
            fs::read_to_string(profile.join("prefs.js")).unwrap(),
            "user_pref(\"browser.theme.toolbar-theme\", 0);\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_malformed_zen_managed_blocks() {
        assert!(managed_zen_prefs("// Kanade appearance sync: begin\n", false).is_err());
        assert!(managed_zen_prefs("// Kanade appearance sync: end\n", false).is_err());
    }

    #[test]
    fn data_dirs_keep_xdg_precedence_while_deduplicating() {
        let paths = unique_paths([
            PathBuf::from("/home/test/.local/share"),
            PathBuf::from("/usr/share"),
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ]);
        assert_eq!(
            paths,
            [
                PathBuf::from("/home/test/.local/share"),
                PathBuf::from("/usr/share"),
                PathBuf::from("/usr/local/share"),
            ]
        );
    }

    #[test]
    fn missing_managed_values_do_not_panic_or_count_as_owned() {
        let mut saved = SavedValue {
            before: None,
            applied: String::from("dark"),
            pending: None,
        };
        begin_saved_update(&mut saved, None, "light");
        assert_eq!(saved.applied, "dark");
        assert_eq!(saved.pending.as_deref(), Some("light"));
        assert_eq!(restore_value(None, &saved), None);
        saved.pending = None;
        assert_eq!(restore_value(None, &saved), None);

        let mut text = String::from("[Appearance]\ncustom_palette=true\n");
        restore_ini_value(
            &mut text,
            "Appearance",
            "style",
            &SavedValue {
                before: Some(String::from("kvantum-dark")),
                applied: String::from("Fusion"),
                pending: None,
            },
        );
        assert_eq!(text, "[Appearance]\ncustom_palette=true\n");
    }

    #[test]
    fn a_stale_parentlock_does_not_keep_a_zen_profile_running() {
        let profile = env::temp_dir().join(format!(
            "kanade-zen-lock-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&profile);
        fs::create_dir_all(&profile).unwrap();
        fs::write(profile.join(".parentlock"), b"").unwrap();
        assert!(!zen_profile_running(&profile));

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(format!("host+{}", std::process::id()), profile.join("lock")).unwrap();
            assert!(zen_profile_running(&profile));
        }
        fs::remove_dir_all(profile).unwrap();
    }

    #[test]
    fn pending_updates_keep_the_last_applied_value_for_restoration() {
        let mut saved = SavedValue {
            before: Some(String::from("original")),
            applied: String::from("dark"),
            pending: None,
        };
        begin_saved_update(&mut saved, Some(String::from("dark")), "light");
        assert_eq!(saved.before.as_deref(), Some("original"));
        assert_eq!(saved.applied, "dark");
        assert_eq!(saved.pending.as_deref(), Some("light"));
        assert_eq!(
            restore_value(Some("dark"), &saved),
            Some(Some(String::from("original")))
        );
        assert_eq!(
            restore_value(Some("light"), &saved),
            Some(Some(String::from("original")))
        );

        observe_saved_value(&mut saved, Some(String::from("light")));
        assert_eq!(saved.applied, "light");
        assert_eq!(saved.pending, None);
        begin_saved_update(&mut saved, Some(String::from("custom")), "dark");
        assert_eq!(saved.before.as_deref(), Some("custom"));
    }

    #[test]
    fn restores_desktop_values_only_while_kanade_still_owns_them() {
        let saved = SavedValue {
            before: Some(String::from("prefer-light")),
            applied: String::from("prefer-dark"),
            pending: None,
        };
        assert_eq!(
            restore_value(Some("prefer-dark"), &saved),
            Some(Some(String::from("prefer-light")))
        );
        assert_eq!(restore_value(Some("custom"), &saved), None);
    }

    #[test]
    fn a_stalled_helper_is_killed_and_reaped_by_its_deadline() {
        let (_sender, receiver) = mpsc::channel();
        let mut command = Command::new("/bin/sleep");
        command.arg("10");
        let started = Instant::now();

        let result = run(command, Duration::from_millis(60), &receiver);

        assert!(matches!(result, Helper::Done(Err(error)) if error.contains("timed out")));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_new_request_cancels_the_running_helper() {
        let (sender, receiver) = mpsc::channel();
        let newer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(60));
            sender.send(Request::Set(Tone::Light)).unwrap();
        });
        let mut command = Command::new("/bin/sleep");
        command.arg("10");

        let result = run(command, Duration::from_secs(2), &receiver);
        newer.join().unwrap();

        assert!(matches!(
            result,
            Helper::Superseded(Request::Set(Tone::Light))
        ));
    }
}
