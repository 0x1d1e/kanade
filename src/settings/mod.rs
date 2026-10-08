//! The Settings window (#148, docs/design.md Settings): a floated normal window over the schema
//! registry, a page for each Module that owns settings, each setting drawn by its Kind. A change
//! writes the settings file (`file`), the top layer, which the reload watch then applies like any
//! other edit: a live-safe key at once, one that takes a restart as pending. It never writes the
//! config directory, so the user's own files and a Nix-managed config stay as they are.
//!
//! The window reads only the `Settings` Service, a snapshot of the files taken when it opens,
//! after each write and after each reload, so a view never reads a file while drawing.

mod file;
mod view;

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io;

use amane::{Service, TextInput};
use toml::Value;

use crate::config::{self, Config, Kind, Setting};
use crate::modules::{self, Module};
use crate::reload;

// the window's name, which tells it apart from any other Amane opens
const WINDOW: &str = "settings";

// what the window shows, read from the files
#[derive(Default)]
pub struct Settings {
    // the Module whose settings show
    page: &'static str,

    // the config the files give now, the settings file included, which a reload applies
    read: Config,

    // the same without the settings file: what a key there overrides
    below: Config,

    // the settings file as it stands
    file: toml::Table,

    // why the settings file cannot be read whole, which also refuses every write
    unreadable: Option<String>,

    // why the last change was not written
    refused: Option<String>,

    // the last reload's problems, so a change that did not apply says why
    error: Option<String>,

    // the keys whose value in the files waits for a restart
    pending: Vec<String>,

    // what in the files does not apply, which a change must fix before it is written
    skipped: Vec<String>,
}

impl Service for Settings {
    fn new() -> Self {
        Settings {
            page: modules::CORE,
            ..Settings::default()
        }
    }

    fn listen() {}
}

impl Settings {
    // whether the settings file sets the key at `path`
    fn set_here(&self, path: &[String]) -> bool {
        let [first, rest @ ..] = path else {
            return false;
        };

        let mut value = self.file.get(first);
        for key in rest {
            value = value.and_then(|value| value.get(key));
        }

        value.is_some()
    }

    fn pending(&self, key: &str) -> bool {
        self.pending.iter().any(|pending| pending == key)
    }
}

// what of the files the window shows, read off the draw thread's Service lock
struct Snapshot {
    read: Config,
    below: Config,
    file: toml::Table,
    unreadable: Option<String>,
    error: Option<String>,
    pending: Vec<String>,
    skipped: Vec<String>,
}

impl Snapshot {
    fn take() -> Self {
        let (read, skipped) = config::layers(true);
        let (below, _) = config::layers(false);

        let (file, unreadable) =
            match config::settings_file().map(|path| match fs::read_to_string(&path) {
                Ok(text) => config::settings_table(&text)
                    .map_err(|problem| format!("{}:{problem}", path.display())),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(toml::Table::new()),
                Err(error) => Err(format!("{} unreadable: {error}", path.display())),
            }) {
                Some(Ok(file)) => (file, None),
                Some(Err(problem)) => (toml::Table::new(), Some(problem)),
                None => (toml::Table::new(), None),
            };

        Snapshot {
            pending: config::pending(&config::get(), &read),
            read,
            below,
            file,
            unreadable,
            error: reload::error(),
            skipped,
        }
    }
}

// the files read again into the Service, keeping the page and why the last change was refused
pub fn refresh() {
    let snapshot = Snapshot::take();
    let mut settings = Settings::write();

    settings.read = snapshot.read;
    settings.below = snapshot.below;
    settings.file = snapshot.file;
    settings.unreadable = snapshot.unreadable;
    settings.error = snapshot.error;
    settings.pending = snapshot.pending;
    settings.skipped = snapshot.skipped;
}

// the Modules that own settings, each a page, in the registry's order
pub fn pages() -> impl Iterator<Item = &'static Module> {
    modules::ALL
        .iter()
        .filter(|module| !module.settings.is_empty())
}

// the page of this name
pub fn page(name: &str) -> Option<&'static str> {
    pages()
        .find(|module| module.name == name)
        .map(|module| module.name)
}

/*
 * opens the window on a page, or the one it last showed; one already open only turns to the page.
 * On the draw thread, which the text inputs live on
 */
pub fn open(page: Option<&'static str>) {
    refresh();

    {
        let mut settings = Settings::write();
        settings.refused = None;
        if let Some(page) = page {
            settings.page = page;
        }
    }

    reset_inputs();
    amane::open_window(WINDOW, view::window);
}

pub fn close() {
    amane::close_window(WINDOW);
}

/*
 * writes the key at `path`, none removing it, and shows the files as they then are; the reload
 * watch applies the write. From a press, on the draw thread
 */
fn change(path: Vec<String>, value: Option<Value>) {
    let written = file::set(&path, value);

    refresh();

    let ok = written.is_ok();
    Settings::write().refused = written.err();

    if ok {
        reset_inputs();
    }
}

// writes the key at `path` to the settings file, as a change in the window does, for `kanade module`
pub fn set(path: &[&str], value: Option<Value>) -> Result<(), String> {
    let path: Vec<String> = path.iter().map(|&key| key.to_owned()).collect();

    file::set(&path, value)
}

fn turn_to(page: &'static str) {
    Settings::write().page = page;
}

// the key as the path of tables to it, with an entry of a table kind after it
fn path(key: &str, entry: Option<&str>) -> Vec<String> {
    key.split('.').chain(entry).map(str::to_owned).collect()
}

// each text input back to what the files give: a path's value, an empty field to add an entry
fn reset_inputs() {
    let settings = Settings::read();

    for setting in pages().flat_map(|module| module.settings) {
        let text = match &setting.kind {
            Kind::Path(field) => (field.get)(&settings.read).unwrap_or_default(),
            Kind::DesktopIds(_) | Kind::Paths(_) | Kind::AppIds(_) => String::new(),
            _ => continue,
        };

        TextInput::set_text(input(setting), &text);
    }
}

thread_local! {
    // each setting's text input id, made once, as an id lives as long as the input's text
    static INPUTS: RefCell<HashMap<&'static str, &'static str>> =
        RefCell::new(HashMap::new());
}

// the id of a setting's one text input
fn input(setting: &'static Setting) -> &'static str {
    INPUTS.with_borrow_mut(|inputs| {
        *inputs
            .entry(setting.key)
            .or_insert_with(|| Box::leak(format!("settings {}", setting.key).into_boxed_str()))
    })
}
