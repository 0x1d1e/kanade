use crate::allocator;
use crate::graphics::font;
use crate::ipc::IpcHandlers;
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

    // lets `kanade <name> [arguments...]` run the handler while the shell is running
    pub fn ipc(mut self, name: &str, handler: fn(&[String]) -> String) -> Self {
        self.handlers.insert(name, handler);

        self
    }

    pub fn run(self) {
        let no_windows = self.windows.is_empty() && self.normal_windows.is_empty();

        if no_windows && self.per_monitor.is_empty() {
            panic!("failed to run: no window set");
        }

        allocator::limit();

        if let Some(family) = &self.font {
            font::set_default(family);
        }

        let mut backend = WaylandApp::new(
            self.windows,
            self.normal_windows,
            self.per_monitor,
            self.handlers,
        );

        backend.run();
    }
}
