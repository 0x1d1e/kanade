//! The Settings window (#148, docs/design.md Settings): a floated normal window over the schema
//! registry, its pages by task (`layout`, ADR 0029), each setting drawn by its Kind. A change
//! writes the settings file (`file`), the top layer, which the reload watch then applies like any
//! other edit: a live-safe key at once, one that takes a restart as pending. It never writes the
//! config directory, so the user's own files and a Nix-managed config stay as they are.
//!
//! The window reads only the `Settings` Service, a snapshot of the files taken when it opens,
//! after each write and after each reload, so a view never reads a file while drawing.

mod file;
mod layout;
mod view;

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::sync::OnceLock;

use kanade_runtime::TextInput;
use kanade_runtime::service::Service;
use toml::Value;

use crate::config::{self, Config, Kind, Setting};
use crate::icon::Icon;
use crate::modules;
use crate::reload;

// the window's name, which tells it apart from any other the runtime opens
const WINDOW: &str = "settings";

// what the window shows, read from the files
#[derive(Default)]
pub struct Settings {
    // the page that shows
    page: &'static str,

    // what the search field holds; any shows what matches it in place of the page
    query: String,

    // whether each setting shows its key in the config, as a file sets it
    inspect: bool,

    // each change written since the window opened, which Undo writes back if the file still holds it
    history: Vec<Vec<file::Replaced>>,

    // the config the files give now, the settings file included, which a reload applies
    read: Config,

    // the same without the settings file: what a key there overrides
    below: Config,

    // the settings file as it stands
    file: toml::Table,

    // why the settings file cannot be read whole, which also refuses every write
    unreadable: Option<String>,

    // why the last change was not written, as the footer says it
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
            page: layout::PAGES[0].name,
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

// the pages, as `kanade settings open` names them
pub fn pages() -> impl Iterator<Item = &'static str> {
    layout::PAGES.iter().map(|page| page.name)
}

// the page of this name, or the one showing a Module's settings, as pages were named before
pub fn page(name: &str) -> Option<&'static str> {
    pages().find(|&page| page == name).or_else(|| {
        let module = modules::ALL.iter().find(|module| module.name == name)?;

        layout::page_of(module.settings.first()?.key).map(|page| page.name)
    })
}

// a page, as the Launcher lists it
pub struct Listed {
    pub name: &'static str,
    pub title: &'static str,
    pub summary: &'static str,
    pub icon: Icon,
}

pub fn listed() -> impl Iterator<Item = Listed> {
    layout::PAGES.iter().map(|page| Listed {
        name: page.name,
        title: page.title,
        summary: page.summary,
        icon: page.icon,
    })
}

// a setting and the page showing it
pub struct Placed {
    pub setting: &'static Setting,
    pub page: &'static str,
    pub title: &'static str,
    pub icon: Icon,
}

// every setting a page shows, in the Modules' order
pub fn placed() -> &'static [Placed] {
    static PLACED: OnceLock<Vec<Placed>> = OnceLock::new();

    PLACED.get_or_init(|| {
        modules::ALL
            .iter()
            .flat_map(|module| module.settings)
            .filter_map(|setting| {
                let page = layout::page_of(setting.key)?;

                Some(Placed {
                    setting,
                    page: page.name,
                    title: page.title,
                    icon: page.icon,
                })
            })
            .collect()
    })
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
            settings.query.clear();
        }
    }

    // a window already open keeps its Undo, and its search unless it turns to a page
    if !kanade_runtime::window_open(WINDOW) {
        Settings::write().history.clear();
        Settings::write().query.clear();
        TextInput::set_text(SEARCH, "");
    } else if page.is_some() {
        TextInput::set_text(SEARCH, "");
    }

    reset_inputs();
    kanade_runtime::open_window(WINDOW, view::window);
}

pub fn close() {
    kanade_runtime::close_window(WINDOW);
}

/*
 * writes the key at `path`, none removing it, and shows the files as they then are; the reload
 * watch applies the write. From a press, on the draw thread
 */
fn change(path: Vec<String>, value: Option<Value>) {
    change_all(vec![(path, value)]);
}

// writes every key in one write or none, as one change Undo takes back whole
fn change_all(changes: Vec<file::Entry>) {
    match file::set_all(changes, &[]) {
        Ok(replaced) => {
            let changed = replaced.iter().any(|each| each.was != each.now);
            written(None, changed.then_some(replaced));
        }
        Err(refusal) => written(Some(format!("Not saved: {}", refusal.why)), None),
    }
}

/*
 * writes back what the last change replaced, if the file still holds what it wrote; one edited
 * since, by hand or `kanade module`, is left as it is and its Undo dropped. A write that failed
 * for another reason, as a file that does not read, keeps its Undo for when that is mended
 */
fn undo() {
    let (back, expected): (Vec<file::Entry>, Vec<file::Entry>) = {
        let settings = Settings::read();
        let Some(last) = settings.history.last() else {
            return;
        };

        (
            last.iter()
                .rev()
                .map(|each| (each.path.clone(), each.was.clone()))
                .collect(),
            last.iter()
                .map(|each| (each.path.clone(), each.now.clone()))
                .collect(),
        )
    };

    let refusal = file::set_all(back, &expected).err();

    // dropped on success too; the file has it back
    if refusal.as_ref().is_none_or(|refusal| refusal.stale) {
        Settings::write().history.pop();
    }

    written(
        refusal.map(|refusal| format!("Not undone: {}", refusal.why)),
        None,
    );
}

// the files read again after a write, keeping a change for Undo
fn written(refused: Option<String>, change: Option<Vec<file::Replaced>>) {
    refresh();

    let ok = refused.is_none();
    {
        let mut settings = Settings::write();
        settings.refused = refused;
        settings.history.extend(change);
    }

    if ok {
        reset_inputs();
    }
}

// writes the key at `path` to the settings file, as a change in the window does, for `kanade module`
pub fn set(path: &[&str], value: Option<Value>) -> Result<(), String> {
    let path: Vec<String> = path.iter().map(|&key| key.to_owned()).collect();

    file::set(&path, value)
}

// turns to a page, leaving any search
fn turn_to(page: &'static str) {
    {
        let mut settings = Settings::write();
        settings.page = page;
        settings.query.clear();
    }

    TextInput::set_text(SEARCH, "");
}

fn search(query: String) {
    Settings::write().query = query;
}

fn inspect() {
    let mut settings = Settings::write();
    settings.inspect = !settings.inspect;
}

// the key as the path of tables to it, with an entry of a table kind after it
fn path(key: &str, entry: Option<&str>) -> Vec<String> {
    key.split('.').chain(entry).map(str::to_owned).collect()
}

// each text input back to what the files give: a path's value, an empty field to add an entry
fn reset_inputs() {
    let settings = Settings::read();

    for setting in modules::ALL.iter().flat_map(|module| module.settings) {
        let text = match &setting.kind {
            Kind::Millis(field) => (field.get)(&settings.read).as_millis().to_string(),
            Kind::Pixels(_, field) => (field.get)(&settings.read).to_string(),
            Kind::Path(field) | Kind::Text(field) => {
                (field.get)(&settings.read).unwrap_or_default()
            }
            Kind::Location(field) => (field.get)(&settings.read)
                .map(|at| format!("{}, {}", at.latitude, at.longitude))
                .unwrap_or_default(),
            Kind::DesktopIds(_) | Kind::Paths(_) | Kind::AppIds(_) => String::new(),
            _ => continue,
        };

        TextInput::set_text(input(setting), &text);
    }
}

// the search field's text input id
const SEARCH: &str = "settings search";

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
