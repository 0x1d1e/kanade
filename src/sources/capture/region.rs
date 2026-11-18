//! Kanade's own region picker (ADR 0116, ADR 0162).
//!
//! A layer window over every monitor on `Layer::Overlay`. For a recording it dims the live screen
//! and feeds the chosen rectangle to the recording runtime; for a screenshot it draws the frozen
//! frame the runtime captured of each monitor, dimmed outside the drag, and answers the frame's
//! pixel rectangle to the screenshot worker. Escape or a right click cancels, and a lock cancels
//! with it.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc;

use kanade_runtime::capture::Frame;
use kanade_runtime::service::Service;
use kanade_runtime::{
    Bitmap, Button, Center, Cursor, Full, Image, Key, Keyboard, Layer, LayerWindow, Monitor,
    Padding, Parent, Point, Rectangle, Stack, Text, Widget, Zone,
};

use crate::sources::recording::{self, Rect};
use crate::theme::{self, radius};

#[derive(Debug, Clone, PartialEq)]
pub struct Drag {
    pub monitor: String,
    pub monitor_size: (f32, f32),
    pub start: (f32, f32),
    pub current: (f32, f32),
}

// what the picker is open for
pub enum Pick {
    // an area to record, by the recording machine's serial: the live screen
    Record(u64),

    // an area to take from the frozen frames; none when the user leaves the picker
    Shot {
        result: mpsc::Sender<Option<(String, Rect)>>,
    },
}

#[derive(Default)]
pub struct Region {
    pub pick: Option<Pick>,
    pub drag: Option<Drag>,

    // the frozen frame of each monitor, one per output, for a shot
    frames: HashMap<String, Frame>,

    // each frame's pixels as one shared picture, so a redraw reuses its GPU upload
    pictures: HashMap<String, Arc<Bitmap>>,
}

impl Service for Region {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {}
}

/*
 * opens the picker for the recording `serial`; false when another picker is already up, which the
 * caller feeds to its machine as a left pick. It never answers the machine itself: the caller runs
 * with the recording runtime locked, which `recording::picked` would take again
 */
pub fn open(serial: u64) -> bool {
    let mut region = Region::write();

    if region.pick.is_some() || crate::lock::holds() {
        region.quiet();

        return false;
    }

    region.pick = Some(Pick::Record(serial));
    region.drag = None;

    true
}

/*
 * opens the picker on the frozen frames of a screenshot. One pick at a time: a picker already up
 * takes nothing, and the sender dropped here reads to the caller as a cancel
 */
pub fn shot(
    result: mpsc::Sender<Option<(String, Rect)>>,
    frames: impl IntoIterator<Item = (String, Frame)>,
) {
    let mut region = Region::write();

    if crate::lock::holds() {
        kanade_runtime::capture::clear_all();
        region.quiet();

        return;
    }

    if region.pick.is_some() {
        region.quiet();

        return;
    }

    region.frames = frames.into_iter().collect();
    region.pictures = region
        .frames
        .iter()
        .map(|(name, frame)| {
            let bitmap = Bitmap::shared(frame.width, frame.height, Arc::clone(&frame.pixels));

            (name.clone(), Arc::new(bitmap))
        })
        .collect();
    region.drag = None;
    region.pick = Some(Pick::Shot { result });
}

// the picker's pick and frozen frames taken out, and the drag let go; none when nothing was open
fn take() -> Option<Pick> {
    let mut region = Region::write();

    if region.pick.is_none() && region.drag.is_none() {
        region.quiet();

        return None;
    }

    region.drag = None;
    region.frames.clear();
    region.pictures.clear();

    region.pick.take()
}

pub fn cancel() {
    match take() {
        Some(Pick::Record(serial)) => recording::picked(serial, None),
        Some(Pick::Shot { result }) => {
            let _ = result.send(None);
        }
        None => {}
    }
}

pub fn close() {
    match take() {
        // the recording machine already left its pick; only a screenshot waits for this answer
        Some(Pick::Shot { result }) => {
            let _ = result.send(None);
        }
        Some(Pick::Record(_)) | None => {}
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

/*
 * the part of a frozen frame, in its own pixels, that a logical drag covers: each axis scales by the
 * frame's pixels over the monitor's logical size, rounded, kept inside the frame, at least 1 px
 */
pub fn crop(
    (min_x, min_y, w, h): (f32, f32, f32, f32),
    logical: (f32, f32),
    frame: (u32, u32),
) -> Rect {
    let scale = |logical: f32, pixels: u32| {
        if logical > 0.0 {
            pixels as f32 / logical
        } else {
            1.0
        }
    };

    let axis = |at: f32, length: f32, scale: f32, pixels: u32| -> (u32, u32) {
        if pixels == 0 {
            return (0, 0);
        }

        let start = (at * scale).round().clamp(0.0, (pixels - 1) as f32) as u32;
        let end = ((at + length) * scale)
            .round()
            .clamp(start as f32 + 1.0, pixels as f32) as u32;

        (start, end - start)
    };

    let (sx, sy) = (scale(logical.0, frame.0), scale(logical.1, frame.1));
    let (x, width) = axis(min_x, w, sx, frame.0);
    let (y, height) = axis(min_y, h, sy, frame.1);

    Rect {
        x: x as i32,
        y: y as i32,
        width,
        height,
    }
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
    let mut region = Region::write();
    if region.pick.is_none() {
        region.quiet();
        return;
    }

    let clamped = (
        point.x.clamp(0.0, monitor_size.0.max(0.0)),
        point.y.clamp(0.0, monitor_size.1.max(0.0)),
    );

    match &mut region.drag {
        Some(drag) if drag.monitor == monitor => {
            drag.monitor_size = monitor_size;
            drag.current = clamped;
        }
        _ => {
            region.drag = Some(Drag {
                monitor: monitor.to_owned(),
                monitor_size,
                start: clamped,
                current: clamped,
            });
        }
    }
}

fn drag_finish() {
    let mut region = Region::write();

    let Some(drag) = region.drag.take() else {
        region.quiet();
        return;
    };

    let (min_x, min_y, w, h) = clamp_bounds(drag.start, drag.current, drag.monitor_size);

    if w < 4.0 || h < 4.0 {
        return;
    }

    /*
     * a shot's frame size, by whether there is a frozen frame on the dragged monitor: a drag there
     * is ignored, and the picker stays open for another monitor or another drag
     */
    let size = match &region.pick {
        Some(Pick::Shot { .. }) => match region.frames.get(&drag.monitor) {
            Some(frame) => Some((frame.width, frame.height)),
            None => return,
        },
        Some(Pick::Record(_)) => None,
        None => return,
    };

    let Some(pick) = region.pick.take() else {
        return;
    };

    region.frames.clear();
    region.pictures.clear();

    drop(region);

    match pick {
        Pick::Record(serial) => {
            let ui = kanade_runtime::ui_scale();
            let (log_x, log_y) = monitor_logical_origin(&drag.monitor);
            let rect = compositor_rect(log_x, log_y, min_x, min_y, w, h, ui);

            recording::picked(serial, Some(rect));
        }
        Pick::Shot { result } => {
            let (fw, fh) = size.expect("a shot pick has its frozen frame");
            let rect = crop((min_x, min_y, w, h), drag.monitor_size, (fw, fh));

            let _ = result.send(Some((drag.monitor, rect)));
        }
    }
}

pub fn window(monitor: &Monitor) -> LayerWindow {
    let region = Region::read();

    let base = LayerWindow::new()
        .width(Full)
        .height(Full)
        .layer(Layer::Overlay)
        .space(Zone::Ignore)
        .namespace("kanade-region");

    let Some(_) = region.pick else {
        return base
            .visible(false)
            .click_through()
            .child(Rectangle::new().width(Parent).height(Parent));
    };

    // a shot draws the frozen frame of this monitor; a recording, and an output with no frozen
    // frame, draw the live screen under the scrim as before
    let frozen = match &region.pick {
        Some(Pick::Shot { .. }) => region
            .frames
            .get(&monitor.name)
            .zip(region.pictures.get(&monitor.name)),
        Some(Pick::Record(_)) | None => None,
    };

    let is_focused = {
        let monitors = kanade_runtime::Monitors::read();
        let focused_name = super::focused_output();
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

    let picture = |bitmap: &Arc<Bitmap>| -> Box<dyn Widget> {
        Box::new(
            Rectangle::new()
                .width(Parent)
                .height(Parent)
                .fill(Image::bitmap(Arc::clone(bitmap))),
        )
    };

    let scene: Box<dyn Widget> = match region
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

            if let Some((_, bitmap)) = frozen {
                layers.push(picture(bitmap));
            }

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

            // a shot's badge counts the pixels the crop keeps; a recording counts compositor pixels
            let (taken_w, taken_h) = match frozen {
                Some((frame, _)) => {
                    let rect = crop((min_x, min_y, w, h), (sw, sh), (frame.width, frame.height));
                    (rect.width, rect.height)
                }
                None => {
                    let ui = kanade_runtime::ui_scale();
                    (
                        ((w * ui).round() as u32) & !1,
                        ((h * ui).round() as u32) & !1,
                    )
                }
            };
            let badge_text = format!("{taken_w} \u{d7} {taken_h}");
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
        None => match frozen {
            Some((_, bitmap)) => Box::new(
                Stack::new(vec![
                    picture(bitmap),
                    Box::new(
                        Rectangle::new()
                            .width(Parent)
                            .height(Parent)
                            .fill(theme::REGION_SCRIM),
                    ),
                ])
                .width(Parent)
                .height(Parent),
            ),
            None => Box::new(
                Rectangle::new()
                    .width(Parent)
                    .height(Parent)
                    .fill(theme::REGION_SCRIM),
            ),
        },
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
    use std::sync::mpsc;
    use std::sync::{Mutex, MutexGuard};

    use super::*;

    // `Region` is one global Service, so the tests that open, drag and close it take turns
    fn region_turn() -> MutexGuard<'static, ()> {
        static TURN: Mutex<()> = Mutex::new(());
        TURN.lock().unwrap_or_else(|poison| poison.into_inner())
    }

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
    fn crop_scales_a_logical_drag_onto_the_frame_pixels() {
        let drag = (10.0, 20.0, 100.0, 200.0);

        // the frame is the logical size
        assert_eq!(
            crop(drag, (1920.0, 1080.0), (1920, 1080)),
            Rect {
                x: 10,
                y: 20,
                width: 100,
                height: 200,
            }
        );

        // twice the logical size on each axis
        assert_eq!(
            crop(drag, (960.0, 540.0), (1920, 1080)),
            Rect {
                x: 20,
                y: 40,
                width: 200,
                height: 400,
            }
        );
    }

    #[test]
    fn crop_rounds_a_frame_that_is_not_a_whole_multiple() {
        // 1366 by 768 pixels over 1280 by 720 logical: 1.0671875 and 1.0666667
        let rect = crop((10.0, 20.0, 100.0, 200.0), (1280.0, 720.0), (1366, 768));

        assert_eq!(
            rect,
            Rect {
                x: 11,
                y: 21,
                width: 106,
                height: 214,
            }
        );
    }

    #[test]
    fn crop_keeps_the_area_inside_the_frame() {
        // a drag to the very edge, and one past it
        assert_eq!(
            crop((95.0, 95.0, 20.0, 20.0), (100.0, 100.0), (200, 200)),
            Rect {
                x: 190,
                y: 190,
                width: 10,
                height: 10,
            }
        );

        assert_eq!(
            crop((200.0, 200.0, 50.0, 50.0), (100.0, 100.0), (200, 200)),
            Rect {
                x: 199,
                y: 199,
                width: 1,
                height: 1,
            }
        );

        assert_eq!(
            crop((-50.0, -50.0, 20.0, 20.0), (100.0, 100.0), (200, 200)),
            Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            }
        );
    }

    #[test]
    fn crop_is_at_least_one_pixel() {
        assert_eq!(
            crop((0.0, 0.0, 0.1, 0.1), (1000.0, 1000.0), (10, 10)),
            Rect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            }
        );
    }

    #[test]
    fn cancel_when_inactive_quiets_write() {
        let _turn = region_turn();

        close();
        assert!(Region::read().pick.is_none());
        assert_eq!(Region::read().drag, None);

        cancel();
        assert!(Region::read().pick.is_none());
        assert_eq!(Region::read().drag, None);
    }

    #[test]
    fn drag_update_when_inactive_quiets_write_and_ignores_event() {
        let _turn = region_turn();

        close();
        assert!(Region::read().pick.is_none());

        drag_update("eDP-1", (1920.0, 1080.0), Point { x: 50.0, y: 50.0 });
        assert_eq!(Region::read().drag, None);
    }

    #[test]
    fn picker_state_transitions_open_drag_and_close() {
        let _turn = region_turn();

        close();
        assert!(Region::read().pick.is_none());
        assert_eq!(Region::read().drag, None);

        assert!(open(42));
        assert!(matches!(Region::read().pick, Some(Pick::Record(42))));
        assert_eq!(Region::read().drag, None);

        drag_update("eDP-1", (1920.0, 1080.0), Point { x: 50.0, y: 50.0 });
        assert_eq!(
            Region::read().drag,
            Some(Drag {
                monitor: "eDP-1".into(),
                monitor_size: (1920.0, 1080.0),
                start: (50.0, 50.0),
                current: (50.0, 50.0),
            })
        );

        drag_update("eDP-1", (1920.0, 1080.0), Point { x: 150.0, y: 150.0 });
        assert_eq!(
            Region::read().drag,
            Some(Drag {
                monitor: "eDP-1".into(),
                monitor_size: (1920.0, 1080.0),
                start: (50.0, 50.0),
                current: (150.0, 150.0),
            })
        );

        close();
        assert!(Region::read().pick.is_none());
        assert_eq!(Region::read().drag, None);
    }

    #[test]
    fn a_shot_pick_sends_its_crop_once_and_closes() {
        let _turn = region_turn();

        close();

        let frame = Frame {
            width: 3840,
            height: 2160,
            pixels: Arc::new(vec![0; 4]),
        };

        let (sender, receiver) = mpsc::channel();

        shot(sender, [("eDP-1".to_owned(), frame)]);
        assert!(matches!(Region::read().pick, Some(Pick::Shot { .. })));

        drag_update("eDP-1", (1920.0, 1080.0), Point { x: 100.0, y: 200.0 });
        drag_update("eDP-1", (1920.0, 1080.0), Point { x: 300.0, y: 500.0 });
        drag_finish();

        assert_eq!(
            receiver.try_recv(),
            Ok(Some((
                String::from("eDP-1"),
                Rect {
                    x: 200,
                    y: 400,
                    width: 400,
                    height: 600,
                }
            )))
        );

        // the picker is closed, and a close after the answer sends nothing more
        assert!(Region::read().pick.is_none());

        close();
        assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    }

    #[test]
    fn canceling_a_shot_sends_none_once_and_closes() {
        let _turn = region_turn();

        close();

        let (sender, receiver) = mpsc::channel();
        shot(sender, Vec::new());

        cancel();

        assert_eq!(receiver.try_recv(), Ok(None));
        assert!(Region::read().pick.is_none());

        // a second cancel, and a close, send nothing
        cancel();
        close();
        assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Disconnected));
    }

    #[test]
    fn a_shot_drag_on_a_monitor_without_a_frame_is_ignored() {
        let _turn = region_turn();

        close();

        let (sender, receiver) = mpsc::channel();
        shot(sender, Vec::new());

        drag_update("eDP-1", (1920.0, 1080.0), Point { x: 0.0, y: 0.0 });
        drag_update("eDP-1", (1920.0, 1080.0), Point { x: 100.0, y: 100.0 });
        drag_finish();

        // still picking, with nothing answered
        assert!(matches!(Region::read().pick, Some(Pick::Shot { .. })));
        assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty));

        close();
    }

    #[test]
    fn a_drag_too_small_to_be_a_shot_keeps_the_picker_open() {
        let _turn = region_turn();

        close();

        let frame = Frame {
            width: 1920,
            height: 1080,
            pixels: Arc::new(vec![0; 4]),
        };

        let (sender, receiver) = mpsc::channel();
        shot(sender, [("eDP-1".to_owned(), frame)]);

        drag_update("eDP-1", (1920.0, 1080.0), Point { x: 10.0, y: 10.0 });
        drag_update("eDP-1", (1920.0, 1080.0), Point { x: 12.0, y: 12.0 });
        drag_finish();

        assert!(matches!(Region::read().pick, Some(Pick::Shot { .. })));
        assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty));

        close();
    }

    #[test]
    fn a_record_pick_while_a_shot_is_up_leaves_the_shot_open() {
        let _turn = region_turn();

        close();

        let (sender, receiver) = mpsc::channel();
        shot(sender, Vec::new());

        // a recording asked for while the shot's picker is up is refused, so its caller leaves it
        assert!(!open(7));
        assert!(matches!(Region::read().pick, Some(Pick::Shot { .. })));

        close();
        assert_eq!(receiver.try_recv(), Ok(None));
    }

    #[test]
    fn a_record_drag_reaches_the_recording_machine_and_closes() {
        let _turn = region_turn();

        close();
        recording::clear_picked();
        assert!(open(9));

        drag_update("eDP-1", (1920.0, 1080.0), Point { x: 100.0, y: 100.0 });
        drag_update("eDP-1", (1920.0, 1080.0), Point { x: 500.0, y: 400.0 });
        drag_finish();

        assert!(matches!(recording::picked_for(9), Some(Some(_))));
        assert!(Region::read().pick.is_none());
        assert_eq!(Region::read().drag, None);
    }

    #[test]
    fn locked_session_refuses_open_and_shot_without_storing_frames() {
        const TEST: &str = "sources::capture::region::tests::locked_session_refuses_open_and_shot_without_storing_frames";
        if std::env::var_os("KANADE_REGION_LOCK_FIXTURE").is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture"])
                .env("KANADE_REGION_LOCK_FIXTURE", "1")
                .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }

        crate::lock::pending_fixture();
        assert!(!open(42));
        assert!(Region::read().pick.is_none());

        let (sender, receiver) = mpsc::channel();
        let frame = Frame {
            width: 100,
            height: 100,
            pixels: Arc::new(vec![0; 100 * 100 * 4]),
        };
        shot(sender, [(String::from("eDP-1"), frame)]);

        assert_eq!(receiver.try_recv(), Err(mpsc::TryRecvError::Disconnected));
        let region = Region::read();
        assert!(region.pick.is_none());
        assert!(region.frames.is_empty());
        assert!(region.pictures.is_empty());
    }
}
