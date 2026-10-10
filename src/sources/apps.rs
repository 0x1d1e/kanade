//! The apps the Launcher lists (#30, #183): the desktop entries no menu is told to hide, with how
//! each starts (`launch`), the same entries and launch as the Dock's. Read off the view thread when
//! the Launcher's Module starts and again each time the Launcher opens, so an app installed since
//! shows without polling; the Service is written only when the list changed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use kanade_runtime::service::Service;

use super::desktop::{self, Entry};
use super::icons;
use super::launch::Launch;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub name: String,

    // `Comment`, else `GenericName`
    pub description: Option<String>,

    // the entry's `Icon`, a name or a path, which a notification may give for its sender
    pub icon: Option<String>,

    // the icon's file, found when the list is read, so a view reads no theme
    pub icon_file: Option<PathBuf>,

    pub launch: Launch,
}

impl App {
    // what a search reads of how it starts: `Exec`, nothing for an app that only activates
    pub fn command(&self) -> &str {
        self.launch.command().unwrap_or_default()
    }
}

#[derive(Debug, Default)]
pub struct Apps {
    // none until first read
    list: Option<Vec<App>>,
}

impl Service for Apps {
    fn new() -> Self {
        Apps::default()
    }

    fn listen() {}
}

impl Apps {
    // by name, ignoring case
    pub fn list(&self) -> &[App] {
        self.list.as_deref().unwrap_or_default()
    }

    // whether they were read yet
    pub fn found(&self) -> bool {
        self.list.is_some()
    }
}

static READING: AtomicBool = AtomicBool::new(false);

// reads the apps again on a thread of its own; asked while a read runs, it leaves that read to
// answer, so an app installed just before may show only from the next refresh
pub fn refresh() {
    if READING.swap(true, Ordering::AcqRel) {
        return;
    }

    thread::spawn(|| {
        let list = listed(desktop::scan(&desktop::dirs()));

        // a write wakes every window even when nothing changed
        if Apps::read().list.as_ref() != Some(&list) {
            Apps::write().list = Some(list);
        }

        READING.store(false, Ordering::Release);
    });
}

// the entries a menu shows and that can start, by name
fn listed(entries: Vec<Entry>) -> Vec<App> {
    let mut apps: Vec<App> = entries
        .into_iter()
        .filter(|entry| !entry.no_display)
        .filter_map(|entry| {
            Some(App {
                icon_file: entry
                    .icon
                    .as_deref()
                    .and_then(|icon| icons::find(icon, None)),
                icon: entry.icon,
                launch: entry.launch?,
                name: entry.name,
                description: entry.description,
            })
        })
        .collect();

    apps.sort_by_key(|app| app.name.to_lowercase());

    apps
}
