//! The privacy cluster (ADR 0005): a microphone, camera or screen capture in use, on every monitor,
//! on the Overlay layer, so no fullscreen window covers it. Never an Activity: it shows whatever the
//! island shows, and takes no pointer, so it never blocks what lies beneath it.

use amane::{
    Center, End, Horizontal, Layer, LayerWindow, Margin, Monitor, Rectangle, Row, Service, Start,
    Vertical, Widget, Zone,
};

use crate::icon::Icon;
use crate::island::geometry;
use crate::sources::privacy::Privacy;
use crate::theme;

const GLYPH: f32 = 16.0;
const GAP: f32 = 8.0;
const INSET: f32 = 12.0;

// as tall as the island at rest, which it lines up with
const HEIGHT: f32 = geometry::REST.height;

// from the monitor's top right corner
const MARGIN: i32 = 8;

// room for all three glyphs; the pill takes what it needs at the right
const WIDTH: f32 = INSET * 2.0 + GLYPH * 3.0 + GAP * 2.0;

// one window per monitor, hidden while nothing captures, so it draws nothing then
pub fn window(_: &Monitor) -> LayerWindow {
    // reading subscribes this window to capture changes
    let privacy = Privacy::read();
    let glyphs = glyphs(&privacy);

    let count = glyphs.len() as f32;
    let width = INSET * 2.0 + GLYPH * count + GAP * (count - 1.0).max(0.0);

    LayerWindow::new()
        .width(WIDTH)
        .height(HEIGHT)
        .anchor_vertical(Vertical::Top)
        .anchor_horizontal(Horizontal::Right)
        .margin(Margin {
            top: geometry::TOP as i32,
            right: MARGIN,
            ..Margin::default()
        })
        .layer(Layer::Overlay)
        .space(Zone::Ignore)
        .namespace("kanade-privacy")
        .visible(privacy.any())
        .click_through()
        .child(
            Rectangle::new()
                .width(WIDTH)
                .height(HEIGHT)
                .align_child(End, Start)
                .child(
                    Rectangle::new()
                        .width(width)
                        .height(HEIGHT)
                        .radius(HEIGHT / 2.0)
                        .fill(theme::body())
                        .align_child(Center, Center)
                        .child(Row::new(glyphs).gap(GAP).align(Center)),
                ),
        )
}

/*
 * what captures, as the cluster and the Controls Surface draw it: glyphs that differ by shape, not
 * color alone (plan 7), green for a microphone or camera, amber for a screen cast
 */
pub fn glyphs(privacy: &Privacy) -> Vec<Box<dyn Widget>> {
    let sensors = privacy.sensors.as_ref();

    [
        (
            sensors.is_some_and(|sensors| sensors.microphone),
            Icon::Microphone,
            theme::GREEN,
        ),
        (
            sensors.is_some_and(|sensors| sensors.camera),
            Icon::Camera,
            theme::GREEN,
        ),
        (privacy.casting, Icon::Capture, theme::AMBER),
    ]
    .into_iter()
    .filter(|(on, _, _)| *on)
    .map(|(_, icon, ink)| Box::new(icon.on(GLYPH, ink)) as Box<dyn Widget>)
    .collect()
}
