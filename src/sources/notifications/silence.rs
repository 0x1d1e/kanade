//! Per-app silence: no Banner and no sound for that app, Critical included, history kept.
//! It is a control, not a config rule, and lives in a state file, not `settings.toml`. A matching
//! rule still wins, applied as the notification arrives. Unsilence restores the normal path for
//! the next one.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use kanade_runtime::MutexExt;
use kanade_runtime::service::Service;

use super::rules::{self, Fields};
use super::{Notification, Notifications};
use crate::atomic::{self, Mode};
use crate::banners::Banners;
use crate::config;

const FILE: &str = "kanade/notification-silence";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    // a desktop id with `.desktop` taken off, lower case; none when the app has no entry
    pub desktop: Option<String>,
    // the app's name, lower case; none when only a desktop id is known
    pub name: Option<String>,
}

impl App {
    pub fn named(name: &str, desktop: &str) -> Option<Self> {
        let name = token(name);
        let desktop = token(rules::desktop_id(desktop));
        if name.is_none() && desktop.is_none() {
            return None;
        }
        Some(Self { desktop, name })
    }

    fn matches_notice(&self, name: &str, desktop: &str) -> bool {
        let name = token(name);
        let desktop = token(rules::desktop_id(desktop));
        match (&self.desktop, &desktop) {
            (Some(have), Some(got)) => have == got,
            _ => self.name.is_some() && self.name == name,
        }
    }

    fn same(&self, other: &App) -> bool {
        match (&self.desktop, &other.desktop) {
            (Some(have), Some(got)) => have == got,
            _ => self.name.is_some() && self.name == other.name,
        }
    }
}

fn token(text: &str) -> Option<String> {
    let text = text.trim().to_lowercase();
    if text.is_empty() { None } else { Some(text) }
}

static APPS: Mutex<Option<Vec<App>>> = Mutex::new(None);

pub fn covers(name: &str, desktop: &str) -> bool {
    apps().iter().any(|app| app.matches_notice(name, desktop))
}

pub fn covers_app(app: &App) -> bool {
    apps().iter().any(|have| have.same(app))
}

// true when the app is silenced after the press
pub fn toggle(app: App) -> bool {
    let silenced = {
        let mut apps = APPS.locked();
        let apps = apps.get_or_insert_with(load);
        if let Some(index) = apps.iter().position(|have| have.same(&app)) {
            apps.remove(index);
            false
        } else {
            apps.push(app);
            true
        }
    };

    if let Err(error) = save() {
        eprintln!("kanade: notification silence: {error}");
    }

    if silenced {
        quiet_shown();
    }

    silenced
}

pub fn toggle_named(name: &str, desktop: &str) {
    if let Some(app) = App::named(name, desktop) {
        toggle(app);
    }
}

fn apps() -> Vec<App> {
    APPS.locked().get_or_insert_with(load).clone()
}

fn path() -> Option<PathBuf> {
    config::base(
        "XDG_STATE_HOME",
        ".local/state",
        std::env::var("HOME").ok().as_deref(),
    )
    .map(|dir| dir.join(FILE))
}

fn load() -> Vec<App> {
    let Some(path) = path() else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    parse(&text)
}

// one app a line, `desktop<TAB>name`, either field empty
fn parse(text: &str) -> Vec<App> {
    text.lines()
        .filter_map(|line| {
            let (desktop, name) = line.split_once('\t')?;
            App::named(name, desktop)
        })
        .collect()
}

fn save() -> Result<(), String> {
    let Some(path) = path() else {
        return Err("HOME is not set".to_owned());
    };
    let apps = apps();
    let mut text = String::new();
    for app in &apps {
        text.push_str(app.desktop.as_deref().unwrap_or(""));
        text.push('\t');
        text.push_str(app.name.as_deref().unwrap_or(""));
        text.push('\n');
    }
    atomic::write(&path, text.as_bytes(), Mode::Private).map_err(|error| error.to_string())
}

// banners already up for a newly silenced app go, history stays; a matching rule still shows
fn quiet_shown() {
    let rules = config::get().notification_rules.clone();
    let ids: Vec<u32> = Notifications::read()
        .list()
        .iter()
        .filter(|note| silenced_without_rule(note, &rules))
        .map(Notification::id)
        .collect();

    if ids.is_empty() {
        return;
    }

    let now = Instant::now();
    let mut banners = Banners::write();
    for id in ids {
        banners.close(id, now);
    }
}

fn silenced_without_rule(note: &Notification, rules: &[rules::Rule]) -> bool {
    let fields = Fields {
        app: note.app_name(),
        desktop_entry: note.desktop_entry(),
        category: note.category(),
        summary: note.summary(),
    };
    !rules::matched(rules, fields) && covers(note.app_name(), note.desktop_entry())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_silence_file_names_desktop_and_app() {
        let apps = parse("org.mozilla.firefox\tfirefox\n\tsignal\norg.kde.dolphin\t\n\nno tab\n");
        assert_eq!(
            apps,
            vec![
                App {
                    desktop: Some("org.mozilla.firefox".to_owned()),
                    name: Some("firefox".to_owned()),
                },
                App {
                    desktop: None,
                    name: Some("signal".to_owned()),
                },
                App {
                    desktop: Some("org.kde.dolphin".to_owned()),
                    name: None,
                },
            ]
        );
    }

    #[test]
    fn a_shared_name_does_not_cover_a_different_desktop_id() {
        let nautilus = App::named("Files", "org.gnome.nautilus").unwrap();
        let other = App::named("Files", "org.example.files").unwrap();
        let same_id = App::named("Nautilus", "org.gnome.nautilus").unwrap();
        let name_only = App::named("Files", "").unwrap();

        assert!(!nautilus.matches_notice("Files", "org.example.files"));
        assert!(nautilus.matches_notice("Anything", "org.gnome.nautilus"));
        assert!(nautilus.matches_notice("Files", ""));
        assert!(!nautilus.same(&other));
        assert!(nautilus.same(&same_id));
        assert!(nautilus.same(&name_only));
    }
}
