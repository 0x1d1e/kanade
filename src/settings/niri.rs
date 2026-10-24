//! The niri page's data and its worker (ADR 0119): which options of niri's `config.kdl` the page
//! offers, what the file gives for each, and the queue that changes them. The engine
//! (`niri_config`) spawns `niri validate` and waits on it, so no call to it is made on the draw
//! thread: a press posts a job, one worker thread runs it, and the result is written into the
//! `Settings` Service, which redraws the window. A row shows its change as pending until then.
//!
//! The values come from the engine's `read`, never from a copy of the file, and are read again
//! after every job and, while the page shows, when the file's modification time changes.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SendError, Sender};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::TextInput;
use kanade_runtime::service::Service;

use crate::config::Choices;
use crate::niri_config::{self, Editor, Reading};
use crate::sources::niri as ipc;

use super::{Settings, WINDOW};

/// The page's name, as `kanade settings open` takes it.
pub const PAGE: &str = "niri";

/// What the footer says on the page when nothing else needs saying.
pub const STANDING: &str =
    "niri applies changes itself within a moment, and keeps its old config if it rejects the file";

// how often the file's modification time is looked at while the page shows; niri polls at 500 ms
const POLL: Duration = Duration::from_secs(1);

/// What sets an option, by the kind of value niri takes.
pub enum Control {
    /// A node with no value: there or not.
    Flag,

    /// A ring, border or shadow, which niri turns on or off by a flag each: `on` wins over `off`.
    /// `bare_on`: a block of the option's path in the top-level config, with neither flag, turns
    /// it on (niri reads `border { width 1 }` as a border that is on), so off needs an `off`.
    Drawn {
        on: &'static str,
        off: &'static str,
        default_on: bool,
        bare_on: bool,
    },

    Number(Number),

    /// One of a few names; `default` is the one niri uses unset, none if it has no name for it.
    Choice {
        options: &'static Choices,
        default: &'static str,
    },

    /// Free text, with an example that is also a value niri accepts.
    Text {
        example: &'static str,
    },
}

/// A number stepped from `least` to `most`, which niri may take beyond: the page keeps to what
/// reads well.
pub struct Number {
    pub least: f64,
    pub most: f64,
    pub step: f64,
    pub decimals: usize,

    /// What niri uses unset, none if it has no number for it.
    pub default: Option<f64>,
    pub unit: &'static str,
}

/// An option of niri's config the page offers.
pub struct Item {
    /// The engine's dotted path, which names the row; a ring's, border's or shadow's is its node.
    pub path: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub control: Control,
}

impl Item {
    /// The engine paths the option reads and writes.
    pub fn paths(&self) -> Vec<&'static str> {
        match self.control {
            Control::Drawn { on, off, .. } => vec![on, off],
            _ => vec![self.path],
        }
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

const fn drawn(
    path: &'static str,
    on: &'static str,
    off: &'static str,
    (default_on, bare_on): (bool, bool),
    label: &'static str,
    help: &'static str,
) -> Item {
    Item {
        path,
        label,
        help,
        control: Control::Drawn {
            on,
            off,
            default_on,
            bare_on,
        },
    }
}

#[allow(clippy::too_many_arguments)]
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

static CENTER: Choices = Choices::named(&["never", "always", "on-overflow"]);
static DISPLAY: Choices = Choices::named(&["normal", "tabbed"]);
static MOD_KEY: Choices = Choices::named(&["Super", "Alt", "Mod3", "Mod5", "Ctrl"]);
static TRACK: Choices = Choices::labeled(
    &["global", "window"],
    &[("global", "One for all"), ("window", "Per window")],
);
static PROFILE: Choices = Choices::named(&["adaptive", "flat"]);
static TOUCHPAD_SCROLL: Choices =
    Choices::named(&["two-finger", "edge", "on-button-down", "no-scroll"]);
static MOUSE_SCROLL: Choices = Choices::named(&["on-button-down", "no-scroll"]);
static CLICK: Choices = Choices::named(&["button-areas", "clickfinger"]);

const GAPS: (f64, f64, f64) = (0.0, 128.0, 4.0);
const WIDTH: (f64, f64, f64) = (1.0, 16.0, 1.0);
const STRUT: (f64, f64, f64) = (-64.0, 64.0, 4.0);
const SPEED: (f64, f64, f64) = (-1.0, 1.0, 0.1);
const FACTOR: (f64, f64, f64) = (0.1, 10.0, 0.1);

/// Every option, by path; the layout names the rows and their sections.
pub static ITEMS: &[Item] = &[
    // Layout
    number(
        "layout.gaps",
        "Gaps",
        "Space between windows, and around them at the screen's edge",
        GAPS,
        0,
        Some(16.0),
        "px",
    ),
    choice(
        "layout.center-focused-column",
        "Center the focused column",
        "When the column you focus is brought to the middle of the screen",
        &CENTER,
        "never",
    ),
    flag(
        "layout.always-center-single-column",
        "Center a single column",
        "Keep a column in the middle when it is the only one on the workspace",
    ),
    flag(
        "layout.empty-workspace-above-first",
        "Empty workspace above the first",
        "Keep an empty workspace above the first one, as there is one below the last",
    ),
    choice(
        "layout.default-column-display",
        "New columns show",
        "Whether the windows of a new column stack, or sit under tabs",
        &DISPLAY,
        "normal",
    ),
    number(
        "layout.struts.left",
        "Strut, left",
        "Space kept clear at the left of the screen, on top of the gaps",
        STRUT,
        0,
        Some(0.0),
        "px",
    ),
    number(
        "layout.struts.right",
        "Strut, right",
        "Space kept clear at the right of the screen, on top of the gaps",
        STRUT,
        0,
        Some(0.0),
        "px",
    ),
    number(
        "layout.struts.top",
        "Strut, top",
        "Space kept clear at the top of the screen, on top of the gaps",
        STRUT,
        0,
        Some(0.0),
        "px",
    ),
    number(
        "layout.struts.bottom",
        "Strut, bottom",
        "Space kept clear at the bottom of the screen, on top of the gaps",
        STRUT,
        0,
        Some(0.0),
        "px",
    ),
    // Focus ring and border
    drawn(
        "layout.focus-ring",
        "layout.focus-ring.on",
        "layout.focus-ring.off",
        (true, false),
        "Focus ring",
        "A ring drawn around the focused window, outside it",
    ),
    number(
        "layout.focus-ring.width",
        "Focus ring width",
        "How thick the ring is",
        WIDTH,
        0,
        Some(4.0),
        "px",
    ),
    text(
        "layout.focus-ring.active-color",
        "Focus ring color",
        "Around the focused window, as #rrggbb or a CSS color name",
        "deepskyblue",
    ),
    text(
        "layout.focus-ring.inactive-color",
        "Focus ring color, unfocused",
        "Around the focused window of an output that is not the focused one, as #rrggbb or a CSS color name",
        "dimgray",
    ),
    drawn(
        "layout.border",
        "layout.border.on",
        "layout.border.off",
        (false, true),
        "Border",
        "A border drawn around every window, inside its edge",
    ),
    number(
        "layout.border.width",
        "Border width",
        "How thick the border is",
        WIDTH,
        0,
        Some(4.0),
        "px",
    ),
    text(
        "layout.border.active-color",
        "Border color",
        "Around the focused window, as #rrggbb or a CSS color name",
        "orange",
    ),
    text(
        "layout.border.inactive-color",
        "Border color, unfocused",
        "Around the other windows, as #rrggbb or a CSS color name",
        "dimgray",
    ),
    drawn(
        "layout.shadow",
        "layout.shadow.on",
        "layout.shadow.off",
        (false, false),
        "Window shadows",
        "A soft shadow behind every window",
    ),
    // Input
    flag(
        "input.warp-mouse-to-focus",
        "Warp the pointer to focus",
        "Move the pointer to a window when the keyboard focuses it",
    ),
    flag(
        "input.focus-follows-mouse",
        "Focus follows the pointer",
        "Focus a window when the pointer moves onto it",
    ),
    flag(
        "input.workspace-auto-back-and-forth",
        "Workspace back and forth",
        "Switching to the workspace you are on by number goes back to the one before",
    ),
    flag(
        "input.disable-power-key-handling",
        "Leave the power key alone",
        "niri does nothing with the power key, so the system or another program can",
    ),
    choice(
        "input.mod-key",
        "Mod key",
        "The key the Mod in niri's key bindings stands for",
        &MOD_KEY,
        "Super",
    ),
    // Keyboard
    number(
        "input.keyboard.repeat-delay",
        "Repeat delay",
        "How long a key is held before it repeats",
        (100.0, 2000.0, 50.0),
        0,
        Some(600.0),
        "ms",
    ),
    number(
        "input.keyboard.repeat-rate",
        "Repeat rate",
        "How many times a second a held key repeats",
        (1.0, 100.0, 1.0),
        0,
        Some(25.0),
        "/s",
    ),
    flag(
        "input.keyboard.numlock",
        "Num Lock on at start",
        "Turn Num Lock on when niri starts",
    ),
    choice(
        "input.keyboard.track-layout",
        "Keyboard layout",
        "Whether the layout you switch to applies to every window or to each window on its own",
        &TRACK,
        "global",
    ),
    text(
        "input.keyboard.xkb.layout",
        "Layouts",
        "Comma-separated, like us,de. Empty uses the system's setting",
        "us",
    ),
    text(
        "input.keyboard.xkb.variant",
        "Variants",
        "One for each layout, comma-separated, like ,nodeadkeys",
        "intl",
    ),
    text(
        "input.keyboard.xkb.options",
        "Options",
        "xkb options, comma-separated, like ctrl:nocaps or grp:win_space_toggle",
        "ctrl:nocaps",
    ),
    // Touchpad
    flag(
        "input.touchpad.tap",
        "Tap to click",
        "A tap on the touchpad is a click",
    ),
    flag(
        "input.touchpad.natural-scroll",
        "Natural scrolling",
        "Content follows the fingers, as on a touch screen",
    ),
    flag(
        "input.touchpad.dwt",
        "Pause while typing",
        "Ignore the touchpad while keys are pressed",
    ),
    flag(
        "input.touchpad.left-handed",
        "Left-handed",
        "Swap the left and right buttons",
    ),
    flag(
        "input.touchpad.middle-emulation",
        "Middle click by both buttons",
        "Pressing the left and right buttons together is a middle click",
    ),
    number(
        "input.touchpad.accel-speed",
        "Pointer speed",
        "From -1 slowest to 1 fastest",
        SPEED,
        1,
        Some(0.0),
        "",
    ),
    choice(
        "input.touchpad.accel-profile",
        "Acceleration",
        "Adaptive speeds the pointer up with the finger; flat keeps it steady",
        &PROFILE,
        "adaptive",
    ),
    number(
        "input.touchpad.scroll-factor",
        "Scroll speed",
        "How far the content moves for the same scroll, 1 being as it is",
        FACTOR,
        1,
        Some(1.0),
        "\u{d7}",
    ),
    choice(
        "input.touchpad.scroll-method",
        "Scroll by",
        "Two fingers, an edge of the touchpad, or a button held while moving",
        &TOUCHPAD_SCROLL,
        "two-finger",
    ),
    choice(
        "input.touchpad.click-method",
        "Click by",
        "Zones at the bottom of the touchpad, or the number of fingers on it",
        &CLICK,
        "button-areas",
    ),
    // Mouse
    flag(
        "input.mouse.natural-scroll",
        "Natural scrolling",
        "Content follows the wheel, as on a touch screen",
    ),
    flag(
        "input.mouse.left-handed",
        "Left-handed",
        "Swap the left and right buttons",
    ),
    flag(
        "input.mouse.middle-emulation",
        "Middle click by both buttons",
        "Pressing the left and right buttons together is a middle click",
    ),
    number(
        "input.mouse.accel-speed",
        "Pointer speed",
        "From -1 slowest to 1 fastest",
        SPEED,
        1,
        Some(0.0),
        "",
    ),
    choice(
        "input.mouse.accel-profile",
        "Acceleration",
        "Adaptive speeds the pointer up with the hand; flat keeps it steady",
        &PROFILE,
        "adaptive",
    ),
    number(
        "input.mouse.scroll-factor",
        "Scroll speed",
        "How far the content moves for the same scroll, 1 being as it is",
        FACTOR,
        1,
        Some(1.0),
        "\u{d7}",
    ),
    choice(
        "input.mouse.scroll-method",
        "Scroll by",
        "A button held while moving, or no scrolling; unset is the wheel",
        &MOUSE_SCROLL,
        "",
    ),
    // Cursor
    text(
        "cursor.xcursor-theme",
        "Cursor theme",
        "The name of an installed cursor theme",
        "Adwaita",
    ),
    number(
        "cursor.xcursor-size",
        "Cursor size",
        "In pixels, for apps that follow niri",
        (8.0, 128.0, 4.0),
        0,
        Some(24.0),
        "px",
    ),
    flag(
        "cursor.hide-when-typing",
        "Hide while typing",
        "Hide the pointer when you type, until it moves",
    ),
    number(
        "cursor.hide-after-inactive-ms",
        "Hide when idle",
        "Hide the pointer after it has stayed still this long; unset never hides it",
        (100.0, 60000.0, 500.0),
        0,
        None,
        "ms",
    ),
    // Other
    number(
        "overview.zoom",
        "Overview zoom",
        "How small the workspaces are in the overview, 1 being their size",
        (0.1, 0.75, 0.05),
        2,
        Some(0.5),
        "",
    ),
    flag(
        "animations.off",
        "Turn animations off",
        "niri's own animations: window, workspace and overview movement",
    ),
    number(
        "animations.slowdown",
        "Animation slowdown",
        "How many times slower niri's animations run; 1 is their speed",
        (0.1, 5.0, 0.1),
        1,
        Some(1.0),
        "\u{d7}",
    ),
    flag(
        "prefer-no-csd",
        "Ask apps to skip their title bars",
        "Ask windows to leave out their own decorations",
    ),
    text(
        "screenshot-path",
        "Screenshot file",
        "Where screenshots are saved, with %Y-%m-%d style time codes",
        "~/Pictures/Screenshots/Screenshot from %Y-%m-%d %H-%M-%S.png",
    ),
];

/// The option of this path.
pub fn item(path: &str) -> Option<&'static Item> {
    ITEMS.iter().find(|item| item.path == path)
}

/// What the file gives for an option, in the form its control shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Current {
    Flag(bool),
    Number(f64),
    Text(String),

    /// Nothing is set and niri has no default to show.
    Empty,

    /// A value the control cannot show, as the file has it.
    Foreign(String),
}

/// An option as its row shows it.
pub struct Shown {
    pub current: Current,

    /// Whether the control acts: the config can be edited and no include owns the option.
    pub editable: bool,

    /// Who owns it, if an include does: "Set by appearance.sync".
    pub owner: Option<String>,

    /// Whether the config itself sets it, so Reset has something to remove.
    pub set: bool,

    /// Whether nothing sets it, so niri's default applies.
    pub default: bool,

    /// Whether a change to it waits for the worker.
    pub busy: bool,

    /// Why its last change was refused.
    pub refused: Option<String>,
}

/// What the page knows of the config, kept in the `Settings` Service.
#[derive(Default)]
pub struct State {
    // whether the config has been read since the window opened
    pub loaded: bool,

    // the engine's reading of each path
    values: HashMap<&'static str, Reading>,

    // status of recommended installation rules
    pub rules: Vec<niri_config::RuleInfo>,

    // why the config cannot be changed, though it can be read
    pub not_editable: Option<String>,

    // why it cannot be read
    pub unreadable: Option<String>,

    // rows with a change waiting, once for each
    busy: Vec<&'static str>,

    // why a row's last change was refused
    refused: HashMap<&'static str, String>,

    // the same, or why an Undo was, as the footer says it
    pub footer: Option<String>,

    // whether the last change this window made can be undone
    pub undoable: bool,

    // hash of the file after the last page change, so a later external change cannot be undone here
    undo_after: Option<String>,

    // the file as that change left it
    undo_stamp: Option<Stamp>,

    // whether the config sets something niri reads only when it starts
    pub restart: bool,

    // whether an Undo waits for the worker
    undoing: bool,

    // what became of the reload niri should have made of the last change
    reload: Option<(String, bool)>,

    // counts the changes that may need a text input set again, so the draw thread looks (`sync_inputs`)
    pub generation: u64,

    // the options with a bare block in the config itself (`Control::Drawn::bare_on`)
    bare: HashSet<&'static str>,

    // counts the windows opened, which set every input again
    epoch: u64,

    // counts the jobs and refusals of each row, which set its own input to what the file holds
    resets: HashMap<&'static str, u64>,
}

impl State {
    /// How the option stands.
    pub fn shown(&self, item: &Item) -> Shown {
        let readings: Vec<&Reading> = item
            .paths()
            .into_iter()
            .filter_map(|path| self.values.get(path))
            .collect();
        let owner = readings.iter().find_map(|reading| match reading {
            Reading::Included(_, file) | Reading::Overridden(_, file) => Some(owner_text(file)),
            _ => None,
        });
        let mut set = readings
            .iter()
            .any(|reading| matches!(reading, Reading::Own(_)));
        let mut default = readings
            .iter()
            .all(|reading| matches!(reading, Reading::Unset));

        // an option a bare block turns on is at niri's default when it shows that, whatever is written
        if let Control::Drawn {
            default_on,
            bare_on: true,
            ..
        } = item.control
        {
            let at_default = self.current(item) == Current::Flag(default_on);

            set &= !at_default;
            default = at_default;
        }
        let editable = self.loaded
            && self.unreadable.is_none()
            && self.not_editable.is_none()
            && owner.is_none();

        Shown {
            current: self.current(item),
            editable,
            owner,
            set,
            default,
            busy: self.busy.contains(&item.path),
            refused: self.refused.get(item.path).cloned(),
        }
    }

    pub fn editable(&self) -> bool {
        self.loaded && self.unreadable.is_none() && self.not_editable.is_none()
    }

    pub fn is_busy(&self, row: &str) -> bool {
        self.busy.contains(&row)
    }

    pub fn rule_status(&self, id: &str) -> Option<&niri_config::RuleStatus> {
        self.rules.iter().find(|r| r.id == id).map(|r| &r.status)
    }

    fn reading(&self, path: &str) -> &Reading {
        self.values.get(path).unwrap_or(&Reading::Unset)
    }

    /// The text, number or switch a control shows for the option.
    pub fn current(&self, item: &Item) -> Current {
        if !self.loaded || self.unreadable.is_some() {
            return Current::Empty;
        }

        let given = |path| match self.reading(path) {
            Reading::Unset => None,
            Reading::Own(shown) | Reading::Included(shown, _) | Reading::Overridden(shown, _) => {
                Some(shown.as_str())
            }
        };
        let set = |shown: Option<&str>| shown.is_some_and(|shown| shown != "false");

        match &item.control {
            Control::Flag => Current::Flag(set(given(item.path))),
            Control::Drawn {
                on,
                off,
                default_on,
                bare_on,
            } => Current::Flag(if set(given(on)) {
                true
            } else if set(given(off)) {
                false
            } else {
                *default_on || (*bare_on && self.bare.contains(item.path))
            }),
            Control::Number(number) => match given(item.path) {
                None => number.default.map_or(Current::Empty, Current::Number),
                Some(shown) => shown
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite())
                    .map_or_else(|| Current::Foreign(shown.to_owned()), Current::Number),
            },
            Control::Choice { default, .. } => match given(item.path) {
                None if default.is_empty() => Current::Empty,
                None => Current::Text((*default).to_owned()),
                Some(shown) => {
                    unquote(shown).map_or_else(|| Current::Foreign(shown.to_owned()), Current::Text)
                }
            },
            Control::Text { .. } => match given(item.path) {
                None => Current::Text(String::new()),
                Some(shown) => {
                    unquote(shown).map_or_else(|| Current::Foreign(shown.to_owned()), Current::Text)
                }
            },
        }
    }

    // what an option's text input holds
    fn input_text(&self, item: &Item) -> String {
        match (&item.control, self.current(item)) {
            (Control::Number(number), Current::Number(value)) => {
                format_number(value, number.decimals)
            }
            (_, Current::Text(text) | Current::Foreign(text)) => text,
            _ => String::new(),
        }
    }

    /// Records that this window's own change was just written, so the footer's Undo is that change
    /// and no other (`Editor::undo_only`): the hash the change left, and the file as it left it. The
    /// Outputs section's Keep and Reset write through the engine themselves and call this, since the
    /// page's rows are not the only ones that change niri's file.
    pub fn set_undoable(after: Option<String>) {
        let stamp = niri_config::target(None).ok().map(|file| Stamp::of(&file));
        let mut settings = Settings::write();

        settings.niri.undoable = true;
        settings.niri.undo_after = after;
        settings.niri.undo_stamp = stamp;
    }

    /// The footer's words and whether they warn, if the page has any beyond its standing line.
    pub fn status(&self) -> Option<(String, bool)> {
        if let Some(refused) = &self.footer {
            return Some((refused.clone(), true));
        }

        if self.undoing {
            return Some((String::from("Undoing\u{2026}"), false));
        }

        if !self.busy.is_empty() {
            return Some((String::from("Saving\u{2026}"), false));
        }

        self.reload.clone()
    }

    /// What became of the reload niri should have made of the last change: `None` when it has not
    /// said, which is no trouble of the page's when no niri is listening.
    fn reloaded(&mut self, applied: Option<bool>) {
        self.reload = Some(match applied {
            Some(true) => (String::from("niri reloaded the config"), false),
            Some(false) => (
                String::from("niri rejected the config; it keeps the one it had"),
                true,
            ),
            None => (String::from("niri has not reloaded the config yet"), true),
        });
        self.generation += 1;
    }

    /// Records why a typed value was not sent.
    pub fn refuse(&mut self, row: &'static str, why: String) {
        self.footer = Some(why.clone());
        self.refused.insert(row, why);

        // its input goes back to what the file holds
        *self.resets.entry(row).or_default() += 1;
        self.generation += 1;
    }

    // a job is posted
    fn begin(&mut self, row: Option<&'static str>) {
        self.footer = None;
        self.reload = None;

        match row {
            Some(row) => {
                self.busy.push(row);
                self.refused.remove(row);
            }
            None => self.undoing = true,
        }
    }

    // a job is done: the reading it ended with, and what became of the change
    fn done(&mut self, done: Done) {
        self.loaded = true;
        self.values = done.read.values;
        self.rules = done.read.rules;
        self.not_editable = done.read.not_editable;
        self.unreadable = done.read.unreadable;
        self.bare = done.read.bare;
        self.restart = done.read.restart;
        self.generation += 1;

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

                // a second Undo would put the change back; the CLI offers the backups. A refused
                // one keeps its button, as the reason may pass
                match outcome {
                    Some(Err(why)) => self.footer = Some(why),
                    _ => {
                        self.undoable = false;
                        self.undo_after = None;
                    }
                }
            }
            // a read shows what the file holds now: a change made after this window's own is not
            // this window's to undo, so its button goes rather than undoing someone else's change
            _ => {
                if self.undoable && Some(done.stamp) != self.undo_stamp {
                    self.undoable = false;
                    self.undo_after = None;
                    self.undo_stamp = None;
                }
            }
        }
    }

    /// The worker died while saving; release every row so the user can try again.
    fn panicked(&mut self) {
        let busy = std::mem::take(&mut self.busy);
        for row in busy {
            *self.resets.entry(row).or_default() += 1;
        }
        self.undoing = false;
        self.footer = Some(String::from(
            "The thread editing niri's config stopped unexpectedly; try again",
        ));
        self.generation += 1;
    }

    /// A window opening starts over: nothing refused, nothing to undo, nothing read.
    pub fn reopen(&mut self) {
        self.footer = None;
        self.reload = None;
        self.refused.clear();
        self.undoable = false;
        self.epoch += 1;
        self.generation += 1;
    }
}

/// Who owns an option an included file sets: "Set by appearance.sync", "Set by x.kdl, an include".
pub fn owner_text(file: &Path) -> String {
    if file.file_name().is_some_and(|name| name == "kanade.kdl") {
        String::from("Set by appearance.sync")
    } else {
        format!(
            "Set by {}, an include",
            file.file_name().map_or_else(
                || file.display().to_string(),
                |name| name.to_string_lossy().into_owned()
            )
        )
    }
}

/// A number as it is written to the file and shown: no zeros after the point, no `-0`.
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

// the text of a string as the engine shows it: quoted with KDL v1's escapes, or raw (`r#"..."#`)
fn unquote(shown: &str) -> Option<String> {
    if let Some(raw) = shown.strip_prefix('r') {
        let hashes = raw.len() - raw.trim_start_matches('#').len();
        let body = raw[hashes..].strip_prefix('"')?;
        let end = format!("\"{}", "#".repeat(hashes));

        return body.strip_suffix(end.as_str()).map(str::to_owned);
    }

    let inner = shown.strip_prefix('"')?.strip_suffix('"')?;
    let mut text = String::new();
    let mut chars = inner.chars();

    while let Some(char) = chars.next() {
        if char != '\\' {
            text.push(char);
            continue;
        }

        text.push(match chars.next()? {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'b' => '\u{8}',
            'f' => '\u{c}',
            '\\' => '\\',
            '"' => '"',
            '/' => '/',
            'u' => {
                if chars.next()? != '{' {
                    return None;
                }

                let mut code = 0u32;
                let mut digits = 0;
                loop {
                    match chars.next()? {
                        '}' if digits > 0 => break,
                        digit if digits < 6 => {
                            code = code * 16 + digit.to_digit(16)?;
                            digits += 1;
                        }
                        _ => return None,
                    }
                }

                char::from_u32(code)?
            }
            _ => return None,
        });
    }

    Some(text)
}

/// The changes that turn an option on or off, in the engine's terms: a node set to `true`, or
/// removed.
pub fn drawn_changes(item: &Item, enable: bool) -> Vec<(&'static str, Option<String>)> {
    let Control::Drawn {
        on,
        off,
        default_on,
        bare_on,
    } = item.control
    else {
        return Vec::new();
    };
    let flag = Some(String::from("true"));

    match (default_on, enable) {
        // an option a bare block turns on is never left to its default: off is written
        _ if bare_on && enable => vec![(on, flag), (off, None)],
        _ if bare_on => vec![(on, None), (off, flag)],
        // back to niri's default: neither node
        (true, true) | (false, false) => vec![(on, None), (off, None)],
        (true, false) => vec![(on, None), (off, flag)],
        (false, true) => vec![(on, flag), (off, None)],
    }
}

/// What Reset sends for an option: its nodes removed, so niri's default applies; an option a bare
/// block turns on is set off instead, as removing leaves it on.
pub fn reset_changes(item: &Item) -> Vec<(&'static str, Option<String>)> {
    match item.control {
        Control::Drawn {
            default_on,
            bare_on: true,
            ..
        } => drawn_changes(item, default_on),
        _ => item.paths().into_iter().map(|path| (path, None)).collect(),
    }
}

/// What a typed value is written as: a name or text, quoted so it stays text.
pub fn quoted(text: &str) -> String {
    format!("\"{text}\"")
}

// ---- the draw thread's side ----

thread_local! {
    // the generation the text inputs were last looked at for
    static LOOKED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };

    // what each input was last set from: the window, the text the file gave, its row's resets
    static SYNCED: RefCell<HashMap<&'static str, (u64, String, u64)>> =
        RefCell::new(HashMap::new());

    // each option's text input id, made once, as an id lives as long as the input's text
    static INPUTS: RefCell<HashMap<&'static str, &'static str>> = RefCell::new(HashMap::new());
}

/// The id of an option's text input.
pub fn input(item: &Item) -> &'static str {
    INPUTS.with_borrow_mut(|inputs| {
        *inputs
            .entry(item.path)
            .or_insert_with(|| Box::leak(format!("settings niri {}", item.path).into_boxed_str()))
    })
}

/// Sets a text input to what the file gives when that is not what it was last set from, or its
/// row had a job or a refusal since: an input being typed in, of a row nothing happened to, is left
/// as it is. On the draw thread, which the inputs live on; the view calls it before it builds them.
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

/// Reads the config again, off the draw thread.
pub fn load() {
    post(Job::Read);
}

/// Sends changes to the engine as one, with the row they are shown on. From a press.
pub fn change(row: &'static str, changes: Vec<(&'static str, Option<String>)>) {
    Settings::write().niri.begin(Some(row));
    post(Job::Change { row, changes });
}

/// Undoes the last change this window made, and no other.
pub fn undo() {
    let mut niri = Settings::write();
    let after = niri.niri.undo_after.clone();

    niri.niri.begin(None);
    drop(niri);
    post(Job::Undo { after });
}

/// Applies a recommended rule from niri's config.kdl.
pub fn apply_rule(id: &'static str) {
    let mut niri = Settings::write();
    niri.niri.begin(Some(id));
    drop(niri);
    post(Job::ApplyRule { id });
}

/// Removes a recommended rule from niri's config.kdl.
pub fn remove_rule(id: &'static str) {
    let mut niri = Settings::write();
    niri.niri.begin(Some(id));
    drop(niri);
    post(Job::RemoveRule { id });
}

// ---- the worker ----

enum Job {
    Read,
    Change {
        row: &'static str,
        changes: Vec<(&'static str, Option<String>)>,
    },
    ApplyRule {
        id: &'static str,
    },
    RemoveRule {
        id: &'static str,
    },
    Undo {
        // the hash the page's own change left, so a later change by hand is not undone
        after: Option<String>,
    },
}

// what the config gave
#[derive(Default)]
struct Read {
    values: HashMap<&'static str, Reading>,
    rules: Vec<niri_config::RuleInfo>,
    not_editable: Option<String>,
    unreadable: Option<String>,

    // the included files, every one of them, which are watched along with the config
    includes: Vec<PathBuf>,

    // those files as they were before the options were read from them: an include that changes
    // while it is being read keeps its stamp and would never be read again otherwise
    include_stamps: Vec<Stamp>,

    // whether niri reads part of the config only when it starts
    restart: bool,

    // the options a bare block in the config itself turns on
    bare: HashSet<&'static str>,
}

// a job's result
struct Done {
    row: Option<&'static str>,
    undo: bool,
    outcome: Option<Result<String, String>>,

    // whether the file differs after the job
    written: bool,
    after: Option<String>,
    stamp: Stamp,
    read: Read,
}

// the config's modification time and length, to see that it changed
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

    // a worker that died is started again once
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
        let why = String::from("could not start the thread that edits niri's config");
        let (row, undo) = row_of(&job);

        Settings::write().niri.done(Done {
            row,
            undo,
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
        .name(String::from("kanade-niri-config"))
        .spawn(move || {
            if let Err(payload) =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&receiver)))
            {
                crate::supervise::panicked("kanade-niri-config", &*payload);
                *WORKER.lock().unwrap_or_else(PoisonError::into_inner) = None;
                Settings::write().niri.panicked();
            }
        })
        .ok()
        .map(|_| sender)
}

fn row_of(job: &Job) -> (Option<&'static str>, bool) {
    match job {
        Job::Change { row, .. } => (Some(row), false),
        Job::ApplyRule { id } | Job::RemoveRule { id } => (Some(id), false),
        Job::Undo { .. } => (None, true),
        Job::Read => (None, false),
    }
}

// the files whose change shows in the page, and their state when the page last read them
struct Watch {
    files: Vec<PathBuf>,
    seen: Vec<Stamp>,

    // the read could not reach the config at all: no stamp can tell that it can now, so every poll
    // reads again until it can
    unreadable: bool,

    // whether the window was seen open; a window asked for opens a moment later, so one not yet
    // open is given a few ticks
    opened: bool,
    ticks: u32,
}

const GRACE: u32 = 5;

fn stamps(files: &[PathBuf]) -> Vec<Stamp> {
    files.iter().map(|file| Stamp::of(file)).collect()
}

// what a read watches when the config could not be reached at all: the config's directory, and the
// config itself. Neither is watched in place of a reading, so the page must also read again at each
// poll: a directory gains the file without its stamp changing, and a file that is not there has no
// stamp to differ, so a page opened before the config existed would never notice it appearing.
fn unwatched() -> Vec<PathBuf> {
    match niri_config::target(None) {
        Ok(file) => file
            .parent()
            .map(|dir| vec![dir.to_owned(), file.clone()])
            .unwrap_or_else(|| vec![file]),
        Err(_) => vec![PathBuf::new()],
    }
}

fn run(receiver: &Receiver<Job>) {
    // while the page shows: the files as the last job left them
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

// a second of watching: stops when the page or the window is gone, reads again when a file changed
fn tick(mut watch: Watch) -> Option<Watch> {
    if Settings::read().page != PAGE {
        return None;
    }

    if kanade_runtime::window_open(WINDOW) {
        watch.opened = true;
    } else if watch.opened || watch.ticks >= GRACE {
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

// whether the files the page watches say it must read the config again. A read that could not reach
// the config at all leaves no stamp to differ, so it reads at every poll until one can: a page
// opened before the config existed must notice it appearing, not wait for the window to be reopened
fn needs_read(watch: &Watch) -> bool {
    watch.unreadable || stamps(&watch.files) != watch.seen
}

// runs a job and posts what came of it; the files to watch after it
fn run_job(job: Job) -> Watch {
    // before the write, so a reload niri makes of it is seen as new
    let loads = ipc::config_load_state();
    let (done, files, config) = work(job);
    let wrote = done.written;
    let unreadable = done.read.unreadable.is_some();
    // the config as the job left it and the includes as they were before they were read; a
    // count that does not match the files leaves a short list, and the next tick reads again
    let mut seen = vec![config];
    seen.extend(done.read.include_stamps.iter().copied());

    Settings::write().niri.done(done);

    // the monitors' blocks are in the same file: a change to it, here or by hand, shows on the cards
    if Settings::read().page == PAGE {
        super::outputs::refresh();
    }

    // niri applies the file itself; the row says Saving... no longer, so the reload is reported after
    if wrote {
        confirm(loads);
    }

    Watch {
        files,
        seen,
        opened: kanade_runtime::window_open(WINDOW),
        ticks: 0,
        unreadable,
    }
}

// how long niri is given to report that it loaded the config
const CONFIRM: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(25);

// waits for niri's own word on the change, when niri has said anything before
fn confirm(before: ipc::ConfigLoadState) {
    // no niri listening: nothing will report a load, so nothing is claimed about it
    if before.count == 0 {
        return;
    }

    let deadline = Instant::now() + CONFIRM;
    let mut applied = None;

    while Instant::now() < deadline {
        thread::sleep(TICK);

        let now = ipc::config_load_state();
        if now.count != before.count {
            applied = Some(!now.last_failed);
            break;
        }
    }

    Settings::write().niri.reloaded(applied);
}

// the result, the files it read (the config first) and the config's stamp before it was read
fn work(job: Job) -> (Done, Vec<PathBuf>, Stamp) {
    let (row, undo) = row_of(&job);
    let setup = niri_config::target(None).and_then(|file| Ok((file, niri_config::state()?)));
    let (file, state) = match setup {
        Ok(setup) => setup,
        Err(why) => {
            return (
                Done {
                    row,
                    undo,
                    outcome: (!matches!(job, Job::Read)).then(|| Err(why.clone())),
                    written: false,
                    after: None,
                    stamp: Stamp(None),
                    read: Read {
                        unreadable: Some(why),
                        ..Read::default()
                    },
                },
                unwatched(),
                Stamp(None),
            );
        }
    };
    let editor = Editor {
        file: file.clone(),
        state,
        validate: &niri_config::niri_validate,
        now_ms: niri_config::now_ms,
    };

    let before = Stamp::of(&file);
    let outcome = match &job {
        Job::Read => None,
        Job::Change { changes, .. } => {
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
    let files = std::iter::once(file).chain(read.includes.clone()).collect();

    (
        Done {
            row,
            undo,
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

/// What the engine reads for every option, and whether it may change the config.
fn read_all(editor: &Editor) -> Read {
    let mut read = Read {
        not_editable: editor.editable().err(),
        ..Read::default()
    };

    // every include is watched, not only the ones that set an option now: one that gains one
    // later must be read again. Stamped before the options are read, not after
    match editor.includes() {
        Ok(files) => {
            read.include_stamps = stamps(&files);
            read.includes = files;
        }
        Err(why) => {
            read.values.clear();
            read.unreadable = Some(why);

            return read;
        }
    }

    read.restart = niri_config::AT_RESTART
        .iter()
        .any(|node| editor.sets_in_config(node).unwrap_or(false));

    for item in ITEMS {
        if matches!(item.control, Control::Drawn { bare_on: true, .. }) {
            match editor.sets_in_config(item.path) {
                Ok(true) => {
                    read.bare.insert(item.path);
                }
                Ok(false) => {}
                Err(why) => {
                    read.values.clear();
                    read.unreadable = Some(why);

                    return read;
                }
            }
        }

        for path in item.paths() {
            match editor.read(path) {
                Ok(reading) => {
                    read.values.insert(path, reading);
                }
                // the file itself is the trouble, the same for every path
                Err(why) => {
                    read.values.clear();
                    read.unreadable = Some(why);

                    return read;
                }
            }
        }
    }

    read.rules = editor.rules().unwrap_or_default();

    read
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    const CONFIG: &str = "layout {\n    gaps 16\n}\n";

    struct Dir(PathBuf);

    impl Dir {
        fn new(config: &str) -> Dir {
            static COUNT: AtomicU32 = AtomicU32::new(0);

            let dir = std::env::temp_dir().join(format!(
                "kanade-niri-page-{}-{}",
                std::process::id(),
                COUNT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(dir.join("state")).unwrap();
            fs::write(dir.join("config.kdl"), config).unwrap();

            Dir(dir)
        }

        fn editor<'a>(&self, validate: niri_config::Validate<'a>) -> Editor<'a> {
            Editor {
                file: self.0.join("config.kdl"),
                state: self.0.join("state"),
                validate,
                now_ms: || 1,
            }
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    // every value the page can send for an option
    fn samples(item: &Item) -> Vec<Vec<(&'static str, Option<String>)>> {
        let one = |value: String| vec![vec![(item.path, Some(value))]];

        match &item.control {
            Control::Flag => one(String::from("true")),
            Control::Drawn { .. } => vec![drawn_changes(item, true), drawn_changes(item, false)],
            Control::Number(number) => [number.least, number.most]
                .into_iter()
                .flat_map(|value| one(format_number(value, number.decimals)))
                .collect(),
            Control::Choice { options, .. } => options
                .names
                .iter()
                .flat_map(|name| one(quoted(name)))
                .collect(),
            Control::Text { example } => one(quoted(example)),
        }
    }

    fn apply(
        editor: &Editor,
        changes: &[(&'static str, Option<String>)],
    ) -> Result<String, String> {
        let changes: Vec<(&str, Option<&str>)> = changes
            .iter()
            .map(|(path, value)| (*path, value.as_deref()))
            .collect();

        editor.apply(&changes)
    }

    #[test]
    fn every_option_is_a_path_the_engine_reads_and_writes() {
        let ok = |_: &Path| Ok(());
        let dir = Dir::new(CONFIG);
        let editor = dir.editor(&ok);

        let mut seen = Vec::new();
        for item in ITEMS {
            assert!(!seen.contains(&item.path), "{} is listed twice", item.path);
            seen.push(item.path);

            for path in item.paths() {
                editor
                    .read(path)
                    .unwrap_or_else(|why| panic!("{path}: {why}"));
            }
            for changes in samples(item) {
                apply(&editor, &changes).unwrap_or_else(|why| panic!("{}: {why}", item.path));
            }
        }
    }

    // niri 26.04 itself on the default config, where it is installed: every extreme the page can
    // send is a value niri takes
    #[test]
    fn niri_accepts_every_value_the_page_sends() {
        let default = Path::new("/usr/share/doc/niri/default-config.kdl");
        let Ok(base) = fs::read_to_string(default) else {
            return;
        };
        if Command::new("niri").arg("--version").output().is_err() {
            return;
        }

        let dir = Dir::new(&base);
        let editor = dir.editor(&niri_config::niri_validate);

        for item in ITEMS {
            for changes in samples(item) {
                apply(&editor, &changes).unwrap_or_else(|why| panic!("{}: {why}", item.path));
            }
        }
    }

    // whether niri 26.04 draws the option for the top-level `text`: `on` wins over `off`, and with
    // neither a block of its own is a border that is on (`appearance.rs`, `layout.rs` of the tag),
    // else niri's default
    fn niri_draws(editor: &Editor, text: &str, item: &Item) -> bool {
        let Control::Drawn {
            on,
            off,
            default_on,
            bare_on,
        } = item.control
        else {
            panic!("{} is not drawn", item.path);
        };
        let set = |path| {
            matches!(editor.read(path).unwrap(),
                Reading::Own(shown) | Reading::Included(shown, _) if shown != "false")
        };
        let name = item.path.rsplit('.').next().unwrap();

        if set(on) {
            true
        } else if set(off) {
            false
        } else {
            default_on || (bare_on && text.contains(&format!("{name} {{")))
        }
    }

    // what the page shows for the option of the config as it stands
    fn page_shows(editor: &Editor, item: &Item) -> Current {
        let mut state = State {
            loaded: true,
            ..State::default()
        };
        for path in item.paths() {
            state.values.insert(path, editor.read(path).unwrap());
        }
        if editor.sets_in_config(item.path).unwrap() {
            state.bare.insert(item.path);
        }

        state.current(item)
    }

    fn rings_turn_on_and_off(validate: niri_config::Validate) {
        for item in ITEMS
            .iter()
            .filter(|item| matches!(item.control, Control::Drawn { .. }))
        {
            let name = item.path.rsplit('.').next().unwrap();
            // a setting of its own, that niri takes there
            let other = if name == "shadow" {
                "softness 30"
            } else {
                "width 1"
            };
            let layout = |inside: &str| format!("layout {{\n    {inside}\n}}\n");
            let starts = [
                String::new(),
                layout("gaps 4"),
                layout(&format!("{name} {{\n        {other}\n    }}")),
                layout(&format!("{name} {{\n    }}")),
                layout(&format!("{name} {{\n        on\n    }}")),
                layout(&format!("{name} {{\n        off\n        {other}\n    }}")),
            ];

            for start in &starts {
                let dir = Dir::new(start);
                let editor = dir.editor(validate);

                // what a user's config already says is what the page shows
                assert_eq!(
                    page_shows(&editor, item),
                    Current::Flag(niri_draws(&editor, start, item)),
                    "{} shown from {start:?}",
                    item.path
                );

                // whatever it was, on and off in turn, twice over
                for enable in [true, false, true, false, false] {
                    apply(&editor, &drawn_changes(item, enable)).unwrap();

                    let text = fs::read_to_string(dir.0.join("config.kdl")).unwrap();
                    assert_eq!(
                        niri_draws(&editor, &text, item),
                        enable,
                        "{} {enable} from {start:?} gave niri\n{text}",
                        item.path
                    );
                    assert_eq!(
                        page_shows(&editor, item),
                        Current::Flag(enable),
                        "{} {enable} from {start:?} showed\n{text}",
                        item.path
                    );
                }
            }
        }
    }

    #[test]
    fn a_ring_turns_on_and_off_as_niri_reads_it() {
        rings_turn_on_and_off(&|_: &Path| Ok(()));

        // and niri takes every file that came of it, where it is installed
        if Command::new("niri").arg("--version").output().is_ok() {
            rings_turn_on_and_off(&niri_config::niri_validate);
        }
    }

    // the field defect: `border { width 1 }` is a border that is on, and off was written as nothing
    #[test]
    fn a_border_off_is_written_as_off_and_a_bare_block_shows_on() {
        let ok = |_: &Path| Ok(());
        let border = ITEMS
            .iter()
            .find(|item| item.path == "layout.border")
            .unwrap();
        let bare = "layout {\n    border {\n        width 1\n    }\n}\n";
        let dir = Dir::new(bare);
        let editor = dir.editor(&ok);
        let file = dir.0.join("config.kdl");

        assert_eq!(page_shows(&editor, border), Current::Flag(true));

        apply(&editor, &drawn_changes(border, false)).unwrap();
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "layout {\n    border {\n        width 1\n        off\n    }\n}\n"
        );
        assert_eq!(page_shows(&editor, border), Current::Flag(false));

        apply(&editor, &drawn_changes(border, true)).unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("on") && !text.contains("off"), "{text}");

        // Reset leaves it off, not on by a block that stays
        apply(&editor, &reset_changes(border)).unwrap();
        assert_eq!(page_shows(&editor, border), Current::Flag(false));

        // an include's bare block is not the config's: it does not turn the border on
        let dir = Dir::new("include \"mine.kdl\"\n");
        fs::write(dir.0.join("mine.kdl"), bare).unwrap();
        let editor = dir.editor(&ok);
        assert!(!editor.sets_in_config("layout.border").unwrap());
        assert_eq!(page_shows(&editor, border), Current::Flag(false));
    }

    // a text with a backslash and a quote is stored raw by the engine and shown back as it was typed
    #[test]
    fn a_text_with_a_backslash_and_a_quote_comes_back_as_typed() {
        let ok = |_: &Path| Ok(());
        let item = ITEMS
            .iter()
            .find(|item| matches!(item.control, Control::Text { .. }))
            .unwrap();

        for typed in [
            r#"C:\dir\"name""#,
            r#"say "hi"#,
            "plain",
            r##"a"#b\"##,
            "tab\there",
        ] {
            let dir = Dir::new("");
            let editor = dir.editor(&ok);
            apply(&editor, &[(item.path, Some(quoted(typed)))]).unwrap();

            let mut state = State {
                loaded: true,
                ..State::default()
            };
            state
                .values
                .insert(item.path, editor.read(item.path).unwrap());

            assert_eq!(
                state.current(item),
                Current::Text(typed.to_owned()),
                "{typed}"
            );
        }
    }

    #[test]
    fn strings_are_read_as_kdl_v1_reads_them() {
        for (shown, text) in [
            (r#""a\nb""#, "a\nb"),
            (r#""\\ \" \/ \b \f \r \t""#, "\\ \" / \u{8} \u{c} \r \t"),
            (r#""\u{e9}\u{1F600}""#, "\u{e9}\u{1f600}"),
            (r#"r"a\b""#, "a\\b"),
            (r##"r#"say "hi""#"##, "say \"hi\""),
        ] {
            assert_eq!(unquote(shown).as_deref(), Some(text), "{shown}");
        }

        for bad in [
            r#""\q""#,
            r#""\u{110000}""#,
            r#""\u{}""#,
            r##"r#"x"##,
            "bare",
            "\"open",
        ] {
            assert_eq!(unquote(bad), None, "{bad}");
        }
    }

    // a node an include sets is read-only whichever of the two files has the last word
    #[test]
    fn a_path_an_include_sets_is_owned_by_it_even_when_the_config_has_the_last_word() {
        let mut state = State {
            loaded: true,
            ..State::default()
        };
        let item = ITEMS
            .iter()
            .find(|item| matches!(item.control, Control::Number(_)))
            .unwrap();
        state.values.insert(
            item.path,
            Reading::Overridden("4".into(), PathBuf::from("/x/mine.kdl")),
        );

        let shown = state.shown(item);
        assert!(!shown.editable);
        assert!(!shown.set);
        assert_eq!(shown.owner.as_deref(), Some("Set by mine.kdl, an include"));
    }

    // an include that sets none of the page's options is watched too: one that gains one later
    // must be read again
    #[test]
    fn an_include_that_sets_nothing_is_watched_too() {
        let ok = |_: &Path| Ok(());
        let dir = Dir::new("include \"extra.kdl\"\n");
        fs::write(dir.0.join("extra.kdl"), "// nothing the page reads\n").unwrap();
        let editor = dir.editor(&ok);

        let read = read_all(&editor);
        assert!(read.unreadable.is_none(), "{:?}", read.unreadable);
        assert!(
            read.includes
                .iter()
                .any(|file| file.file_name().is_some_and(|name| name == "extra.kdl")),
            "{:?}",
            read.includes
        );

        // stamped before the options are read, so a change made in between is not lost under a
        // fresh stamp
        assert_eq!(read.include_stamps.len(), read.includes.len());

        // and the next read picks up an option the include gains
        fs::write(dir.0.join("extra.kdl"), "layout {\n    gaps 42\n}\n").unwrap();
        let read = read_all(&editor);
        assert!(
            matches!(
                read.values.get("layout.gaps"),
                Some(Reading::Included(_, file)) if file.ends_with("extra.kdl")
            ),
            "{:?}",
            read.values.get("layout.gaps")
        );
    }

    #[test]
    fn only_the_inputs_of_rows_that_changed_are_set_again() {
        let mut state = State {
            loaded: true,
            ..State::default()
        };
        let mut was = HashMap::new();
        let keys = |state: &State| {
            ITEMS
                .iter()
                .map(|item| {
                    (
                        item.path,
                        (
                            state.epoch,
                            state.input_text(item),
                            state.resets.get(item.path).copied().unwrap_or(0),
                        ),
                    )
                })
                .collect::<HashMap<_, _>>()
        };
        was.extend(keys(&state));

        state.done(Done {
            row: Some("layout.gaps"),
            undo: false,
            outcome: Some(Ok(String::new())),
            written: true,
            after: None,
            stamp: Stamp(None),
            read: Read::default(),
        });
        state.refuse("layout.border.width", String::from("no"));

        let now = keys(&state);
        let moved: Vec<_> = now
            .iter()
            .filter(|(path, key)| was[*path] != **key)
            .map(|(path, _)| *path)
            .collect();
        assert_eq!(moved.len(), 2, "{moved:?}");
        assert!(moved.contains(&"layout.gaps") && moved.contains(&"layout.border.width"));
    }

    // a worker that panics never posts its Done: without this the row would say Saving... for good
    #[test]
    fn a_panicked_worker_releases_the_row_it_was_saving() {
        let mut state = State {
            loaded: true,
            ..State::default()
        };

        state.begin(Some("layout.gaps"));
        state.begin(None);
        assert_eq!(state.status().unwrap().0, "Undoing\u{2026}");

        state.panicked();

        assert!(!state.shown(item("layout.gaps").unwrap()).busy);
        assert!(state.status().unwrap().0.contains("stopped"));
        assert!(state.status().unwrap().1, "it warns");

        // nothing waits any more, so the next press is taken at once
        state.begin(Some("layout.gaps"));
        assert_eq!(state.status().unwrap().0, "Saving\u{2026}");
    }

    // a page opened before the config could be read must notice it appearing: the stamp of a file
    // that is not there is none, so no comparison of stamps can tell, and the poll reads instead
    #[test]
    fn a_config_that_could_not_be_read_is_read_again_at_every_poll() {
        let dir = Dir::new("");
        let file = dir.0.join("config.kdl");
        fs::remove_file(&file).unwrap();

        // the read that found no config: what it watched has no stamp to differ
        let missing = Watch {
            files: unwatched(),
            seen: stamps(&unwatched()),
            unreadable: true,
            opened: true,
            ticks: 0,
        };

        assert!(needs_read(&missing), "an unreadable config is read again");
        assert!(
            !missing.files.contains(&PathBuf::new()),
            "the config's own directory is watched, not an empty path"
        );

        // once it is there and read, an unchanged file is not read again
        fs::write(&file, CONFIG).unwrap();
        let read = Watch {
            files: vec![file.clone()],
            seen: stamps(std::slice::from_ref(&file)),
            unreadable: false,
            opened: true,
            ticks: 0,
        };

        assert!(!needs_read(&read));
    }

    // a change made after the page's own is not the page's to undo
    #[test]
    fn a_change_made_after_the_pages_own_takes_its_undo_away() {
        let mut state = State {
            loaded: true,
            ..State::default()
        };

        state.begin(Some("layout.gaps"));
        state.done(Done {
            row: Some("layout.gaps"),
            undo: false,
            outcome: Some(Ok(String::from("layout.gaps = 12 (was 4)"))),
            written: true,
            after: Some(String::from("after")),
            stamp: Stamp(Some((1, 0, 10))),
            read: Read::default(),
        });

        assert!(state.undoable);
        assert_eq!(state.undo_after.as_deref(), Some("after"));

        // a read that finds the file as the change left it keeps the button
        state.done(Done {
            row: None,
            undo: false,
            outcome: None,
            written: false,
            after: None,
            stamp: Stamp(Some((1, 0, 10))),
            read: Read::default(),
        });

        assert!(state.undoable);

        // one that finds it changed, by the command line or by hand, takes it away
        state.done(Done {
            row: None,
            undo: false,
            outcome: None,
            written: false,
            after: None,
            stamp: Stamp(Some((2, 0, 10))),
            read: Read::default(),
        });

        assert!(!state.undoable);
        assert_eq!(state.undo_after, None);
    }

    #[test]
    fn the_reload_niri_makes_is_said_in_the_footers_place() {
        let mut state = State {
            loaded: true,
            ..State::default()
        };

        assert_eq!(state.status(), None);

        state.reloaded(Some(true));
        assert_eq!(
            state.status().unwrap(),
            (String::from("niri reloaded the config"), false)
        );

        state.reloaded(Some(false));
        assert!(state.status().unwrap().1, "a rejection warns");

        state.reloaded(None);
        assert!(state.status().unwrap().0.contains("not reloaded"));

        // a press takes it away, and so does a new window
        state.reloaded(Some(true));
        state.begin(Some("layout.gaps"));
        assert_eq!(state.status().unwrap().0, "Saving\u{2026}");
        assert!(!state.status().unwrap().1);

        state.done(Done {
            row: Some("layout.gaps"),
            undo: false,
            outcome: Some(Ok(String::new())),
            written: false,
            after: None,
            stamp: Stamp(None),
            read: Read::default(),
        });
        state.reloaded(Some(true));
        state.reopen();
        assert_eq!(state.status(), None);
    }

    #[test]
    fn numbers_are_written_plainly() {
        assert_eq!(format_number(0.30000000000000004, 1), "0.3");
        assert_eq!(format_number(16.0, 0), "16");
        assert_eq!(format_number(-0.04, 1), "0");
        assert_eq!(format_number(-0.5, 1), "-0.5");
        assert_eq!(format_number(0.75, 2), "0.75");
    }
}
