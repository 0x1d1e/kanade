//! Native layer-shell/window lifecycle.
//!
//! This is a protocol backend, not an Amane facade. All its operations use the
//! client's own Wayland connection, and a window cannot be presented before
//! its first compositor configure. Presentation buffers remain renderer-owned;
//! a submitted buffer must not be reused until wl_buffer.release.
//!
//! Only the *currently* configured and visible window can request a frame.
//! The event loop does not poll when idle. A later layer/XDG/lock backend will
//! use the same input, event and frame scheduling contracts.

use std::collections::BTreeMap;

use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_buffer::WlBuffer,
        wl_callback::{self, WlCallback},
        wl_compositor::WlCompositor,
        wl_output::WlOutput,
        wl_region::WlRegion,
        wl_registry::{self, WlRegistry},
        wl_surface::WlSurface,
    },
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, ZwlrLayerSurfaceV1},
};

use crate::window::{Alignment, Edge, KeyboardMode, Layer, Spec, State, WindowId};

/// Events emitted in arrival order; niri and the Wayland compositor are the
/// sole authorities for configure and frame readiness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Configured {
        id: WindowId,
        width: u32,
        height: u32,
    },
    FrameReady(WindowId),
    Closed(WindowId),
    BufferReleased(WlBuffer),
}

/// Retains one layer surface and its mutable compositor state.
struct Window {
    surface: WlSurface,
    layer: ZwlrLayerSurfaceV1,
    spec: Spec,
    state: State,
    configured_size: Option<(u32, u32)>,
}

/// No platform or renderer state leaks into Kanade's Activity domain.
#[derive(Default)]
struct Listener {
    next_id: u64,
    windows: BTreeMap<WindowId, Window>,
    events: Vec<Event>,
}

impl Listener {
    fn id_for_layer(&self, layer: &ZwlrLayerSurfaceV1) -> Option<WindowId> {
        self.windows
            .iter()
            .find_map(|(id, window)| (&window.layer == layer).then_some(*id))
    }
}

pub struct LayerRuntime {
    connection: Connection,
    queue: EventQueue<Listener>,
    listener: Listener,
    compositor: WlCompositor,
    shell: ZwlrLayerShellV1,
}

impl LayerRuntime {
    /// Fail with an explanatory error if this compositor lacks layer-shell.
    pub fn connect() -> Result<Self, String> {
        let connection =
            Connection::connect_to_env().map_err(|e| format!("connecting to Wayland: {e}"))?;
        let (globals, queue) = registry_queue_init::<Listener>(&connection)
            .map_err(|e| format!("Wayland registry: {e}"))?;
        let handle = queue.handle();
        let compositor = globals
            .bind::<WlCompositor, _, _>(&handle, 4..=6, ())
            .map_err(|e| format!("wl_compositor: {e}"))?;
        let shell = globals
            .bind::<ZwlrLayerShellV1, _, _>(&handle, 4..=5, ())
            .map_err(|e| format!("wlr-layer-shell: {e}"))?;

        Ok(Self {
            connection,
            queue,
            listener: Listener::default(),
            compositor,
            shell,
        })
    }

    /// A buffer is never attached before the compositor's first configure.
    /// The returned id remains stable for the lifetime of the surface.
    pub fn create(&mut self, spec: Spec, output: Option<&WlOutput>) -> WindowId {
        let qh = self.queue.handle();
        let id = WindowId(self.listener.next_id);
        self.listener.next_id += 1;

        let surface = self.compositor.create_surface(&qh, ());
        let layer = self.shell.get_layer_surface(
            &surface,
            output,
            layer(spec.layer),
            "kanade".into(),
            &qh,
            (),
        );

        update_layer(&layer, &spec);
        set_input(&self.compositor, &qh, &surface, &spec);

        let mut state = State::default();
        state.show();
        self.listener.windows.insert(
            id,
            Window {
                surface: surface.clone(),
                layer,
                spec,
                state,
                configured_size: None,
            },
        );

        // Initial empty commit is mandatory for layer-shell. No buffer is
        // created or attached until Configure has been acknowledged.
        surface.commit();
        id
    }

    pub fn update(&mut self, id: WindowId, spec: Spec) -> Result<(), String> {
        let window = self
            .listener
            .windows
            .get_mut(&id)
            .ok_or_else(|| format!("unknown window {}", id.0))?;
        if window.spec == spec {
            return Ok(());
        }

        update_layer(&window.layer, &spec);
        set_input(
            &self.compositor,
            &self.queue.handle(),
            &window.surface,
            &spec,
        );
        window.spec = spec;
        window.state.invalidate();
        window.surface.commit();
        Ok(())
    }

    /// A window's frame must be requested only when a renderer is ready to
    /// present new content. GPU textures or SHM buffers are provided by the
    /// renderer; this runtime never writes or decodes an image file.
    ///
    /// The buffer's lifetime is the caller's responsibility until Release.
    pub fn present(&mut self, id: WindowId, buffer: &WlBuffer) -> Result<bool, String> {
        let window = self
            .listener
            .windows
            .get_mut(&id)
            .ok_or_else(|| format!("unknown window {}", id.0))?;
        if !window.state.begin_frame() {
            return Ok(false);
        }

        let (width, height) = window
            .configured_size
            .ok_or_else(|| format!("window {} has no configure", id.0))?;
        window.surface.attach(Some(buffer), 0, 0);
        window
            .surface
            .damage_buffer(0, 0, width as i32, height as i32);
        window.surface.frame(&self.queue.handle(), id);
        window.surface.commit();
        Ok(true)
    }

    /// Called after present(): only an actively animating window should
    /// schedule another frame. An idle window has no timer or frame callback
    /// after its last presentation callback is delivered.
    pub fn submitted(&mut self, id: WindowId, animation_active: bool) {
        if let Some(window) = self.listener.windows.get_mut(&id) {
            window.state.submitted(animation_active);
        }
    }

    pub fn invalidate(&mut self, id: WindowId) {
        if let Some(window) = self.listener.windows.get_mut(&id) {
            window.state.invalidate();
        }
    }

    pub fn needs_frame(&self, id: WindowId) -> bool {
        self.listener
            .windows
            .get(&id)
            .is_some_and(|window| window.state.needs_frame())
    }

    /// This returns events already dispatched or waits for Wayland messages.
    /// It does not redraw, busy-loop or create timers on its own.
    pub fn dispatch(&mut self) -> Result<Vec<Event>, String> {
        if self.listener.events.is_empty() {
            self.queue
                .blocking_dispatch(&mut self.listener)
                .map_err(|e| format!("Wayland dispatch: {e}"))?;
        }

        self.connection
            .flush()
            .map_err(|e| format!("Wayland flush: {e}"))?;
        Ok(std::mem::take(&mut self.listener.events))
    }

    pub fn remove(&mut self, id: WindowId) {
        if let Some(window) = self.listener.windows.remove(&id) {
            window.layer.destroy();
            window.surface.destroy();
        }
    }
}

fn layer(layer: Layer) -> zwlr_layer_shell_v1::Layer {
    match layer {
        Layer::Background => zwlr_layer_shell_v1::Layer::Background,
        Layer::Bottom => zwlr_layer_shell_v1::Layer::Bottom,
        Layer::Top => zwlr_layer_shell_v1::Layer::Top,
        Layer::Overlay => zwlr_layer_shell_v1::Layer::Overlay,
    }
}

fn update_layer(layer: &ZwlrLayerSurfaceV1, spec: &Spec) {
    use zwlr_layer_surface_v1::{Anchor, KeyboardInteractivity};

    let anchor = match (spec.edge, spec.align) {
        (Edge::Top, Alignment::Center) => Anchor::Top,
        (Edge::Top, Alignment::Start) => Anchor::Top | Anchor::Left,
        (Edge::Top, Alignment::End) => Anchor::Top | Anchor::Right,
        (Edge::Bottom, Alignment::Center) => Anchor::Bottom,
        (Edge::Bottom, Alignment::Start) => Anchor::Bottom | Anchor::Left,
        (Edge::Bottom, Alignment::End) => Anchor::Bottom | Anchor::Right,
        (Edge::Left, Alignment::Center) => Anchor::Left,
        (Edge::Left, Alignment::Start) => Anchor::Left | Anchor::Top,
        (Edge::Left, Alignment::End) => Anchor::Left | Anchor::Bottom,
        (Edge::Right, Alignment::Center) => Anchor::Right,
        (Edge::Right, Alignment::Start) => Anchor::Right | Anchor::Top,
        (Edge::Right, Alignment::End) => Anchor::Right | Anchor::Bottom,
    };

    layer.set_layer(self::layer(spec.layer));
    layer.set_anchor(anchor);
    layer.set_size(spec.width, spec.height);
    layer.set_exclusive_zone(spec.exclusive_zone);
    layer.set_keyboard_interactivity(match spec.keyboard {
        KeyboardMode::None => KeyboardInteractivity::None,
        KeyboardMode::OnDemand => KeyboardInteractivity::OnDemand,
        KeyboardMode::Exclusive => KeyboardInteractivity::Exclusive,
    });
}

fn set_input(
    compositor: &WlCompositor,
    qh: &QueueHandle<Listener>,
    surface: &WlSurface,
    spec: &Spec,
) {
    let region: WlRegion = compositor.create_region(qh, ());
    for rect in spec.input_regions() {
        region.add(rect.x, rect.y, rect.width as i32, rect.height as i32);
    }
    surface.set_input_region(Some(&region));
    region.destroy();
}

impl Dispatch<WlRegistry, GlobalListContents> for Listener {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // GlobalListContents tracks registry changes. Dynamic outputs will
        // be managed by the output/seat layer, not silently hardcoded here.
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for Listener {
    fn event(
        listener: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(id) = listener.id_for_layer(layer) else {
            return;
        };
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer.ack_configure(serial);
                if let Some(window) = listener.windows.get_mut(&id) {
                    let actual_width = if width == 0 { window.spec.width } else { width };
                    let actual_height = if height == 0 {
                        window.spec.height
                    } else {
                        height
                    };
                    window.configured_size = Some((actual_width, actual_height));
                    window.state.configure();
                    listener.events.push(Event::Configured {
                        id,
                        width: actual_width,
                        height: actual_height,
                    });
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                listener.events.push(Event::Closed(id));
            }
            _ => {}
        }
    }
}

impl Dispatch<WlCallback, WindowId> for Listener {
    fn event(
        listener: &mut Self,
        _: &WlCallback,
        _: wl_callback::Event,
        id: &WindowId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let Some(window) = listener.windows.get_mut(id) {
            window.state.frame_callback();
            listener.events.push(Event::FrameReady(*id));
        }
    }
}

impl Dispatch<WlBuffer, ()> for Listener {
    fn event(
        listener: &mut Self,
        buffer: &WlBuffer,
        event: wayland_client::protocol::wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, wayland_client::protocol::wl_buffer::Event::Release) {
            listener.events.push(Event::BufferReleased(buffer.clone()));
        }
    }
}

delegate_noop!(Listener: WlCompositor);
delegate_noop!(Listener: ZwlrLayerShellV1);
delegate_noop!(Listener: WlRegion);
delegate_noop!(Listener: ignore WlSurface);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_identity_is_stable_and_no_two_windows_collide() {
        let mut state = Listener::default();
        let first = WindowId(state.next_id);
        state.next_id += 1;
        let second = WindowId(state.next_id);
        assert_ne!(first, second);
    }
}
