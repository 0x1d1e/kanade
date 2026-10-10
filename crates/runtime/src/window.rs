use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Mutex, PoisonError};

use crate::changes;
use crate::input::KeyHandler;
use crate::{Key, Widget};

// a regular desktop window with a title bar, like a settings app, next to the layer windows
pub struct Window {
    pub(crate) title: String,

    // the size it opens at, the user can resize it after that if it is resizable
    pub(crate) width: f32,
    pub(crate) height: f32,

    pub(crate) resizable: bool,

    // the smallest a resizable window may be made, if any
    pub(crate) min_size: Option<(f32, f32)>,

    pub(crate) root: Option<Box<dyn Widget>>,

    pub(crate) on_key: Option<KeyHandler>,
}

impl Window {
    pub fn new() -> Self {
        Self {
            title: String::from("Kanade"),

            width: 640.0,
            height: 480.0,

            resizable: true,
            min_size: None,

            root: None,

            on_key: None,
        }
    }

    // read once, when the window opens
    pub fn title(mut self, title: &str) -> Self {
        self.title = String::from(title);

        self
    }

    // read once, when the window opens
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.width = width;
        self.height = height;

        self
    }

    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;

        self
    }

    // read once, when the window opens
    pub fn min_size(mut self, width: f32, height: f32) -> Self {
        self.min_size = Some((width, height));

        self
    }

    pub fn child(mut self, child: impl Widget + 'static) -> Self {
        self.root = Some(Box::new(child));

        self
    }

    pub fn on_key(mut self, handler: impl Fn(Key) + 'static) -> Self {
        self.on_key = Some(Rc::new(handler));

        self
    }
}

impl Default for Window {
    fn default() -> Self {
        Self::new()
    }
}

// windows asked for from handlers, opened by the event loop when it wakes
// a normal window's name, which close_window takes, and its view
pub type Named = (&'static str, fn() -> Window);

pub static REQUESTED: Mutex<Vec<Named>> = Mutex::new(Vec::new());

/*
 * opens a normal window later on, like settings from a button or an ipc call;
 * the name tells windows apart, so asking for a name already open does nothing
 */
pub fn open_window(name: &'static str, view: fn() -> Window) {
    REQUESTED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((name, view));

    changes::mark_all();
}

// the normal windows open now, by name, kept by the event loop
pub static OPEN: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

// whether a normal window of this name is open; one asked for but not yet opened is not
pub fn window_open(name: &str) -> bool {
    OPEN.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(&name)
}

// windows asked to close from handlers, closed by the event loop when it wakes
pub static CLOSING: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

// closes the window open with this name, like a close button inside it; a closed one is left alone
pub fn close_window(name: &'static str) {
    CLOSING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(name);

    changes::mark_all();
}

thread_local! {
    // the size of the window whose view is running, set by the backend before each run
    static SIZE: Cell<(f32, f32)> = const { Cell::new((0.0, 0.0)) };
}

/*
 * the window being drawn, in logical pixels; a compositor can give a
 * window another size than it asked for, like a tiling one does. 0 by 0
 * before the compositor has said
 */
pub fn window_size() -> (f32, f32) {
    SIZE.get()
}

pub(crate) fn set_size(width: f32, height: f32) {
    SIZE.set((width, height));
}
