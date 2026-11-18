use std::time::Instant;

use smithay_client_toolkit::compositor::FrameCallbackData;

use crate::clock;
use crate::frame;
use crate::graphics::Renderer;
use crate::timing::{self, Timing};
use crate::ui_scale;

use super::surface::{Content, OpenWindow, View};

impl OpenWindow {
    // whether the view drawn says the window is to be shown; a normal window always is
    pub(super) fn shown(&self, content: &Content) -> bool {
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

        // keep view layout stable while awaiting configure; the live dimensions below gate drawing
        let (w, h) = if self.width == 0 {
            self.configured.unwrap_or((0, 0))
        } else {
            (self.width, self.height)
        };
        let (width, height) = (w as f32 / ui, h as f32 / ui);

        /*
         * one instant for the whole frame, moved to where this frame is likely to present:
         * the views sample it rather than the frame callback's own arrival. A window that is
         * hidden or unconfigured resets the clock, so no gap across that period teaches it
         */
        let unconfigured = self.width == 0 || self.height == 0;
        let target = if unconfigured {
            self.frame_clock.reset();

            started
        } else {
            self.frame_clock.next(started)
        };

        clock::set(target);

        let (content, reads) = frame::run_view(|| self.view.run(), width, height);

        // a later change to one of these services draws this window again
        self.reads = reads;

        let viewed = Instant::now();

        let name = content.name();

        self.name = match &self.view {
            View::Normal(shown, ..) => shown,
            _ => name,
        };

        // An unconfigured or hidden window cannot promise its next view's key handler yet.
        self.keyboard_ready.drawn(None);

        // whether a layer change still needs a commit, which only the frame below would make
        let changed = self.update_surface(&content);

        // 0 until the compositor configures the window, and again while it is hidden
        if self.width == 0 || self.height == 0 {
            clock::clear();

            if changed {
                self.role.commit();
                crate::graphics::after_frame();
                crate::memory::note_drawn();
            }

            return;
        }

        /*
         * a hidden window can still be configured, by a layer surface the compositor resized while
         * it is unmapped; presenting a buffer then shows it again, so its view says first
         */
        if !self.shown(&content) {
            self.frame_clock.reset();
            clock::clear();

            return;
        }

        let parts = content.into_parts();

        // a window without a child has nothing to show, which is a mistake but not a crash
        let Some(root) = parts.root else {
            eprintln!("kanade: {name} has no child to draw, see child()");

            self.frame_clock.reset();
            clock::clear();

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

        // the frame's predicted instant is done; what follows measures real time
        clock::clear();

        self.keyboard_ready
            .drawn(self.on_key.as_ref().and(parts.on_keyboard_ready));

        /*
         * the frame is on screen, so its time is measured; the wait below frees what it held and
         * would otherwise be counted as part of presenting, drain.rs
         */
        let shown = Instant::now();

        crate::graphics::after_frame();

        // the trim runs on the next wake, after this frame returns and its tree is dropped
        crate::memory::note_drawn();

        let timing = Timing {
            started,
            viewed,
            drawn,
            presented: shown,
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

        if matches!(self.view, View::Normal(..)) {
            if !self.frame_requested {
                self.ask_for_frame();
                self.role.commit();
            }

            return;
        }

        /*
         * a window that wants to hide should do so promptly rather than waiting
         * for a frame callback on a surface that is about to hide.
         */
        let ui = ui_scale::ui_scale();
        clock::set(self.frame_clock.probe_now(Instant::now()));

        let (content, _) = frame::run_view(
            || self.view.run(),
            self.width as f32 / ui,
            self.height as f32 / ui,
        );

        clock::clear();

        if !self.shown(&content) {
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

    // draw a transparent 1x1 frame to keep the layer surface mapped without visual artifacts or unmap races
    pub fn clear_to_hidden(&mut self) -> bool {
        let (buffer_width, buffer_height) = match &self.fractional {
            Some(fractional) => {
                fractional.viewport.set_destination(1, 1);

                (to_real_pixels(1, self.scale), to_real_pixels(1, self.scale))
            }

            None => {
                let whole = self.scale.round().max(1.0) as u32;

                self.role.wl_surface().set_buffer_scale(whole as i32);

                (whole, whole)
            }
        };

        self.gpu.draw(Vec::new(), buffer_width, buffer_height)
    }
}

// a logical size in real pixels at a fractional scale
pub(super) fn to_real_pixels(size: u32, scale: f32) -> u32 {
    let pixels = size as f32 * scale;

    (pixels.round() as u32).max(1)
}
