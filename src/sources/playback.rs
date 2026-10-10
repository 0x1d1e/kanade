//! What the Media Surface draws (plan 7): every open player, and the one it shows with its
//! timeline. Written by `media::follow` only while the Surface is watched, so a closed Surface
//! costs nothing.

use std::time::{Duration, Instant};

use kanade_runtime::Color;
use kanade_runtime::service::Service;

use crate::island::activity::Track;
use crate::island::fade::Dissolve;
use crate::island::motion::{Mode, Spring};
use crate::theme::{self, channels, contrast, mix, saturation};

// the timeline and volume fills stand on the theme's dot, so an accent must stand out from it
const CONTRAST: f32 = 3.0;

// less colorful than this, artwork reads as grey and the Surface stays neutral
const VIVID: f32 = 0.2;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Playback {
    // every open player, in a stable order, for the player selection
    pub players: Vec<Choice>,

    // none while no player is open: "Nothing playing"
    pub shown: Option<Deck>,

    // the chip page turned to; none shows the page with the shown player
    pub page: Option<usize>,
}

// written only by `media::follow`, read by the Media Surface
impl Service for Playback {
    fn new() -> Self {
        Playback::default()
    }

    fn listen() {}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    // the bus name, which commands go to
    pub name: String,

    // like "Spotify"
    pub identity: String,
}

// the player the Surface shows, and what it can do
#[derive(Debug, Clone, PartialEq)]
pub struct Deck {
    pub name: String,

    // dissolving from the track shown before, of this player or the one chosen before it
    pub track: Dissolve<Track>,

    pub timeline: Timeline,

    // toward the artwork's accent, or the neutral foreground without art or for grey art
    pub accent: Tint,

    // MPRIS keeps these apart: a live stream may play but not pause
    pub can_play: bool,
    pub can_pause: bool,
    pub can_previous: bool,
    pub can_next: bool,
}

/*
 * the accent as it changes, on its own spring: with the track, and again whenever the artwork's
 * accent turns up late, since a file not there yet is read again on a later tick. A change of
 * mind goes on from where it is and how fast it moves, so it never jumps; under reduced motion it
 * fades from where it is over motion::REDUCED_FADE
 */
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tint {
    // what showed when the current leg began, for the reduced motion fade
    from: Color,

    to: Color,

    // 0 to 255 per channel
    rgb: Spring<3>,
}

impl Tint {
    // at rest on `color`
    pub fn new(color: Color, mode: Mode) -> Self {
        Self {
            from: color,
            to: color,
            rgb: Spring::new(rgb(color), mode),
        }
    }

    // a new leg toward `color` unless it already heads there
    pub fn to(&mut self, color: Color, response: Duration, now: Instant) {
        if self.to == color {
            return;
        }

        self.from = self.at(now);
        self.to = color;
        self.rgb.to(rgb(color), response, now);
    }

    pub fn at(&self, now: Instant) -> Color {
        if self.rgb.settled(now) {
            return self.to;
        }

        match self.rgb.mode() {
            Mode::Spring => {
                let [r, g, b] = self
                    .rgb
                    .at(now)
                    .map(|channel| channel.round().clamp(0.0, 255.0) as u8);

                Color::rgb(r, g, b)
            }
            Mode::Reduced => mix(self.from, self.to, self.rgb.progress(now)),
        }
    }

    pub fn settled(&self, now: Instant) -> bool {
        self.rgb.settled(now)
    }
}

fn rgb(color: Color) -> [f32; 3] {
    channels(color).map(|channel| channel * 255.0)
}

/*
 * where the player was when last asked, which the Surface moves on by itself between polls.
 * Equal when both say the same at any moment, so a paused timeline asked again changes nothing
 */
#[derive(Debug, Clone, Copy)]
pub struct Timeline {
    pub position: Duration,
    pub at: Instant,

    // zero when the player does not know, like a live stream
    pub length: Duration,

    // how fast the position moves, zero while paused
    pub rate: f64,
}

impl Timeline {
    pub fn position(&self, now: Instant) -> Duration {
        let moved = now
            .saturating_duration_since(self.at)
            .mul_f64(self.rate.max(0.0));
        let position = self.position + moved;

        if self.length.is_zero() {
            position
        } else {
            position.min(self.length)
        }
    }

    // when the whole second it shows next changes, none while it stands still
    pub fn next_second(&self, now: Instant) -> Option<Instant> {
        let position = self.position(now);

        if self.rate <= 0.0 || (!self.length.is_zero() && position >= self.length) {
            return None;
        }

        let left = Duration::from_secs(position.as_secs() + 1) - position;

        Some(now + left.div_f64(self.rate))
    }

    // how far along, 0 to 1; 0 without a length
    pub fn fraction(&self, now: Instant) -> f32 {
        if self.length.is_zero() {
            return 0.0;
        }

        (self.position(now).as_secs_f64() / self.length.as_secs_f64()) as f32
    }
}

impl PartialEq for Timeline {
    fn eq(&self, other: &Timeline) -> bool {
        let later = self.at.max(other.at);

        self.length == other.length
            && self.rate == other.rate
            && self.position(later) == other.position(later)
    }
}

// every open player by bus name; players of one kind, like two mpv, are told apart by a number
pub fn choices(open: impl IntoIterator<Item = (String, String)>) -> Vec<Choice> {
    let mut seen: Vec<String> = Vec::new();

    open.into_iter()
        .map(|(name, identity)| {
            let before = seen.iter().filter(|&seen| *seen == identity).count();
            seen.push(identity.clone());

            let identity = match before {
                0 => identity,
                _ => format!("{identity} {}", before + 1),
            };

            Choice { name, identity }
        })
        .collect()
}

/*
 * the most vivid of the artwork's colors, lightened until it stands out from the bar's track; none
 * when the artwork is grey, so the Surface keeps its neutral foreground (plan 7)
 */
pub fn accent(colors: &[Color]) -> Option<Color> {
    let vivid = colors
        .iter()
        .copied()
        .max_by(|a, b| saturation(*a).total_cmp(&saturation(*b)))
        .filter(|&color| saturation(color) >= VIVID)?;

    (0..=10)
        .map(|step| mix(vivid, theme::island().on_surface, step as f32 / 10.0))
        .find(|&color| contrast(color, theme::island().track()) >= CONTRAST)
}
