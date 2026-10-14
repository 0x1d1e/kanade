//! The privacy cluster (ADRs 0005, 0033): a microphone, camera or screen capture in use, as a dot
//! of its color and shape each at the trailing end of the Island's body, inside it. Never an Activity: it
//! shows whatever the island shows, and takes no pointer. Over a fullscreen window it does not
//! show. `privacy.indicators` turns it off.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use kanade_runtime::service::Service;
use kanade_runtime::{
    Center, Color, End, Monitors, MutexExt, Padding, Rectangle, Stack, Start, Widget,
};

use crate::config;
use crate::icon::Icon;
use crate::island::geometry::{
    DOT, DOT_CLEAR as CLEAR, DOT_GAP, DOT_INSET as INSET, DOTS, MEDIA, PEEK, Rect,
};
use crate::island::motion::{Mode, Spring};
use crate::island::presentation::Presentation;
use crate::sources::privacy::Privacy;
use crate::theme;

const GLYPH: f32 = 16.0;

// a microphone, a camera and a screen cast
const SENSORS: usize = DOTS as usize;

// the room the dots of every sensor take, which each slides within
const FULL: f32 = DOT * SENSORS as f32 + DOT_GAP * (SENSORS - 1) as f32;

// from the top of a body as large as Media, above its header and its controls
const CORNER: f32 = 12.0;

// the shape of a sensor's dot, so color is not the only thing that tells them apart
#[derive(Clone, Copy)]
enum Mark {
    // a microphone
    Round,

    // a camera
    Square,

    // a screen cast
    Ring,
}

// the corner of a camera's square dot
const SQUARE_CORNER: f32 = 2.0;

// the stroke of a screen cast's ring dot
const RING: f32 = 1.5;

// what captures, a dot each in the colors `glyphs` uses and in the same order, each with its opacity
pub struct Cluster {
    dots: Vec<(Mark, Color, f32)>,

    // the dots still fade, so the view keeps asking for frames
    pub moving: bool,
}

// one sensor's dot on one output, which stays while it fades out
struct Dot {
    presence: Spring<1>,

    // its opacity when the presence's leg began, so a change of mind fades on from there
    faded: f32,
    shown: bool,
}

impl Dot {
    fn new(mode: Mode) -> Self {
        Dot {
            presence: Spring::new([0.0], mode),
            faded: 0.0,
            shown: false,
        }
    }

    fn opacity(&self, now: Instant) -> f32 {
        let goal = if self.shown { 1.0 } else { 0.0 };

        self.faded + (goal - self.faded) * self.presence.progress(now)
    }

    fn follow(&mut self, shown: bool, enter: Duration, leave: Duration, now: Instant) {
        if shown != self.shown {
            self.faded = self.opacity(now);
            self.shown = shown;
            self.presence.to(
                [f32::from(u8::from(shown))],
                if shown { enter } else { leave },
                now,
            );
        }
    }

    fn moving(&self, now: Instant) -> bool {
        !self.presence.settled(now)
    }
}

// the dots of one output
struct Fade([Dot; SENSORS]);

// by output: one covered by a fullscreen window fades out while another still shows
static FADE: Mutex<Option<HashMap<String, Fade>>> = Mutex::new(None);

// whether the settings ask for the indicators, which the Controls Surface follows too
pub fn on() -> bool {
    config::get().privacy_indicators
}

/*
 * the cluster as it shows at `now`, none once every dot has faded out or while it is off; reading
 * subscribes. Each dot fades in and out on its own, and so do all of a `covered` output's
 */
pub fn read(monitor: &str, covered: bool, now: Instant) -> Option<Cluster> {
    let active = if on() && !covered {
        active(&Privacy::read())
    } else {
        [false; SENSORS]
    };

    let config = config::get();
    let timings = config.animation_timings();
    let mut fades = FADE.locked();
    let Fade(dots) = fades
        .get_or_insert_with(HashMap::new)
        .entry(monitor.to_owned())
        .or_insert_with(|| Fade(std::array::from_fn(|_| Dot::new(timings.motion))));

    let mut shown = Vec::new();
    let mut moving = false;

    for ((dot, active), (_, mark, ink)) in dots.iter_mut().zip(active).zip(sensors()) {
        dot.follow(
            active,
            timings.for_motion(timings.expand),
            timings.for_motion(timings.collapse),
            now,
        );
        moving |= dot.moving(now);

        if dot.shown || dot.moving(now) {
            shown.push((mark, ink, dot.opacity(now)));
        }
    }

    (!shown.is_empty()).then_some(Cluster {
        dots: shown,
        moving,
    })
}

// how many sensors capture now, which sizes the Tray strip's end for their dots
pub fn count() -> u8 {
    if on() {
        active(&Privacy::read()).into_iter().map(u8::from).sum()
    } else {
        0
    }
}

// an output that left keeps no dots, so one replugged under its name starts out with none showing
pub fn forget_gone() {
    let monitors = Monitors::read();

    if let Some(fades) = FADE.locked().as_mut() {
        fades.retain(|monitor, _| monitors.has(monitor));
    }
}

// whether a body in this Presentation carries the cluster: the small forms, not a Tray or a Surface
pub fn beside(presentation: Presentation) -> bool {
    matches!(
        presentation,
        Presentation::Rest | Presentation::Compact | Presentation::Split | Presentation::Peek
    )
}

// whether the cluster stands at the body's end rather than over a Surface's top corner: a small form or a Tray strip
pub fn at_end(presentation: Presentation) -> bool {
    beside(presentation) || matches!(presentation, Presentation::Tray(_))
}

impl Cluster {
    // the most opaque dot, which the form's room to the dots follows
    fn presence(&self) -> f32 {
        self.dots
            .iter()
            .map(|&(_, _, opacity)| opacity)
            .fold(0.0, f32::max)
    }

    // each dot's room and its gap, as far as it shows, less the gap after the last
    fn length(&self) -> f32 {
        let shown: f32 = self.dots.iter().map(|&(_, _, opacity)| opacity).sum();

        ((DOT + DOT_GAP) * shown - DOT_GAP * self.presence()).max(0.0)
    }

    // how much of its trailing end the cluster takes from a small form, which lays out in the rest
    pub fn room(&self) -> f32 {
        (INSET + CLEAR) * self.presence() + self.length()
    }

    /*
     * the dots in a layer as large as the body, from its trailing end: across the top, centered on
     * a Peek's height there and rising to the corner as the body grows to Media, or along a side
     * edge down it from the end, centered across. A dot slides into its place as the ones after
     * it come and go
     */
    pub fn draw(self, body: Rect, sideways: bool) -> Rectangle {
        let mut after = 0.0;
        let mut dots: Vec<Box<dyn Widget>> = Vec::new();

        for &(mark, ink, opacity) in self.dots.iter().rev() {
            let at = FULL - DOT - after;
            let dot = Rectangle::new().width(DOT).height(DOT).opacity(opacity);
            let dot = match mark {
                Mark::Round => dot.radius(DOT / 2.0).fill(ink),
                Mark::Square => dot.radius(SQUARE_CORNER).fill(ink),
                Mark::Ring => dot.radius(DOT / 2.0).border(RING, ink),
            };

            dots.push(Box::new(if sideways {
                dot.translate(0.0, at)
            } else {
                dot.translate(at, 0.0)
            }));
            after += (DOT + DOT_GAP) * opacity;
        }

        let layer = Rectangle::new().width(body.width).height(body.height);

        if sideways {
            layer
                .padding(Padding {
                    bottom: INSET,
                    ..Padding::default()
                })
                .align_child(Center, End)
                .child(Stack::new(dots).width(DOT).height(FULL))
        } else {
            layer
                .padding(Padding {
                    right: INSET,
                    top: {
                        let grown = (body.height - PEEK.height) / (MEDIA.height - PEEK.height);
                        let centered = (body.height.min(PEEK.height) - DOT) / 2.0;

                        centered + (CORNER - centered) * grown.clamp(0.0, 1.0)
                    },
                    ..Padding::default()
                })
                .align_child(End, Start)
                .child(Stack::new(dots).width(FULL).height(DOT))
        }
    }
}

/*
 * what captures, as the Controls Surface draws it beside the apps: glyphs that differ by shape, not
 * color alone, green for a microphone or camera, amber for a screen cast
 */
pub fn glyphs(privacy: &Privacy) -> Vec<Box<dyn Widget>> {
    sensors()
        .into_iter()
        .zip(active(privacy))
        .filter(|(_, active)| *active)
        .map(|((icon, _, ink), _)| Box::new(icon.on(GLYPH, ink)) as Box<dyn Widget>)
        .collect()
}

// a microphone, a camera and a screen cast, each with its glyph, dot shape and color
fn sensors() -> [(Icon, Mark, Color); SENSORS] {
    let signals = theme::signals();

    [
        (Icon::Microphone, Mark::Round, signals.privacy),
        (Icon::Camera, Mark::Square, signals.privacy),
        (Icon::Capture, Mark::Ring, signals.capture),
    ]
}

// which of `sensors` capture now
fn active(privacy: &Privacy) -> [bool; SENSORS] {
    let sensors = privacy.sensors.as_ref();

    [
        sensors.is_some_and(|sensors| sensors.microphone),
        sensors.is_some_and(|sensors| sensors.camera),
        privacy.casting(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const FADE: Duration = Duration::from_millis(200);

    // an indicator reaches whole and leaves for good, whichever way it was interrupted
    #[test]
    fn a_dot_fades_in_whole_and_out_to_nothing() {
        let start = Instant::now();
        let later = |ms| start + Duration::from_millis(ms);
        let mut dot = Dot::new(Mode::Spring);

        dot.follow(true, FADE, FADE, start);
        assert!(dot.opacity(later(20)) < 1.0);
        assert!(!dot.moving(later(1000)));
        assert_eq!(dot.opacity(later(1000)), 1.0);

        // the capture ends and returns mid-fade: it fades back in from where it was
        dot.follow(false, FADE, FADE, later(1000));
        let halfway = dot.opacity(later(1060));
        dot.follow(true, FADE, FADE, later(1060));
        assert!((dot.opacity(later(1060)) - halfway).abs() < 0.01);
        assert_eq!(dot.opacity(later(3000)), 1.0);

        dot.follow(false, FADE, FADE, later(3000));
        assert!(!dot.moving(later(4000)));
        assert_eq!(dot.opacity(later(4000)), 0.0);
    }
}
