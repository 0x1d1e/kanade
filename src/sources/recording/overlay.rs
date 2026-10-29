//! Native region selection overlay for area recording (ADR 0116).
//!
//! A layer window drawn over every monitor on `Layer::Overlay`. When active,
//! it dims the screen with `REGION_SCRIM`, shows a crosshair cursor, grabs exclusive
//! keyboard on the focused monitor, and lets the user drag an area with a live
//! 100% transparent cutout, selection border, and floating dimension pill.
//! Releasing the pointer rounds the area to even pixel bounds, maps it to compositor
//! layout coordinates, and feeds it to the recording runtime. Esc or right click cancels.

use kanade_runtime::service::Service;
use kanade_runtime::{
    Button, Center, Cursor, Full, Key, Keyboard, Layer, LayerWindow, Monitor, Padding, Parent,
    Point, Rectangle, Stack, Text, Widget, Zone,
};

use super::Rect;
use crate::theme::{self, radius};

#[derive(Debug, Clone, PartialEq)]
pub struct Drag {
    pub monitor: String,
    pub monitor_size: (f32, f32),
    pub start: (f32, f32),
    pub current: (f32, f32),
}

#[derive(Debug, Default)]
pub struct RegionOverlay {
    pub serial: Option<u64>,
    pub drag: Option<Drag>,
}

impl Service for RegionOverlay {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {}
}

pub fn open(serial: u64) {
    let mut overlay = RegionOverlay::write();
    overlay.serial = Some(serial);
    overlay.drag = None;
}

pub fn cancel() {
    let serial = {
        let mut overlay = RegionOverlay::write();
        if overlay.serial.is_none() && overlay.drag.is_none() {
            overlay.quiet();
            return;
        }
        overlay.drag = None;
        overlay.serial.take()
    };

    if let Some(serial) = serial {
        super::picked(serial, None);
    }
}

pub fn close() {
    let mut overlay = RegionOverlay::write();
    if overlay.serial.is_none() && overlay.drag.is_none() {
        overlay.quiet();
    } else {
        overlay.serial = None;
        overlay.drag = None;
    }
}

pub fn spawn() {
    kanade_runtime::service::watch::<crate::lock::Said>(on_lock);
}

fn on_lock() {
    if crate::lock::holds() {
        cancel();
    }
}

pub fn normalize_drag(start: (f32, f32), current: (f32, f32)) -> (f32, f32, f32, f32) {
    let min_x = start.0.min(current.0);
    let max_x = start.0.max(current.0);
    let min_y = start.1.min(current.1);
    let max_y = start.1.max(current.1);
    let w = max_x - min_x;
    let h = max_y - min_y;

    (min_x, min_y, w, h)
}

pub fn clamp_bounds(
    start: (f32, f32),
    current: (f32, f32),
    size: (f32, f32),
) -> (f32, f32, f32, f32) {
    let sw = size.0.max(0.0);
    let sh = size.1.max(0.0);
    let start = (start.0.clamp(0.0, sw), start.1.clamp(0.0, sh));
    let current = (current.0.clamp(0.0, sw), current.1.clamp(0.0, sh));
    normalize_drag(start, current)
}

pub fn compositor_rect(
    log_x: i32,
    log_y: i32,
    min_x: f32,
    min_y: f32,
    w: f32,
    h: f32,
    scale: f32,
) -> Rect {
    Rect {
        x: log_x + (min_x * scale).round() as i32,
        y: log_y + (min_y * scale).round() as i32,
        width: ((w * scale).round() as u32) & !1,
        height: ((h * scale).round() as u32) & !1,
    }
}

fn monitor_logical_origin(monitor_name: &str) -> (i32, i32) {
    let Ok(reply) = crate::sources::niri::ask("\"Outputs\"") else {
        return (0, 0);
    };
    let outputs = crate::settings::outputs::parse_outputs(&reply);
    outputs
        .iter()
        .find(|o| o.name == monitor_name)
        .and_then(|o| o.logical.as_ref())
        .map_or((0, 0), |logical| (logical.x, logical.y))
}

fn drag_update(monitor: &str, monitor_size: (f32, f32), point: Point) {
    let mut overlay = RegionOverlay::write();
    if overlay.serial.is_none() {
        overlay.quiet();
        return;
    }

    let clamped = (
        point.x.clamp(0.0, monitor_size.0.max(0.0)),
        point.y.clamp(0.0, monitor_size.1.max(0.0)),
    );

    match &mut overlay.drag {
        Some(drag) if drag.monitor == monitor => {
            drag.monitor_size = monitor_size;
            drag.current = clamped;
        }
        _ => {
            overlay.drag = Some(Drag {
                monitor: monitor.to_owned(),
                monitor_size,
                start: clamped,
                current: clamped,
            });
        }
    }
}

fn drag_finish() {
    let (serial, drag) = {
        let mut overlay = RegionOverlay::write();
        let serial = overlay.serial;
        let drag = overlay.drag.take();
        (serial, drag)
    };

    let Some(serial) = serial else {
        return;
    };

    let Some(drag) = drag else {
        return;
    };

    let (min_x, min_y, w, h) = clamp_bounds(drag.start, drag.current, drag.monitor_size);

    if w >= 4.0 && h >= 4.0 {
        let ui = kanade_runtime::ui_scale();
        let (log_x, log_y) = monitor_logical_origin(&drag.monitor);
        let rect = compositor_rect(log_x, log_y, min_x, min_y, w, h, ui);

        close();
        super::picked(serial, Some(rect));
    }
}

pub fn window(monitor: &Monitor) -> LayerWindow {
    let overlay = RegionOverlay::read();
    let serial = overlay.serial;

    let base = LayerWindow::new()
        .width(Full)
        .height(Full)
        .layer(Layer::Overlay)
        .space(Zone::Ignore)
        .namespace("kanade-region");

    let Some(_) = serial else {
        return base
            .visible(false)
            .click_through()
            .child(Rectangle::new().width(Parent).height(Parent));
    };

    let is_focused = {
        let niri = crate::sources::niri::Niri::read();
        let monitors = kanade_runtime::Monitors::read();
        let focused_name = niri.seen.focused_output.as_deref();
        if let Some(focused) = focused_name.filter(|name| monitors.has(name)) {
            focused == monitor.name
        } else {
            monitors
                .all()
                .first()
                .is_some_and(|m| m.name == monitor.name)
        }
    };

    let base = if is_focused {
        base.keyboard(Keyboard::Exclusive).on_key(|key| {
            if key == Key::Escape {
                cancel();
            }
        })
    } else {
        base.keyboard(Keyboard::None)
    };

    let scene: Box<dyn Widget> = match overlay
        .drag
        .as_ref()
        .filter(|drag| drag.monitor == monitor.name)
    {
        Some(drag) => {
            let sw = monitor.width as f32;
            let sh = monitor.height as f32;
            let (min_x, min_y, w, h) = clamp_bounds(drag.start, drag.current, (sw, sh));
            let max_x = min_x + w;
            let max_y = min_y + h;

            let mut layers: Vec<Box<dyn Widget>> = Vec::new();

            if min_y > 0.0 {
                layers.push(Box::new(
                    Rectangle::new()
                        .width(sw)
                        .height(min_y)
                        .fill(theme::REGION_SCRIM),
                ));
            }

            if max_y < sh {
                layers.push(Box::new(
                    Rectangle::new()
                        .width(sw)
                        .height((sh - max_y).max(0.0))
                        .translate(0.0, max_y)
                        .fill(theme::REGION_SCRIM),
                ));
            }

            if min_x > 0.0 && h > 0.0 {
                layers.push(Box::new(
                    Rectangle::new()
                        .width(min_x)
                        .height(h)
                        .translate(0.0, min_y)
                        .fill(theme::REGION_SCRIM),
                ));
            }

            if max_x < sw && h > 0.0 {
                layers.push(Box::new(
                    Rectangle::new()
                        .width((sw - max_x).max(0.0))
                        .height(h)
                        .translate(max_x, min_y)
                        .fill(theme::REGION_SCRIM),
                ));
            }

            layers.push(Box::new(
                Rectangle::new()
                    .width(w)
                    .height(h)
                    .translate(min_x, min_y)
                    .border(1.5, theme::REGION_BORDER),
            ));

            let ui = kanade_runtime::ui_scale();
            let even_w = ((w * ui).round() as u32) & !1;
            let even_h = ((h * ui).round() as u32) & !1;
            let badge_text = format!("{even_w} \u{d7} {even_h}");
            let badge_w = (16.0 + 7.5 * badge_text.len() as f32).max(64.0);
            let badge_h = 24.0;
            let badge_x = (min_x + (w - badge_w) / 2.0).clamp(8.0, (sw - badge_w - 8.0).max(8.0));
            let badge_y = if max_y + badge_h + 8.0 <= sh {
                max_y + 8.0
            } else if min_y - badge_h - 8.0 >= 0.0 {
                min_y - badge_h - 8.0
            } else {
                (max_y - badge_h - 8.0).max(8.0)
            };

            layers.push(Box::new(
                Rectangle::new()
                    .width(badge_w)
                    .height(badge_h)
                    .translate(badge_x, badge_y)
                    .fill(theme::REGION_BADGE_BG)
                    .border(1.0, theme::REGION_BADGE_BORDER)
                    .radius(radius::ROW)
                    .padding(Padding {
                        top: 2.0,
                        bottom: 2.0,
                        left: 8.0,
                        right: 8.0,
                    })
                    .align_child(Center, Center)
                    .child(
                        Text::new(badge_text)
                            .size(theme::text::LABEL_SMALL)
                            .color(theme::REGION_BADGE_TEXT)
                            .weight(theme::text::MEDIUM),
                    ),
            ));

            Box::new(Stack::new(layers).width(Parent).height(Parent))
        }
        None => Box::new(
            Rectangle::new()
                .width(Parent)
                .height(Parent)
                .fill(theme::REGION_SCRIM),
        ),
    };

    let root = Rectangle::new()
        .width(Parent)
        .height(Parent)
        .cursor(Cursor::Crosshair)
        .on_click(|button| {
            if button == Button::Right {
                cancel();
            }
        })
        .on_drag({
            let monitor_name = monitor.name.clone();
            let monitor_size = (monitor.width as f32, monitor.height as f32);
            move |point| {
                drag_update(&monitor_name, monitor_size, point);
            }
        })
        .on_drag_end(|_| drag_finish())
        .child(scene);

    base.child(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_normalization_handles_any_drag_direction() {
        // top-left to bottom-right
        let (min_x, min_y, w, h) = normalize_drag((10.0, 20.0), (110.0, 220.0));
        assert_eq!((min_x, min_y, w, h), (10.0, 20.0, 100.0, 200.0));

        // bottom-right to top-left
        let (min_x, min_y, w, h) = normalize_drag((110.0, 220.0), (10.0, 20.0));
        assert_eq!((min_x, min_y, w, h), (10.0, 20.0, 100.0, 200.0));

        // bottom-left to top-right
        let (min_x, min_y, w, h) = normalize_drag((10.0, 220.0), (110.0, 20.0));
        assert_eq!((min_x, min_y, w, h), (10.0, 20.0, 100.0, 200.0));

        // top-right to bottom-left
        let (min_x, min_y, w, h) = normalize_drag((110.0, 20.0), (10.0, 220.0));
        assert_eq!((min_x, min_y, w, h), (10.0, 20.0, 100.0, 200.0));
    }

    #[test]
    fn compositor_rect_rounds_sides_down_to_even_and_offsets_by_logical_origin() {
        let rect = compositor_rect(1920, 100, 50.0, 60.0, 101.9, 203.4, 1.0);
        assert_eq!(
            rect,
            Rect {
                x: 1970,
                y: 160,
                width: 102,
                height: 202,
            }
        );
    }

    #[test]
    fn compositor_rect_scales_to_compositor_logical_pixels_and_rounds_even() {
        // scale 1.25
        let rect = compositor_rect(100, 50, 40.0, 40.0, 81.0, 83.0, 1.25);
        assert_eq!(
            rect,
            Rect {
                x: 150,
                y: 100,
                width: 100,
                height: 104,
            }
        );

        // scale 1.5
        let rect = compositor_rect(0, 0, 10.0, 20.0, 100.0, 150.0, 1.5);
        assert_eq!(
            rect,
            Rect {
                x: 15,
                y: 30,
                width: 150,
                height: 224,
            }
        );

        // scale 2.0
        let rect = compositor_rect(500, 300, 25.0, 30.0, 100.0, 100.0, 2.0);
        assert_eq!(
            rect,
            Rect {
                x: 550,
                y: 360,
                width: 200,
                height: 200,
            }
        );
    }

    #[test]
    fn clamp_bounds_clamps_drag_to_monitor_geometry() {
        let monitor_size = (1920.0, 1080.0);

        // drag completely outside top-left and bottom-right
        let (min_x, min_y, w, h) = clamp_bounds((-50.0, -100.0), (2500.0, 1500.0), monitor_size);
        assert_eq!((min_x, min_y, w, h), (0.0, 0.0, 1920.0, 1080.0));

        // drag in reverse (bottom-right to top-left beyond edges)
        let (min_x, min_y, w, h) = clamp_bounds((2500.0, 1500.0), (-50.0, -100.0), monitor_size);
        assert_eq!((min_x, min_y, w, h), (0.0, 0.0, 1920.0, 1080.0));

        // partial overflow past right and bottom
        let (min_x, min_y, w, h) = clamp_bounds((1800.0, 1000.0), (2000.0, 1200.0), monitor_size);
        assert_eq!((min_x, min_y, w, h), (1800.0, 1000.0, 120.0, 80.0));

        // completely inside monitor
        let (min_x, min_y, w, h) = clamp_bounds((100.0, 200.0), (300.0, 500.0), monitor_size);
        assert_eq!((min_x, min_y, w, h), (100.0, 200.0, 200.0, 300.0));
    }

    #[test]
    fn cancel_when_inactive_quiets_write() {
        close();
        assert_eq!(RegionOverlay::read().serial, None);
        assert_eq!(RegionOverlay::read().drag, None);

        cancel();
        assert_eq!(RegionOverlay::read().serial, None);
        assert_eq!(RegionOverlay::read().drag, None);
    }

    #[test]
    fn drag_update_when_inactive_quiets_write_and_ignores_event() {
        close();
        assert_eq!(RegionOverlay::read().serial, None);

        drag_update("eDP-1", (1920.0, 1080.0), Point { x: 50.0, y: 50.0 });
        assert_eq!(RegionOverlay::read().drag, None);
    }

    #[test]
    fn overlay_state_transitions_open_drag_and_close() {
        close();
        assert_eq!(RegionOverlay::read().serial, None);
        assert_eq!(RegionOverlay::read().drag, None);

        open(42);
        assert_eq!(RegionOverlay::read().serial, Some(42));
        assert_eq!(RegionOverlay::read().drag, None);

        drag_update("eDP-1", (1920.0, 1080.0), Point { x: 50.0, y: 50.0 });
        assert_eq!(
            RegionOverlay::read().drag,
            Some(Drag {
                monitor: "eDP-1".into(),
                monitor_size: (1920.0, 1080.0),
                start: (50.0, 50.0),
                current: (50.0, 50.0),
            })
        );

        drag_update("eDP-1", (1920.0, 1080.0), Point { x: 150.0, y: 150.0 });
        assert_eq!(
            RegionOverlay::read().drag,
            Some(Drag {
                monitor: "eDP-1".into(),
                monitor_size: (1920.0, 1080.0),
                start: (50.0, 50.0),
                current: (150.0, 150.0),
            })
        );

        close();
        assert_eq!(RegionOverlay::read().serial, None);
        assert_eq!(RegionOverlay::read().drag, None);
    }
}
