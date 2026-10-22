use smithay_client_toolkit::reexports::protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::{self, WpFractionalScaleManagerV1},
    wp_fractional_scale_v1::{self, WpFractionalScaleV1},
};
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::{
    wp_viewport::{self, WpViewport},
    wp_viewporter::{self, WpViewporter},
};
use wayland_client::{Connection, Dispatch, QueueHandle, protocol::wl_surface::WlSurface};

use super::WaylandState;

// the compositor counts a fractional scale in 120ths, so 150 means 1.25
const DENOMINATOR: f32 = 120.0;

// the least and the most a scale may be, the range the UI scale has (ui_scale.rs)
const LEAST: f32 = 0.5;
const MOST: f32 = 4.0;

/*
 * a scale as the compositor sent it, kept to one a surface can be told: a scale of 0 or less is a
 * wl_surface protocol error that ends the client, and a huge one overflows the buffer; none when it
 * is not a number, since no surface can be drawn at that either
 */
pub(super) fn sane(scale: f32) -> Option<f32> {
    scale.is_finite().then(|| scale.clamp(LEAST, MOST))
}

/*
 * what a surface needs to show at a scale like 1.25: the compositor says the
 * exact scale, and the viewport shows the bigger buffer at the window's size
 */
pub struct Fractional {
    pub scale: WpFractionalScaleV1,
    pub viewport: WpViewport,
}

// both have to go before the surface they belong to
impl Drop for Fractional {
    fn drop(&mut self) {
        self.scale.destroy();
        self.viewport.destroy();
    }
}

impl WaylandState {
    // none when the compositor lacks either protocol, then windows use whole scales
    pub fn make_fractional(&self, surface: &WlSurface) -> Option<Fractional> {
        let (Some(manager), Some(viewporter)) = (&self.fractional_scale, &self.viewporter) else {
            return None;
        };

        let scale = manager.get_fractional_scale(surface, &self.qh, surface.clone());
        let viewport = viewporter.get_viewport(surface, &self.qh, ());

        Some(Fractional { scale, viewport })
    }
}

impl Dispatch<WpFractionalScaleV1, WlSurface> for WaylandState {
    fn event(
        state: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        surface: &WlSurface,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let wp_fractional_scale_v1::Event::PreferredScale { scale } = event else {
            return;
        };

        let Some(window) = state.window(surface) else {
            return;
        };

        // a scale the compositor cannot be told leaves the last one, which draws a sane surface
        let Some(scale) = sane(scale as f32 / DENOMINATOR) else {
            return;
        };

        window.scale = scale;

        state.redraw_on(surface);
    }
}

// the manager, the viewporter and viewports never send events, but wayland-client still needs somewhere to send them
impl Dispatch<WpFractionalScaleManagerV1, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WpFractionalScaleManagerV1,
        _: wp_fractional_scale_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpViewporter, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WpViewporter,
        _: wp_viewporter::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpViewport, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WpViewport,
        _: wp_viewport::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
