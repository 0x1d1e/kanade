//! Native compositor-managed settings-window handshake; no Amane UI.
//!
//! cargo run -p kanade-runtime --example settings_smoke
//!
//! Draws a plain diagnostic panel to verify xdg-shell configure, resize and
//! wl_buffer presentation. Close the window to stop the test. This is NOT
//! the finished Settings UI.

use kanade_runtime::{
    chrome::{ChromeAction, hit_test},
    raster,
    wayland::{Event, LayerRuntime, ShmFrame},
    window::WindowId,
};

fn frame_pixels(width: u32, height: u32) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err("settings dimensions must be positive".into());
    }
    let size = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|n| n.checked_mul(4))
        .and_then(|n| usize::try_from(n).ok())
        .ok_or("settings buffer is too large")?;
    if size > i32::MAX as usize {
        return Err("settings buffer exceeds Wayland SHM limit".into());
    }
    let mut data = vec![0u8; size];
    for pixel in data.as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&[35, 28, 23, 255]);
    }
    let panel_width = width.saturating_sub(64).min(480);
    let panel_height = height.saturating_sub(64).min(100);
    if panel_width > 0 && panel_height > 0 {
        let panel = raster::pill(panel_width, panel_height, 18.0)?;
        let start_x = (width - panel_width) / 2;
        let start_y = (height - panel_height) / 2;
        for y in 0..panel_height as usize {
            let target = (((start_y as usize + y) * width as usize) + start_x as usize) * 4;
            let source = y * panel_width as usize * 4;
            for x in 0..panel_width as usize {
                let at = source + x * 4;
                if panel[at + 3] != 0 {
                    data[target + x * 4..target + x * 4 + 4].copy_from_slice(&panel[at..at + 4]);
                }
            }
        }
    }
    Ok(data)
}

fn run() -> Result<(), String> {
    let mut runtime = LayerRuntime::connect()?;
    let id: WindowId = runtime.create_toplevel(
        680,
        520,
        "Kanade Native Settings — Protocol Probe",
        "kanade",
    )?;
    let mut active: Vec<ShmFrame> = Vec::new();
    let mut last_size = (680u32, 520u32);
    let mut pointer = (0.0, 0.0);
    loop {
        for event in runtime.dispatch()? {
            match event {
                Event::Configured {
                    id: configured,
                    width,
                    height,
                } if configured == id => {
                    last_size = (width, height);
                    // A size may change while a previous buffer is in use.
                    // Each presented buffer has its own backing file.
                    let frame = runtime.shm_frame(width, height, &frame_pixels(width, height)?)?;
                    if runtime.present(id, &frame.buffer)? {
                        active.push(frame);
                    }
                }
                Event::PointerEnter { id: on, x, y } | Event::PointerMotion { id: on, x, y }
                    if on == id =>
                {
                    pointer = (x, y);
                }
                Event::PointerButton {
                    id: on,
                    button: 0x110,
                    pressed: true,
                    serial,
                } if on == id => match hit_test(last_size.0, last_size.1, pointer.0, pointer.1) {
                    ChromeAction::Move => runtime.move_toplevel(id, serial)?,
                    ChromeAction::Resize(edge) => runtime.resize_toplevel(id, serial, edge)?,
                    ChromeAction::Content => {}
                },
                Event::BufferReleased(buffer) => {
                    active.retain(|frame| frame.buffer != buffer);
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

fn main() {
    if let Err(error) = run() {
        eprintln!("kanade native settings probe: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::frame_pixels;

    #[test]
    fn diagnostic_panel_respects_resizing() {
        let a = frame_pixels(640, 480).unwrap();
        let b = frame_pixels(360, 260).unwrap();
        assert_eq!(a.len(), 640 * 480 * 4);
        assert_eq!(b.len(), 360 * 260 * 4);
        assert_eq!(a[3], 255);
        assert!(frame_pixels(0, u32::MAX).is_err());
    }
}
