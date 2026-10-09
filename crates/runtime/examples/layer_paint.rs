//! Native visible-layer smoke test, without replacing the running shell.
//! cargo run -p kanade-runtime --example layer_paint
//! Run in a real Wayland/niri session. Ctrl-C stops it. No input is captured.
//! Uses SHM solely to test surface/buffer/commit lifecycle.

use kanade_runtime::{
    raster,
    wayland::{Event, LayerRuntime, ShmFrame},
    window::{Alignment, Edge, KeyboardMode, Layer, Spec},
};

fn main() -> Result<(), String> {
    let mut runtime = LayerRuntime::connect()?;
    let id = runtime.create(
        Spec {
            width: 300,
            height: 60,
            layer: Layer::Top,
            edge: Edge::Top,
            align: Alignment::Center,
            keyboard: KeyboardMode::None,
            exclusive_zone: -1,
            input: vec![],
        },
        None,
    );

    // Keep every displayed buffer alive until its release. This demo only
    // presents one static frame and therefore never needs to recycle it.
    let mut shown: Option<ShmFrame> = None;
    loop {
        for event in runtime.dispatch()? {
            match event {
                Event::Configured {
                    id: configured,
                    width,
                    height,
                } if configured == id => {
                    if shown.is_none() {
                        let pixels = raster::pill(width, height, height as f32 * 0.5)?;
                        let frame = runtime.shm_frame(width, height, &pixels)?;
                        if !runtime.present(id, &frame.buffer)? {
                            return Err("configured surface declined first frame".into());
                        }
                        runtime.submitted(id, false);
                        shown = Some(frame);
                        println!("native layer visible: {width}x{height}");
                    }
                }
                Event::BufferReleased(buffer) => {
                    if shown.as_ref().is_some_and(|frame| frame.buffer == buffer) {
                        shown = None;
                    }
                }
                Event::Closed(closed) if closed == id => {
                    runtime.remove(id);
                    return Ok(());
                }
                _ => {}
            }
        }
    }
}
