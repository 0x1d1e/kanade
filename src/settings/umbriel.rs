//! The Umbriel page's data and its worker: which options of Umbriel's `config.toml` the page
//! offers, what the file gives for each, and the queue that changes them. A change stages the
//! edit in memory and the row shows it as not saved; Save sends every staged change as one
//! transaction, which the engine validates and writes with one backup, and Discard drops them. The
//! file is not touched until then.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SendError, Sender};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use kanade_runtime::TextInput;
use kanade_runtime::service::Service;

use crate::config::Choices;
use crate::umbriel_config::{self, Editor, Reading, RuleInfo, RuleStatus};

use super::{Settings, WINDOW};

/// The page's name, as `kanade settings open` takes it.
pub const PAGE: &str = "umbriel";

/// What the footer says on the page when nothing else needs saying.
pub const STANDING: &str = "Umbriel applies changes when reloaded or restarted";

// how often the file's modification time is looked at while the page shows
const POLL: Duration = Duration::from_secs(1);

/// What sets an option, by the kind of value Umbriel takes.
pub enum Control {
    /// A boolean setting: true or false.
    Flag,

    Number(Number),

    /// One of a few options; `default` is the one Umbriel uses unset.
    Choice {
        options: &'static Choices,
        default: &'static str,
    },

    /// Free text, with an example.
    Text {
        example: &'static str,
    },
}

/// A number stepped from `least` to `most`.
pub struct Number {
    pub least: f64,
    pub most: f64,
    pub step: f64,
    pub decimals: usize,
    pub default: Option<f64>,
    pub unit: &'static str,
}

/// An option of Umbriel's config the page offers.
pub struct Item {
    pub path: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub control: Control,
}

impl Item {
    pub fn paths(&self) -> Vec<&'static str> {
        vec![self.path]
    }
}

const fn flag(path: &'static str, label: &'static str, help: &'static str) -> Item {
    Item {
        path,
        label,
        help,
        control: Control::Flag,
    }
}

const fn number(
    path: &'static str,
    label: &'static str,
    help: &'static str,
    (least, most, step): (f64, f64, f64),
    decimals: usize,
    default: Option<f64>,
    unit: &'static str,
) -> Item {
    Item {
        path,
        label,
        help,
        control: Control::Number(Number {
            least,
            most,
            step,
            decimals,
            default,
            unit,
        }),
    }
}

const fn choice(
    path: &'static str,
    label: &'static str,
    help: &'static str,
    options: &'static Choices,
    default: &'static str,
) -> Item {
    Item {
        path,
        label,
        help,
        control: Control::Choice { options, default },
    }
}

const fn text(
    path: &'static str,
    label: &'static str,
    help: &'static str,
    example: &'static str,
) -> Item {
    Item {
        path,
        label,
        help,
        control: Control::Text { example },
    }
}

static MODE: Choices = Choices::named(&["scrolling", "floating", "tiling"]);

/// Every option, by path; the layout names the rows and their sections.
pub static ITEMS: &[Item] = &[
    // Layout
    choice(
        "layout.mode",
        "Layout mode",
        "How windows are arranged: scrolling columns, floating, or tiling",
        &MODE,
        "scrolling",
    ),
    number(
        "layout.gap",
        "Gap",
        "Space between windows and screen edges",
        (0.0, 128.0, 1.0),
        0,
        Some(8.0),
        "px",
    ),
    text(
        "layout.extent_presets",
        "Extent presets",
        "Width presets for scrolling windows, e.g. [0.333, 0.5, 0.667]",
        "[0.333, 0.5, 0.667]",
    ),
    number(
        "layout.scrolling.default_extent_fraction",
        "Default extent fraction",
        "Initial fraction of screen width for new windows in scrolling mode",
        (0.1, 1.0, 0.05),
        2,
        Some(0.5),
        "",
    ),
    // Appearance & Blur
    number(
        "appearance.border_width",
        "Border width",
        "Width of the active window border",
        (0.0, 32.0, 1.0),
        0,
        Some(2.0),
        "px",
    ),
    number(
        "appearance.outer_border_width",
        "Outer border width",
        "Width of the secondary outer border",
        (0.0, 32.0, 1.0),
        0,
        Some(0.0),
        "px",
    ),
    number(
        "appearance.corner_radius",
        "Corner radius",
        "Corner rounding radius for client windows",
        (0.0, 64.0, 1.0),
        0,
        Some(10.0),
        "px",
    ),
    flag(
        "appearance.blur.enabled",
        "Enable blur",
        "Blur translucent backgrounds behind windows and surfaces",
    ),
    number(
        "appearance.blur.radius",
        "Blur radius",
        "Gaussian blur filter radius",
        (0.0, 64.0, 1.0),
        0,
        Some(5.0),
        "px",
    ),
    number(
        "appearance.blur.passes",
        "Blur passes",
        "Number of blur rendering passes",
        (1.0, 10.0, 1.0),
        0,
        Some(2.0),
        "",
    ),
    flag(
        "appearance.shadow.enabled",
        "Window shadows",
        "Render drop shadows under floating and active windows",
    ),
    // Input & Focus
    flag(
        "input.middle_click_paste",
        "Middle click paste",
        "Paste primary selection when middle mouse button is pressed",
    ),
    flag(
        "input.client_window_drag",
        "Client window drag",
        "Allow clients to initiate interactive window move or resize",
    ),
    flag(
        "input.focus.follows_mouse",
        "Focus follows mouse",
        "Window focus follows pointer movement into a window",
    ),
    flag(
        "input.focus.hover_raises",
        "Hover raises",
        "Raise window to front when hovered",
    ),
    // Keyboard
    text(
        "input.keyboard.layout",
        "Keyboard layout",
        "XKB layout code, e.g. us, de, or us,ru",
        "us",
    ),
    number(
        "input.keyboard.repeat_delay",
        "Repeat delay",
        "Delay in milliseconds before key repetition starts",
        (100.0, 2000.0, 25.0),
        0,
        Some(600.0),
        "ms",
    ),
    number(
        "input.keyboard.repeat_rate",
        "Repeat rate",
        "Number of repeated key characters per second",
        (1.0, 100.0, 1.0),
        0,
        Some(25.0),
        "/s",
    ),
    flag(
        "input.keyboard.numlock_toggle",
        "NumLock on startup",
        "Turn on NumLock when the compositor starts",
    ),
    // Touchpad
    flag(
        "input.touchpad.tap",
        "Tap to click",
        "A tap on the touchpad is treated as a mouse click",
    ),
    flag(
        "input.touchpad.natural_scroll",
        "Natural scrolling",
        "Touchpad scrolling direction follows finger movement",
    ),
    flag(
        "input.touchpad.disable_while_typing",
        "Pause while typing",
        "Ignore touchpad input while typing on the keyboard",
    ),
    // Mouse & Cursor
    flag(
        "input.mouse.natural_scroll",
        "Natural scrolling",
        "Mouse wheel scrolling direction follows content movement",
    ),
    text(
        "input.cursor.theme",
        "Cursor theme",
        "Name of the XCursor theme",
        "Adwaita",
    ),
    number(
        "input.cursor.size",
        "Cursor size",
        "Size of the cursor pointer in pixels",
        (8.0, 128.0, 4.0),
        0,
        Some(24.0),
        "px",
    ),
];

/// The option of this path.
pub fn item(path: &str) -> Option<&'static Item> {
    ITEMS.iter().find(|item| item.path == path)
}

/// What the file gives for an option.
#[derive(Debug, Clone, PartialEq)]
pub enum Current {
    Flag(bool),
    Number(f64),
    Text(String),
    Empty,
    Foreign(String),
}

/// An option as its row shows it.
pub struct Shown {
    pub current: Current,
    pub editable: bool,
    pub set: bool,
    pub default: bool,
    pub busy: bool,
    pub pending: bool,
    pub writing: bool,
    pub refused: Option<String>,
}

#[derive(Clone, PartialEq)]
struct Staged {
    row: &'static str,
    changes: Vec<(&'static str, Option<String>)>,
}

/// What the page knows of the config, kept in the `Settings` Service.
#[derive(Default)]
pub struct State {
    pub loaded: bool,
    pub installed: bool,
    pub config_missing: bool,
    values: HashMap<&'static str, Reading>,
    pub rules: Vec<RuleInfo>,
    pub not_editable: Option<String>,
    pub unreadable: Option<String>,
    busy: Vec<&'static str>,
    staged: Vec<Staged>,
    staged_values: HashMap<&'static str, Reading>,
    saving: bool,
    in_flight: Vec<Staged>,
    saved: Option<String>,
    refused: HashMap<&'static str, String>,
    pub footer: Option<String>,
    pub undoable: bool,
    undo_after: Option<String>,
    undo_stamp: Option<Stamp>,
    undoing: bool,
    pub generation: u64,
    epoch: u64,
    resets: HashMap<&'static str, u64>,
}

impl State {
    pub fn shown(&self, item: &Item) -> Shown {
        let reading = self.reading(item.path);
        let set = matches!(reading, Reading::Own(_));
        let default = matches!(reading, Reading::Unset);
        let editable = self.loaded
            && !self.config_missing
            && self.unreadable.is_none()
            && self.not_editable.is_none();

        Shown {
            current: self.current(item),
            editable,
            set,
            default,
            busy: self.busy.contains(&item.path),
            pending: self.staged.iter().any(|staged| staged.row == item.path),
            writing: self.in_flight.iter().any(|staged| staged.row == item.path),
            refused: self.refused.get(item.path).cloned(),
        }
    }

    pub fn editable(&self) -> bool {
        self.loaded
            && !self.config_missing
            && self.unreadable.is_none()
            && self.not_editable.is_none()
    }

    pub fn has_staged(&self) -> bool {
        !self.staged.is_empty()
    }

    pub fn is_saving(&self) -> bool {
        self.saving
    }

    pub fn is_busy(&self, row: &str) -> bool {
        self.busy.contains(&row)
    }

    pub fn rule_status(&self, id: &str) -> Option<&RuleStatus> {
        self.rules.iter().find(|r| r.id == id).map(|r| &r.status)
    }

    fn reading(&self, path: &str) -> &Reading {
        self.staged_values
            .get(path)
            .or_else(|| self.values.get(path))
            .unwrap_or(&Reading::Unset)
    }

    pub fn current(&self, item: &Item) -> Current {
        if !self.loaded || self.unreadable.is_some() {
            return Current::Empty;
        }

        let given = match self.reading(item.path) {
            Reading::Unset => None,
            Reading::Own(val) => Some(val.as_str()),
        };

        match &item.control {
            Control::Flag => match given {
                Some("true") => Current::Flag(true),
                Some("false") => Current::Flag(false),
                Some(foreign) => Current::Foreign(foreign.to_owned()),
                None => Current::Flag(false),
            },
            Control::Number(spec) => match given {
                None => spec.default.map_or(Current::Empty, Current::Number),
                Some(shown) => shown
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .map_or_else(|| Current::Foreign(shown.to_owned()), Current::Number),
            },
            Control::Choice { default, .. } => match given {
                None if default.is_empty() => Current::Empty,
                None => Current::Text((*default).to_owned()),
                Some(shown) => Current::Text(unquote(shown)),
            },
            Control::Text { .. } => match given {
                None => Current::Text(String::new()),
                Some(shown) => Current::Text(unquote(shown)),
            },
        }
    }

    fn input_text(&self, item: &Item) -> String {
        match (&item.control, self.current(item)) {
            (Control::Number(spec), Current::Number(value)) => format_number(value, spec.decimals),
            (_, Current::Text(text) | Current::Foreign(text)) => text,
            _ => String::new(),
        }
    }

    pub fn status(&self) -> Option<(String, bool)> {
        if let Some(refused) = &self.footer {
            return Some((refused.clone(), true));
        }

        if self.undoing {
            return Some((String::from("Undoing\u{2026}"), false));
        }

        if self.saving || !self.busy.is_empty() {
            return Some((String::from("Saving\u{2026}"), false));
        }

        if !self.staged.is_empty() {
            let count = self.staged.len();
            let changes = if count == 1 { "change" } else { "changes" };

            return Some((format!("{count} {changes} not saved"), false));
        }

        self.saved.clone().map(|said| (said, false))
    }

    pub fn refuse(&mut self, row: &'static str, why: String) {
        self.footer = Some(why.clone());
        self.refused.insert(row, why);
        *self.resets.entry(row).or_default() += 1;
        self.generation += 1;
    }

    fn begin(&mut self, row: Option<&'static str>) {
        self.footer = None;
        self.saved = None;

        match row {
            Some(row) => {
                self.busy.push(row);
                self.refused.remove(row);
            }
            None => self.undoing = true,
        }
    }

    fn stage(&mut self, row: &'static str, changes: Vec<(&'static str, Option<String>)>) {
        self.footer = None;
        self.saved = None;
        self.refused.remove(row);

        let writing = self.in_flight.iter().any(|flying| flying.row == row);
        let differs = writing
            || changes.iter().any(|(path, value)| {
                let after = match value {
                    Some(val) => Reading::Own(val.clone()),
                    None => Reading::Unset,
                };
                match self.values.get(path) {
                    Some(before) => *before != after,
                    None => !matches!(after, Reading::Unset),
                }
            });
        let at = self.staged.iter().position(|staged| staged.row == row);

        match (differs, at) {
            (false, Some(at)) => {
                self.staged.remove(at);
            }
            (false, None) => {}
            (true, Some(at)) => self.staged[at].changes = changes,
            (true, None) => self.staged.push(Staged { row, changes }),
        }

        self.restage();
        self.generation += 1;
    }

    fn restage(&mut self) {
        self.staged_values.clear();

        for staged in &self.staged {
            for (path, value) in &staged.changes {
                let reading = match value {
                    Some(val) => Reading::Own(val.clone()),
                    None => Reading::Unset,
                };
                self.staged_values.insert(path, reading);
            }
        }
    }

    fn flush(&mut self) -> Option<Vec<(&'static str, Option<String>)>> {
        if self.staged.is_empty() || self.saving {
            return None;
        }

        self.footer = None;
        self.saved = None;
        self.saving = true;
        self.in_flight = self.staged.clone();
        self.generation += 1;

        Some(
            self.staged
                .iter()
                .flat_map(|staged| staged.changes.iter().cloned())
                .collect(),
        )
    }

    fn discard(&mut self) {
        self.saving = false;
        self.in_flight.clear();
        self.take_staged();
        self.footer = None;
        self.saved = None;
        self.generation += 1;
    }

    fn take_staged(&mut self) {
        for staged in std::mem::take(&mut self.staged) {
            *self.resets.entry(staged.row).or_default() += 1;
        }
        self.staged_values.clear();
    }

    fn done(&mut self, done: Done) {
        self.loaded = true;
        self.installed = done.read.installed;
        self.config_missing = done.read.config_missing;
        self.values = done.read.values;
        self.rules = done.read.rules;
        self.not_editable = done.read.not_editable;
        self.unreadable = done.read.unreadable;
        self.generation += 1;

        if done.save {
            self.saving = false;
            let flushed = std::mem::take(&mut self.in_flight);

            match done.outcome {
                Some(Ok(said)) => {
                    for staged in flushed {
                        if let Some(at) = self.staged.iter().position(|w| *w == staged) {
                            self.staged.remove(at);
                            *self.resets.entry(staged.row).or_default() += 1;
                        }
                    }
                    self.restage();

                    if !said.is_empty() {
                        self.saved = Some(if done.written {
                            format!("Saved: {said}")
                        } else {
                            said
                        });
                    }

                    if done.written {
                        self.undoable = true;
                        self.undo_after = done.after;
                        self.undo_stamp = Some(done.stamp);
                    }
                }
                Some(Err(why)) => self.footer = Some(why),
                None => {}
            }

            return;
        }

        if let Some(row) = done.row {
            if let Some(at) = self.busy.iter().position(|&busy| busy == row) {
                self.busy.remove(at);
            }
            *self.resets.entry(row).or_default() += 1;
        }

        match (done.row, done.undo, done.outcome) {
            (Some(row), _, Some(Err(why))) => {
                self.footer = Some(why.clone());
                self.refused.insert(row, why);
            }
            (Some(_), _, Some(Ok(_))) => {
                if done.written {
                    self.undoable = true;
                    self.undo_after = done.after;
                    self.undo_stamp = Some(done.stamp);
                }
            }
            (None, true, outcome) => {
                self.undoing = false;
                match outcome {
                    Some(Err(why)) => self.footer = Some(why),
                    _ => {
                        self.undoable = false;
                        self.undo_after = None;
                    }
                }
            }
            _ => {
                if self.undoable && Some(done.stamp) != self.undo_stamp {
                    self.undoable = false;
                    self.undo_after = None;
                    self.undo_stamp = None;
                }
            }
        }
    }

    fn panicked(&mut self) {
        let busy = std::mem::take(&mut self.busy);
        for row in busy {
            *self.resets.entry(row).or_default() += 1;
        }
        self.undoing = false;
        self.saving = false;
        self.in_flight.clear();
        self.footer = Some(String::from(
            "The thread editing Umbriel's config stopped unexpectedly; try again",
        ));
        self.generation += 1;
    }

    pub fn reopen(&mut self) {
        self.footer = None;
        self.saved = None;
        self.refused.clear();
        self.undoable = false;
        self.saving = false;
        self.in_flight.clear();
        self.take_staged();
        self.epoch += 1;
        self.generation += 1;
    }
}

pub fn format_number(value: f64, decimals: usize) -> String {
    let text = format!("{value:.decimals$}");
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.')
    } else {
        text.as_str()
    };

    if text == "-0" {
        String::from("0")
    } else {
        text.to_owned()
    }
}

pub fn unquote(shown: &str) -> String {
    let trimmed = shown.trim();
    if (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2)
        || (trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2)
    {
        trimmed[1..trimmed.len() - 1].to_owned()
    } else {
        trimmed.to_owned()
    }
}

pub fn reset_changes(item: &Item) -> Vec<(&'static str, Option<String>)> {
    vec![(item.path, None)]
}

thread_local! {
    static LOOKED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static SYNCED: RefCell<HashMap<&'static str, (u64, String, u64)>> =
        RefCell::new(HashMap::new());
    static INPUTS: RefCell<HashMap<&'static str, &'static str>> = RefCell::new(HashMap::new());
}

pub fn input(item: &Item) -> &'static str {
    INPUTS.with_borrow_mut(|inputs| {
        *inputs.entry(item.path).or_insert_with(|| {
            Box::leak(format!("settings umbriel {}", item.path).into_boxed_str())
        })
    })
}

pub fn sync_inputs(state: &State) {
    if LOOKED.get() == state.generation {
        return;
    }
    LOOKED.set(state.generation);

    SYNCED.with_borrow_mut(|synced| {
        for item in ITEMS {
            if !matches!(item.control, Control::Number(_) | Control::Text { .. }) {
                continue;
            }

            let reset = state.resets.get(item.path).copied().unwrap_or(0);
            let key = (state.epoch, state.input_text(item), reset);

            if synced.get(item.path) != Some(&key) {
                TextInput::set_text(input(item), &key.1);
                synced.insert(item.path, key);
            }
        }
    });
}

pub fn load() {
    post(Job::Read);
}

pub fn change(row: &'static str, changes: Vec<(&'static str, Option<String>)>) {
    Settings::write().umbriel.stage(row, changes);
}

pub fn save() {
    let changes = {
        let mut settings = Settings::write();
        settings.umbriel.flush()
    };
    let Some(changes) = changes else {
        return;
    };

    post(Job::Save { changes });
}

pub fn discard() {
    Settings::write().umbriel.discard();
    post(Job::Read);
}

pub fn undo() {
    let mut s = Settings::write();
    let after = s.umbriel.undo_after.clone();
    s.umbriel.begin(None);
    drop(s);
    post(Job::Undo { after });
}

pub fn apply_rule(id: &'static str) {
    let mut s = Settings::write();
    s.umbriel.begin(Some(id));
    drop(s);
    post(Job::ApplyRule { id });
}

pub fn remove_rule(id: &'static str) {
    let mut s = Settings::write();
    s.umbriel.begin(Some(id));
    drop(s);
    post(Job::RemoveRule { id });
}

pub fn create_default() {
    let mut s = Settings::write();
    s.umbriel.saving = true;
    s.umbriel.generation += 1;
    drop(s);
    post(Job::CreateDefault);
}

// Worker

enum Job {
    Read,
    Save {
        changes: Vec<(&'static str, Option<String>)>,
    },
    ApplyRule {
        id: &'static str,
    },
    RemoveRule {
        id: &'static str,
    },
    Undo {
        after: Option<String>,
    },
    CreateDefault,
}

#[derive(Default)]
struct Read {
    values: HashMap<&'static str, Reading>,
    rules: Vec<RuleInfo>,
    not_editable: Option<String>,
    unreadable: Option<String>,
    installed: bool,
    config_missing: bool,
}

struct Done {
    row: Option<&'static str>,
    undo: bool,
    save: bool,
    outcome: Option<Result<String, String>>,
    written: bool,
    after: Option<String>,
    stamp: Stamp,
    read: Read,
}

#[derive(Clone, Copy, PartialEq)]
struct Stamp(Option<(i64, i64, u64)>);

impl Stamp {
    fn of(file: &Path) -> Stamp {
        Stamp(
            fs::metadata(file)
                .ok()
                .map(|meta| (meta.mtime(), meta.mtime_nsec(), meta.len())),
        )
    }
}

static WORKER: Mutex<Option<Sender<Job>>> = Mutex::new(None);

fn post(job: Job) {
    let mut worker = WORKER.lock().unwrap_or_else(PoisonError::into_inner);
    let mut job = Some(job);

    for _ in 0..2 {
        if worker.is_none() {
            *worker = spawn();
        }
        let (Some(sender), Some(posted)) = (worker.as_ref(), job.take()) else {
            break;
        };

        match sender.send(posted) {
            Ok(()) => return,
            Err(SendError(back)) => {
                job = Some(back);
                *worker = None;
            }
        }
    }
    drop(worker);

    if let Some(job) = job {
        let why = String::from("could not start the thread that edits Umbriel's config");
        let (row, undo, save) = row_of(&job);

        Settings::write().umbriel.done(Done {
            row,
            undo,
            save,
            outcome: (!matches!(job, Job::Read)).then_some(Err(why.clone())),
            written: false,
            after: None,
            stamp: Stamp(None),
            read: Read {
                unreadable: Some(why),
                ..Read::default()
            },
        });
    }
}

fn spawn() -> Option<Sender<Job>> {
    let (sender, receiver) = mpsc::channel();

    thread::Builder::new()
        .name(String::from("kanade-umbriel-config"))
        .spawn(move || {
            if let Err(payload) =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&receiver)))
            {
                crate::supervise::panicked("kanade-umbriel-config", &*payload);
                *WORKER.lock().unwrap_or_else(PoisonError::into_inner) = None;
                Settings::write().umbriel.panicked();
            }
        })
        .ok()
        .map(|_| sender)
}

fn row_of(job: &Job) -> (Option<&'static str>, bool, bool) {
    match job {
        Job::Save { .. } | Job::CreateDefault => (None, false, true),
        Job::ApplyRule { id } | Job::RemoveRule { id } => (Some(id), false, false),
        Job::Undo { .. } => (None, true, false),
        Job::Read => (None, false, false),
    }
}

struct Watch {
    files: Vec<PathBuf>,
    seen: Vec<Stamp>,
    unreadable: bool,
    opened: bool,
    ticks: u32,
}

const GRACE: u32 = 5;

fn stamps(files: &[PathBuf]) -> Vec<Stamp> {
    files.iter().map(|file| Stamp::of(file)).collect()
}

fn run(receiver: &Receiver<Job>) {
    let mut watching: Option<Watch> = None;

    loop {
        let job = if watching.is_some() {
            match receiver.recv_timeout(POLL) {
                Ok(job) => job,
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {
                    watching = watching.take().and_then(tick);
                    continue;
                }
            }
        } else {
            match receiver.recv() {
                Ok(job) => job,
                Err(_) => return,
            }
        };

        let watch = run_job(job);
        watching = (Settings::read().page == PAGE).then_some(watch);
    }
}

fn tick(mut watch: Watch) -> Option<Watch> {
    if Settings::read().page != PAGE {
        return None;
    }

    if kanade_runtime::window_open(WINDOW) {
        watch.opened = true;
    } else if watch.opened || watch.ticks >= GRACE {
        Settings::write().umbriel.discard();
        return None;
    } else {
        watch.ticks += 1;
    }

    if !needs_read(&watch) {
        return Some(watch);
    }

    let mut fresh = run_job(Job::Read);
    fresh.opened = watch.opened;
    fresh.ticks = watch.ticks;

    Some(fresh)
}

fn needs_read(watch: &Watch) -> bool {
    watch.unreadable || stamps(&watch.files) != watch.seen
}

fn run_job(job: Job) -> Watch {
    let (done, files, config) = work(job);
    let unreadable = done.read.unreadable.is_some();
    let seen = vec![config];

    Settings::write().umbriel.done(done);

    Watch {
        files,
        seen,
        opened: kanade_runtime::window_open(WINDOW),
        ticks: 0,
        unreadable,
    }
}

fn work(job: Job) -> (Done, Vec<PathBuf>, Stamp) {
    let (row, undo, save) = row_of(&job);
    let setup = umbriel_config::target(None).and_then(|file| Ok((file, umbriel_config::state()?)));
    let (file, state) = match setup {
        Ok(setup) => setup,
        Err(why) => {
            return (
                Done {
                    row,
                    undo,
                    save,
                    outcome: (!matches!(job, Job::Read)).then(|| Err(why.clone())),
                    written: false,
                    after: None,
                    stamp: Stamp(None),
                    read: Read {
                        unreadable: Some(why),
                        ..Read::default()
                    },
                },
                vec![PathBuf::new()],
                Stamp(None),
            );
        }
    };
    let editor = Editor {
        file: file.clone(),
        state,
        validate: &umbriel_config::validate_umbriel,
        now_ms: umbriel_config::now_ms,
    };

    let before = Stamp::of(&file);
    let outcome = match &job {
        Job::Read => None,
        Job::CreateDefault => Some(
            editor
                .create_default()
                .map(|()| String::from("created default configuration")),
        ),
        Job::Save { changes } => {
            let changes: Vec<(&str, Option<&str>)> = changes
                .iter()
                .map(|(path, value)| (*path, value.as_deref()))
                .collect();
            Some(editor.apply(&changes))
        }
        Job::ApplyRule { id } => Some(editor.apply_rule(id)),
        Job::RemoveRule { id } => Some(editor.remove_rule(id)),
        Job::Undo { after } => Some(editor.undo_only(after.as_deref())),
    };
    let after = Stamp::of(&file);
    let read = read_all(&editor);
    let files = vec![file];

    (
        Done {
            row,
            undo,
            save,
            outcome,
            written: before != after,
            after: if before != after {
                editor.last_change_hash().ok()
            } else {
                None
            },
            stamp: after,
            read,
        },
        files,
        after,
    )
}

fn read_all(editor: &Editor) -> Read {
    let installed = umbriel_config::is_installed();
    let config_missing = !editor.file.exists();
    let not_editable = if config_missing {
        None
    } else {
        editor.editable().err()
    };

    let mut read = Read {
        installed,
        config_missing,
        not_editable,
        ..Read::default()
    };

    if !config_missing && let Err(why) = editor.check() {
        read.unreadable = Some(format!("Umbriel's config cannot be read: {why}"));
        return read;
    }

    for item in ITEMS {
        read.values.insert(item.path, editor.read(item.path));
    }

    read.rules = editor.rules().unwrap_or_default();

    read
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_lookup_and_paths() {
        let item = item("layout.gap").unwrap();
        assert_eq!(item.label, "Gap");
        assert_eq!(item.paths(), vec!["layout.gap"]);
    }

    #[test]
    fn state_staging_and_discard() {
        let mut state = State {
            loaded: true,
            ..State::default()
        };

        assert!(!state.has_staged());
        state.stage("layout.gap", vec![("layout.gap", Some(String::from("16")))]);
        assert!(state.has_staged());

        let flushed = state.flush().unwrap();
        assert_eq!(flushed.len(), 1);
        assert_eq!(flushed[0], ("layout.gap", Some(String::from("16"))));
        assert!(state.is_saving());

        state.discard();
        assert!(!state.has_staged());
        assert!(!state.is_saving());
    }

    #[test]
    fn format_number_formatting() {
        assert_eq!(format_number(8.0, 0), "8");
        assert_eq!(format_number(0.5, 2), "0.5");
        assert_eq!(format_number(0.0, 0), "0");
        assert_eq!(format_number(-0.0, 0), "0");
    }

    #[test]
    fn unquote_strings() {
        assert_eq!(unquote("\"scrolling\""), "scrolling");
        assert_eq!(unquote("'scrolling'"), "scrolling");
        assert_eq!(unquote("[0.333, 0.5]"), "[0.333, 0.5]");
    }
}
