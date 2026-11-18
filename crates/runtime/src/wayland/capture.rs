//! The runtime's one-shot `wlr-screencopy` client (ADR 0162): each output asked for is copied once
//! into shared memory and handed back as RGBA. The copy and its conversion run here, on the
//! runtime's loop, which `capture::request` wakes; only the caller's wait is elsewhere. No damage
//! tracking, pacing, pause or dmabuf path.

use std::collections::HashMap;
use std::sync::Arc;

use smithay_client_toolkit::reexports::calloop::{LoopHandle, ping::make_ping};
use smithay_client_toolkit::shm::raw::RawPool;
use wayland_client::globals::GlobalList;
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_shm;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_frame_v1::{
    self as frame, ZwlrScreencopyFrameV1,
};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::{
    self as manager, ZwlrScreencopyManagerV1,
};

use crate::capture::{self, Frame};

use super::WaylandState;

// the outputs being captured, none without the compositor's screencopy
pub struct Captures {
    manager: ZwlrScreencopyManagerV1,

    // one grab in flight per output, by the output's name
    grabs: HashMap<String, Grab>,
}

// a capture in flight
struct Grab {
    frame: ZwlrScreencopyFrameV1,
    output: WlOutput,

    // the request this grab answers; an outcome settled for any other is dropped
    generation: u64,

    // the shared memory layout offered, then the memory it is copied into
    layout: Option<Layout>,
    memory: Option<(RawPool, WlBuffer)>,
    flipped: bool,
}

#[derive(Clone, Copy)]
struct Layout {
    order: Order,
    width: u32,
    height: u32,
    stride: u32,
}

// which byte of a captured pixel is which channel
#[derive(Clone, Copy)]
enum Order {
    // blue, green, red, alpha in memory
    Bgra,

    // red, green, blue, alpha in memory
    Rgba,
}

// a buffer a capture is copied into; the compositor never sends it anything
pub struct CaptureBuffer;

pub fn bind(globals: &GlobalList, qh: &QueueHandle<WaylandState>) -> Option<Captures> {
    let manager = globals
        .bind::<ZwlrScreencopyManagerV1, _, _>(qh, 1..=3, ())
        .ok()?;

    Some(Captures {
        manager,
        grabs: HashMap::new(),
    })
}

// carries out what the shell asked when woken
pub fn insert(handle: &LoopHandle<'static, WaylandState>, capable: bool) {
    let (ping, source) = make_ping().expect("failed to create the capture ping");

    handle
        .insert_source(source, |_, _, state| state.serve_captures())
        .expect("failed to insert the capture ping");

    capture::connected(capable, ping);
}

impl WaylandState {
    // what the shell asked for since the last time
    fn serve_captures(&mut self) {
        for output in capture::take_requests() {
            self.capture_output(&output);
        }

        if let Some(captures) = &mut self.captures {
            let stale: Vec<String> = captures
                .grabs
                .keys()
                .filter(|output| capture::generation(output) == 0)
                .cloned()
                .collect();

            for output in stale {
                if let Some(grab) = captures.grabs.remove(&output) {
                    grab.destroy();
                }
            }
        }
    }

    fn capture_output(&mut self, output: &str) {
        // the request this serves; a later one bumps it, and this grab's outcome is then dropped
        let generation = capture::generation(output);
        if generation == 0 {
            return;
        }

        let object = self.output_named(output);

        let Some(captures) = &mut self.captures else {
            capture::settle(
                output,
                generation,
                Err(String::from("the compositor has no screencopy")),
            );

            return;
        };

        // a fresh request replaces one still in flight
        if let Some(grab) = captures.grabs.remove(output) {
            grab.destroy();
        }

        let Some(object) = object else {
            capture::settle(output, generation, Err(format!("no output named {output}")));

            return;
        };

        let frame = captures
            .manager
            .capture_output(0, &object, &self.qh, output.to_owned());

        captures.grabs.insert(
            output.to_owned(),
            Grab {
                frame,
                output: object,
                generation,
                layout: None,
                memory: None,
                flipped: false,
            },
        );
    }

    fn output_named(&self, name: &str) -> Option<WlOutput> {
        self.output.outputs().find(|output| {
            self.output
                .info(output)
                .is_some_and(|info| info.name.as_deref() == Some(name))
        })
    }

    // an output gone: its grab is let go and the caller told rather than left waiting
    pub(super) fn captures_gone(&mut self, output: &WlOutput) {
        let Some(captures) = &mut self.captures else {
            return;
        };

        let gone: Vec<String> = captures
            .grabs
            .iter()
            .filter(|(_, grab)| grab.output == *output)
            .map(|(name, _)| name.clone())
            .collect();

        for name in &gone {
            let Some(grab) = captures.grabs.remove(name) else {
                continue;
            };

            let generation = grab.generation;

            grab.destroy();

            capture::settle(name, generation, Err(String::from("the output went away")));
        }
    }
}

impl Grab {
    fn destroy(self) {
        self.frame.destroy();

        if let Some((_, buffer)) = self.memory {
            buffer.destroy();
        }
    }

    // the captured pixels as rgba, top row first
    fn pixels(&mut self) -> Result<Frame, String> {
        let layout = self
            .layout
            .ok_or_else(|| String::from("the compositor offered no usable shared-memory format"))?;

        let (width, height, stride) = (
            layout.width as usize,
            layout.height as usize,
            layout.stride as usize,
        );

        let flipped = self.flipped;

        let (pool, _) = self
            .memory
            .as_mut()
            .ok_or_else(|| String::from("no shared-memory buffer"))?;

        let raw = pool.mmap();

        let Some(span) = stride.checked_mul(height) else {
            return Err(String::from(
                "the shared-memory buffer does not hold the frame",
            ));
        };

        if width == 0 || height == 0 || raw.len() < span {
            return Err(String::from(
                "the shared-memory buffer does not hold the frame",
            ));
        }

        let Some(len) = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
        else {
            return Err(String::from(
                "the shared-memory buffer does not hold the frame",
            ));
        };

        let mut pixels = vec![0; len];

        for row in 0..height {
            let from = if flipped { height - 1 - row } else { row };
            let line = &raw[from * stride..from * stride + width * 4];

            let out = &mut pixels[row * width * 4..(row + 1) * width * 4];

            for (pixel, out) in line
                .as_chunks::<4>()
                .0
                .iter()
                .zip(out.as_chunks_mut::<4>().0)
            {
                *out = match layout.order {
                    Order::Bgra => [pixel[2], pixel[1], pixel[0], 255],
                    Order::Rgba => [pixel[0], pixel[1], pixel[2], 255],
                };
            }
        }

        Ok(Frame {
            width: layout.width,
            height: layout.height,
            pixels: Arc::new(pixels),
        })
    }
}

// what one frame event asks for after its own borrow of the grab ends
enum Step {
    None,

    // the layout is known and the frame is copied into it
    Copy,

    // the copy landed: the pixels are read, the grab let go and the caller told
    Ready,

    // this output's capture cannot finish
    Failed(String),
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &ZwlrScreencopyManagerV1,
        _: manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlBuffer, CaptureBuffer> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlBuffer,
        _: wl_buffer::Event,
        _: &CaptureBuffer,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, String> for WaylandState {
    fn event(
        state: &mut Self,
        copied: &ZwlrScreencopyFrameV1,
        event: frame::Event,
        output: &String,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let Some(captures) = &mut state.captures else {
            return;
        };

        /*
         * events already queued for a frame since destroyed, as a new request replaces one, still
         * arrive: they belong to no grab
         */
        let Some(grab) = captures
            .grabs
            .get_mut(output)
            .filter(|grab| grab.frame == *copied)
        else {
            return;
        };

        let step = match event {
            frame::Event::Buffer {
                format: WEnum::Value(format),
                width,
                height,
                stride,
            } if grab.layout.is_none() => {
                let order = match format {
                    wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888 => Some(Order::Bgra),
                    wl_shm::Format::Abgr8888 | wl_shm::Format::Xbgr8888 => Some(Order::Rgba),
                    _ => None,
                };

                // a row too short for its pixels is a layout this cannot read
                let size = (stride as usize)
                    .checked_mul(height as usize)
                    .filter(|&bytes| i32::try_from(bytes).is_ok());
                let valid = width > 0
                    && height > 0
                    && width.checked_mul(4).is_some_and(|row| stride >= row)
                    && size.is_some();

                if let (Some(order), Some(size)) = (order.filter(|_| valid), size.filter(|_| valid))
                {
                    if let Ok(mut pool) = RawPool::new(size, &state.shm) {
                        let buffer = pool.create_buffer(
                            0,
                            width as i32,
                            height as i32,
                            stride as i32,
                            format,
                            CaptureBuffer,
                            qh,
                        );

                        grab.layout = Some(Layout {
                            order,
                            width,
                            height,
                            stride,
                        });

                        grab.memory = Some((pool, buffer));

                        // before version 3 there is no buffer_done: the buffer event is the copy
                        if grab.frame.version() < 3 {
                            Step::Copy
                        } else {
                            Step::None
                        }
                    } else if grab.frame.version() < 3 {
                        // no buffer_done will come to settle a v1/v2 frame that has no buffer
                        Step::Failed(String::from("no shared-memory buffer"))
                    } else {
                        Step::None
                    }
                } else if grab.frame.version() < 3 {
                    // likewise for a v1/v2 frame whose format this cannot read
                    Step::Failed(String::from(
                        "the compositor offered no usable shared-memory format",
                    ))
                } else {
                    Step::None
                }
            }

            frame::Event::Flags { flags } => {
                grab.flipped = matches!(
                    flags,
                    WEnum::Value(flags) if flags.contains(frame::Flags::YInvert)
                );

                Step::None
            }

            // before version 3 there is no buffer_done; a stray one must not copy a second time
            frame::Event::BufferDone if grab.frame.version() < 3 => Step::None,

            frame::Event::BufferDone => match &grab.memory {
                Some(_) => Step::Copy,

                // no format it could copy into
                None => Step::Failed(String::from(
                    "the compositor offered no usable shared-memory format",
                )),
            },

            frame::Event::Ready { .. } => Step::Ready,

            frame::Event::Failed => {
                Step::Failed(String::from("the compositor could not copy the output"))
            }

            _ => Step::None,
        };

        match step {
            Step::None => {}

            Step::Copy => {
                if let Some(grab) = captures.grabs.get_mut(output)
                    && let Some((_, buffer)) = &grab.memory
                {
                    grab.frame.copy(buffer);
                }
            }

            Step::Ready => {
                if let Some(mut grab) = captures.grabs.remove(output) {
                    let result = grab.pixels();

                    let generation = grab.generation;

                    grab.destroy();

                    capture::settle(output, generation, result);
                }
            }

            Step::Failed(why) => {
                if let Some(grab) = captures.grabs.remove(output) {
                    let generation = grab.generation;

                    grab.destroy();

                    capture::settle(output, generation, Err(why));
                }
            }
        }
    }
}
