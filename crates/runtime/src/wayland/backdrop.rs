use std::collections::HashMap;
use std::collections::hash_map::Entry;

use smithay_client_toolkit::reexports::calloop::{LoopHandle, ping::make_ping};
use smithay_client_toolkit::shm::raw::RawPool;
use wayland_client::globals::GlobalList;
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::{wl_output::WlOutput, wl_shm};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_frame_v1::{
    self as frame, ZwlrScreencopyFrameV1,
};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::{
    self as manager, ZwlrScreencopyManagerV1,
};

use crate::backdrop::{self, Call, Request, Spot};

use super::WaylandState;

// the panes being captured, none without the compositor's screencopy
pub struct Captures {
    manager: ZwlrScreencopyManagerV1,

    // nothing is captured, as while the session is locked
    paused: bool,

    panes: HashMap<Spot, Pane>,
}

struct Pane {
    output: WlOutput,

    // the newest request; none after a capture failed, so the next is carried out, or while nothing is asked
    wanted: Option<Request>,

    grab: Option<Grab>,
}

// a capture in flight
struct Grab {
    frame: ZwlrScreencopyFrameV1,
    request: Request,

    // the shared memory layout offered, then the memory it is copied into
    layout: Option<Layout>,
    memory: Option<(RawPool, WlBuffer)>,
    flipped: bool,

    // waits for the screen to change before copying, as the last capture still holds
    patient: bool,
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
    Rgba,
}

// a buffer a capture is copied into; the compositor never sends it anything
pub struct CaptureBuffer;

pub fn bind(globals: &GlobalList, qh: &QueueHandle<WaylandState>) -> Option<Captures> {
    let manager = globals
        .bind::<ZwlrScreencopyManagerV1, _, _>(qh, 3..=3, ())
        .ok()?;

    Some(Captures {
        manager,
        paused: false,
        panes: HashMap::new(),
    })
}

// carries out what the views asked when woken
pub fn insert(handle: &LoopHandle<'static, WaylandState>, capable: bool) {
    let (ping, source) = make_ping().expect("failed to create the capture ping");

    handle
        .insert_source(source, |_, _, state| state.serve_backdrops())
        .expect("failed to insert the capture ping");

    backdrop::connected(capable, ping);
}

impl Captures {
    // a request asked for a pane, or none to stop looking
    fn watch(
        &mut self,
        spot: Spot,
        request: Option<Request>,
        output: WlOutput,
        qh: &QueueHandle<WaylandState>,
    ) {
        let pane = match self.panes.entry(spot.clone()) {
            Entry::Occupied(pane) => pane.into_mut(),

            // nothing to stop where nothing was asked
            Entry::Vacant(_) if request.is_none() => return,

            Entry::Vacant(vacant) => vacant.insert(Pane {
                output,
                wanted: None,
                grab: None,
            }),
        };

        if pane.wanted == request {
            return;
        }

        pane.wanted = request;

        if pane.wanted.is_none() {
            pane.stop();

            return;
        }

        // a capture waiting for the screen to change may wait long, so the new request is captured now
        if pane.grab.as_ref().is_none_or(|grab| grab.patient) {
            pane.stop();
            self.capture(&spot, false, qh);
        }
    }

    // captures around the wanted request of a pane, if there is one
    fn capture(&mut self, spot: &Spot, patient: bool, qh: &QueueHandle<WaylandState>) {
        if self.paused {
            return;
        }

        let Some(pane) = self.panes.get_mut(spot) else {
            return;
        };

        let Some(request) = pane.wanted.clone() else {
            return;
        };

        let region = request.region;

        if region.width < 1.0 || region.height < 1.0 {
            return;
        }

        let frame = self.manager.capture_output_region(
            0,
            &pane.output,
            region.x as i32,
            region.y as i32,
            region.width as i32,
            region.height as i32,
            qh,
            spot.clone(),
        );

        pane.grab = Some(Grab {
            frame,
            request,
            layout: None,
            memory: None,
            flipped: false,
            patient,
        });
    }
}

impl Pane {
    // stops a capture in flight
    fn stop(&mut self) {
        if let Some(grab) = self.grab.take() {
            grab.destroy();
        }
    }

    // a capture that cannot finish: the next request asks again, even the same one
    fn fail(&mut self, spot: &Spot) {
        self.stop();
        self.wanted = None;

        backdrop::failed(spot);
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
    fn pixels(&mut self) -> Option<(Vec<u8>, u32, u32)> {
        let layout = self.layout?;
        let (pool, _) = self.memory.as_mut()?;
        let raw = pool.mmap();

        let (width, height, stride) = (
            layout.width as usize,
            layout.height as usize,
            layout.stride as usize,
        );

        if raw.len() < stride * height {
            return None;
        }

        let mut pixels = vec![0; width * height * 4];

        for row in 0..height {
            let from = if self.flipped { height - 1 - row } else { row };
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

        Some((pixels, layout.width, layout.height))
    }
}

impl WaylandState {
    // what the views asked since the last time
    fn serve_backdrops(&mut self) {
        for call in backdrop::take_calls() {
            match call {
                Call::Watch(spot, request) => self.watch_backdrop(spot, request),

                Call::Release(spot) => {
                    if let Some(captures) = &mut self.captures
                        && let Some(mut pane) = captures.panes.remove(&spot)
                    {
                        pane.stop();
                    }

                    backdrop::dropped(|dropped| *dropped == spot);
                }

                Call::Pause(paused) => self.pause_backdrops(paused),
            }
        }
    }

    fn watch_backdrop(&mut self, spot: Spot, request: Option<Request>) {
        let Some(output) = self.output_named(&spot.output) else {
            // the output is not there (any more), so there is nothing to look at
            backdrop::failed(&spot);

            return;
        };

        if let Some(captures) = &mut self.captures {
            captures.watch(spot, request, output, &self.qh);
        }
    }

    fn pause_backdrops(&mut self, paused: bool) {
        let Some(captures) = &mut self.captures else {
            return;
        };

        captures.paused = paused;

        // what was behind the session before a lock is not to be shown after it
        if paused {
            backdrop::cleared(|_| true);
        }

        let spots: Vec<Spot> = captures.panes.keys().cloned().collect();

        for spot in spots {
            if let Some(pane) = captures.panes.get_mut(&spot) {
                pane.stop();
            }

            if !paused {
                captures.capture(&spot, false, &self.qh);
            }
        }
    }

    // an output gone: its panes are let go
    pub(super) fn backdrops_gone(&mut self, output: &WlOutput) {
        let Some(captures) = &mut self.captures else {
            return;
        };

        let gone: Vec<Spot> = captures
            .panes
            .iter()
            .filter(|(_, pane)| pane.output == *output)
            .map(|(spot, _)| spot.clone())
            .collect();

        for spot in &gone {
            if let Some(mut pane) = captures.panes.remove(spot) {
                pane.stop();
            }
        }

        backdrop::dropped(|dropped| gone.contains(dropped));
    }

    fn output_named(&self, name: &str) -> Option<WlOutput> {
        self.output.outputs().find(|output| {
            self.output
                .info(output)
                .is_some_and(|info| info.name.as_deref() == Some(name))
        })
    }
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

impl Dispatch<ZwlrScreencopyFrameV1, Spot> for WaylandState {
    fn event(
        state: &mut Self,
        copied: &ZwlrScreencopyFrameV1,
        event: frame::Event,
        spot: &Spot,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let Some(captures) = &mut state.captures else {
            return;
        };

        let Some(pane) = captures.panes.get_mut(spot) else {
            return;
        };

        /*
         * events already queued for a frame since destroyed, as a new request replaces one, still
         * arrive: they belong to no grab
         */
        let Some(grab) = pane.grab.as_mut().filter(|grab| grab.frame == *copied) else {
            return;
        };

        match event {
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
                let Some(order) = order.filter(|_| stride >= width.saturating_mul(4)) else {
                    return;
                };

                let size = (stride as usize).saturating_mul(height as usize);

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
                }
            }

            frame::Event::Flags { flags } => {
                grab.flipped =
                    matches!(flags, WEnum::Value(flags) if flags.contains(frame::Flags::YInvert));
            }

            frame::Event::BufferDone => match &grab.memory {
                Some((_, buffer)) if grab.patient => grab.frame.copy_with_damage(buffer),
                Some((_, buffer)) => grab.frame.copy(buffer),

                // no layout it could copy into: the last copy stays as it was
                None => pane.fail(spot),
            },

            frame::Event::Ready { .. } => {
                let Some(mut grab) = pane.grab.take() else {
                    return;
                };

                // a copy that lands as the session locks shows what the lock covers
                let pixels = grab.pixels().filter(|_| !captures.paused);
                let request = grab.request.clone();

                grab.destroy();

                // the next capture waits for the screen to change, unless the request moved on
                let current = pane.wanted.as_ref() == Some(&request);

                captures.capture(spot, current, qh);

                if let Some((rgba, width, height)) = pixels {
                    backdrop::arrived(spot, &request, width, height, rgba);
                }
            }

            frame::Event::Failed => pane.fail(spot),

            _ => {}
        }
    }
}
