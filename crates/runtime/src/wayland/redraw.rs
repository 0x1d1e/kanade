use std::time::Instant;

use smithay_client_toolkit::compositor::FrameCallbackData;

use crate::frame;
use crate::graphics::Renderer;
use crate::timing::{self, Timing};
use crate::ui_scale;

use super::surface::{Content, OpenWindow, View};

impl OpenWindow {
    // whether the view drawn says the window is to be shown; a normal window always is
    fn shown(&self, content: &Content) -> bool {
        let Content::Layer(window) = content else {
            return true;
        };

        window.visible
    }

    // one frame: run the view, send its settings, draw it and show it
    pub fn redraw(&mut self) {
        let started = Instant::now();

        // views and drawing work in the virtual space, the compositor's pixels over the UI scale
        let ui = ui_scale::ui_scale();

        let (width, height) = (self.width as f32 / ui, self.height as f32 / ui);

        let (content, reads) = frame::run_view(|| self.view.run(), width, height);

        // a later change to one of these services draws this window again
        self.reads = reads;

        let viewed = Instant::now();

        let name = content.name();

        self.name = match &self.view {
            View::Normal(shown, _) => shown,
            _ => name,
        };

        // whether a layer change still needs a commit, which only the frame below would make
        let changed = self.update_surface(&content);

        // 0 until the compositor configures the window, and again while it is hidden
        if self.width == 0 || self.height == 0 {
            if changed {
                self.role.commit();
            }

            return;
        }

        /*
         * a hidden window can still be configured, by a layer surface the compositor resized while
         * it is unmapped; presenting a buffer then shows it again, so its view says first
         */
        if !self.shown(&content) {
            return;
        }

        let parts = content.into_parts();

        // a window without a child has nothing to show, which is a mistake but not a crash
        let Some(root) = parts.root else {
            eprintln!("kanade: {name} has no child to draw, see child()");

            if changed {
                self.role.commit();
            }

            return;
        };

        // the output's scale times the UI scale puts the virtual space on the real pixels
        let frame = frame::build(root.as_ref(), width, height, self.scale * ui);

        // some widgets read services while drawing
        self.reads.extend(frame.reads);

        // the handlers are rebuilt with the view, so each frame replaces the last frame's
        self.pointer.set_targets(frame.targets);

        self.on_key = parts.on_key;
        self.on_lost = parts.on_lost;

        /*
         * an animation that has not arrived yet needs the next frame too; asking
         * in the same commit as this frame gets the answer on the next refresh,
         * a hidden window never gets here so it waits until it shows again
         */
        if frame.moving {
            self.ask_for_frame();
        }

        let drawn = Instant::now();

        let presented = self.present(frame.renderer);

        // the trim runs on the next wake, after this frame returns and its tree is dropped
        crate::memory::note_drawn();

        let timing = Timing {
            started,
            viewed,
            drawn,
            presented: Instant::now(),
        };

        timing::log(name, self.width, self.height, self.last_frame, &timing);

        self.last_frame = Some(started);

        // a skipped frame commits nothing, so the request goes out on its own
        if !presented && (frame.moving || changed) {
            self.role.commit();
        }
    }

    pub fn request_frame(&mut self) {
        /*
         * a hidden or unconfigured window gets no frame callbacks,
         * so the view runs right away to see if the window should show
         */
        if self.width == 0 {
            self.redraw();

            return;
        }

        // changes that land before the next frame all draw together in it
        if self.frame_requested {
            return;
        }

        self.ask_for_frame();

        self.role.commit();
    }

    // the compositor answers once, when the next frame is due
    fn ask_for_frame(&mut self) {
        if self.frame_requested {
            return;
        }

        self.frame_requested = true;

        let surface = self.role.wl_surface();

        surface.frame(&self.qh, FrameCallbackData(surface.clone()));
    }

    /*
     * false when the gpu skipped the frame, so nothing was committed; the window
     * is measured in the compositor's logical pixels, the buffer in real ones, and the
     * output's scale (not the UI scale, which is in the drawing) goes out with the commit
     * that presenting the frame makes
     */
    fn present(&mut self, renderer: Renderer) -> bool {
        let (buffer_width, buffer_height) = match &self.fractional {
            // a buffer at the exact scale, shown back at the window's size
            Some(fractional) => {
                let width = self.width as i32;
                let height = self.height as i32;

                fractional.viewport.set_destination(width, height);

                (
                    to_real_pixels(self.width, self.scale),
                    to_real_pixels(self.height, self.scale),
                )
            }

            None => {
                // a whole number of pixels per logical pixel, at least one: set_buffer_scale(0)
                // is a protocol error, and a buffer of no pixels shows nothing
                let whole = self.scale.round().max(1.0) as u32;

                self.role.wl_surface().set_buffer_scale(whole as i32);

                (self.width * whole, self.height * whole)
            }
        };

        self.gpu
            .draw(renderer.finish(), buffer_width, buffer_height)
    }
}

// a logical size in real pixels at a fractional scale
fn to_real_pixels(size: u32, scale: f32) -> u32 {
    let pixels = size as f32 * scale;

    pixels.round() as u32
}
