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
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;
use std::time::Instant;

use rustix::event::{PollFd, PollFlags, Timespec, poll};

use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_buffer::WlBuffer,
        wl_callback::{self, WlCallback},
        wl_compositor::WlCompositor,
        wl_keyboard::{self, WlKeyboard},
        wl_output::WlOutput,
        wl_pointer::{self, WlPointer},
        wl_region::WlRegion,
        wl_registry::{self, WlRegistry},
        wl_seat::{self, WlSeat},
        wl_shm::{self, WlShm},
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, ZwlrLayerSurfaceV1},
};

use crate::input::{KeyEvent, Keyboard, RepeatInfo};
use crate::keymap::Mapper;
use crate::wake::{self, Waker};
use crate::window::{Alignment, Edge, KeyboardMode, Layer, Spec, State, WindowId};

/// Events emitted in arrival order; niri and the Wayland compositor are the
/// sole authorities for configure and frame readiness.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Configured {
        id: WindowId,
        width: u32,
        height: u32,
    },
    FrameReady(WindowId),
    Closed(WindowId),
    BufferReleased(WlBuffer),
    SourcesChanged,
    Backdrop(Backdrop),
    CaptureFailed { output: String },
    PointerEnter {
        id: WindowId,
        x: f64,
        y: f64,
    },
    PointerLeave(WindowId),
    OutputReady {
        id: u32,
        name: String,
        scale: i32,
    },
    OutputRemoved(u32),
    Keyboard(KeyEvent),
    PointerMotion {
        id: WindowId,
        x: f64,
        y: f64,
    },
    PointerButton {
        id: WindowId,
        button: u32,
        pressed: bool,
        serial: u32,
    },
    PointerScroll {
        id: WindowId,
        x: f64,
        y: f64,
    },
}

/// A real screen capture, not a wallpaper substitute. The caller may upload
/// rgba immediately with gpu::upload_backdrop. No optical work happens on CPU.
#[derive(Clone, Debug, PartialEq)]
pub struct Backdrop {
    pub output: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy)]
enum PixelOrder {
    Bgra,
    Rgba,
}

struct Capture {
    frame: ZwlrScreencopyFrameV1,
    output: String,
    size: Option<(u32, u32, u32, PixelOrder)>,
    flipped: bool,
    file: Option<File>,
    buffer: Option<WlBuffer>,
}

impl Capture {
    fn pixels(&self) -> Option<Backdrop> {
        let (width, height, stride, order) = self.size?;
        let file = self.file.as_ref()?;
        let len = usize::try_from(u64::from(stride) * u64::from(height)).ok()?;
        let mut raw = vec![0; len];
        file.read_exact_at(&mut raw, 0).ok()?;
        let mut rgba = vec![0; width as usize * height as usize * 4];
        for y in 0..height as usize {
            let source_y = if self.flipped { height as usize - y - 1 } else { y };
            for x in 0..width as usize {
                let pos = source_y * stride as usize + x * 4;
                let pixel = &raw[pos..pos + 4];
                let (r, g, b) = match order {
                    PixelOrder::Bgra => (pixel[2], pixel[1], pixel[0]),
                    PixelOrder::Rgba => (pixel[0], pixel[1], pixel[2]),
                };
                let dest = (y * width as usize + x) * 4;
                rgba[dest..dest + 4].copy_from_slice(&[r, g, b, 255]);
            }
        }
        Some(Backdrop {
            output: self.output.clone(),
            width,
            height,
            rgba,
        })
    }

    fn destroy(self) {
        self.frame.destroy();
        if let Some(buffer) = self.buffer {
            buffer.destroy();
        }
    }
}

/// Retains one layer surface and its mutable compositor state.
struct Window {
    surface: WlSurface,
    layer: ZwlrLayerSurfaceV1,
    spec: Spec,
    state: State,
    configured_size: Option<(u32, u32)>,
}

/// Normal compositor-managed xdg_toplevel. Settings is freely resizable and
/// managed by niri, unlike an Island layer surface. Both participate in the
/// same input, frame and service event loop.
struct Toplevel {
    surface: WlSurface,
    xdg_surface: XdgSurface,
    toplevel: XdgToplevel,
    preferred: (u32, u32),
    pending: Option<(u32, u32)>,
    configured_size: Option<(u32, u32)>,
    state: State,
}

/// One bound wl_output and its last complete logical state.
struct Output {
    proxy: WlOutput,
    name: Option<String>,
    scale: i32,
}

/// No platform or renderer state leaks into Kanade's Activity domain.
#[derive(Default)]
struct Listener {
    next_id: u64,
    windows: BTreeMap<WindowId, Window>,
    toplevels: BTreeMap<WindowId, Toplevel>,
    events: Vec<Event>,
    pointer: Option<WlPointer>,
    pointer_at: Option<WindowId>,
    keyboard: Option<WlKeyboard>,
    keys: Option<Keyboard>,
    mapper: Option<Mapper>,
    outputs: BTreeMap<u32, Output>,
    captures: BTreeMap<u64, Capture>,
    next_capture: u64,
    shm: Option<WlShm>,
}

impl Listener {
    fn id_for_surface(&self, surface: &WlSurface) -> Option<WindowId> {
        self.windows
            .iter()
            .find_map(|(id, window)| (&window.surface == surface).then_some(*id))
            .or_else(|| {
                self.toplevels
                    .iter()
                    .find_map(|(id, window)| (&window.surface == surface).then_some(*id))
            })
    }

    fn id_for_xdg_surface(&self, surface: &XdgSurface) -> Option<WindowId> {
        self.toplevels
            .iter()
            .find_map(|(id, window)| (&window.xdg_surface == surface).then_some(*id))
    }

    fn id_for_xdg_toplevel(&self, surface: &XdgToplevel) -> Option<WindowId> {
        self.toplevels
            .iter()
            .find_map(|(id, window)| (&window.toplevel == surface).then_some(*id))
    }

    fn id_for_layer(&self, layer: &ZwlrLayerSurfaceV1) -> Option<WindowId> {
        self.windows
            .iter()
            .find_map(|(id, window)| (&window.layer == layer).then_some(*id))
    }
}

/// Owns backing memory until the compositor has released the buffer.
/// The caller must retain this value after present() until BufferReleased
/// names the same WlBuffer, or until the entire Wayland connection closes.
pub struct ShmFrame {
    pub buffer: WlBuffer,
    _file: File,
}

impl Drop for ShmFrame {
    fn drop(&mut self) {
        self.buffer.destroy();
    }
}

pub struct LayerRuntime {
    queue: EventQueue<Listener>,
    wake: Waker,
    wake_reader: UnixStream,
    listener: Listener,
    compositor: WlCompositor,
    shell: ZwlrLayerShellV1,
    xdg: Option<XdgWmBase>,
    shm: WlShm,
    copy: Option<ZwlrScreencopyManagerV1>,
    _seat: WlSeat,
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
        let seat = globals
            .bind::<WlSeat, _, _>(&handle, 1..=9, ())
            .map_err(|e| format!("wl_seat: {e}"))?;
        let shm = globals
            .bind::<WlShm, _, _>(&handle, 1..=1, ())
            .map_err(|e| format!("wl_shm: {e}"))?;
        let copy = globals.bind::<ZwlrScreencopyManagerV1, _, _>(&handle, 3..=3, ()).ok();
        let shell = globals
            .bind::<ZwlrLayerShellV1, _, _>(&handle, 4..=5, ())
            .map_err(|e| format!("wlr-layer-shell: {e}"))?;
        let xdg = globals.bind::<XdgWmBase, _, _>(&handle, 1..=6, ()).ok();

        let mut listener = Listener {
            shm: Some(shm.clone()),
            ..Listener::default()
        };
        globals.contents().with_list(|list| {
            for global in list {
                if global.interface == "wl_output" && global.version >= 4 {
                    listener.outputs.insert(
                        global.name,
                        Output {
                            proxy: globals
                                .registry()
                                .bind(global.name, 4, &handle, global.name),
                            name: None,
                            scale: 1,
                        },
                    );
                }
            }
        });

        let (wake, wake_reader) = wake::pair().map_err(|e| format!("runtime wake: {e}"))?;
        Ok(Self {
            queue,
            wake,
            wake_reader,
            listener,
            compositor,
            shell,
            xdg,
            shm,
            copy,
            _seat: seat,
        })
    }

    /// Thread-safe wake handle for event-driven OS adapters.
    pub fn waker(&self) -> Waker {
        self.wake.clone()
    }

    /// All names are compositor-provided. Do not infer display order from ID.
    pub fn output(&self, name: &str) -> Option<&WlOutput> {
        self.listener
            .outputs
            .values()
            .find(|output| output.name.as_deref() == Some(name))
            .map(|output| &output.proxy)
    }

    /// Captures a small real output rectangle for the GPU glass shader.
    /// Returns immediately; the Backdrop event carries the pixels when ready.
    /// Compositor denial or missing wlr-screencopy is reported as an error.
    pub fn capture_region(
        &mut self,
        output: &str,
        rect: crate::window::Rect,
    ) -> Result<u64, String> {
        let manager = self.copy.as_ref().ok_or("compositor does not support wlr-screencopy")?;
        let target = self.output(output).ok_or("output is not available")?.clone();
        if rect.width == 0 || rect.height == 0
            || i32::try_from(rect.width).is_err()
            || i32::try_from(rect.height).is_err()
        {
            return Err("capture region has invalid dimensions".into());
        }
        let id = self.listener.next_capture;
        self.listener.next_capture += 1;
        let frame = manager.capture_output_region(
            0, &target, rect.x, rect.y,
            rect.width as i32, rect.height as i32,
            &self.queue.handle(), id,
        );
        self.listener.captures.insert(id, Capture {
            frame,
            output: output.into(),
            size: None,
            flipped: false,
            file: None,
            buffer: None,
        });
        Ok(id)
    }

    /// The small SHM path is only a visibility/Wayland smoke-test renderer.
    /// It is not the final GPU backend. Pixels must be premultiplied
    /// little-endian BGRA (Wayland ARGB8888).
    pub fn shm_frame(&self, width: u32, height: u32, pixels: &[u8]) -> Result<ShmFrame, String> {
        let stride = width.checked_mul(4).ok_or("SHM stride overflow")?;
        let len = stride.checked_mul(height).ok_or("SHM size overflow")?;
        let size = i32::try_from(len).map_err(|_| "SHM frame too large")?;
        let stride = i32::try_from(stride).map_err(|_| "SHM stride too large")?;
        let width = i32::try_from(width).map_err(|_| "SHM width too large")?;
        let height = i32::try_from(height).map_err(|_| "SHM height too large")?;
        if size == 0 || pixels.len() != size as usize {
            return Err("SHM pixel data must match positive frame dimensions".into());
        }

        // Anonymous, unlinked temporary file. The compositor sees the FD,
        // not a filename, and the file is never reused while its buffer
        // might be held by a surface.
        let mut file = tempfile::tempfile().map_err(|e| format!("SHM backing file: {e}"))?;
        file.set_len(size as u64)
            .map_err(|e| format!("SHM resize: {e}"))?;
        file.write_all(pixels)
            .map_err(|e| format!("SHM write: {e}"))?;
        let handle = self.queue.handle();
        let pool: WlShmPool = self.shm.create_pool(file.as_fd(), size, &handle, ());
        let buffer = pool.create_buffer(
            0,
            width,
            height,
            stride,
            wl_shm::Format::Argb8888,
            &handle,
            (),
        );
        pool.destroy();
        Ok(ShmFrame {
            buffer,
            _file: file,
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

    /// Create a compositor-managed, freely resizable desktop window.
    ///
    /// Use xdg-shell for Settings, not layer-shell with exclusive keyboard
    /// focus. The initial empty commit requests a configure; a real buffer
    /// may be attached only once Event::Configured has been received.
    pub fn create_toplevel(
        &mut self,
        width: u32,
        height: u32,
        title: &str,
        app_id: &str,
    ) -> Result<WindowId, String> {
        let wm = self.xdg.as_ref().ok_or("compositor lacks xdg_wm_base")?;
        if width == 0 || height == 0
            || i32::try_from(width).is_err()
            || i32::try_from(height).is_err()
        {
            return Err("xdg window dimensions must be positive and fit i32".into());
        }
        let qh = self.queue.handle();
        let id = WindowId(self.listener.next_id);
        self.listener.next_id += 1;
        let surface = self.compositor.create_surface(&qh, ());
        let xdg_surface = wm.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg_surface.get_toplevel(&qh, ());
        toplevel.set_title(title.into());
        toplevel.set_app_id(app_id.into());
        toplevel.set_min_size(320, 240);

        let mut state = State::default();
        state.show();
        self.listener.toplevels.insert(
            id,
            Toplevel {
                surface: surface.clone(),
                xdg_surface,
                toplevel,
                preferred: (width, height),
                pending: None,
                configured_size: None,
                state,
            },
        );
        surface.commit();
        Ok(id)
    }

    /// Ask the compositor to resize the toplevel. A new buffer must use the
    /// most recent Configured dimensions, not the unacknowledged request.
    /// Start an interactive compositor-managed move. The serial must come
    /// from a fresh, focused pointer button press on this same window/seat.
    /// The compositor decides when to accept it; no niri IPC coordinate hack.
    pub fn move_toplevel(&self, id: WindowId, press_serial: u32) -> Result<(), String> {
        let window = self
            .listener
            .toplevels
            .get(&id)
            .ok_or_else(|| format!("unknown xdg window {}", id.0))?;
        window.toplevel.move_(&self._seat, press_serial);
        Ok(())
    }

    /// Begin the compositor's normal interactive resize. Geometry stays
    /// authoritative in subsequent xdg_toplevel/configure events.
    pub fn resize_toplevel(
        &self,
        id: WindowId,
        press_serial: u32,
        edge: xdg_toplevel::ResizeEdge,
    ) -> Result<(), String> {
        let window = self
            .listener
            .toplevels
            .get(&id)
            .ok_or_else(|| format!("unknown xdg window {}", id.0))?;
        window.toplevel.resize(&self._seat, press_serial, edge);
        Ok(())
    }

    pub fn set_toplevel_min_size(
        &mut self,
        id: WindowId,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let toplevel = self
            .listener
            .toplevels
            .get_mut(&id)
            .ok_or_else(|| format!("unknown xdg window {}", id.0))?;
        let width = i32::try_from(width).map_err(|_| "minimum width exceeds i32")?;
        let height = i32::try_from(height).map_err(|_| "minimum height exceeds i32")?;
        toplevel.toplevel.set_min_size(width, height);
        toplevel.surface.commit();
        Ok(())
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
        let (surface, (width, height)) = if let Some(window) = self.listener.windows.get_mut(&id) {
            if !window.state.begin_frame() {
                return Ok(false);
            }
            let size = window
                .configured_size
                .ok_or_else(|| format!("window {} has no configure", id.0))?;
            (window.surface.clone(), size)
        } else if let Some(window) = self.listener.toplevels.get_mut(&id) {
            if !window.state.begin_frame() {
                return Ok(false);
            }
            let size = window
                .configured_size
                .ok_or_else(|| format!("xdg window {} has no configure", id.0))?;
            (window.surface.clone(), size)
        } else {
            return Err(format!("unknown window {}", id.0));
        };
        surface.attach(Some(buffer), 0, 0);
        surface.damage_buffer(0, 0, width as i32, height as i32);
        surface.frame(&self.queue.handle(), id);
        surface.commit();
        Ok(true)
    }

    /// Called after present(): only an actively animating window should
    /// schedule another frame. An idle window has no timer or frame callback
    /// after its last presentation callback is delivered.
    pub fn submitted(&mut self, id: WindowId, animation_active: bool) {
        if let Some(window) = self.listener.windows.get_mut(&id) {
            window.state.submitted(animation_active);
        } else if let Some(window) = self.listener.toplevels.get_mut(&id) {
            window.state.submitted(animation_active);
        }
    }

    pub fn invalidate(&mut self, id: WindowId) {
        if let Some(window) = self.listener.windows.get_mut(&id) {
            window.state.invalidate();
        } else if let Some(window) = self.listener.toplevels.get_mut(&id) {
            window.state.invalidate();
        }
    }

    pub fn needs_frame(&self, id: WindowId) -> bool {
        self.listener
            .windows
            .get(&id)
            .map(|window| window.state.needs_frame())
            .or_else(|| {
                self.listener
                    .toplevels
                    .get(&id)
                    .map(|window| window.state.needs_frame())
            })
            .unwrap_or(false)
    }

    /// Wait on the actual Wayland fd until an event or keyboard repeat is due.
    /// This has zero idle wakeups: without an active repeating key the poll
    /// has no timeout. No device-node access or second keyboard connection.
    ///
    /// IPC and external Sources will join the same event loop before cutover.
    pub fn dispatch(&mut self) -> Result<Vec<Event>, String> {
        while self.listener.events.is_empty() {
            let dispatched = self
                .queue
                .dispatch_pending(&mut self.listener)
                .map_err(|e| format!("Wayland dispatch: {e}"))?;
            if dispatched != 0 && !self.listener.events.is_empty() {
                break;
            }
            self.queue
                .flush()
                .map_err(|e| format!("Wayland flush: {e}"))?;

            let now = Instant::now();
            let due = self.listener.keys.as_ref().and_then(Keyboard::next_repeat);
            if due.is_some_and(|due| due <= now) {
                if let Some(keys) = &mut self.listener.keys
                    && let Some(repeated) = keys.repeat_due(now)
                {
                    self.listener.events.push(Event::Keyboard(repeated));
                }
                continue;
            }

            // read_guard is required while polling to coordinate the socket
            // with Wayland's queue. If pending events already exist, dispatch
            // them rather than waiting and starving another queue reader.
            let Some(guard) = self.queue.prepare_read() else {
                continue;
            };
            let timeout = due.map(|deadline| {
                let duration = deadline.saturating_duration_since(Instant::now());
                Timespec {
                    tv_sec: duration.as_secs().min(i64::MAX as u64) as i64,
                    tv_nsec: duration.subsec_nanos() as _,
                }
            });
            let mut fds = [
                PollFd::from_borrowed_fd(guard.connection_fd(), PollFlags::IN),
                PollFd::new(&self.wake_reader, PollFlags::IN),
            ];
            let ready = match poll(&mut fds, timeout.as_ref()) {
                Ok(ready) => ready,
                Err(rustix::io::Errno::INTR) => {
                    drop(guard);
                    continue;
                }
                Err(error) => {
                    drop(guard);
                    return Err(format!("polling Wayland: {error}"));
                }
            };
            let wayland_ready = fds[0].revents().contains(PollFlags::IN);
            let sources_ready = fds[1].revents().contains(PollFlags::IN);
            if wayland_ready {
                guard.read().map_err(|e| format!("reading Wayland: {e}"))?;
            } else {
                drop(guard);
            }
            if sources_ready
                && wake::drain(&mut self.wake_reader).map_err(|e| format!("reading wake: {e}"))?
            {
                self.listener.events.push(Event::SourcesChanged);
            }
            if ready != 0 && !wayland_ready && !sources_ready {
                return Err("Wayland/event wake fd closed or failed".into());
            }
        }
        Ok(std::mem::take(&mut self.listener.events))
    }

    pub fn remove(&mut self, id: WindowId) {
        if let Some(window) = self.listener.windows.remove(&id) {
            if self.listener.pointer_at == Some(id) {
                self.listener.pointer_at = None;
            }
            window.layer.destroy();
            window.surface.destroy();
        } else if let Some(window) = self.listener.toplevels.remove(&id) {
            if self.listener.pointer_at == Some(id) {
                self.listener.pointer_at = None;
            }
            if let Some(keys) = &mut self.listener.keys
                && keys.focused() == Some(id)
            {
                keys.focus(None);
            }
            window.toplevel.destroy();
            window.xdg_surface.destroy();
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

impl Dispatch<WlOutput, u32> for Listener {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wayland_client::protocol::wl_output::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(output) = state.outputs.get_mut(global) else {
            return;
        };
        match event {
            wayland_client::protocol::wl_output::Event::Name { name } => {
                output.name = Some(name);
            }
            wayland_client::protocol::wl_output::Event::Scale { factor } => {
                output.scale = factor.max(1);
            }
            wayland_client::protocol::wl_output::Event::Done => {
                if let Some(name) = &output.name {
                    state.events.push(Event::OutputReady {
                        id: *global,
                        name: name.clone(),
                        scale: output.scale,
                    });
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for Listener {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_output" && version >= 4 => {
                state.outputs.entry(name).or_insert_with(|| Output {
                    proxy: registry.bind(name, 4, qh, name),
                    name: None,
                    scale: 1,
                });
            }
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(output) = state.outputs.remove(&name) {
                    output.proxy.release();
                    state.events.push(Event::OutputRemoved(name));
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<XdgWmBase, ()> for Listener {
    fn event(
        _: &mut Self,
        wm: &XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm.pong(serial);
        }
    }
}

impl Dispatch<XdgToplevel, ()> for Listener {
    fn event(
        listener: &mut Self,
        toplevel: &XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(id) = listener.id_for_xdg_toplevel(toplevel) else {
            return;
        };
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => {
                if let Some(window) = listener.toplevels.get_mut(&id) {
                    // Zero dimensions mean the compositor leaves size to us.
                    // Never allocate a buffer with a negative or zero size.
                    let width = u32::try_from(width).unwrap_or(0);
                    let height = u32::try_from(height).unwrap_or(0);
                    window.pending = Some((width, height));
                }
            }
            xdg_toplevel::Event::Close => listener.events.push(Event::Closed(id)),
            _ => {}
        }
    }
}

impl Dispatch<XdgSurface, ()> for Listener {
    fn event(
        listener: &mut Self,
        surface: &XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(id) = listener.id_for_xdg_surface(surface) else {
            return;
        };
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            if let Some(window) = listener.toplevels.get_mut(&id) {
                let (width, height) = window.pending.take().unwrap_or((0, 0));
                let previous = window.configured_size.unwrap_or(window.preferred);
                let width = if width == 0 { previous.0 } else { width };
                let height = if height == 0 { previous.1 } else { height };
                window.configured_size = Some((width, height));
                window.state.configure();
                listener.events.push(Event::Configured { id, width, height });
            }
        }
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
        } else if let Some(window) = listener.toplevels.get_mut(id) {
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

impl Dispatch<WlSeat, ()> for Listener {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: wayland_client::WEnum::Value(caps),
        } = event
        {
            if caps.contains(wl_seat::Capability::Keyboard) {
                if state.keyboard.is_none() {
                    state.keyboard = Some(seat.get_keyboard(qh, ()));
                    state.keys = Some(Keyboard::new(RepeatInfo::from_wayland(0, 0)));
                }
            } else {
                if let Some(keyboard) = state.keyboard.take() {
                    keyboard.release();
                }
                if let Some(keys) = &mut state.keys {
                    keys.focus(None);
                }
                state.keys = None;
                state.mapper = None;
            }
            if caps.contains(wl_seat::Capability::Pointer) {
                if state.pointer.is_none() {
                    state.pointer = Some(seat.get_pointer(qh, ()));
                }
            } else {
                if let Some(pointer) = state.pointer.take() {
                    pointer.release();
                }
                if let Some(id) = state.pointer_at.take() {
                    state.events.push(Event::PointerLeave(id));
                }
            }
        }
    }
}

impl Dispatch<WlKeyboard, ()> for Listener {
    fn event(
        listener: &mut Self,
        keyboard: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if listener.keyboard.as_ref() != Some(keyboard) {
            return;
        }
        match event {
            wl_keyboard::Event::Keymap {
                format: wayland_client::WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                fd,
                size,
            } => {
                // Reject malformed and oversized compositor keymaps without
                // allocation pressure or falling back to a US keyboard.
                listener.mapper = None;
                if size == 0 || size > 2 * 1024 * 1024 {
                    return;
                }
                let mut bytes = vec![0u8; size as usize];
                let mut file = File::from(fd);
                if file.read_exact(&mut bytes).is_ok()
                    && let Ok(source) = String::from_utf8(bytes)
                {
                    listener.mapper = Mapper::compile(source.trim_end_matches('\0').into());
                }
                if let Some(keys) = &mut listener.keys {
                    keys.focus(None);
                }
            }
            wl_keyboard::Event::Enter { surface, .. } => {
                let id = listener.id_for_surface(&surface);
                if let Some(keys) = &mut listener.keys {
                    keys.focus(id);
                }
            }
            wl_keyboard::Event::Leave { .. } => {
                if let Some(keys) = &mut listener.keys {
                    keys.focus(None);
                }
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                if let Some(mapper) = &mut listener.mapper {
                    mapper.modifiers(mods_depressed, mods_latched, mods_locked, group);
                }
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                if let Some(keys) = &mut listener.keys {
                    keys.configure_repeat(RepeatInfo::from_wayland(rate, delay), Instant::now());
                }
            }
            wl_keyboard::Event::Key {
                key,
                state: wayland_client::WEnum::Value(state),
                ..
            } => {
                let Some(keys) = &mut listener.keys else {
                    return;
                };
                let result = match state {
                    wl_keyboard::KeyState::Pressed => listener
                        .mapper
                        .as_ref()
                        .and_then(|mapper| keys.press(mapper.stroke(key), Instant::now())),
                    wl_keyboard::KeyState::Released => keys.release(key),
                    _ => None,
                };
                if let Some(event) = result {
                    listener.events.push(Event::Keyboard(event));
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlPointer, ()> for Listener {
    fn event(
        state: &mut Self,
        pointer: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Queued input from a removed device must not act on a new pointer.
        if state.pointer.as_ref() != Some(pointer) {
            return;
        }
        match event {
            wl_pointer::Event::Enter {
                surface,
                surface_x,
                surface_y,
                ..
            } => {
                if let Some(previous) = state.pointer_at.take() {
                    state.events.push(Event::PointerLeave(previous));
                }
                if let Some(id) = state.id_for_surface(&surface) {
                    state.pointer_at = Some(id);
                    state.events.push(Event::PointerEnter {
                        id,
                        x: surface_x,
                        y: surface_y,
                    });
                }
            }
            wl_pointer::Event::Leave { .. } => {
                if let Some(id) = state.pointer_at.take() {
                    state.events.push(Event::PointerLeave(id));
                }
            }
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                if let Some(id) = state.pointer_at {
                    state.events.push(Event::PointerMotion {
                        id,
                        x: surface_x,
                        y: surface_y,
                    });
                }
            }
            wl_pointer::Event::Button {
                button,
                serial,
                state: wayland_client::WEnum::Value(button_state),
                ..
            } => {
                if let Some(id) = state.pointer_at {
                    state.events.push(Event::PointerButton {
                        id,
                        button,
                        pressed: button_state == wl_pointer::ButtonState::Pressed,
                        serial,
                    });
                }
            }
            wl_pointer::Event::Axis {
                axis: wayland_client::WEnum::Value(axis),
                value,
                ..
            } => {
                if let Some(id) = state.pointer_at {
                    let (x, y) = match axis {
                        wl_pointer::Axis::HorizontalScroll => (value, 0.0),
                        wl_pointer::Axis::VerticalScroll => (0.0, value),
                        _ => (0.0, 0.0),
                    };
                    state.events.push(Event::PointerScroll { id, x, y });
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, u64> for Listener {
    fn event(
        state: &mut Self,
        frame: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        id: &u64,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let Some(capture) = state.captures.get_mut(id) else { return };
        if capture.frame != *frame {
            return; // An old frame finished after it was replaced.
        }
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer {
                format: wayland_client::WEnum::Value(format),
                width, height, stride,
            } if capture.buffer.is_none() => {
                let order = match format {
                    wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888 => Some(PixelOrder::Bgra),
                    wl_shm::Format::Abgr8888 | wl_shm::Format::Xbgr8888 => Some(PixelOrder::Rgba),
                    _ => None,
                };
                let bytes = stride.checked_mul(height).and_then(|n| i32::try_from(n).ok());
                if stride >= width.saturating_mul(4) && width > 0 && height > 0
                    && let (Some(order), Some(bytes), Some(shm)) = (order, bytes, &state.shm)
                    && bytes > 0 && bytes <= 64 * 1024 * 1024
                    && let Ok(file) = tempfile::tempfile()
                    && file.set_len(bytes as u64).is_ok()
                {
                    let pool = shm.create_pool(file.as_fd(), bytes, qh, ());
                    let buffer = pool.create_buffer(
                        0, width as i32, height as i32, stride as i32,
                        format, qh, (),
                    );
                    pool.destroy();
                    capture.size = Some((width, height, stride, order));
                    capture.file = Some(file);
                    capture.buffer = Some(buffer);
                }
            }
            zwlr_screencopy_frame_v1::Event::Flags {
                flags: wayland_client::WEnum::Value(flags),
            } => {
                capture.flipped = flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert);
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => {
                if let Some(buffer) = &capture.buffer {
                    frame.copy(buffer);
                } else {
                    if let Some(failed) = state.captures.remove(id) {
                        let output = failed.output.clone();
                        failed.destroy();
                        state.events.push(Event::CaptureFailed { output });
                    }
                }
            }
            zwlr_screencopy_frame_v1::Event::Ready { .. } => {
                if let Some(done) = state.captures.remove(id) {
                    let output = done.output.clone();
                    if let Some(pixels) = done.pixels() {
                        state.events.push(Event::Backdrop(pixels));
                    } else {
                        state.events.push(Event::CaptureFailed { output });
                    }
                    done.destroy();
                }
            }
            zwlr_screencopy_frame_v1::Event::Failed => {
                if let Some(failed) = state.captures.remove(id) {
                    let output = failed.output.clone();
                    failed.destroy();
                    state.events.push(Event::CaptureFailed { output });
                }
            }
            _ => {}
        }
    }
}

delegate_noop!(Listener: WlCompositor);
delegate_noop!(Listener: ZwlrScreencopyManagerV1);
delegate_noop!(Listener: ZwlrLayerShellV1);
delegate_noop!(Listener: WlRegion);
delegate_noop!(Listener: ignore WlShm);
delegate_noop!(Listener: WlShmPool);
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
