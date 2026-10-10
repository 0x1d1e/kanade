//! The privacy cluster (ADRs 0005, 0033): a microphone, camera or screen capture in use, as a dot
//! of its color each at the trailing end of the Island's body, inside it. Never an Activity: it
//! shows whatever the island shows, and takes no pointer. Over a fullscreen window it does not
//! show. `privacy.indicators` turns it off.

use kanade_runtime::service::Service;
use kanade_runtime::{Center, Color, Column, End, Padding, Rectangle, Row, Start, Widget};

use crate::config;
use crate::icon::Icon;
use crate::island::geometry::{PEEK, Rect};
use crate::island::presentation::Presentation;
use crate::sources::privacy::Privacy;
use crate::theme;

const GLYPH: f32 = 16.0;

const DOT: f32 = 8.0;
const DOT_GAP: f32 = 5.0;

// from the body's trailing end to the last dot, and from the first dot to what the form draws
const INSET: f32 = 12.0;
const CLEAR: f32 = 10.0;

// what captures now: a dot each, in the colors `glyphs` uses and in the same order
pub struct Cluster {
    inks: Vec<Color>,
}

// whether the settings ask for the indicators, which the Controls Surface follows too
pub fn on() -> bool {
    config::get().privacy_indicators
}

// the cluster for what captures now, none while nothing does or it is off; reading subscribes
pub fn read() -> Option<Cluster> {
    if !on() {
        return None;
    }

    let privacy = Privacy::read();

    privacy.any().then(|| Cluster {
        inks: captures(&privacy).map(|(_, ink)| ink).collect(),
    })
}

// whether a body in this Presentation carries the cluster: the small forms, not a Tray or a Surface
pub fn beside(presentation: Presentation) -> bool {
    matches!(
        presentation,
        Presentation::Rest | Presentation::Compact | Presentation::Split | Presentation::Peek
    )
}

impl Cluster {
    fn length(&self) -> f32 {
        let count = self.inks.len() as f32;

        DOT * count + DOT_GAP * (count - 1.0).max(0.0)
    }

    // how much of its trailing end the cluster takes from a small form, which lays out in the rest
    pub fn room(&self) -> f32 {
        INSET + self.length() + CLEAR
    }

    /*
     * the dots in a layer as large as the body, from its trailing end: across the top, centered on
     * the small form there (as tall as the body, up to a Peek), or along a side edge down it from
     * the end, centered across
     */
    pub fn draw(self, body: Rect, sideways: bool) -> Rectangle {
        let dots = self.inks.into_iter().map(|ink| {
            Box::new(
                Rectangle::new()
                    .width(DOT)
                    .height(DOT)
                    .radius(DOT / 2.0)
                    .fill(ink),
            ) as Box<dyn Widget>
        });
        let layer = Rectangle::new().width(body.width).height(body.height);

        if sideways {
            layer
                .padding(Padding {
                    bottom: INSET,
                    ..Padding::default()
                })
                .align_child(Center, End)
                .child(Column::new(dots.collect()).gap(DOT_GAP).align(Center))
        } else {
            layer
                .padding(Padding {
                    right: INSET,
                    top: (body.height.min(PEEK.height) - DOT) / 2.0,
                    ..Padding::default()
                })
                .align_child(End, Start)
                .child(Row::new(dots.collect()).gap(DOT_GAP).align(Center))
        }
    }
}

/*
 * what captures, as the Controls Surface draws it beside the apps: glyphs that differ by shape, not
 * color alone (plan 7), green for a microphone or camera, amber for a screen cast
 */
pub fn glyphs(privacy: &Privacy) -> Vec<Box<dyn Widget>> {
    captures(privacy)
        .map(|(icon, ink)| Box::new(icon.on(GLYPH, ink)) as Box<dyn Widget>)
        .collect()
}

// what captures now, each with its glyph and color
fn captures(privacy: &Privacy) -> impl Iterator<Item = (Icon, Color)> {
    let sensors = privacy.sensors.as_ref();

    [
        (
            sensors.is_some_and(|sensors| sensors.microphone),
            Icon::Microphone,
            theme::SEMANTIC.privacy,
        ),
        (
            sensors.is_some_and(|sensors| sensors.camera),
            Icon::Camera,
            theme::SEMANTIC.privacy,
        ),
        (privacy.casting, Icon::Capture, theme::SEMANTIC.capture),
    ]
    .into_iter()
    .filter(|(on, _, _)| *on)
    .map(|(_, icon, ink)| (icon, ink))
}
