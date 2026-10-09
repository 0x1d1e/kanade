//! An integrated, intentionally limited native shell protocol probe.
//!
//! Run beside the existing Kanade shell:
//!   cargo run -p kanade-runtime --example native_shell
//!   cargo run -p kanade-runtime --example native_cli -- status
//!   cargo run -p kanade-runtime --example native_cli -- settings open
//!
//! Exercises a layer Island + resizable xdg Settings on one Wayland connection,
//! keyboard/pointer events, compositor frame callbacks and native CLI IPC.
//! This is NOT the replacement UI, login/session lock, or a second production
//! shell implementation. No Amane or niri processes are altered.

use std::collections::BTreeMap;
use std::env;
use std::path::Path;
use std::process::ExitCode;

use kanade_runtime::{
    chrome::{ChromeAction, hit_test},
    ipc::{Incoming, Server},
    raster,
    wayland::{Event, LayerRuntime, ShmFrame},
    window::{Alignment, Edge, KeyboardMode, Layer, Rect, Spec, WindowId},
};

const W: u32 = 560;
const H: u32 = 380;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Status,
    OpenSettings,
    CloseSettings,
    Exit,
    Unsupported,
}

fn command(arguments: &[String]) -> Command {
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["status"] => Command::Status,
        ["settings", "open"] => Command::OpenSettings,
        ["settings", "close"] => Command::CloseSettings,
        ["exit"] => Command::Exit,
        _ => Command::Unsupported,
    }
}

struct Shell {
    runtime: LayerRuntime,
    _server: Server,
    island: WindowId,
    settings: Option<WindowId>,
    buffers: Vec<ShmFrame>,
    sizes: BTreeMap<WindowId, (u32, u32)>,
    pointer: (f64, f64),
    running: bool,
}

impl Shell {
    fn new() -> Result<Self, String> {
        let mut runtime = LayerRuntime::connect()?;
        let directory = env::var_os("XDG_RUNTIME_DIR")
            .ok_or("XDG_RUNTIME_DIR is required for the native shell probe")?;
        let server = Server::bind(Path::new(&directory), runtime.waker())
            .map_err(|e| format!("binding native CLI socket: {e}"))?;
        let island = runtime.create(
            Spec {
                width: W,
                height: H,
                layer: Layer::Top,
                edge: Edge::Top,
                align: Alignment::Center,
                keyboard: KeyboardMode::None,
                exclusive_zone: -1,
                input: vec![Rect {
                    x: 150,
                    y: 0,
                    width: 260,
                    height: 40,
                }],
            },
            None,
        );

        Ok(Self {
            runtime,
            _server: server,
            island,
            settings: None,
            buffers: Vec::new(),
            sizes: BTreeMap::new(),
            pointer: (0.0, 0.0),
            running: true,
        })
    }

    fn open_settings(&mut self) -> Result<(), String> {
        if self.settings.is_none() {
            let id = self
                .runtime
                .create_toplevel(680, 520, "Kanade Native Probe", "kanade")?;
            self.settings = Some(id);
        }
        Ok(())
    }

    fn answer(&mut self, request: Incoming) {
        let (reply, should_exit) = match command(&request.argv) {
            Command::Status => (
                format!(
                    "ok\nnative shell probe; Settings {}; no Amane UI",
                    if self.settings.is_some() {
                        "open"
                    } else {
                        "closed"
                    }
                ),
                false,
            ),
            Command::OpenSettings => (
                match self.open_settings() {
                    Ok(()) => "ok\n".into(),
                    Err(e) => format!("refused\n{e}"),
                },
                false,
            ),
            Command::CloseSettings => {
                if let Some(id) = self.settings.take() {
                    self.runtime.remove(id);
                    self.sizes.remove(&id);
                }
                ("ok\n".into(), false)
            }
            Command::Exit => ("ok\n".into(), true),
            Command::Unsupported => ("refused\nunsupported native probe command".into(), false),
        };
        let _ = request.respond(reply);
        if should_exit {
            self.running = false;
        }
    }

    fn present(&mut self, id: WindowId, width: u32, height: u32) -> Result<(), String> {
        if id != self.island && Some(id) != self.settings {
            return Ok(());
        }
        let pixels = if id == self.island {
            if width != W || height != H {
                return Err("the Island canvas must retain its configured dimensions".into());
            }
            island_pixels()?
        } else {
            settings_pixels(width, height)?
        };
        let frame = self.runtime.shm_frame(width, height, &pixels)?;
        if self.runtime.present(id, &frame.buffer)? {
            self.buffers.push(frame);
        }
        Ok(())
    }

    fn run(&mut self) -> Result<(), String> {
        while self.running {
            for event in self.runtime.dispatch()? {
                match event {
                    Event::Configured { id, width, height } => {
                        self.sizes.insert(id, (width, height));
                        self.present(id, width, height)?;
                    }
                    Event::FrameReady(id) if self.runtime.needs_frame(id) => {
                        if let Some((width, height)) = self.sizes.get(&id).copied() {
                            self.present(id, width, height)?;
                        }
                    }
                    Event::BufferReleased(buffer) => {
                        self.buffers.retain(|frame| frame.buffer != buffer);
                    }
                    Event::SourcesChanged => {
                        for request in self._server.drain() {
                            self.answer(request);
                        }
                    }
                    Event::Closed(id) if Some(id) == self.settings => {
                        self.runtime.remove(id);
                        self.settings = None;
                        self.sizes.remove(&id);
                    }
                    Event::PointerEnter { id, x, y } | Event::PointerMotion { id, x, y }
                        if Some(id) == self.settings =>
                    {
                        self.pointer = (x, y);
                    }
                    Event::PointerButton {
                        id,
                        button: 0x110,
                        pressed: true,
                        serial,
                    } if Some(id) == self.settings => {
                        if let Some((width, height)) = self.sizes.get(&id).copied() {
                            match hit_test(width, height, self.pointer.0, self.pointer.1) {
                                ChromeAction::Move => self.runtime.move_toplevel(id, serial)?,
                                ChromeAction::Resize(edge) => {
                                    self.runtime.resize_toplevel(id, serial, edge)?;
                                }
                                ChromeAction::Content => {}
                            }
                        }
                    }
                    Event::PointerButton {
                        id,
                        button: 0x110,
                        pressed: true,
                        ..
                    } if id == self.island => self.open_settings()?,
                    _ => {}
                }
            }
        }
        if let Some(id) = self.settings.take() {
            self.runtime.remove(id);
        }
        self.runtime.remove(self.island);
        Ok(())
    }
}

fn island_pixels() -> Result<Vec<u8>, String> {
    let mut pixels = vec![0; (W * H * 4) as usize];
    let body = raster::pill(260, 40, 20.0)?;
    for y in 0..40 {
        let dest = ((y * W + 150) * 4) as usize;
        let src = (y * 260 * 4) as usize;
        pixels[dest..dest + 260 * 4].copy_from_slice(&body[src..src + 260 * 4]);
    }
    Ok(pixels)
}

fn settings_pixels(width: u32, height: u32) -> Result<Vec<u8>, String> {
    let bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|n| n.checked_mul(4))
        .and_then(|n| usize::try_from(n).ok())
        .ok_or("invalid Settings dimensions")?;
    if width == 0 || height == 0 || bytes > 64 * 1024 * 1024 {
        return Err("Settings dimensions exceed native SHM probe limit".into());
    }
    let mut pixels = vec![0u8; bytes];
    for px in pixels.as_chunks_mut::<4>().0 {
        px.copy_from_slice(&[32, 26, 22, 255]);
    }
    Ok(pixels)
}

fn main() -> ExitCode {
    match Shell::new().and_then(|mut shell| shell.run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("kanade native shell probe: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_commands_do_not_claim_unimplemented_features() {
        assert_eq!(command(&["status".into()]), Command::Status);
        assert_eq!(
            command(&["settings".into(), "open".into()]),
            Command::OpenSettings
        );
        assert_eq!(
            command(&["settings".into(), "close".into()]),
            Command::CloseSettings
        );
        assert_eq!(command(&["exit".into()]), Command::Exit);
        assert_eq!(command(&["lock".into()]), Command::Unsupported);
        assert_eq!(command(&["screenshot".into()]), Command::Unsupported);
    }

    #[test]
    fn rendered_canvas_passes_unused_pointer_region() {
        let image = island_pixels().unwrap();
        let alpha = |x: usize, y: usize| image[(y * W as usize + x) * 4 + 3];
        assert_eq!(alpha(0, 100), 0);
        assert_eq!(alpha(280, 20), 218);
    }

    #[test]
    fn settings_window_pixels_are_bounded() {
        assert_eq!(settings_pixels(320, 240).unwrap().len(), 320 * 240 * 4);
        assert!(settings_pixels(0, 240).is_err());
        assert!(settings_pixels(10_000, 10_000).is_err());
    }
}
