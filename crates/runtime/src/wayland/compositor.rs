use smithay_client_toolkit::compositor::CompositorHandler;
use wayland_client::{
    Connection, QueueHandle,
    protocol::{wl_output::Transform, wl_output::WlOutput, wl_surface::WlSurface},
};

use super::{WaylandState, scale};

impl CompositorHandler for WaylandState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &WlSurface,
        new_factor: i32,
    ) {
        let Some(window) = self.window(surface) else {
            return;
        };

        // a fractional scale, when there is one, already says the exact scale
        if window.fractional.is_some() {
            return;
        }

        // a scale the compositor cannot be told leaves the last one, which draws a sane surface
        let Some(scale) = scale::sane(new_factor as f32) else {
            return;
        };

        window.scale = scale;
        self.redraw_on(surface);
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, _: u32) {
        let Some(window) = self.window(surface) else {
            return;
        };

        window.frame_requested = false;

        self.redraw_on(surface);
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }
}
