use crate::allocator;
use crate::graphics::font;
use crate::input::released;
use crate::ipc::{Draws, IpcHandlers, IpcReply};
use crate::wayland::WaylandApp;
use crate::window::Named;
use crate::{LayerWindow, Monitor, Window};

#[derive(Default)]
pub struct App {
    font: Option<String>,

    windows: Vec<fn() -> LayerWindow>,
    normal_windows: Vec<Named>,
    per_monitor: Vec<fn(&Monitor) -> LayerWindow>,

    handlers: IpcHandlers,
    on_key_release: Option<fn()>,

    data_control: bool,
}

impl App {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn font(mut self, family: &str) -> Self {
        self.font = Some(String::from(family));

        self
    }

    // each call adds one more window, on the monitor the compositor chooses
    pub fn window(mut self, view: fn() -> LayerWindow) -> Self {
        self.windows.push(view);

        self
    }

    // a regular desktop window with a title bar, next to layer windows; close_window takes the name
    pub fn normal_window(mut self, name: &'static str, view: fn() -> Window) -> Self {
        self.normal_windows.push((name, view));

        self
    }

    // one window on every monitor, following monitors as they are plugged in and out
    pub fn window_per_monitor(mut self, view: fn(&Monitor) -> LayerWindow) -> Self {
        self.per_monitor.push(view);

        self
    }

    // lets `kanade <name> [arguments...]` run the handler while the shell runs; it answers through
    // the reply, at once or later. `draws` is read from the call's words before it runs, and a call
    // that only writes Services draws no frame of its own.
    pub fn ipc(
        mut self,
        name: &str,
        handler: fn(&[String], IpcReply),
        draws: fn(&[String]) -> Draws,
    ) -> Self {
        self.handlers.insert(name, handler, draws);

        self
    }

    /*
     * tells when any key goes up, or the keyboard leaves its window, for a press that counts only
     * while the key is held. It does not say which key: a hold that any release ends fails safe
     */
    pub fn on_key_release(mut self, handler: fn()) -> Self {
        self.on_key_release = Some(handler);

        self
    }

    // opt-in to clipboard selection tracking via data-control protocols
    pub fn data_control(mut self) -> Self {
        self.data_control = true;

        self
    }

    pub fn on_clipboard(mut self, handler: fn(Option<String>, Vec<u8>)) -> Self {
        self.data_control = true;
        crate::wayland::data_control::set_handler(handler);

        self
    }

    pub fn run(self) {
        let no_windows = self.windows.is_empty() && self.normal_windows.is_empty();

        if no_windows && self.per_monitor.is_empty() {
            panic!("failed to run: no window set");
        }

        allocator::limit();

        if let Some(handler) = self.on_key_release {
            released::set(handler);
        }

        if let Some(family) = &self.font {
            font::set_default(family);
        }

        let mut backend = WaylandApp::new(
            self.windows,
            self.normal_windows,
            self.per_monitor,
            self.handlers,
            self.data_control,
        );

        backend.run();
    }
}
