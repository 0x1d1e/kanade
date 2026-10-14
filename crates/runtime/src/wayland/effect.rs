//! Blur behind a layer window's panes, asked of the compositor with `ext-background-effect` (ADR 0057).

use std::sync::atomic::{AtomicBool, Ordering};

use smithay_client_toolkit::reexports::protocols::ext::background_effect::v1::client::{
    ext_background_effect_manager_v1::{self, Capability, ExtBackgroundEffectManagerV1},
    ext_background_effect_surface_v1::{self, ExtBackgroundEffectSurfaceV1},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum,
    protocol::{wl_compositor::WlCompositor, wl_surface::WlSurface},
};

use crate::InputArea;

use super::WaylandState;

// set by the compositor's capabilities event, which follows the bind
static AVAILABLE: AtomicBool = AtomicBool::new(false);

/// Whether the compositor blurs what lies behind a surface that asks, so a pane can be drawn
/// translucent only where it will be seen through to something blurred.
pub fn blur_available() -> bool {
    AVAILABLE.load(Ordering::Relaxed)
}

// the compositor says what it can do as the global is bound, and when that changes
impl Dispatch<ExtBackgroundEffectManagerV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &ExtBackgroundEffectManagerV1,
        event: ext_background_effect_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let ext_background_effect_manager_v1::Event::Capabilities { flags } = event else {
            return;
        };

        let blur = match flags {
            WEnum::Value(flags) => flags.contains(Capability::Blur),
            WEnum::Unknown(_) => false,
        };

        if AVAILABLE.swap(blur, Ordering::Relaxed) != blur {
            state.request_frames();
        }
    }
}

impl Dispatch<ExtBackgroundEffectSurfaceV1, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &ExtBackgroundEffectSurfaceV1,
        _: ext_background_effect_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/*
 * what one window asks of the compositor's blur: the rects of its surface to blur behind, the
 * panes' outlines. Only a changed region is sent; nothing is asked of a compositor without it
 */
pub struct Blur {
    manager: Option<ExtBackgroundEffectManagerV1>,
    effect: Option<ExtBackgroundEffectSurfaceV1>,

    // the region the compositor holds, none while it holds none
    held: Vec<InputArea>,
}

impl Blur {
    pub fn new(manager: Option<ExtBackgroundEffectManagerV1>) -> Self {
        Self {
            manager,
            effect: None,
            held: Vec::new(),
        }
    }

    // takes effect with the surface's next commit, which a redraw makes
    pub fn set(
        &mut self,
        areas: &[InputArea],
        surface: &WlSurface,
        compositor: &WlCompositor,
        qh: &QueueHandle<WaylandState>,
    ) {
        if self.held == areas {
            return;
        }

        self.held = areas.to_vec();

        if areas.is_empty() {
            if let Some(effect) = self.effect.take() {
                effect.set_blur_region(None);
                effect.destroy();
            }

            return;
        }

        let Some(manager) = &self.manager else {
            return;
        };

        let effect = self
            .effect
            .get_or_insert_with(|| manager.get_background_effect(surface, qh, ()));

        let region = compositor.create_region(qh, ());

        for area in areas {
            region.add(area.x, area.y, area.width, area.height);
        }

        effect.set_blur_region(Some(&region));

        // the compositor keeps its own copy
        region.destroy();
    }
}

// it has to go before its surface
impl Drop for Blur {
    fn drop(&mut self) {
        if let Some(effect) = self.effect.take() {
            effect.destroy();
        }
    }
}
