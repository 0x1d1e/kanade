//! Running windows and apps (#143, docs/design.md Modules, ADR 0014): niri's windows, grouped by
//! the app each belongs to, for the Dock. niri.rs reads them off the island's niri stream and hands
//! them here as `Heard`; this keeps them, matches each `app_id` to a `.desktop` entry and publishes
//! the result as the `Windows` Service, which holds nothing of niri's but its window ids.
//!
//! The `.desktop` entries are read once, when the first window comes or the Dock pins an app, and
//! read again only when an `app_id` or pinned id comes that none of them matches, or the config is
//! reloaded, so nothing polls.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::{env, fs};

use amane::Service;

use super::icons;
use crate::config;

// niri's window id, which stays with the window for as long as it is open
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub id: WindowId,

    // the Wayland app id; none until the app sets one, and some never do
    pub app_id: Option<String>,

    pub focused: bool,

    // asks for attention
    pub urgent: bool,
}

// what niri said of its windows, as niri.rs reads it
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Heard {
    // every window, replacing those known
    All(Vec<Window>),

    // one window, new or changed; one that is focused takes the focus from every other
    Opened(Window),

    Closed(WindowId),

    // none: no window has the focus
    Focused(Option<WindowId>),

    Urgent(WindowId, bool),

    // the stream ended, so no window is known
    Lost,
}

// the `.desktop` entry an app matched, as the Dock shows and launches it
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    // the desktop file id, like `org.kde.dolphin.desktop`
    pub id: String,

    pub name: String,

    // the icon's name in the icon theme, or a path
    pub icon: Option<String>,

    // the file `icon` stands for, found when the entry is published, so a view reads no theme
    pub icon_file: Option<PathBuf>,

    // what launches it: `Exec` with its field codes dropped, as no file or URL is given
    pub exec: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum App {
    Desktop(DesktopEntry),

    // no entry matched; its `app_id`, none for a window without one
    Unmatched(Option<String>),
}

// one app's open windows, by id
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub app: App,
    pub windows: Vec<Window>,
}

impl Running {
    pub fn focused(&self) -> bool {
        self.windows.iter().any(|window| window.focused)
    }

    pub fn urgent(&self) -> bool {
        self.windows.iter().any(|window| window.urgent)
    }
}

// an app the Dock pins, by desktop file id
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pinned {
    pub id: String,

    // none while no entry has the id
    pub entry: Option<DesktopEntry>,
}

// the running apps, in the order their first open window opened, and the pinned apps in their order
#[derive(Debug, Default)]
pub struct Windows {
    running: Vec<Running>,
    pinned: Vec<Pinned>,
}

impl Service for Windows {
    fn new() -> Self {
        Windows::default()
    }

    fn listen() {}
}

impl Windows {
    pub fn running(&self) -> &[Running] {
        &self.running
    }

    pub fn pinned(&self) -> &[Pinned] {
        &self.pinned
    }

    #[cfg(test)]
    pub fn with(running: Vec<Running>, pinned: Vec<Pinned>) -> Self {
        Windows { running, pinned }
    }

    // for `kanade status`
    pub fn status(&self) -> String {
        let windows: usize = self.running.iter().map(|app| app.windows.len()).sum();

        let mut line = format!("windows: {windows} windows, {} apps", self.running.len());

        if let Some(focused) = self.running.iter().find(|app| app.focused()) {
            line.push_str(&format!(", focused {}", name(&focused.app)));
        }

        let urgent: Vec<&str> = self
            .running
            .iter()
            .filter(|app| app.urgent())
            .map(|app| name(&app.app))
            .collect();

        if !urgent.is_empty() {
            line.push_str(&format!(", urgent {}", urgent.join(" ")));
        }

        let unmatched: Vec<&str> = self
            .running
            .iter()
            .filter(|app| matches!(app.app, App::Unmatched(_)))
            .map(|app| name(&app.app))
            .collect();

        if !unmatched.is_empty() {
            line.push_str(&format!(", unmatched {}", unmatched.join(" ")));
        }

        line
    }
}

fn name(app: &App) -> &str {
    match app {
        App::Desktop(entry) => &entry.id,
        App::Unmatched(app_id) => app_id.as_deref().unwrap_or("(no app_id)"),
    }
}

// what niri said, applied; the Service is written only when the running apps change
pub fn hear(heard: Heard) {
    let mut tracked = TRACKED.lock().unwrap_or_else(PoisonError::into_inner);

    if tracked.apply(heard) {
        tracked.publish(&data_dirs());
    }
}

// the desktop file ids the Dock pins, published with their entries whether they run or not
pub fn pin(ids: Vec<String>) {
    let mut tracked = TRACKED.lock().unwrap_or_else(PoisonError::into_inner);

    tracked.pins = ids;
    tracked.publish(&data_dirs());
}

// matches every window again, with the overrides of a reloaded config and the entries as they are now
pub fn rematch() {
    let mut tracked = TRACKED.lock().unwrap_or_else(PoisonError::into_inner);

    tracked.entries = None;
    tracked.missed.clear();
    tracked.publish(&data_dirs());
}

static TRACKED: Mutex<Tracked> = Mutex::new(Tracked {
    windows: BTreeMap::new(),
    pins: Vec::new(),
    entries: None,
    missed: Vec::new(),
    published: (Vec::new(), Vec::new()),
});

#[derive(Debug, Default)]
struct Tracked {
    windows: BTreeMap<WindowId, Window>,

    // the desktop file ids the Dock pins
    pins: Vec<String>,

    // none until a window or pin needs them
    entries: Option<Vec<Entry>>,

    // the app ids and pinned ids that matched nothing since the entries were last read, each once
    missed: Vec<String>,

    published: (Vec<Running>, Vec<Pinned>),
}

impl Tracked {
    // whether it changed any window; a title or layout change niri sends changes none
    fn apply(&mut self, heard: Heard) -> bool {
        let before = self.windows.clone();

        match heard {
            Heard::All(windows) => {
                self.windows = windows
                    .into_iter()
                    .map(|window| (window.id, window))
                    .collect();
            }
            Heard::Opened(window) => {
                if window.focused {
                    self.focus(Some(window.id));
                }
                self.windows.insert(window.id, window);
            }
            Heard::Closed(id) => {
                self.windows.remove(&id);
            }
            Heard::Focused(id) => self.focus(id),
            Heard::Urgent(id, urgent) => {
                if let Some(window) = self.windows.get_mut(&id) {
                    window.urgent = urgent;
                }
            }
            Heard::Lost => self.windows.clear(),
        }

        self.windows != before
    }

    fn focus(&mut self, id: Option<WindowId>) {
        for window in self.windows.values_mut() {
            window.focused = Some(window.id) == id;
        }
    }

    // groups the windows by app, finds the pinned apps, and writes the Service if that changed
    fn publish(&mut self, dirs: &[PathBuf]) {
        let published = (self.running(dirs), self.pinned(dirs));

        if published != self.published {
            let mut windows = Windows::write();

            windows.running.clone_from(&published.0);
            windows.pinned.clone_from(&published.1);
            drop(windows);

            self.published = published;
        }
    }

    // each pinned id with its entry, reading the entries again once for an id none has
    fn pinned(&mut self, dirs: &[PathBuf]) -> Vec<Pinned> {
        if self.pins.is_empty() {
            return Vec::new();
        }

        let entries = self.entries.get_or_insert_with(|| scan(dirs));
        let find = |entries: &[Entry], id: &str| {
            entries
                .iter()
                .find(|entry| entry.desktop.id == id)
                .map(shown)
        };

        let new_misses = self
            .pins
            .iter()
            .any(|id| find(entries, id).is_none() && !self.missed.contains(id));

        if new_misses {
            *entries = scan(dirs);
        }

        let mut pinned = Vec::new();

        for id in &self.pins {
            let entry = find(entries, id);

            if entry.is_none() && !self.missed.contains(id) {
                self.missed.push(id.clone());
            }

            pinned.push(Pinned {
                id: id.clone(),
                entry,
            });
        }

        pinned
    }

    /*
     * the windows by app, reading the entries again once for app ids that match none, in case
     * their app was installed since; one that still matches none is not read for again
     */
    fn running(&mut self, dirs: &[PathBuf]) -> Vec<Running> {
        if self.windows.is_empty() {
            return Vec::new();
        }

        let config = config::get();
        let overrides = &config.apps;

        let entries = self.entries.get_or_insert_with(|| scan(dirs));
        let new_misses = self.windows.values().any(|window| {
            window.app_id.as_deref().is_some_and(|app_id| {
                matched(app_id, overrides, entries).is_none()
                    && !self.missed.iter().any(|missed| missed == app_id)
            })
        });

        if new_misses {
            *entries = scan(dirs);
        }

        let mut running: Vec<Running> = Vec::new();

        for window in self.windows.values() {
            let app = match window.app_id.as_deref() {
                Some(app_id) => match matched(app_id, overrides, entries) {
                    Some(entry) => App::Desktop(shown(entry)),
                    None => {
                        if !self.missed.iter().any(|missed| missed == app_id) {
                            self.missed.push(app_id.to_owned());
                        }
                        App::Unmatched(Some(app_id.to_owned()))
                    }
                },
                None => App::Unmatched(None),
            };

            // windows come by id, so each app's first window opened first
            let same = running
                .iter_mut()
                .find(|running| running.app == app && app != App::Unmatched(None));

            match same {
                Some(running) => running.windows.push(window.clone()),
                None => running.push(Running {
                    app,
                    windows: vec![window.clone()],
                }),
            }
        }

        running
    }
}

// the entry as published, its icon found; each name is looked up once a run
fn shown(entry: &Entry) -> DesktopEntry {
    let mut desktop = entry.desktop.clone();

    desktop.icon_file = desktop
        .icon
        .as_deref()
        .and_then(|icon| icons::find(icon, None));
    desktop
}

// a `.desktop` entry as matching needs it
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    desktop: DesktopEntry,

    // `StartupWMClass`: the app id its windows have, when that is not its file id
    wm_class: Option<String>,
}

/*
 * the entry an app id belongs to (ADR 0014), first of: the override in `windows.apps`, which is
 * final; the file id `<app_id>.desktop`; `StartupWMClass`; either ignoring case; the file id's last
 * dot-separated part ignoring case, so `dolphin` is `org.kde.dolphin.desktop`. Within one rule the
 * entry read first wins, so the user's own files win over the system's
 */
fn matched<'a>(
    app_id: &str,
    overrides: &BTreeMap<String, String>,
    entries: &'a [Entry],
) -> Option<&'a Entry> {
    if let Some(id) = overrides.get(app_id) {
        return entries.iter().find(|entry| entry.desktop.id == *id);
    }

    let file = format!("{app_id}.desktop");
    let stem = |entry: &Entry| entry.desktop.id.strip_suffix(".desktop").map(String::from);

    let rules: [&dyn Fn(&Entry) -> bool; 5] = [
        &|entry| entry.desktop.id == file,
        &|entry| entry.wm_class.as_deref() == Some(app_id),
        &|entry| entry.desktop.id.eq_ignore_ascii_case(&file),
        &|entry| {
            entry
                .wm_class
                .as_deref()
                .is_some_and(|class| class.eq_ignore_ascii_case(app_id))
        },
        &|entry| {
            stem(entry).is_some_and(|stem| {
                stem.rsplit('.')
                    .next()
                    .is_some_and(|last| last.eq_ignore_ascii_case(app_id))
            })
        },
    ];

    rules
        .iter()
        .find_map(|rule| entries.iter().find(|entry| rule(entry)))
}

// the `applications` dirs of the XDG data dirs, the user's first, so their files win
fn data_dirs() -> Vec<PathBuf> {
    let home = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| env::var_os("HOME").map(|home| Path::new(&home).join(".local/share")));

    // unset or empty means these two, as the spec says
    let system = env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| String::from("/usr/local/share:/usr/share"));

    home.into_iter()
        .chain(
            system
                .split(':')
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from),
        )
        .map(|dir| dir.join("applications"))
        .collect()
}

// how deep in `applications` a file may be, so a link loop ends
const DEPTH: usize = 8;

/*
 * every application entry in the dirs, in order: an id in an earlier dir hides the same id in a
 * later one, even when it is `Hidden`, which is how a user deletes a system entry. A file that
 * cannot be read, or is no desktop entry, hides nothing, as the spec skips it
 */
fn scan(dirs: &[PathBuf]) -> Vec<Entry> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();

    for dir in dirs {
        let mut files = Vec::new();
        walk(dir, DEPTH, &mut files);
        files.sort();

        for path in files {
            let Some(id) = id(dir, &path) else {
                continue;
            };

            if seen.contains(&id) {
                continue;
            }

            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };

            match read(id.clone(), &text) {
                Read::App(entry) => entries.push(entry),
                Read::Other => {}
                Read::Invalid => continue,
            }

            seen.insert(id);
        }
    }

    entries
}

// follows links, which is how nix puts files into the profile dirs; a missing dir has no files
fn walk(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };

    for path in read.flatten().map(|entry| entry.path()) {
        if path.is_dir() {
            if depth > 0 {
                walk(&path, depth - 1, files);
            }
        } else {
            files.push(path);
        }
    }
}

// the desktop file id: its path inside `applications`, with `/` turned into `-`
fn id(dir: &Path, path: &Path) -> Option<String> {
    if path.extension()? != "desktop" {
        return None;
    }

    Some(path.strip_prefix(dir).ok()?.to_str()?.replace('/', "-"))
}

// what a `.desktop` file is
#[derive(Debug, PartialEq, Eq)]
enum Read {
    // an application; `NoDisplay` still names its windows
    App(Entry),

    // an entry, so it hides the same id in a later dir, but no app: `Hidden`, meaning deleted, or of
    // another `Type`
    Other,

    // no desktop entry: no `[Desktop Entry]`, or no `Type` or `Name`
    Invalid,
}

fn read(id: String, text: &str) -> Read {
    let mut fields = BTreeMap::new();
    let mut inside = false;
    let mut group = false;

    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == "[Desktop Entry]";
            group |= inside;
        } else if inside && let Some((key, value)) = line.split_once('=') {
            // the first wins, as the spec forbids a key twice
            fields.entry(key.trim()).or_insert(value.trim());
        }
    }

    let field = |key| {
        fields
            .get(key)
            .filter(|value| !value.is_empty())
            .map(|value| unescape(value))
    };

    if !group {
        return Read::Invalid;
    }

    // `Hidden` alone deletes, with no `Type` or `Name`
    if fields.get("Hidden") == Some(&"true") {
        return Read::Other;
    }

    let (Some(kind), Some(name)) = (field("Type"), field("Name")) else {
        return Read::Invalid;
    };

    if kind != "Application" {
        return Read::Other;
    }

    Read::App(Entry {
        desktop: DesktopEntry {
            id,
            name,
            icon: field("Icon"),
            icon_file: None,
            exec: field("Exec")
                .map(|exec| without_field_codes(&exec))
                .filter(|exec| !exec.is_empty()),
        },
        wm_class: field("StartupWMClass"),
    })
}

// `%f`, `%U` and the others dropped, `%%` kept as `%`, as Amane launches its apps
fn without_field_codes(exec: &str) -> String {
    let mut command = String::with_capacity(exec.len());
    let mut chars = exec.chars();

    while let Some(char) = chars.next() {
        if char != '%' {
            command.push(char);
        } else if chars.next() == Some('%') {
            command.push('%');
        }
    }

    command.trim().to_owned()
}

// a string value as written, with the spec's escapes decoded; another escape stays as it is
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();

    while let Some(char) = chars.next() {
        if char != '\\' {
            out.push(char);
            continue;
        }

        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, wm_class: Option<&str>) -> Entry {
        Entry {
            desktop: DesktopEntry {
                id: id.to_owned(),
                name: id.to_owned(),
                icon: None,
                icon_file: None,
                exec: None,
            },
            wm_class: wm_class.map(String::from),
        }
    }

    // the id of the entry `app_id` matches among `entries`
    fn match_id(app_id: &str, overrides: &[(&str, &str)], entries: &[Entry]) -> Option<String> {
        let overrides = overrides
            .iter()
            .map(|&(app_id, id)| (app_id.to_owned(), id.to_owned()))
            .collect();

        matched(app_id, &overrides, entries).map(|entry| entry.desktop.id.clone())
    }

    fn entries() -> Vec<Entry> {
        vec![
            entry("firefox.desktop", Some("firefox")),
            entry("org.telegram.desktop.desktop", Some("TelegramDesktop")),
            entry("siyuan.desktop", Some("org.b3log.siyuan")),
            entry("org.kde.dolphin.desktop", Some("dolphin")),
            entry("org.gnome.Nautilus.desktop", None),
            entry("org.kde.konsole.desktop", None),
            entry("Alacritty.desktop", None),
            entry("code.desktop", Some("Code")),
            entry("code-url-handler.desktop", Some("Code")),
        ]
    }

    #[test]
    fn the_file_id_matches_first() {
        let entries = entries();

        assert_eq!(
            match_id("firefox", &[], &entries).as_deref(),
            Some("firefox.desktop")
        );
        assert_eq!(
            match_id("org.telegram.desktop", &[], &entries).as_deref(),
            Some("org.telegram.desktop.desktop")
        );
        assert_eq!(
            match_id("org.gnome.Nautilus", &[], &entries).as_deref(),
            Some("org.gnome.Nautilus.desktop")
        );
    }

    #[test]
    fn startup_wm_class_names_an_app_whose_id_is_not_its_file() {
        let entries = entries();

        assert_eq!(
            match_id("org.b3log.siyuan", &[], &entries).as_deref(),
            Some("siyuan.desktop")
        );
        assert_eq!(
            match_id("dolphin", &[], &entries).as_deref(),
            Some("org.kde.dolphin.desktop")
        );

        // two entries with one class: the first read
        assert_eq!(
            match_id("Code", &[], &entries).as_deref(),
            Some("code.desktop")
        );
    }

    // an exact rule beats a looser one even for an entry read later
    #[test]
    fn exact_rules_beat_loose_ones() {
        let entries = vec![entry("Kitty.desktop", None), entry("kitty.desktop", None)];

        assert_eq!(
            match_id("kitty", &[], &entries).as_deref(),
            Some("kitty.desktop")
        );

        let entries = vec![
            entry("org.example.Code.desktop", None),
            entry("ide.desktop", Some("code")),
        ];

        assert_eq!(
            match_id("code", &[], &entries).as_deref(),
            Some("ide.desktop")
        );
    }

    #[test]
    fn case_and_the_last_part_of_a_reverse_dns_id_match_last() {
        let entries = entries();

        assert_eq!(
            match_id("alacritty", &[], &entries).as_deref(),
            Some("Alacritty.desktop")
        );
        assert_eq!(
            match_id("telegramdesktop", &[], &entries).as_deref(),
            Some("org.telegram.desktop.desktop")
        );
        assert_eq!(
            match_id("konsole", &[], &entries).as_deref(),
            Some("org.kde.konsole.desktop")
        );

        // only the file id's last part: an app id's own is no match
        assert_eq!(match_id("org.example.firefox", &[], &entries), None);
    }

    #[test]
    fn an_app_id_nothing_names_is_unmatched() {
        assert_eq!(match_id("steam_app_1086940", &[], &entries()), None);
        assert_eq!(match_id("", &[], &entries()), None);
        assert_eq!(match_id("firefox", &[], &[]), None);
    }

    #[test]
    fn an_override_wins_and_is_final() {
        let entries = entries();

        assert_eq!(
            match_id("firefox", &[("firefox", "code.desktop")], &entries).as_deref(),
            Some("code.desktop")
        );
        assert_eq!(
            match_id(
                "jetbrains-idea",
                &[("jetbrains-idea", "org.kde.konsole.desktop")],
                &entries
            )
            .as_deref(),
            Some("org.kde.konsole.desktop")
        );

        // an override naming an entry that is not there leaves the app unmatched, not guessed
        assert_eq!(
            match_id("firefox", &[("firefox", "gone.desktop")], &entries),
            None
        );

        // keyed by the exact app id
        assert_eq!(
            match_id("Firefox", &[("firefox", "code.desktop")], &entries).as_deref(),
            Some("firefox.desktop")
        );
    }

    #[test]
    fn a_desktop_file_reads_its_main_section() {
        let text = "\
# comment
[Desktop Entry]
Type=Application
Name=Files
Name[de]=Dateien
Icon = org.gnome.Nautilus
StartupWMClass=
Exec=nautilus --new-window

[Desktop Action new-window]
Name=New Window
StartupWMClass=other
";

        assert_eq!(
            read(String::from("org.gnome.Nautilus.desktop"), text),
            Read::App(Entry {
                desktop: DesktopEntry {
                    id: String::from("org.gnome.Nautilus.desktop"),
                    name: String::from("Files"),
                    icon: Some(String::from("org.gnome.Nautilus")),
                    icon_file: None,
                    exec: Some(String::from("nautilus --new-window")),
                },
                wm_class: None,
            })
        );
    }

    #[test]
    fn exec_drops_its_field_codes() {
        let exec = |line: &str| {
            let Read::App(entry) = read(String::from("a.desktop"), &format!("{APP}A\n{line}"))
            else {
                panic!("an app");
            };

            entry.desktop.exec
        };

        assert_eq!(exec("Exec=firefox %u").as_deref(), Some("firefox"));
        assert_eq!(
            exec("Exec=code --new-window %F").as_deref(),
            Some("code --new-window")
        );
        assert_eq!(exec("Exec=printf 100%%").as_deref(), Some("printf 100%"));
        assert_eq!(exec("Exec=%U"), None);
        assert_eq!(exec(""), None);
    }

    // the spec's escapes in a string value: `\s`, `\n`, `\t`, `\r`, `\\`; any other stays as written
    #[test]
    fn string_values_decode_their_escapes() {
        let text = "[Desktop Entry]\nType=Application\nName=Foo\\sBar\\t\\\\s\\q\nIcon=foo\\\\bar\nStartupWMClass=a\\sb\\";
        let Read::App(entry) = read(String::from("a.desktop"), text) else {
            panic!("an app");
        };

        assert_eq!(entry.desktop.name, "Foo Bar\t\\s\\q");
        assert_eq!(entry.desktop.icon.as_deref(), Some("foo\\bar"));
        assert_eq!(entry.wm_class.as_deref(), Some("a b\\"));
    }

    #[test]
    fn only_applications_that_are_not_hidden_are_entries() {
        let read = |text: &str| read(String::from("a.desktop"), text);
        let app = |extra: &str| {
            read(&format!(
                "[Desktop Entry]\nType=Application\nName=A\n{extra}"
            ))
        };

        assert!(matches!(app(""), Read::App(_)));
        assert!(matches!(app("NoDisplay=true"), Read::App(_)));
        assert!(matches!(app("Hidden=false"), Read::App(_)));
        assert_eq!(app("Hidden=true"), Read::Other);
        assert_eq!(read("[Desktop Entry]\nHidden=true"), Read::Other);
        assert_eq!(read("[Desktop Entry]\nType=Link\nName=A"), Read::Other);

        assert_eq!(read("[Desktop Entry]\nType=Application"), Read::Invalid);
        assert_eq!(read("[Desktop Entry]\nName=A"), Read::Invalid);
        assert_eq!(read("Type=Application\nName=A"), Read::Invalid);
        assert_eq!(read("Hidden=true"), Read::Invalid);
        assert_eq!(read(""), Read::Invalid);
    }

    // a fresh dir under the temp dir, gone first if a run before left it
    fn temp(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("kanade-windows-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, file: &str, text: &str) {
        let path = dir.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    const APP: &str = "[Desktop Entry]\nType=Application\nName=";

    #[test]
    fn the_scan_reads_ids_in_order_and_the_user_hides_the_system() {
        let root = temp("scan");
        let (user, system) = (root.join("user"), root.join("system"));

        write(&user, "editor.desktop", &format!("{APP}Mine"));
        write(&user, "gone.desktop", &format!("{APP}Gone\nHidden=true"));
        write(&system, "editor.desktop", &format!("{APP}Theirs"));
        write(&system, "gone.desktop", &format!("{APP}Gone"));
        write(&system, "kde/dolphin.desktop", &format!("{APP}Dolphin"));
        write(&system, "a.desktop", &format!("{APP}A"));
        write(&system, "notes.txt", &format!("{APP}Notes"));

        let read: Vec<(String, String)> = scan(&[user, system, root.join("missing")])
            .into_iter()
            .map(|entry| (entry.desktop.id, entry.desktop.name))
            .collect();

        fs::remove_dir_all(&root).unwrap();

        assert_eq!(
            read,
            [
                (String::from("editor.desktop"), String::from("Mine")),
                (String::from("a.desktop"), String::from("A")),
                (String::from("kde-dolphin.desktop"), String::from("Dolphin")),
            ]
        );
    }

    // a user's file that cannot be read, or is no desktop entry, leaves the id to the system's
    #[test]
    fn an_unreadable_or_invalid_user_entry_falls_back_to_the_system() {
        let root = temp("fallback");
        let (user, system) = (root.join("user"), root.join("system"));

        fs::create_dir_all(&user).unwrap();
        std::os::unix::fs::symlink(root.join("nowhere"), user.join("firefox.desktop")).unwrap();
        write(&user, "editor.desktop", "not a desktop entry");
        write(
            &user,
            "nameless.desktop",
            "[Desktop Entry]\nType=Application",
        );
        write(&system, "firefox.desktop", &format!("{APP}Firefox"));
        write(&system, "editor.desktop", &format!("{APP}Editor"));
        write(&system, "nameless.desktop", &format!("{APP}Named"));

        let read: Vec<String> = scan(&[user, system])
            .into_iter()
            .map(|entry| entry.desktop.name)
            .collect();

        fs::remove_dir_all(&root).unwrap();

        assert_eq!(read, ["Editor", "Firefox", "Named"]);
    }

    fn window(id: u64, app_id: Option<&str>, focused: bool) -> Window {
        Window {
            id: WindowId(id),
            app_id: app_id.map(String::from),
            focused,
            urgent: false,
        }
    }

    // each running app as its name and window ids, the focused one marked
    fn shown(running: &[Running]) -> Vec<String> {
        running
            .iter()
            .map(|app| {
                let ids: Vec<String> = app
                    .windows
                    .iter()
                    .map(|window| window.id.0.to_string())
                    .collect();
                let mark = if app.focused() { "*" } else { "" };

                format!("{}{mark} {}", name(&app.app), ids.join(","))
            })
            .collect()
    }

    // a tracker over one applications dir holding `files`
    fn tracker(name: &str, files: &[&str]) -> (Tracked, Vec<PathBuf>, PathBuf) {
        let dir = temp(name);

        for file in files {
            write(&dir, &format!("{file}.desktop"), &format!("{APP}{file}"));
        }

        (Tracked::default(), vec![dir.clone()], dir)
    }

    #[test]
    fn windows_group_by_app_in_the_order_apps_opened() {
        let (mut tracked, dirs, dir) = tracker("group", &["firefox", "kitty"]);

        tracked.apply(Heard::All(vec![
            window(9, Some("kitty"), false),
            window(3, Some("firefox"), false),
            window(5, Some("kitty"), true),
            window(7, None, false),
            window(8, None, false),
            window(4, Some("steam_app_1"), false),
        ]));

        assert_eq!(
            shown(&tracked.running(&dirs)),
            [
                "firefox.desktop 3",
                "steam_app_1 4",
                "kitty.desktop* 5,9",
                "(no app_id) 7",
                "(no app_id) 8",
            ]
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn open_close_focus_and_urgency_follow_niri() {
        let (mut tracked, dirs, dir) = tracker("follow", &["firefox", "kitty"]);

        assert!(tracked.apply(Heard::All(vec![window(1, Some("kitty"), true)])));
        assert!(tracked.apply(Heard::Opened(window(2, Some("firefox"), true))));
        assert_eq!(
            shown(&tracked.running(&dirs)),
            ["kitty.desktop 1", "firefox.desktop* 2"]
        );

        // a title change niri sends again changes nothing
        assert!(!tracked.apply(Heard::Opened(window(2, Some("firefox"), true))));

        assert!(tracked.apply(Heard::Focused(Some(WindowId(1)))));
        assert_eq!(
            shown(&tracked.running(&dirs)),
            ["kitty.desktop* 1", "firefox.desktop 2"]
        );

        assert!(tracked.apply(Heard::Urgent(WindowId(2), true)));
        assert!(tracked.running(&dirs)[1].urgent());
        assert!(!tracked.apply(Heard::Urgent(WindowId(9), true)));

        assert!(tracked.apply(Heard::Focused(None)));
        assert!(!tracked.running(&dirs).iter().any(Running::focused));

        assert!(tracked.apply(Heard::Closed(WindowId(1))));
        assert_eq!(shown(&tracked.running(&dirs)), ["firefox.desktop 2"]);
        assert!(!tracked.apply(Heard::Closed(WindowId(1))));

        // an app id set late moves the window to its app
        assert!(tracked.apply(Heard::Opened(window(3, None, false))));
        assert!(tracked.apply(Heard::Opened(window(3, Some("firefox"), false))));
        assert_eq!(shown(&tracked.running(&dirs)), ["firefox.desktop 2,3"]);

        assert!(tracked.apply(Heard::Lost));
        assert_eq!(tracked.running(&dirs), []);

        fs::remove_dir_all(dir).unwrap();
    }

    // an app installed while its window was unknown matches once a new app id makes the scan run again
    #[test]
    fn a_new_unmatched_app_id_reads_the_entries_again_once() {
        let (mut tracked, dirs, dir) = tracker("rescan", &["kitty"]);

        tracked.apply(Heard::All(vec![window(1, Some("kitty"), false)]));
        assert_eq!(shown(&tracked.running(&dirs)), ["kitty.desktop 1"]);

        write(&dir, "firefox.desktop", &format!("{APP}firefox"));
        tracked.apply(Heard::Opened(window(2, Some("firefox"), false)));
        assert_eq!(
            shown(&tracked.running(&dirs)),
            ["kitty.desktop 1", "firefox.desktop 2"]
        );

        // one that matches nothing is read for once, then stays unmatched
        tracked.apply(Heard::Opened(window(3, Some("zed"), false)));
        tracked.running(&dirs);
        assert_eq!(tracked.missed, ["zed"]);

        write(&dir, "zed.desktop", &format!("{APP}zed"));
        tracked.apply(Heard::Opened(window(4, Some("zed"), false)));
        assert_eq!(shown(&tracked.running(&dirs))[2], "zed 3,4");

        fs::remove_dir_all(dir).unwrap();
    }

    // a pinned id is found whether or not it runs; one none has is read for once, then stays missing
    #[test]
    fn pinned_ids_find_their_entries() {
        let (mut tracked, dirs, dir) = tracker("pinned", &["firefox"]);

        tracked.pins = vec![String::from("firefox.desktop"), String::from("zed.desktop")];

        let found = |pinned: Vec<Pinned>| -> Vec<(String, Option<String>)> {
            pinned
                .into_iter()
                .map(|pinned| (pinned.id, pinned.entry.map(|entry| entry.name)))
                .collect()
        };

        assert_eq!(
            found(tracked.pinned(&dirs)),
            [
                (
                    String::from("firefox.desktop"),
                    Some(String::from("firefox"))
                ),
                (String::from("zed.desktop"), None),
            ]
        );
        assert_eq!(tracked.missed, ["zed.desktop"]);

        // installed since, but read for already: found once the entries are read again
        write(&dir, "zed.desktop", &format!("{APP}zed"));
        assert_eq!(tracked.pinned(&dirs)[1].entry, None);

        tracked.entries = None;
        tracked.missed.clear();
        assert_eq!(
            tracked.pinned(&dirs)[1]
                .entry
                .as_ref()
                .map(|entry| entry.name.as_str()),
            Some("zed")
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn status_names_the_focused_and_unmatched_apps() {
        let firefox = App::Desktop(entry("firefox.desktop", None).desktop);
        let windows = Windows {
            running: vec![
                Running {
                    app: firefox,
                    windows: vec![
                        window(1, Some("firefox"), true),
                        window(2, Some("firefox"), false),
                    ],
                },
                Running {
                    app: App::Unmatched(Some(String::from("steam_app_1"))),
                    windows: vec![Window {
                        urgent: true,
                        ..window(3, Some("steam_app_1"), false)
                    }],
                },
                Running {
                    app: App::Unmatched(None),
                    windows: vec![window(4, None, false)],
                },
            ],
            pinned: Vec::new(),
        };

        assert_eq!(
            windows.status(),
            "windows: 4 windows, 3 apps, focused firefox.desktop, urgent steam_app_1, unmatched steam_app_1 (no app_id)"
        );
        assert_eq!(Windows::default().status(), "windows: 0 windows, 0 apps");
    }
}
