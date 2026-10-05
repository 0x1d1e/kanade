//! What the Media Surface draws (plan 7): every open player, and the one it shows with its
//! timeline. Written by `media::follow` only while the Surface is watched, so a closed Surface
//! costs nothing.

use std::time::{Duration, Instant};

use amane::{Color, Service};

use crate::island::activity::Track;
use crate::island::fade::Dissolve;
use crate::island::motion::{Mode, Spring};
use crate::theme;

// the timeline and volume fills stand on theme::DOT, so an accent must stand out from it
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
        .map(|step| mix(vivid, theme::FG, step as f32 / 10.0))
        .find(|&color| contrast(color, theme::DOT) >= CONTRAST)
}

// chroma, how far from grey; unlike HSL saturation a near-black red does not count as vivid
fn saturation(color: Color) -> f32 {
    let [r, g, b] = channels(color);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));

    max - min
}

fn mix(from: Color, to: Color, amount: f32) -> Color {
    let [from, to] = [channels(from), channels(to)];
    let channel = |index: usize| {
        let value = from[index] + (to[index] - from[index]) * amount;

        (value * 255.0).round() as u8
    };

    Color::rgb(channel(0), channel(1), channel(2))
}

// WCAG contrast ratio, 1 to 21
fn contrast(a: Color, b: Color) -> f32 {
    let (a, b) = (luminance(a), luminance(b));

    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn luminance(color: Color) -> f32 {
    let linear = |channel: f32| {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = channels(color).map(linear);

    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn channels(color: Color) -> [f32; 3] {
    [color.red(), color.green(), color.blue()].map(|channel| f32::from(channel) / 255.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::motion::REDUCED_FADE;

    fn timeline(position: u64, rate: f64, at: Instant) -> Timeline {
        Timeline {
            position: Duration::from_secs(position),
            at,
            length: Duration::from_secs(180),
            rate,
        }
    }

    const RED: Color = Color::rgb(255, 0, 0);
    const BLUE: Color = Color::rgb(0, 0, 255);
    const TINT: Duration = Duration::from_millis(300);

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    // the most any channel moves in one millisecond from `from` until it rests
    fn steepest(tint: &Tint, from: Instant) -> u8 {
        (0..2_000)
            .map(|step| {
                let [a, b] = [step, step + 1].map(|step| channels(tint.at(from + ms(step))));

                (0..3)
                    .map(|i| ((a[i] - b[i]).abs() * 255.0).round() as u8)
                    .max()
                    .unwrap()
            })
            .max()
            .unwrap()
    }

    #[test]
    fn the_tint_moves_to_the_new_accent_and_rests_there() {
        let now = Instant::now();
        let mut tint = Tint::new(RED, Mode::Spring);
        assert!(tint.settled(now));

        tint.to(BLUE, TINT, now);
        assert_eq!(tint.at(now), RED);

        let mid = tint.at(now + ms(60));
        assert!(mid != RED && mid != BLUE, "{mid:?}");
        assert!(steepest(&tint, now) <= 8);

        assert_eq!(tint.at(now + ms(2_000)), BLUE);
        assert!(tint.settled(now + ms(2_000)));
    }

    // #37: the artwork not readable yet heads for the neutral foreground; its accent turning up a
    // tick later turns the tint from where it is, mid-leg or at rest
    #[test]
    fn an_accent_found_late_goes_on_from_where_the_tint_is() {
        let now = Instant::now();

        for found in [ms(60), ms(2_000)] {
            let mut tint = Tint::new(RED, Mode::Spring);
            tint.to(theme::FG, TINT, now);

            let late = now + found;
            let before = tint.at(late);
            tint.to(BLUE, TINT, late);

            assert_eq!(tint.at(late), before, "found after {found:?}");
            assert!(steepest(&tint, now) <= 8, "found after {found:?}");
            assert_eq!(tint.at(late + ms(2_000)), BLUE);
        }
    }

    #[test]
    fn the_same_accent_again_keeps_the_leg() {
        let now = Instant::now();
        let mut tint = Tint::new(RED, Mode::Spring);
        tint.to(BLUE, TINT, now);

        let leg = tint;
        tint.to(BLUE, TINT, now + ms(60));

        assert_eq!(tint, leg);
    }

    #[test]
    fn under_reduced_motion_the_tint_fades_from_where_it_is() {
        let now = Instant::now();
        let mut tint = Tint::new(RED, Mode::Reduced);
        tint.to(theme::FG, TINT, now);

        let late = now + ms(40);
        let before = tint.at(late);
        assert!(before != RED && before != theme::FG, "{before:?}");

        tint.to(BLUE, TINT, late);
        assert_eq!(tint.at(late), before);
        assert_eq!(tint.at(late + REDUCED_FADE), BLUE);
        assert!(tint.settled(late + REDUCED_FADE));
    }

    #[test]
    fn a_playing_timeline_moves_on_and_stops_at_the_end() {
        let now = Instant::now();
        let playing = timeline(10, 1.0, now);

        assert_eq!(
            playing.position(now + Duration::from_millis(2500)),
            Duration::from_millis(12_500)
        );
        assert_eq!(
            playing.position(now + Duration::from_secs(600)),
            Duration::from_secs(180)
        );
        assert_eq!(playing.fraction(now + Duration::from_secs(80)), 0.5);
    }

    #[test]
    fn a_paused_timeline_stands_still_and_never_ticks() {
        let now = Instant::now();
        let paused = timeline(10, 0.0, now);

        assert_eq!(
            paused.position(now + Duration::from_secs(5)),
            Duration::from_secs(10)
        );
        assert_eq!(paused.next_second(now), None);

        // asked again later it says the same, so nothing is written
        assert_eq!(paused, timeline(10, 0.0, now + Duration::from_secs(1)));
    }

    #[test]
    fn the_next_tick_is_when_the_shown_second_changes() {
        let now = Instant::now();
        let mut playing = timeline(10, 1.0, now);
        playing.position += Duration::from_millis(300);

        assert_eq!(
            playing.next_second(now),
            Some(now + Duration::from_millis(700))
        );

        // at double speed the second goes by in half the time
        playing.rate = 2.0;
        assert_eq!(
            playing.next_second(now),
            Some(now + Duration::from_millis(350))
        );

        // at the end there is nothing left to tick
        let ended = timeline(180, 1.0, now);
        assert_eq!(ended.next_second(now), None);
    }

    #[test]
    fn a_playing_timeline_asked_again_is_the_same() {
        let now = Instant::now();
        let later = now + Duration::from_secs(1);

        assert_eq!(timeline(10, 1.0, now), timeline(11, 1.0, later));
        assert_ne!(timeline(10, 1.0, now), timeline(15, 1.0, later));
    }

    #[test]
    fn players_of_one_kind_are_numbered() {
        let open = [("a", "mpv"), ("b", "Spotify"), ("c", "mpv")]
            .map(|(name, identity)| (name.to_owned(), identity.to_owned()));
        let identities: Vec<String> = choices(open)
            .into_iter()
            .map(|choice| choice.identity)
            .collect();

        assert_eq!(identities, ["mpv", "Spotify", "mpv 2"]);
    }

    #[test]
    fn grey_art_has_no_accent() {
        assert_eq!(accent(&[]), None);
        assert_eq!(
            accent(&[Color::rgb(30, 30, 30), Color::rgb(200, 200, 205)]),
            None
        );
    }

    #[test]
    fn the_accent_is_the_most_vivid_color_made_to_stand_out() {
        let orange = Color::rgb(250, 150, 40);

        assert_eq!(accent(&[Color::rgb(90, 80, 80), orange]), Some(orange));

        // a deep blue is too dark on the track, so it lightens until it reads, still blue
        let blue = accent(&[Color::rgb(20, 20, 140)]).unwrap();
        assert!(contrast(blue, theme::DOT) >= CONTRAST);
        assert!(blue.blue() > blue.red());
    }
}
