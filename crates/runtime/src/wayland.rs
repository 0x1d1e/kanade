mod backdrop;
mod compositor;
mod keyboard;
mod layer;
mod normal;
mod output;
mod pointer;
mod redraw;
mod scale;
mod seat;
mod socket;
mod surface;
mod toplevels;
mod update;
mod wake;
mod windows;

use std::collections::HashMap;

use smithay_client_toolkit::reexports::protocols::wp::{
    fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    viewporter::client::wp_viewporter::WpViewporter,
};
use smithay_client_toolkit::{
    compositor::CompositorState,
    delegate_dispatch2, delegate_registry,
    output::OutputState,
    reexports::{
        calloop::{EventLoop, LoopHandle},
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{SeatState, pointer::ThemedPointer},
    shell::{wlr_layer::LayerShell, xdg::XdgShell},
    shm::{Shm, ShmHandler},
};
use wayland_client::{
    Connection, QueueHandle,
    backend::ObjectId,
    globals::registry_queue_init,
    protocol::{wl_keyboard::WlKeyboard, wl_surface::WlSurface},
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1;

use crate::ipc::IpcHandlers;
use crate::window::Named;
use crate::{Cursor, LayerWindow, Monitor};
use backdrop::Captures;
use toplevels::Toplevel;

use surface::{OpenWindow, View};

struct WaylandState {
    // every window has its own layer surface and gpu, and is dropped before the connection
    windows: Vec<OpenWindow>,

    // each of these gets a window on every monitor, including ones plugged in later
    per_monitor: Vec<fn(&Monitor) -> LayerWindow>,

    // keys do not say which window they are for, so the window that has focus is kept
    keyboard_focus: Option<WlSurface>,

    registry: RegistryState,
    output: OutputState,
    seat: SeatState,

    // kept so new windows can be made while the shell runs
    compositor: CompositorState,
    layer_shell: LayerShell,
    xdg_shell: XdgShell,
    shm: Shm,

    // both are needed for fractional scales like 1.25, none when the compositor lacks one
    fractional_scale: Option<WpFractionalScaleManagerV1>,
    viewporter: Option<WpViewporter>,

    // the windows the compositor lists, none without its toplevel management
    toplevel_manager: Option<ZwlrForeignToplevelManagerV1>,
    toplevels: HashMap<ObjectId, Toplevel>,

    // the panes whose backdrops are copied from the compositor, none without its screencopy
    captures: Option<Captures>,

    // the gpu draws through libwayland's own display, and cursors are set through it
    connection: Connection,

    // kept so they can be released when the mouse or keyboard is unplugged
    pointer_device: Option<ThemedPointer<()>>,
    keyboard_device: Option<WlKeyboard>,

    // what the pointer was last set to on one of the windows, none after it leaves
    cursor_shown: Option<Cursor>,

    qh: QueueHandle<WaylandState>,

    // the keyboard repeats a held key on a timer in the event loop
    loop_handle: LoopHandle<'static, WaylandState>,

    running: bool,
}

// the state is dropped before the event loop, which holds the connection the gpu draws through
pub struct WaylandApp {
    state: WaylandState,

    event_loop: EventLoop<'static, WaylandState>,
}

impl WaylandApp {
    pub fn new(
        views: Vec<fn() -> LayerWindow>,
        normal_views: Vec<Named>,
        per_monitor: Vec<fn(&Monitor) -> LayerWindow>,
        handlers: IpcHandlers,
    ) -> Self {
        let connection = connect();

        let event_loop = EventLoop::try_new().expect("failed to create event loop");

        let (globals, event_queue) =
            registry_queue_init(&connection).expect("failed to discover Wayland globals");

        let qh = event_queue.handle();

        let compositor =
            CompositorState::bind(&globals, &qh).expect("failed to bind wl_compositor");

        let layer_shell = LayerShell::bind(&globals, &qh).expect("failed to bind wlr-layer-shell");

        let xdg_shell = XdgShell::bind(&globals, &qh).expect("failed to bind xdg-shell");

        let shm = Shm::bind(&globals, &qh).expect("failed to bind wl_shm");

        let toplevel_manager = toplevels::bind(&globals, &qh);

        let captures = backdrop::bind(&globals, &qh);
        let capable = captures.is_some();

        let fractional_scale = globals.bind(&qh, 1..=1, ()).ok();
        let viewporter = globals.bind(&qh, 1..=1, ()).ok();

        // windows per monitor are opened once the compositor describes each monitor
        let mut state = WaylandState {
            windows: Vec::new(),

            per_monitor,

            keyboard_focus: None,

            registry: RegistryState::new(&globals),
            output: OutputState::new(&globals, &qh),
            seat: SeatState::new(&globals, &qh),

            compositor,
            layer_shell,
            xdg_shell,
            shm,

            fractional_scale,
            viewporter,

            toplevel_manager,
            toplevels: HashMap::new(),

            captures,

            connection: connection.clone(),

            pointer_device: None,
            cursor_shown: None,
            keyboard_device: None,

            qh,

            loop_handle: event_loop.handle(),

            running: true,
        };

        for view in views {
            state.open(View::Plain(view), None);
        }

        for (name, view) in normal_views {
            state.open_normal(name, view);
        }

        WaylandSource::new(connection, event_queue)
            .insert(event_loop.handle())
            .expect("failed to insert Wayland source");

        socket::insert(&event_loop.handle(), handlers);

        wake::insert(&event_loop.handle());

        backdrop::insert(&event_loop.handle(), capable);

        Self { state, event_loop }
    }

    pub fn run(&mut self) {
        while self.state.running {
            self.event_loop
                .dispatch(None, &mut self.state)
                .expect("failed to dispatch events");
        }
    }
}

impl ProvidesRegistryState for WaylandState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers![OutputState, SeatState];
}

// the cursor theme fallback draws its icons into shared memory
impl ShmHandler for WaylandState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

fn connect() -> Connection {
    Connection::connect_to_env().expect("failed to connect to Wayland")
}

delegate_registry!(WaylandState);

delegate_dispatch2!(WaylandState);
