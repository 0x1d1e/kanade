use smithay_client_toolkit::shell::{
    WaylandSurface,
    wlr_layer::{
        self, Anchor, KeyboardInteractivity, LayerShell, LayerShellHandler, LayerSurface,
        LayerSurfaceConfigure,
    },
};
use wayland_client::{
    Connection, QueueHandle,
    protocol::{wl_output::WlOutput, wl_surface::WlSurface},
};

use crate::{Horizontal, Keyboard, Layer, LayerWindow, Margin, Vertical, WindowSize, Zone};

use super::WaylandState;

// the parts of the window the compositor knows about, kept to see what the next view changes
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub width: WindowSize,
    pub height: WindowSize,

    pub vertical: Vertical,
    pub horizontal: Horizontal,

    pub margin: Margin,
    pub layer: Layer,
    pub keyboard: Keyboard,
    pub zone: Zone,

    pub namespace: &'static str,

    pub visible: bool,
}

impl Settings {
    /*
     * what the compositor is told of a window whose view works in the virtual space: sizes and
     * margins in its logical pixels, so a changed UI scale is a changed setting like any other
     */
    pub fn new(window: &LayerWindow, ui: f32) -> Self {
        let up = |length: f32| length * ui;

        let size = |size: WindowSize| match size {
            WindowSize::Full => WindowSize::Full,
            WindowSize::Fixed(pixels) => WindowSize::Fixed(up(pixels)),
        };

        let margin = |side: i32| up(side as f32).round() as i32;

        Self {
            width: size(window.width),
            height: size(window.height),

            vertical: window.vertical,
            horizontal: window.horizontal,

            margin: Margin {
                top: margin(window.margin.top),
                right: margin(window.margin.right),
                bottom: margin(window.margin.bottom),
                left: margin(window.margin.left),
            },
            layer: window.layer,
            keyboard: window.keyboard,
            zone: window.zone,

            namespace: window.namespace,

            visible: window.visible,
        }
    }
}

impl LayerShellHandler for WaylandState {
    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer_surface: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(window) = self.window(layer_surface.wl_surface()) else {
            return;
        };

        let (width, height) = configure.new_size;

        window.resize(width, height);
        self.refresh_cursor(layer_surface.wl_surface());
    }

    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer_surface: &LayerSurface) {
        self.close(layer_surface.wl_surface());
    }
}

pub fn create(
    layer_shell: &LayerShell,
    surface: WlSurface,
    output: Option<&WlOutput>,
    qh: &QueueHandle<WaylandState>,
    settings: &Settings,
) -> LayerSurface {
    // with no output given, the compositor chooses the monitor
    let layer_surface = layer_shell.create_layer_surface(
        qh,
        surface,
        to_layer(settings.layer),
        Some(settings.namespace),
        output,
    );

    apply(&layer_surface, settings);

    // every layer surface must make an initial commit without a buffer so the compositor can configure it
    layer_surface.commit();

    layer_surface
}

// the requests only take effect with the next commit
pub fn apply(layer_surface: &LayerSurface, settings: &Settings) {
    layer_surface.set_layer(to_layer(settings.layer));
    let (width, height) = if settings.visible {
        (to_pixels(settings.width), to_pixels(settings.height))
    } else {
        (1, 1)
    };

    layer_surface.set_size(width, height);

    layer_surface.set_anchor(to_anchor(settings));

    let margin = if settings.visible {
        settings.margin
    } else {
        Margin::default()
    };

    layer_surface.set_margin(margin.top, margin.right, margin.bottom, margin.left);

    let interactivity = if settings.visible {
        to_interactivity(settings.keyboard)
    } else {
        KeyboardInteractivity::None
    };

    layer_surface.set_keyboard_interactivity(interactivity);

    layer_surface.set_exclusive_zone(to_exclusive_zone(settings));
}

// 0 tells the compositor to stretch between the anchored edges
pub fn to_pixels(size: WindowSize) -> u32 {
    match size {
        WindowSize::Full => 0,
        WindowSize::Fixed(pixels) => pixels.round() as u32,
    }
}

fn to_anchor(settings: &Settings) -> Anchor {
    let mut anchor = Anchor::empty();

    match settings.vertical {
        Vertical::Top => anchor |= Anchor::TOP,
        Vertical::Middle => {}
        Vertical::Bottom => anchor |= Anchor::BOTTOM,
    }

    match settings.horizontal {
        Horizontal::Left => anchor |= Anchor::LEFT,
        Horizontal::Middle => {}
        Horizontal::Right => anchor |= Anchor::RIGHT,
    }

    // stretching only works between two opposite anchored edges
    if settings.width == WindowSize::Full {
        anchor |= Anchor::LEFT | Anchor::RIGHT;
    }

    if settings.height == WindowSize::Full {
        anchor |= Anchor::TOP | Anchor::BOTTOM;
    }

    anchor
}

fn to_layer(layer: Layer) -> wlr_layer::Layer {
    match layer {
        Layer::Background => wlr_layer::Layer::Background,
        Layer::Bottom => wlr_layer::Layer::Bottom,
        Layer::Top => wlr_layer::Layer::Top,
        Layer::Overlay => wlr_layer::Layer::Overlay,
    }
}

fn to_interactivity(keyboard: Keyboard) -> KeyboardInteractivity {
    match keyboard {
        Keyboard::None => KeyboardInteractivity::None,
        Keyboard::Exclusive => KeyboardInteractivity::Exclusive,
        Keyboard::OnDemand => KeyboardInteractivity::OnDemand,
    }
}

fn to_exclusive_zone(settings: &Settings) -> i32 {
    if !settings.visible {
        return 0;
    }

    match settings.zone {
        Zone::Reserve => measure_thickness(settings),
        Zone::Respect => 0,
        Zone::Ignore => -1,
    }
}

// a bar on the left or right edge is as thick as its width, any other bar as its height
fn measure_thickness(settings: &Settings) -> i32 {
    let size = match (settings.horizontal, settings.width) {
        (Horizontal::Left | Horizontal::Right, WindowSize::Fixed(_)) => settings.width,
        _ => settings.height,
    };

    let reserved = to_pixels(size);

    i32::try_from(reserved).expect("failed to convert reserved space")
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
    use smithay_client_toolkit::delegate_dispatch2;
    use smithay_client_toolkit::delegate_registry;
    use smithay_client_toolkit::output::{OutputHandler, OutputState};
    use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
    use smithay_client_toolkit::registry_handlers;
    use wayland_client::globals::registry_queue_init;
    use wayland_client::protocol::wl_output;

    struct TestState {
        registry: RegistryState,
        output: OutputState,
        configured_serials: Vec<u32>,
    }

    impl ProvidesRegistryState for TestState {
        fn registry(&mut self) -> &mut RegistryState {
            &mut self.registry
        }
        registry_handlers![OutputState];
    }

    impl OutputHandler for TestState {
        fn output_state(&mut self) -> &mut OutputState {
            &mut self.output
        }
        fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
        fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        }
        fn output_destroyed(
            &mut self,
            _: &Connection,
            _: &QueueHandle<Self>,
            _: wl_output::WlOutput,
        ) {
        }
    }

    impl CompositorHandler for TestState {
        fn scale_factor_changed(
            &mut self,
            _: &Connection,
            _: &QueueHandle<Self>,
            _: &WlSurface,
            _: i32,
        ) {
        }
        fn transform_changed(
            &mut self,
            _: &Connection,
            _: &QueueHandle<Self>,
            _: &WlSurface,
            _: wl_output::Transform,
        ) {
        }
        fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlSurface, _: u32) {}
        fn surface_enter(
            &mut self,
            _: &Connection,
            _: &QueueHandle<Self>,
            _: &WlSurface,
            _: &wl_output::WlOutput,
        ) {
        }
        fn surface_leave(
            &mut self,
            _: &Connection,
            _: &QueueHandle<Self>,
            _: &WlSurface,
            _: &wl_output::WlOutput,
        ) {
        }
    }

    impl LayerShellHandler for TestState {
        fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {}
        fn configure(
            &mut self,
            _: &Connection,
            _: &QueueHandle<Self>,
            _: &LayerSurface,
            _configure: LayerSurfaceConfigure,
            serial: u32,
        ) {
            eprintln!("Test received configure with serial: {serial}");
            self.configured_serials.push(serial);
        }
    }

    delegate_registry!(TestState);
    delegate_dispatch2!(TestState);

    #[test]
    fn test_layer_surface_lifecycle_on_compositor() {
        let Ok(conn) = Connection::connect_to_env() else {
            eprintln!("Wayland not available, skipping test");
            return;
        };

        let (globals, mut event_queue) =
            registry_queue_init(&conn).expect("failed to discover globals");
        let qh = event_queue.handle();

        let compositor = CompositorState::bind(&globals, &qh).expect("bind compositor");
        let layer_shell = LayerShell::bind(&globals, &qh).expect("bind layer_shell");

        let mut state = TestState {
            registry: RegistryState::new(&globals),
            output: OutputState::new(&globals, &qh),
            configured_serials: Vec::new(),
        };

        let wl_surface = compositor.create_surface(&qh);

        // Phase 1: Create LayerSurface and map it
        eprintln!("Phase 1: create layer surface");
        let surf = layer_shell.create_layer_surface(
            &qh,
            wl_surface.clone(),
            wlr_layer::Layer::Top,
            Some("test-lifecycle"),
            None,
        );
        surf.set_size(100, 100);
        surf.set_anchor(Anchor::TOP);
        surf.commit();
        conn.flush().unwrap();

        for _ in 0..10 {
            event_queue.blocking_dispatch(&mut state).unwrap();
            if !state.configured_serials.is_empty() {
                break;
            }
        }
        assert!(
            !state.configured_serials.is_empty(),
            "expected configure for surf"
        );
        eprintln!(
            "Phase 1 success: configured with serial {}",
            state.configured_serials[0]
        );

        // Phase 2: Unmap using attach(None) + commit
        eprintln!("Phase 2: unmap surface with attach(None)");
        state.configured_serials.clear();
        wl_surface.attach(None, 0, 0);
        wl_surface.commit();
        conn.flush().unwrap();
        event_queue.roundtrip(&mut state).unwrap();
        assert!(
            conn.protocol_error().is_none(),
            "protocol error after unmap"
        );
        eprintln!("Phase 2 success: unmapped cleanly without protocol error");

        // Phase 3: Remap by committing without buffer
        eprintln!("Phase 3: remap by committing without buffer");
        surf.set_size(120, 120);
        surf.commit();
        conn.flush().unwrap();

        for _ in 0..10 {
            event_queue.blocking_dispatch(&mut state).unwrap();
            if !state.configured_serials.is_empty() {
                break;
            }
        }
        assert!(
            !state.configured_serials.is_empty(),
            "expected configure after remap"
        );
        eprintln!(
            "Phase 3 success: reconfigured with serial {}",
            state.configured_serials[0]
        );
        assert!(
            conn.protocol_error().is_none(),
            "protocol error after remap"
        );
        eprintln!("ALL PHASES COMPLETED WITH ZERO PROTOCOL ERRORS!");
    }

    #[test]
    fn test_hidden_layer_surface_transitions_on_compositor() {
        let Ok(conn) = Connection::connect_to_env() else {
            eprintln!("Wayland not available, skipping test");
            return;
        };

        let (globals, mut event_queue) =
            registry_queue_init(&conn).expect("failed to discover globals");
        let qh = event_queue.handle();

        let compositor = CompositorState::bind(&globals, &qh).expect("bind compositor");
        let layer_shell = LayerShell::bind(&globals, &qh).expect("bind layer_shell");

        let mut state = TestState {
            registry: RegistryState::new(&globals),
            output: OutputState::new(&globals, &qh),
            configured_serials: Vec::new(),
        };

        let wl_surface = compositor.create_surface(&qh);

        let window = LayerWindow::new().width(200.0).height(50.0).visible(false);
        let settings = Settings::new(&window, 1.0);

        // Phase 1: create layer surface hidden
        eprintln!("Phase 1: create layer surface hidden with initial commit");
        let surf = layer_shell.create_layer_surface(
            &qh,
            wl_surface.clone(),
            to_layer(settings.layer),
            Some("test-hidden"),
            None,
        );
        apply(&surf, &settings);
        surf.commit();
        conn.flush().unwrap();

        for _ in 0..10 {
            event_queue.blocking_dispatch(&mut state).unwrap();
            if !state.configured_serials.is_empty() {
                break;
            }
        }
        assert!(
            !state.configured_serials.is_empty(),
            "expected initial configure for hidden surface"
        );
        eprintln!(
            "Phase 1 success: configured with serial {}",
            state.configured_serials[0]
        );
        assert!(conn.protocol_error().is_none());

        // Phase 2: transition to visible with reserve zone
        eprintln!("Phase 2: transition to visible with reserve zone");
        state.configured_serials.clear();
        let window_vis = LayerWindow::new()
            .width(200.0)
            .height(50.0)
            .space(Zone::Reserve)
            .visible(true);
        let visible_settings = Settings::new(&window_vis, 1.0);
        apply(&surf, &visible_settings);
        surf.commit();
        conn.flush().unwrap();

        for _ in 0..10 {
            event_queue.blocking_dispatch(&mut state).unwrap();
            if !state.configured_serials.is_empty() {
                break;
            }
        }
        assert!(
            !state.configured_serials.is_empty(),
            "expected configure after becoming visible"
        );
        eprintln!(
            "Phase 2 success: reconfigured with serial {}",
            state.configured_serials[0]
        );
        assert!(conn.protocol_error().is_none());

        // Phase 3: transition back to hidden (exclusive zone 0, size 1x1)
        eprintln!("Phase 3: transition back to hidden without unmapping");
        state.configured_serials.clear();
        let window_hid = LayerWindow::new().width(200.0).height(50.0).visible(false);
        let hidden_settings = Settings::new(&window_hid, 1.0);
        apply(&surf, &hidden_settings);
        surf.commit();
        conn.flush().unwrap();

        event_queue.roundtrip(&mut state).unwrap();
        assert!(
            conn.protocol_error().is_none(),
            "protocol error after transition to hidden"
        );
        eprintln!("Phase 3 success: hidden cleanly with zero protocol errors!");
    }
}
