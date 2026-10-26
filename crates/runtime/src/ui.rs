//! The semantic tree (ADR 0095): the roles, names and states of the widgets that already take input,
//! said by the views where they build those, and read from outside as `kanade ui`.
//!
//! The tree is the input targets of each window's last frame, so it is complete by construction
//! where views say what a target is, and never a second description kept beside the drawing. It is
//! shaped for an accessibility adapter later (a flat list of nodes with a parent, a role, a label,
//! states and bounds), which this crate does not have.
//!
//! What a node says splits in two. A `name` is `&'static str`, written in the source, so it can
//! never hold what a user or the world wrote. A `content` label or value is a `String`, and is
//! redacted unless the mode is `Content` (`ui.describe`), scrubbed before it is even stored on a
//! target. A password's text is never stored in any mode.

mod act;
mod tree;

use std::borrow::Cow;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::changes;

pub(crate) use act::{parse_key, typed};
pub(crate) use tree::{nodes, resolve};

// what a node says in place of content, when the mode keeps it
pub const REDACTED: &str = "<redacted>";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    // no node is kept and every request is refused
    Off,

    // roles, names, states and bounds; content is `<redacted>`
    Structure,

    // as Structure, with content as it is (never a password), for debugging on this machine
    Content,
}

static MODE: AtomicU8 = AtomicU8::new(0);

// takes effect for every window as it next draws, so the views are asked to draw again
pub fn set_mode(mode: Mode) {
    if store(mode) {
        changes::mark_all();
    }
}

// whether the mode changed
pub(crate) fn store(mode: Mode) -> bool {
    let code = mode as u8;

    MODE.swap(code, Ordering::Relaxed) != code
}

pub fn mode() -> Mode {
    match MODE.load(Ordering::Relaxed) {
        1 => Mode::Structure,
        2 => Mode::Content,
        _ => Mode::Off,
    }
}

// whether views should say what their targets are
pub fn enabled() -> bool {
    mode() != Mode::Off
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    // a group, named for the path of the nodes inside it
    Region,
    Button,

    // a switch that is on or off (`checked`)
    Toggle,
    Slider,
    TextInput,
    List,
    Item,
    Menu,
    MenuItem,
    Tab,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Region => "region",
            Role::Button => "button",
            Role::Toggle => "toggle",
            Role::Slider => "slider",
            Role::TextInput => "textinput",
            Role::List => "list",
            Role::Item => "item",
            Role::Menu => "menu",
            Role::MenuItem => "menuitem",
            Role::Tab => "tab",
        }
    }
}

/// What a target is, set on a `Rectangle` where a view builds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Semantics {
    pub(crate) role: Role,

    // a node's name among its siblings and part of its id: fixed text of the source, never content
    pub(crate) name: Cow<'static, str>,

    // what the shell or installed software calls it, like an app's name: said in every mode
    pub(crate) label: Option<String>,

    // what the user or the world wrote on it, like a network, a title or a track; redacted by mode
    pub(crate) content: Option<String>,

    // a slider's place from 0 to 1
    pub(crate) level: Option<f32>,

    // a text input's text; content
    pub(crate) text: Option<String>,

    pub(crate) disabled: bool,
    pub(crate) selected: bool,
    pub(crate) checked: Option<bool>,
    pub(crate) focused: bool,

    // holds text that is never said, like a password
    pub(crate) secret: bool,

    // the runtime text input this stands for, which a click focuses
    pub(crate) input: Option<&'static str>,
}

impl Semantics {
    pub fn new(role: Role, name: impl Into<Cow<'static, str>>) -> Self {
        Self {
            role,
            name: name.into(),
            label: None,
            content: None,
            level: None,
            text: None,
            disabled: false,
            selected: false,
            checked: None,
            focused: false,
            secret: false,
            input: None,
        }
    }

    // what the shell or installed software calls the node, like an app from its desktop entry. It
    // is said in every mode and a name can find the node by it, so never text a user or a program
    // sent: that is `content`
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());

        self
    }

    // what the node shows, as the user or the world wrote it: redacted unless `ui.describe` allows it
    pub fn content(mut self, content: impl Into<String>) -> Self {
        self.content = Some(content.into());

        self
    }

    // a slider's place, 0 to 1
    pub fn level(mut self, level: f32) -> Self {
        self.level = Some(level.clamp(0.0, 1.0));

        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;

        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;

        self
    }

    // on or off, for a toggle
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);

        self
    }

    // the text of a field a view draws itself, like the Launcher's search: content
    pub fn text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());

        self
    }

    // for a field of the view's own that takes the window's keys, as the Launcher's search does
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;

        self
    }

    // for a field of the view's own that holds a secret, like a password: it is never said, nor its text
    pub fn secret(mut self) -> Self {
        self.secret = true;
        self.text = None;

        self
    }

    // a text input of the runtime: its text is content, and none at all for a password
    pub(crate) fn input(
        mut self,
        id: &'static str,
        text: &str,
        password: bool,
        focused: bool,
    ) -> Self {
        self.input = Some(id);
        self.focused = focused;
        self.secret = password;

        if !password {
            self.text = Some(text.to_owned());
        }

        self
    }

    // as much as the mode lets be kept: under Structure the strings are emptied, so nothing of
    // them reaches a target, and a node still knows it had some
    pub(crate) fn scrubbed(mut self, mode: Mode) -> Self {
        if mode != Mode::Content {
            self.content = self.content.map(|_| String::new());
            self.text = self.text.map(|_| String::new());
        }

        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Level(f32),
    Text(String),
}

/// One node of a window's tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// The path of the names of the nodes it is in, then its own: `controls/wifi`. A sibling of the
    /// same name after the first is numbered, `item#2`, so an id holds while earlier ones stay.
    pub id: String,

    /// The index in the window's list of its parent.
    pub parent: Option<usize>,
    pub role: Role,
    pub label: Option<String>,
    pub value: Option<Value>,

    /// Left, top, width and height in the window's logical pixels, before the compositor's scale.
    pub bounds: [f32; 4],
    pub disabled: bool,
    pub selected: bool,
    pub checked: Option<bool>,
    pub focused: bool,
    pub pressed: bool,
    pub secret: bool,

    // where in the window's targets, for acting on it
    pub(crate) target: usize,
}

/// A window's tree, as of its last drawn frame.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub name: String,
    pub width: f32,
    pub height: f32,
    pub scale: f32,
    pub nodes: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    // a left click at the node's middle
    Press,

    // what a click on a text input does
    Focus,

    // each letter as a key, after a click on a text input that does not have the keyboard
    Type(String),

    // one key, like `enter` or `ctrl+a`
    Key(String),

    // wheel lines over the node, positive down
    Scroll(i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Describe {
        window: Option<String>,
    },

    // whether a name is in the tree now and one node
    Find {
        window: Option<String>,
        name: String,
    },
    Act {
        window: Option<String>,
        name: String,
        act: Act,
    },
}

pub enum Answer {
    Windows(Vec<Window>),
    Present(bool),
    Done(String),
}

type Answering = Box<dyn FnOnce(Result<Answer, String>)>;

thread_local! {
    // asked from the event loop's own thread, answered by it as it next looks
    static PENDING: RefCell<Vec<(Request, Answering)>> = const { RefCell::new(Vec::new()) };
}

/*
 * asks for the tree or an act on a node, on the event loop's thread (the IPC handler's). The answer
 * is given once the loop has the windows to look at; refused at once while the mode is off
 */
pub fn ask(request: Request, answer: impl FnOnce(Result<Answer, String>) + 'static) {
    if !enabled() {
        answer(Err(String::from("ui.describe is off")));

        return;
    }

    PENDING.with_borrow_mut(|pending| pending.push((request, Box::new(answer))));
}

pub(crate) fn take() -> Vec<(Request, Answering)> {
    PENDING.with_borrow_mut(std::mem::take)
}
