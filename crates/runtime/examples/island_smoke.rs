//! Native Island rendering smoke test, completely independent of Amane.
//!
//! cargo run -p kanade-runtime --example island_smoke
//!
//! Displays a static 260x40 Island centered inside a 560x380 layer-shell
//! canvas; empty pixels and input outside the Island pass through. Exits
//! after the compositor signals a presented frame. This is an SHM protocol
//! smoke test; the production renderer will use the wgpu path.

use kanade_runtime::{
    raster,
    wayland::{Event, LayerRuntime, ShmFrame},
    window::{Alignment, Edge, KeyboardMode, Layer, Rect, Spec, WindowId},
};

const WIDTH: u32 = 560;
const HEIGHT: u32 = 380;
const PILL_WIDTH: u32 = 260;
const PILL_HEIGHT: u32 = 40;
const PILL_X: u32 = (WIDTH - PILL_WIDTH) / 2;

fn pixels() -> Result<Vec<u8>, String> {
    let mut canvas = vec![0; (WIDTH * HEIGHT * 4) as usize];
    let pill = raster::pill(PILL_WIDTH, PILL_HEIGHT, 20.0)?;
    for y in 0..PILL_HEIGHT as usize {
        let dst = ((y * WIDTH as usize) + PILL_X as usize) * 4;
        let src = y * PILL_WIDTH as usize * 4;
        canvas[dst..dst + PILL_WIDTH as usize * 4]
            .copy_from_slice(&pill[src..src + PILL_WIDTH as usize * 4]);
    }
    Ok(canvas)
}

fn spec() -> Spec {
    Spec {
        width: WIDTH,
        height: HEIGHT,
        layer: Layer::Top,
        edge: Edge::Top,
        align: Alignment::Center,
        keyboard: KeyboardMode::None,
        exclusive_zone: -1,
        input: vec![Rect {
            x: PILL_X as i32,
            y: 0,
            width: PILL_WIDTH,
            height: PILL_HEIGHT,
        }],
    }
}

fn run() -> Result<(), String> {
    let mut runtime = LayerRuntime::connect()?;
    let id: WindowId = runtime.create(spec(), None);
    let mut frame: Option<ShmFrame> = None;
    let mut presented = false;

    loop {
        for event in runtime.dispatch()? {
            match event {
                Event::Configured {
                    id: configured,
                    width,
                    height,
                } if configured == id => {
                    if width != WIDTH || height != HEIGHT {
                        return Err(format!(
                            "unexpected compositor dimensions: {width}x{height}"
                        ));
                    }
                    if frame.is_none() {
                        frame = Some(runtime.shm_frame(WIDTH, HEIGHT, &pixels()?)?);
                    }
                    if let Some(buffer) = &frame {
                        presented |= runtime.present(id, &buffer.buffer)?;
                    }
                }
                Event::FrameReady(ready) if ready == id && presented => {
                    println!("native Island presented at {WIDTH}x{HEIGHT}, no Amane");
                    runtime.remove(id);
                    return Ok(());
                }
                Event::Closed(closed) if closed == id => {
                    return Err("compositor closed the Island smoke-test surface".into());
                }
                _ => {}
            }
        }
    }
}

fn main() {
    if let Err(message) = run() {
        eprintln!("kanade native Island smoke test: {message}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_pill_is_clickable() {
        let region = spec().input_regions();
        assert_eq!(region.len(), 1);
        assert_eq!(region[0].x, 150);
        assert_eq!(region[0].width, 260);
        assert_eq!(region[0].height, 40);
    }

    #[test]
    fn outside_canvas_is_transparent() {
        let bytes = pixels().unwrap();
        let at = |x: usize, y: usize| bytes[(y * WIDTH as usize + x) * 4 + 3];
        assert_eq!(at(0, 0), 0);
        assert_eq!(at(559, 379), 0);
        assert_eq!(at(280, 20), 218);
        assert_eq!(at(280, 41), 0);
    }
}
