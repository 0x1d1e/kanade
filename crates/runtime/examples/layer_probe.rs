//! Probe the actual native layer-shell handshake (does not map a buffer).
//! Usage: cargo run -p kanade-runtime --example layer_probe
//! This intentionally cannot replace the running shell: it verifies only the
//! first configure and does not render anything or capture keyboard input.

use kanade_runtime::{
    wayland::{Event, LayerRuntime},
    window::{Alignment, Edge, KeyboardMode, Layer, Spec},
};

fn main() -> Result<(), String> {
    let mut runtime = LayerRuntime::connect()?;
    let id = runtime.create(
        Spec {
            width: 560,
            height: 380,
            layer: Layer::Top,
            edge: Edge::Top,
            align: Alignment::Center,
            keyboard: KeyboardMode::None,
            exclusive_zone: -1,
            input: vec![],
        },
        None,
    );

    loop {
        for event in runtime.dispatch()? {
            match event {
                Event::Configured { id: configured, width, height } if configured == id => {
                    println!("native layer configured: {width}x{height}");
                    runtime.remove(id);
                    return Ok(());
                }
                Event::Closed(closed) if closed == id => {
                    runtime.remove(id);
                    return Err("the compositor closed the probe before configuring".into());
                }
                _ => {}
            }
        }
    }
}
