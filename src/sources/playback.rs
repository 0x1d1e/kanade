//! What the Media Surface draws: every open player, and the one it shows with its
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

    // the synced lyric line at the player's position, when `media.lyrics` found lyrics (ADR 0063)
    pub lyric: Option<String>,

    // toward the artwork's accent, or the neutral foreground without art or for grey art
    pub accent: Tint,

    // MPRIS keeps these apart: a live stream may play but not pause
    pub can_play: bool,
    pub can_pause: bool,
    pub can_previous: bool,
    pub can_next: bool,

    // SetPosition works: CanSeek, a track to name and a length to seek within
    pub can_seek: bool,
}

/*
 * the accent as it changes, on its own spring: with the track, and again whenever the artwork's
 * accent turns up late, since a file not there yet is read again on a later tick. A change of mind
 * goes on from where it is and how fast it moves, so it never jumps; under reduced motion it fades
 * from where it is over the configured reduced-motion response.
 *
 * It holds the artwork's raw color and how vivid it shows, and is fitted to the island's ramp only
 * when read, so a finish switched meanwhile recolors it with no new publish
 */
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tint {
    // what showed when the current leg began, for the reduced motion fade
    from: [f32; 4],

    to: [f32; 4],

    // red, green, blue 0 to 255, then 255 for the accent and 0 for the neutral foreground
    value: Spring<4>,
}

impl Tint {
    // at rest on `color`, the neutral foreground for none
    pub fn new(color: Option<Color>, mode: Mode) -> Self {
        let target = target(color, None);

        Self {
            from: target,
            to: target,
            value: Spring::new(target, mode),
        }
    }

    // a new leg toward `color` unless it already heads there
    pub fn to(&mut self, color: Option<Color>, response: Duration, now: Instant) {
        let target = target(color, Some(self.to));
        if self.to == target {
            return;
        }

        self.from = self.value_at(now);
        self.to = target;
        self.value.to(target, response, now);
    }

    // fitted to the island's ramp as it is now
    pub fn at(&self, now: Instant) -> Color {
        let [r, g, b, vivid] = self.value_at(now);
        let accent = fit(Some(Color::rgb(
            r.round().clamp(0.0, 255.0) as u8,
            g.round().clamp(0.0, 255.0) as u8,
            b.round().clamp(0.0, 255.0) as u8,
        )));

        mix(
            theme::island().on_surface,
            accent,
            (vivid / 255.0).clamp(0.0, 1.0),
        )
    }

    pub fn settled(&self, now: Instant) -> bool {
        self.value.settled(now)
    }

    fn value_at(&self, now: Instant) -> [f32; 4] {
        if self.value.settled(now) {
            return self.to;
        }

        match self.value.mode() {
            Mode::Spring => self.value.at(now),
            Mode::Reduced => {
                let progress = self.value.progress(now);

                std::array::from_fn(|i| self.from[i] + (self.to[i] - self.from[i]) * progress)
            }
        }
    }
}

// none keeps the color it had, so fading to neutral does not drift through another hue
fn target(color: Option<Color>, before: Option<[f32; 4]>) -> [f32; 4] {
    match (color, before) {
        (Some(color), _) => {
            let [r, g, b] = channels(color).map(|channel| channel * 255.0);

            [r, g, b, 255.0]
        }
        (None, Some([r, g, b, _])) => [r, g, b, 0.0],
        (None, None) => [128.0, 128.0, 128.0, 0.0],
    }
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

/*
 * how long `left` of the track lasts at `rate`, so far off that an Instant cannot hold it: a rate
 * is the player's own, and one near zero would overflow the sum, which panics. Such a tick is none
 * rather than a panic: the caller has a nearer one
 */
pub fn until(now: Instant, left: Duration, rate: f64) -> Option<Instant> {
    // a NaN, infinite or non-positive rate has no wait it could mean
    if !rate.is_finite() || rate <= 0.0 {
        return None;
    }

    let after = Duration::try_from_secs_f64(left.as_secs_f64() / rate).ok()?;

    now.checked_add(after)
}

// `length` of the track at `rate`, or what a Duration holds when that is past it
fn moved(length: Duration, rate: f64) -> Duration {
    Duration::try_from_secs_f64(length.as_secs_f64() * rate.max(0.0)).unwrap_or(Duration::MAX)
}

impl Timeline {
    pub fn position(&self, now: Instant) -> Duration {
        let moved = moved(now.saturating_duration_since(self.at), self.rate);
        let position = self.position.saturating_add(moved);

        if self.length.is_zero() {
            position
        } else {
            position.min(self.length)
        }
    }

    // `seconds` on from where it is now, or back when negative, kept within the track
    pub fn shifted(&self, seconds: i64, now: Instant) -> Duration {
        let position = self.position(now);
        let by = Duration::from_secs(seconds.unsigned_abs());
        let to = if seconds < 0 {
            position.saturating_sub(by)
        } else {
            position.saturating_add(by)
        };

        if self.length.is_zero() {
            to
        } else {
            to.min(self.length)
        }
    }

    // when the whole second it shows next changes, none while it stands still
    pub fn next_second(&self, now: Instant) -> Option<Instant> {
        let position = self.position(now);

        if self.rate <= 0.0 || (!self.length.is_zero() && position >= self.length) {
            return None;
        }

        // a rate the player reports as enormous puts the position at a Duration's end, where the
        // second after it is not a span this can subtract
        let next = Duration::from_secs(position.as_secs().checked_add(1)?);
        let left = next.checked_sub(position)?;

        until(now, left, self.rate)
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

// the most vivid of the artwork's colors; none when the artwork is grey, so the Surface keeps its
// neutral foreground
pub fn accent(colors: &[Color]) -> Option<Color> {
    colors
        .iter()
        .copied()
        .max_by(|a, b| saturation(*a).total_cmp(&saturation(*b)))
        .filter(|&color| saturation(color) >= VIVID)
}

// an accent lightened until it stands out from the island's bar track, as the finish has it now;
// the neutral foreground for none, or for one no lightening gets there
pub fn fit(accent: Option<Color>) -> Color {
    let island = theme::island();

    accent
        .and_then(|vivid| {
            (0..=10)
                .map(|step| mix(vivid, island.on_surface, step as f32 / 10.0))
                .find(|&color| contrast(color, island.track()) >= CONTRAST)
        })
        .unwrap_or(island.on_surface)
}

#[cfg(test)]
mod tests {
    use super::*;

    // a player reports its own rate, and one near zero, a NaN or an enormous one would panic the
    // tick it asks for: it is none then, so the caller polls instead
    #[test]
    fn a_rate_no_wait_could_mean_asks_for_no_tick() {
        let now = Instant::now();
        let timeline = |rate: f64| Timeline {
            position: Duration::ZERO,
            at: now,
            length: Duration::ZERO,
            rate,
        };

        // a second of the track at these rates is further off than a Duration holds
        for rate in [1e-20, 1e-300, f64::MIN_POSITIVE] {
            assert_eq!(timeline(rate).next_second(now), None, "{rate}");
            assert_eq!(until(now, Duration::from_secs(1), rate), None, "{rate}");
        }

        // and no rate at all is no wait either, nor a tick that an Instant cannot reach
        for rate in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(timeline(rate).next_second(now), None, "{rate}");
            assert_eq!(until(now, Duration::from_secs(1), rate), None, "{rate}");
        }

        assert_eq!(until(now, Duration::from_secs(1), 1e300), Some(now));

        // the position moves on without panicking too, and a live stream has no length to stop it
        assert_eq!(
            timeline(1e300).position(now + Duration::from_secs(1)),
            Duration::MAX
        );

        // a position at a Duration's end has no next second to wait for
        assert_eq!(
            timeline(1e300).next_second(now + Duration::from_secs(1)),
            None
        );
        assert_eq!(
            timeline(1e-9).position(now + Duration::from_secs(3600)),
            Duration::try_from_secs_f64(3600.0 * 1e-9).unwrap()
        );

        // and a normal rate still says when the next second is
        assert_eq!(
            timeline(1.0).next_second(now),
            Some(now + Duration::from_secs(1))
        );
    }

    // a seek by key never leaves the track: before its start, or past its end
    #[test]
    fn a_shift_stays_within_the_track() {
        let now = Instant::now();
        let timeline = Timeline {
            position: Duration::from_secs(3),
            at: now,
            length: Duration::from_secs(10),
            rate: 0.0,
        };

        assert_eq!(timeline.shifted(-5, now), Duration::ZERO);
        assert_eq!(timeline.shifted(5, now), Duration::from_secs(8));
        assert_eq!(timeline.shifted(50, now), Duration::from_secs(10));
        assert_eq!(timeline.shifted(i64::MIN, now), Duration::ZERO);
    }
}
