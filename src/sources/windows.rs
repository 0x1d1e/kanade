//! Running windows and apps (#143, docs/design.md Modules, ADR 0014): niri's windows, grouped by
//! the app each belongs to, for the Dock. niri.rs reads them off its stream and hands them here as
//! `Heard`, which it defines; this keeps them, matches each `app_id` to a `.desktop` entry and
//! publishes the result as the `Windows` Service, which holds nothing of niri's but its window ids.
//!
//! The `.desktop` entries are read once, when the first window comes or the Dock pins an app, and
//! read again only when an `app_id` or pinned id comes that none of them matches, or the config is
//! reloaded, so nothing polls.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use kanade_runtime::service::Service;

use super::desktop::{self, Entry};
use super::icons;
use super::launch::Launch;
use super::niri::{Heard, Window, WindowId};
use crate::config;

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

    // how it starts, none for an entry with nothing to run
    pub launch: Option<Launch>,
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
        tracked.publish(&desktop::dirs());
    }
}

// the desktop file ids the Dock pins, published with their entries whether they run or not
pub fn pin(ids: Vec<String>) {
    let mut tracked = TRACKED.lock().unwrap_or_else(PoisonError::into_inner);

    tracked.pins = ids;
    tracked.publish(&desktop::dirs());
}

// matches every window again, with the overrides of a reloaded config and the entries as they are now
pub fn rematch() {
    let mut tracked = TRACKED.lock().unwrap_or_else(PoisonError::into_inner);

    tracked.entries = None;
    tracked.missed.clear();
    tracked.publish(&desktop::dirs());
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

        let entries = self.entries.get_or_insert_with(|| desktop::scan(dirs));
        let find =
            |entries: &[Entry], id: &str| entries.iter().find(|entry| entry.id == id).map(shown);

        let new_misses = self
            .pins
            .iter()
            .any(|id| find(entries, id).is_none() && !self.missed.contains(id));

        if new_misses {
            *entries = desktop::scan(dirs);
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

        let entries = self.entries.get_or_insert_with(|| desktop::scan(dirs));
        let new_misses = self.windows.values().any(|window| {
            window.app_id.as_deref().is_some_and(|app_id| {
                matched(app_id, overrides, entries).is_none()
                    && !self.missed.iter().any(|missed| missed == app_id)
            })
        });

        if new_misses {
            *entries = desktop::scan(dirs);
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
    DesktopEntry {
        id: entry.id.clone(),
        name: entry.name.clone(),
        icon: entry.icon.clone(),
        icon_file: entry
            .icon
            .as_deref()
            .and_then(|icon| icons::find(icon, None)),
        launch: entry.launch.clone(),
    }
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
        return entries.iter().find(|entry| entry.id == *id);
    }

    let file = format!("{app_id}.desktop");
    let stem = |entry: &Entry| entry.id.strip_suffix(".desktop").map(String::from);

    let rules: [&dyn Fn(&Entry) -> bool; 5] = [
        &|entry| entry.id == file,
        &|entry| entry.wm_class.as_deref() == Some(app_id),
        &|entry| entry.id.eq_ignore_ascii_case(&file),
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
