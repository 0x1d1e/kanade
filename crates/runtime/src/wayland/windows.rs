use wayland_client::{
    Proxy,
    protocol::{wl_output::WlOutput, wl_surface::WlSurface},
};

use crate::changes::{self, Reads};
use crate::graphics::Gpu;
use crate::input::{Pointer, focus, released};
use crate::ui_scale;
use crate::unpoisoned::MutexExt;
use crate::window::OPEN;
use crate::{Modifiers, frame, service};

use super::{
    WaylandState,
    effect::Blur,
    layer::{self, Settings},
    output::describe,
    surface::{Content, OpenWindow, Role, View},
};

impl WaylandState {
    pub fn open(&mut self, mut view: View, output: Option<WlOutput>) {
        let surface = self.compositor.create_surface(&self.qh);

        /*
         * later views can change the settings, each redraw compares them with the
         * last ones; a window that starts hidden never gets a configure, so it never
         * draws, and what its first view read is all that can wake it to show
         */
        let (content, reads) = frame::run_view(|| view.run(), 0.0, 0.0);

        // normal windows open in open_normal
        let Content::Layer(window) = content else {
            unreachable!("only layer views open as layer windows");
        };

        let settings = Settings::new(&window, ui_scale::ui_scale());

        let layer_surface = layer::create(
            &self.layer_shell,
            surface,
            output.as_ref(),
            &self.qh,
            &settings,
        );

        let role = Role::Layer {
            surface: layer_surface,
            settings,
        };

        self.add(view, output, role);

        if let Some(window) = self.windows.last_mut() {
            window.reads = reads;
        }
    }

    // everything a window needs besides its surface is the same for every role
    pub fn add(&mut self, view: View, output: Option<WlOutput>, role: Role) {
        // the gpu draws straight into the surface, so it gets libwayland's own pointers
        let display = self.connection.backend().display_ptr().cast();
        let surface = role.wl_surface().id().as_ptr().cast();

        let gpu = Gpu::new(display, surface);

        let fractional = self.make_fractional(role.wl_surface());

        let window = OpenWindow {
            view,

            output,

            input_region: None,

            name: "",

            width: 0,
            height: 0,

            scale: 1.0,

            frame_requested: false,
            last_frame: None,
            reads: Reads::default(),

            pointer: Pointer::default(),
            on_key: None,
            on_lost: None,

            gpu,

            fractional,

            blur: Blur::new(self.background_effect.clone()),

            role,

            compositor: self.compositor.wl_compositor().clone(),

            qh: self.qh.clone(),
        };

        self.windows.push(window);
    }

    /*
     * puts a changed UI scale (ADR 0091) into effect: the monitors the views see, every layer
     * window's size, margin and regions, and the backdrop regions follow it as the windows draw
     * again, so nothing is drawn at one scale and told to the compositor at another
     */
    pub fn follow_ui_scale(&mut self) {
        if !ui_scale::adopt() {
            return;
        }

        for window in &mut self.windows {
            let Some(output) = &window.output else {
                continue;
            };

            let Some(info) = self.output.info(output) else {
                continue;
            };

            if let View::Monitor(_, current) = &mut window.view {
                *current = describe(&info);
            }
        }

        self.publish(None);
        self.request_frames();
    }

    // wayland events name the surface they are about, this finds its window
    pub fn window(&mut self, surface: &WlSurface) -> Option<&mut OpenWindow> {
        self.windows
            .iter_mut()
            .find(|window| window.role.wl_surface() == surface)
    }

    // draws the window, then sets the cursor again: a pointer that did not move may now be over
    // something else
    pub fn redraw_on(&mut self, surface: &WlSurface) {
        if let Some(window) = self.window(surface) {
            window.redraw();
        }

        // the caret is known once the window is drawn
        self.sync_ime();

        self.refresh_cursor(surface);
    }

    pub fn refresh_cursor(&mut self, surface: &WlSurface) {
        let cursor = self
            .window(surface)
            .and_then(|window| window.pointer.cursor_inside());

        if let Some(cursor) = cursor {
            self.show_cursor(cursor);
        }
    }

    // a service or handler may have changed what any of the windows shows
    pub fn request_frames(&mut self) {
        for window in &mut self.windows {
            window.request_frame();
        }
    }

    // input only changes the window it arrived in, besides the services it wrote
    pub fn request_frame_on(&mut self, surface: &WlSurface) {
        if let Some(window) = self.window(surface) {
            window.request_frame();
        }
    }

    // only windows that read a service that changed draw again
    pub fn request_changed_frames(&mut self) {
        // writes derived from a change are changes too, so they come before what is taken
        service::derive();

        let Some(changed) = changes::take() else {
            self.request_frames();

            return;
        };

        for window in &mut self.windows {
            if !changed.touches(&window.reads) {
                continue;
            }

            window.request_frame();
        }
    }

    pub fn close(&mut self, surface: &WlSurface) {
        let normal = self.windows.iter().any(|window| {
            window.role.wl_surface() == surface && matches!(window.role, Role::Normal { .. })
        });

        // a window that is gone takes the keyboard with it, whether or not the compositor says so
        self.forget_focus(surface);

        let mut open = OPEN.locked();
        for window in &self.windows {
            if window.role.wl_surface() == surface {
                open.retain(|&name| !window.view.shows(name));
            }
        }
        drop(open);

        self.windows
            .retain(|window| window.role.wl_surface() != surface);

        // windows made per monitor come back when a monitor is plugged in again
        if self.windows.is_empty() && self.per_monitor.is_empty() {
            self.running = false;
        }

        // the window's gpu is already dropped; the shared pool is what a larger target left behind
        if normal {
            // its last frames are freed before the pool is built again
            crate::graphics::drain();
            crate::graphics::release_pool();
            crate::allocator::trim_heap();
        }
    }

    /*
     * the same teardown the keyboard's leave does, for a window that closes instead: until the
     * compositor sends `wl_keyboard.leave`, keys would still go to the closed window's focused text
     * input first and be swallowed by it, and the input method would stay enabled with a stale caret
     */
    fn forget_focus(&mut self, surface: &WlSurface) {
        if self.keyboard_focus.as_ref() != Some(surface) {
            return;
        }

        self.keyboard_focus = None;
        self.modifiers = Modifiers::default();

        focus::clear();
        released::send();

        // the compositor's input method has no surface with the keyboard left, so it is disabled
        self.sync_ime();

        /*
         * the window hears the keyboard leave as it would from the compositor, so what it holds is
         * flushed before it goes; taken out of the window first, since the handler may redraw
         */
        let lost = self
            .window(surface)
            .and_then(|window| window.on_lost.clone());

        if let Some(lost) = lost {
            lost();
        }
    }
}
