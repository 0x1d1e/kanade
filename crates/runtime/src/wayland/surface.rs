use std::time::Instant;

use smithay_client_toolkit::shell::{
    WaylandSurface, wlr_layer::LayerSurface, xdg::window::Window as XdgWindow,
};
use wayland_client::{
    QueueHandle,
    protocol::{wl_compositor::WlCompositor, wl_output::WlOutput, wl_surface::WlSurface},
};

use crate::changes::Reads;
use crate::graphics::Gpu;
use crate::input::{KeyHandler, LostHandler, Pointer, ReadyHandler};
use crate::{InputArea, LayerWindow, Monitor, Widget, Window};

use super::{
    WaylandState,
    effect::Blur,
    layer::{self, Settings},
    scale::Fractional,
};

// one surface on screen, with everything it needs to draw and take input
pub struct OpenWindow {
    pub view: View,

    // only windows made per monitor are tied to one, the rest let the compositor choose
    pub output: Option<WlOutput>,

    // the input region last sent, so only a real change is sent again
    pub input_region: Option<Vec<InputArea>>,

    // what `kanade ui` calls the window: the layer's namespace or the normal window's name, as last drawn
    pub name: &'static str,

    pub width: u32,
    pub height: u32,

    pub scale: f32,

    pub frame_requested: bool,

    // when the last frame started drawing, for KANADE_FRAMES
    pub last_frame: Option<Instant>,

    // the services the last view read, a change to any of them draws the window again
    pub reads: Reads,

    // both come from the last drawn view, so input matches what is on screen
    pub pointer: Pointer,
    pub on_key: Option<KeyHandler>,
    pub keyboard_ready: super::ready::KeyboardReady,

    // the keyboard left this window, which a window that only stands while focused closes on
    pub on_lost: Option<LostHandler>,

    // the gpu draws into the surface, so it has to go first when both are dropped
    pub gpu: Gpu,

    // none at whole scales; dropped before the role, since it belongs to its surface
    pub fractional: Option<Fractional>,

    // blur behind the panes of the window, asked of the compositor as the view says
    pub blur: Blur,

    pub role: Role,

    // kept to make input regions, which come from the compositor
    pub compositor: WlCompositor,

    pub qh: QueueHandle<WaylandState>,
}

// what a window is to the compositor: a layer like a bar, or a normal window
pub enum Role {
    Layer {
        surface: LayerSurface,

        // what the compositor was last told, so only a real change is sent again
        settings: Settings,
    },

    Normal {
        window: XdgWindow,

        // the size it opened at, kept for as long as the compositor leaves the size to it
        size: (u32, u32),
    },
}

#[derive(Debug)]
pub struct NormalInfo {
    pub title: String,
    pub width: f32,
    pub height: f32,
    pub resizable: bool,
    pub min_size: Option<(f32, f32)>,
    pub reads: Reads,
}

pub struct InitialNormal {
    pub window: Window,
    pub reads: Reads,
    pub moving: bool,
}

// what a window runs on every redraw to find out what it shows
pub enum View {
    Plain(fn() -> LayerWindow),

    // the monitor is replaced when the compositor reports a change to it
    Monitor(fn(&Monitor) -> LayerWindow, Monitor),

    // the name tells it apart from other normal windows, with the initial window kept for the first frame
    Normal(&'static str, fn() -> Window, Option<InitialNormal>),
}

// what a view gave back
pub enum Content {
    Layer(LayerWindow),

    Normal(Window),
}

impl OpenWindow {
    // a size of 0 leaves the choice to the window, which then keeps the size it asked for
    pub fn resize(&mut self, width: u32, height: u32) {
        let visible = !matches!(&self.role, Role::Layer { settings, .. } if !settings.visible);
        let Some((width, height)) = configured_size(visible, self.asked_size(), (width, height))
        else {
            return;
        };

        self.width = width;
        self.height = height;
        self.redraw();
    }

    fn asked_size(&self) -> (u32, u32) {
        match &self.role {
            Role::Layer { settings, .. } => {
                let width = layer::to_pixels(settings.width);
                let height = layer::to_pixels(settings.height);

                (width, height)
            }

            Role::Normal { size, .. } => *size,
        }
    }
}

fn configured_size(visible: bool, asked: (u32, u32), configured: (u32, u32)) -> Option<(u32, u32)> {
    // A configure sent before unmapping can arrive after hide zeroed the dimensions. Restoring
    // them would make reopening await a frame callback on an unmapped surface forever.
    if !visible {
        return None;
    }
    Some((
        if configured.0 == 0 {
            asked.0
        } else {
            configured.0
        },
        if configured.1 == 0 {
            asked.1
        } else {
            configured.1
        },
    ))
}

impl Role {
    pub fn wl_surface(&self) -> &WlSurface {
        match self {
            Role::Layer { surface, .. } => surface.wl_surface(),
            Role::Normal { window, .. } => window.wl_surface(),
        }
    }

    pub fn commit(&self) {
        self.wl_surface().commit();
    }
}

impl View {
    // runs the view once to inspect sizing and title, holding the window for the first frame
    pub fn normal(name: &'static str, view: fn() -> Window) -> (Self, NormalInfo) {
        let (window, reads) = crate::frame::run_view(view, 0.0, 0.0);
        let moving = crate::animation::moving::take();

        let info = NormalInfo {
            title: window.title.clone(),
            width: window.width,
            height: window.height,
            resizable: window.resizable,
            min_size: window.min_size,
            reads: reads.clone(),
        };

        let initial = InitialNormal {
            window,
            reads,
            moving,
        };

        (View::Normal(name, view, Some(initial)), info)
    }

    pub fn run(&mut self) -> Content {
        match self {
            View::Plain(view) => Content::Layer(view()),
            View::Monitor(view, monitor) => Content::Layer(view(monitor)),
            View::Normal(_, view, initial) => match initial.take() {
                Some(initial) => {
                    crate::changes::note_reads(initial.reads);

                    if initial.moving {
                        crate::animation::moving::set();
                    }

                    Content::Normal(initial.window)
                }
                None => Content::Normal(view()),
            },
        }
    }

    pub fn shows(&self, name: &str) -> bool {
        let View::Normal(shown, ..) = self else {
            return false;
        };

        *shown == name
    }
}

// what a window draws and what it does with input, which every kind of window gives the same way
pub struct Parts {
    pub root: Option<Box<dyn Widget>>,
    pub on_key: Option<KeyHandler>,
    pub on_keyboard_ready: Option<ReadyHandler>,
    pub on_lost: Option<LostHandler>,
}

impl Content {
    // the name KANADE_FRAMES logs the window under
    pub fn name(&self) -> &'static str {
        match self {
            Content::Layer(window) => window.namespace,
            Content::Normal(_) => "normal",
        }
    }

    // the widgets and the key handler, drawn and used the same way by every kind of window
    pub fn into_parts(self) -> Parts {
        match self {
            Content::Layer(window) => Parts {
                root: window.root,
                on_key: window.on_key,
                on_keyboard_ready: window.on_keyboard_ready,
                on_lost: window.on_lost,
            },
            Content::Normal(window) => Parts {
                root: window.root,
                on_key: window.on_key,
                on_keyboard_ready: None,
                on_lost: window.on_lost,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::service::Service;
    use crate::unpoisoned::MutexExt;

    #[test]
    fn canceled_open_ignores_late_configure_until_reopened() {
        let asked = (568, 448);
        let mut size = (0, 0); // hide resets the drawable size before the first configure arrives
        for configured in [(568, 448), (600, 480)] {
            if let Some(next) = configured_size(false, asked, configured) {
                size = next;
            }
        }
        assert_eq!(size, (0, 0)); // request_frame must redraw immediately while unmapped

        size = configured_size(true, asked, (0, 480)).unwrap();
        assert_eq!(size, (568, 480)); // the configure for the new opening is accepted
    }

    #[derive(Default)]
    struct NormalTestState;

    impl Service for NormalTestState {
        fn new() -> Self {
            Self
        }

        fn listen() {}
    }

    static RUNS: AtomicUsize = AtomicUsize::new(0);

    fn test_normal_view() -> Window {
        RUNS.fetch_add(1, Ordering::SeqCst);
        drop(NormalTestState::read());

        Window::new()
            .title("Test Normal Window")
            .size(720.0, 480.0)
            .min_size(360.0, 240.0)
            .resizable(true)
    }

    #[test]
    fn open_normal_runs_view_once_across_open_and_initial_frame() {
        let _turn = crate::service::DERIVING.locked();

        RUNS.store(0, Ordering::SeqCst);

        // opening the normal window runs the view once to read title and size
        let (mut view, info) = View::normal("test_normal", test_normal_view);

        assert_eq!(RUNS.load(Ordering::SeqCst), 1);
        assert_eq!(info.title, "Test Normal Window");
        assert_eq!(info.width, 720.0);
        assert_eq!(info.height, 480.0);
        assert_eq!(info.min_size, Some((360.0, 240.0)));
        assert!(info.resizable);
        assert!(info.reads.contains_service::<NormalTestState>());

        // initial frame run: reuses the retained Window, view is not called again
        let (content, reads) = crate::frame::run_view(|| view.run(), info.width, info.height);

        assert_eq!(RUNS.load(Ordering::SeqCst), 1);
        assert!(reads.contains_service::<NormalTestState>());

        let Content::Normal(window) = content else {
            panic!("expected Content::Normal");
        };
        assert_eq!(window.title, "Test Normal Window");

        // subsequent frame run: view function runs again
        let (_next_content, next_reads) =
            crate::frame::run_view(|| view.run(), info.width, info.height);

        assert_eq!(RUNS.load(Ordering::SeqCst), 2);
        assert!(next_reads.contains_service::<NormalTestState>());
    }

    static MOVING_RUNS: AtomicUsize = AtomicUsize::new(0);

    fn moving_normal_view() -> Window {
        MOVING_RUNS.fetch_add(1, Ordering::SeqCst);
        crate::animation::moving::set();

        Window::new().title("Moving Window")
    }

    #[test]
    fn open_normal_preserves_moving_animation_across_first_frame() {
        let _turn = crate::service::DERIVING.locked();

        MOVING_RUNS.store(0, Ordering::SeqCst);

        let (mut view, _info) = View::normal("moving_normal", moving_normal_view);
        assert_eq!(MOVING_RUNS.load(Ordering::SeqCst), 1);

        // initial frame run: reuses window and restores moving status
        let _ = view.run();
        assert_eq!(MOVING_RUNS.load(Ordering::SeqCst), 1);
        assert!(crate::animation::moving::take());
    }
}
