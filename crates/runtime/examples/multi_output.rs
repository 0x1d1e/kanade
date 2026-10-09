//! Multi-output native Island smoke test.
//! cargo run -p kanade-runtime --example multi_output
//! Run alone on a niri session, not as a replacement for Kanade.
//! This paints a placeholder translucent shape (not the full shell UI).
//! No polling, pointers, keyboard focus, screenshots or input interception.

use std::collections::BTreeMap;

use kanade_runtime::{
    raster,
    wayland::{Event, LayerRuntime, ShmFrame},
    window::{Alignment, Edge, KeyboardMode, Layer, Spec, WindowId},
};

fn spec() -> Spec {
    Spec {
        width: 280,
        height: 44,
        layer: Layer::Top,
        edge: Edge::Top,
        align: Alignment::Center,
        keyboard: KeyboardMode::None,
        exclusive_zone: -1,
        input: Vec::new(),
    }
}

fn main() -> Result<(), String> {
    let mut runtime = LayerRuntime::connect()?;
    let mut outputs = BTreeMap::<u32, WindowId>::new();
    let mut frames = BTreeMap::<WindowId, ShmFrame>::new();

    loop {
        for event in runtime.dispatch()? {
            match event {
                Event::OutputReady { id, name, .. } => {
                    if outputs.contains_key(&id) {
                        continue; // Scale/name updates do not create duplicate surfaces.
                    }
                    let output = runtime.output(&name).cloned();
                    if let Some(output) = output {
                        let window = runtime.create(spec(), Some(&output));
                        outputs.insert(id, window);
                        println!("native output {name}: surface {}", window.0);
                    }
                }
                Event::OutputRemoved(output) => {
                    if let Some(window) = outputs.remove(&output) {
                        runtime.remove(window);
                        frames.remove(&window);
                    }
                }
                Event::Configured { id, width, height } => {
                    // A shape has no text, so it is only a compositor/render
                    // smoke test. Each buffer stays owned until release.
                    let pixels = raster::pill(width, height, height as f32 * 0.5)?;
                    let next = runtime.shm_frame(width, height, &pixels)?;
                    if runtime.present(id, &next.buffer)? {
                        frames.insert(id, next);
                        runtime.submitted(id, false);
                    }
                }
                Event::BufferReleased(released) => {
                    frames.retain(|_, frame| frame.buffer != released);
                }
                Event::Closed(window) => {
                    runtime.remove(window);
                    frames.remove(&window);
                    outputs.retain(|_, id| *id != window);
                }
                _ => {}
            }
        }
    }
}
