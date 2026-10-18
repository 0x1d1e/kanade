//! The Media forms, and the tiles, art and visualizer other forms borrow.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use kanade_runtime::{
    Center, Color, Column, End, Image, Padding, Parent, Rectangle, Row, Stack, Start, Text, Widget,
    children, request_frame,
};

use crate::config;
use crate::island::activity::Track;
use crate::island::fade::{Dissolve, InPlace, swap};
use crate::island::motion::Mode;
use crate::island::presentation::Presentation;
use crate::look::Visualizer;
use crate::sources::{cava, playback};
use crate::theme::{self, ThemeRoles};

use super::forms::sized;
use super::shape::{shape, upright};
use super::standing::{stacked, stacked_tile};

/*
 * art, title, then whether it plays. The art is inset evenly top and bottom and further from the
 * body's round end, which would crowd its corner; the state mark keeps clear of the other end
 */
pub(super) fn media_compact(change: Change, now: Instant) -> Rectangle {
    if upright() {
        return stacked(children![
            change.art(stacked_tile(), theme::radius::ART_COMPACT),
            state(change.to.playing, accent(change.to), now),
        ]);
    }

    let shape = shape(Presentation::Compact);
    let inset = 7.0;
    let lead = 11.0;

    sized(Presentation::Compact)
        .padding(Padding {
            top: 0.0,
            right: 15.0,
            bottom: 0.0,
            left: lead,
        })
        .align_child(Start, Center)
        .child(
            Row::new(children![
                change.art(shape.height - 2.0 * inset, theme::radius::ART_COMPACT),
                change
                    .line(|track| &track.title, theme::island().on_surface)
                    .size(theme::text::LABEL)
                    .weight(theme::text::MEDIUM)
                    .elide(),
                state(change.to.playing, accent(change.to), now),
            ])
            .width(Parent)
            .gap(9.0)
            .align(Center),
        )
}

// the Compact with room for the artist under the title
pub(super) fn media_peek(change: Change, now: Instant) -> Rectangle {
    let shape = shape(Presentation::Peek);
    let inset = 7.0;
    let lead = 13.0;

    sized(Presentation::Peek)
        .padding(Padding {
            top: 0.0,
            right: 20.0,
            bottom: 0.0,
            left: lead,
        })
        .align_child(Start, Center)
        .child(
            Row::new(children![
                change.art(shape.height - 2.0 * inset, theme::radius::ART_PEEK),
                peek_lines(change),
                state(change.to.playing, accent(change.to), now),
            ])
            .width(Parent)
            .gap(11.0)
            .align(Center),
        )
}

/*
 * the title over the artist; the artist's line is there even when it names none, so a track
 * without one stands where the one before it did and nothing moves
 */
fn peek_lines(change: Change) -> Column {
    Column::new(children![
        change
            .line(|track| &track.title, theme::island().on_surface)
            .size(theme::text::BODY)
            .weight(theme::text::SEMIBOLD)
            .elide(),
        change
            .line(|track| &track.artist, theme::island().on_surface_variant)
            .size(theme::text::LABEL_SMALL)
            .weight(theme::text::MEDIUM)
            .elide(),
    ])
    .width(Parent)
    .gap(1.0)
}

/*
 * a track as it dissolves where it stands: the one it replaces still beneath, and how far
 * the new one has risen over it. Only a dissolve to the track drawn counts, since a Media Activity
 * that fades out whole has a Crossfade that moved on from it
 */
#[derive(Clone, Copy)]
pub(crate) struct Change<'a> {
    from: Option<&'a Track>,
    to: &'a Track,
    rise: f32,
}

impl<'a> Change<'a> {
    pub(crate) fn of(to: &'a Track, dissolve: Option<&'a Dissolve<Track>>, now: Instant) -> Self {
        match dissolve.filter(|dissolve| dissolve.target().in_place(to)) {
            Some(dissolve) => Self {
                from: dissolve.from(now),
                to,
                rise: dissolve.rise(now),
            },
            None => Self {
                from: None,
                to,
                rise: 1.0,
            },
        }
    }

    // the new cover rises over the old, so no tile shows between them and nothing dips
    pub(crate) fn art(self, side: f32, radius: f32) -> Stack {
        let to = Rectangle::new()
            .width(side)
            .height(side)
            .opacity(self.rise)
            .child(art(self.to, side, radius));

        let layers = match self.from {
            Some(from) => children![art(from, side, radius), to],
            None => children![to],
        };

        Stack::new(layers).width(side).height(side)
    }

    // what one line shows and how strongly; a line both tracks share stays as it is
    fn text(self, line: fn(&Track) -> &str) -> (&'a str, f32) {
        let from = self.from.map(line).filter(|&from| from != line(self.to));

        swap(from, line(self.to), self.rise)
    }

    // one line of text, the old fading out before the new fades in at the same place
    pub(crate) fn line(self, line: fn(&Track) -> &str, color: Color) -> Text {
        let (text, opacity) = self.text(line);

        Text::new(text).color(theme::faded(color, opacity))
    }
}

/*
 * the cover over a quiet tile with a note, so the tile shows while it decodes, when it never
 * will, and for web art, all at the same size
 */
pub(crate) fn art(track: &Track, side: f32, radius: f32) -> Stack {
    tile(
        track.art.as_deref(),
        "\u{266a}",
        side,
        radius,
        &theme::island(),
    )
}

// a picture over a quiet tile with a mark, like `art`
pub(crate) fn tile(
    picture: Option<&str>,
    mark: &str,
    side: f32,
    radius: f32,
    roles: &ThemeRoles,
) -> Stack {
    let tile = Rectangle::new()
        .width(side)
        .height(side)
        .radius(radius)
        .fill(roles.surface_container_high)
        .align_child(Center, Center)
        .child(
            Text::new(mark)
                .size(side * 0.5)
                .color(roles.on_surface_variant),
        );

    let mut layers = children![tile];

    if let Some(path) = picture {
        // decoded at twice its size, crisp at scale 2, and the cache keeps no full-size covers
        let pixels = (side * 2.0) as u32;

        layers.push(Box::new(
            Rectangle::new()
                .width(side)
                .height(side)
                .radius(radius)
                .fill(Image::cover(path).thumbnail(pixels, pixels)),
        ));
    }

    Stack::new(layers).width(side).height(side)
}

// seconds a playing track's disc takes to turn once, slow as a record
const TURN: f32 = 8.0;

// the longest step the disc turns by between two draws, so one after a long rest does not spin on
const STEP: f32 = 0.1;

// where the disc stands, in degrees, and when it last turned
static DISC: Mutex<Option<(Instant, f32)>> = Mutex::new(None);

/*
 * the cover as a disc in a Satellite: it turns clockwise while the track plays and keeps its angle
 * while paused, so pausing never snaps it back. Reduced motion holds it still
 */
pub(crate) fn disc(track: &Track, side: f32, now: Instant) -> Rectangle {
    let mut disc = DISC.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (last, angle) = disc.unwrap_or((now, 0.0));

    let angle = if track.playing && config::get().island.motion != Mode::Reduced {
        request_frame();

        let step = now.saturating_duration_since(last).as_secs_f32().min(STEP);

        (angle + step * 360.0 / TURN) % 360.0
    } else {
        angle
    };

    *disc = Some((now.max(last), angle));

    Rectangle::new()
        .width(side)
        .height(side)
        .rotate(angle)
        .child(art(track, side, side / 2.0))
}

// the track's accent fitted to the island, or the neutral foreground for grey art and none
pub(crate) fn accent(track: &Track) -> Color {
    playback::fit(track.accent.map(|(r, g, b)| Color::rgb(r, g, b)))
}

// how tall the visualizer's row is
const TALL: f32 = 14.0;

/*
 * what moves beside a playing track, as `media.visualizer` says, in `ink`, the artwork's accent:
 * when cava is available and gives audio spectrum data, the bars, wave, and dots follow real audio
 * frequencies; otherwise, or while cava is initializing/silent, they fall back to the synthetic
 * swing animation so motion never freezes. `off` draws still bars, and a paused track draws the pause
 * mark.
 */
pub(crate) fn state(playing: bool, ink: Color, now: Instant) -> Row {
    let style = config::get().visualizer;
    if !playing || style == Visualizer::Off {
        let bar = |height: f32| mark(3.0, height, theme::island().on_surface, 1.0);
        let bars = if playing {
            children![bar(8.0), bar(13.0), bar(10.0)]
        } else {
            children![bar(11.0), bar(11.0)]
        };

        return Row::new(bars).height(TALL).gap(2.0).align(End);
    }

    cava::want();

    if let Some(bars) = cava::bars() {
        return match style {
            Visualizer::Bars | Visualizer::Audio => audio_bars(bars, ink),
            Visualizer::Wave => audio_wave(bars, ink),
            Visualizer::Dots => audio_dots(bars, ink),
            Visualizer::Off => unreachable!("handled above"),
        };
    }

    let style = if style == Visualizer::Audio {
        Visualizer::Bars
    } else {
        style
    };

    request_frame();

    let time = now
        .duration_since(*VISUALIZER_START.get_or_init(|| now))
        .as_secs_f32();
    let swing = |speed: f32, phase: f32| 0.5 + 0.5 * (time * speed + phase).sin();

    let (marks, gap): (Vec<Box<dyn Widget>>, f32) = match style {
        // each bar at its own pace, so they never move together
        Visualizer::Bars => (
            [
                (7.1, 0.0, 5.3),
                (9.3, 1.7, 4.1),
                (6.2, 3.1, 6.7),
                (8.4, 4.4, 3.9),
            ]
            .into_iter()
            .map(|(speed, phase, slow)| {
                let height =
                    3.0 + (TALL - 3.0) * swing(speed, phase) * (0.55 + 0.45 * swing(slow, phase));

                Box::new(mark(3.0, height, ink, 1.0)) as Box<dyn Widget>
            })
            .collect(),
            2.0,
        ),

        // a sine running through thin bars
        Visualizer::Wave => (
            (0..7)
                .map(|at| {
                    let height = 3.0 + (TALL - 5.0) * swing(6.0, -0.9 * at as f32);

                    Box::new(mark(2.0, height, ink, 1.0)) as Box<dyn Widget>
                })
                .collect(),
            1.5,
        ),

        // three dots swelling one after another
        Visualizer::Dots => (
            (0..3)
                .map(|at| {
                    let swell = swing(5.0, -1.2 * at as f32);
                    let side = 4.0 + 2.0 * swell;

                    Box::new(mark(side, side, ink, 0.35 + 0.65 * swell)) as Box<dyn Widget>
                })
                .collect(),
            3.0,
        ),
        Visualizer::Audio | Visualizer::Off => unreachable!("drawn still or as bars above"),
    };

    Row::new(marks).height(TALL).gap(gap).align(Center)
}

// the 4 frequency bars cava gave, each up to `TALL`
fn audio_bars(bars: [u8; cava::BARS], ink: Color) -> Row {
    let marks: Vec<Box<dyn Widget>> = bars
        .into_iter()
        .map(|bar| {
            let height = 3.0 + (TALL - 3.0) * f32::from(bar) / f32::from(cava::PEAK);

            Box::new(mark(3.0, height, ink, 1.0)) as Box<dyn Widget>
        })
        .collect();

    Row::new(marks).height(TALL).gap(2.0).align(Center)
}

// 7 wave bars interpolated from cava's frequency bins
fn audio_wave(bars: [u8; cava::BARS], ink: Color) -> Row {
    let norm = |b: u8| f32::from(b) / f32::from(cava::PEAK);
    let marks: Vec<Box<dyn Widget>> = (0..7)
        .map(|at| {
            let pos = at as f32 * 0.5;
            let idx = pos.floor() as usize;
            let frac = pos.fract();
            let val = if idx >= 3 {
                norm(bars[3])
            } else {
                norm(bars[idx]) * (1.0 - frac) + norm(bars[idx + 1]) * frac
            };
            let height = 3.0 + (TALL - 5.0) * val;

            Box::new(mark(2.0, height, ink, 1.0)) as Box<dyn Widget>
        })
        .collect();

    Row::new(marks).height(TALL).gap(1.5).align(Center)
}

// 3 dots scaled and faded by bass, mids, and treble
fn audio_dots(bars: [u8; cava::BARS], ink: Color) -> Row {
    let norm = |b: u8| f32::from(b) / f32::from(cava::PEAK);
    let bass = norm(bars[0]);
    let mids = 0.5 * (norm(bars[1]) + norm(bars[2]));
    let treble = norm(bars[3]);

    let marks: Vec<Box<dyn Widget>> = [bass, mids, treble]
        .into_iter()
        .map(|val| {
            let side = 4.0 + 2.0 * val;
            let opacity = 0.35 + 0.65 * val;

            Box::new(mark(side, side, ink, opacity)) as Box<dyn Widget>
        })
        .collect();

    Row::new(marks).height(TALL).gap(3.0).align(Center)
}

// when the first moving visualizer drew, so its motion runs on from there
static VISUALIZER_START: OnceLock<Instant> = OnceLock::new();

// a rounded bar or dot of the visualizer
pub(super) fn mark(width: f32, height: f32, ink: Color, opacity: f32) -> Rectangle {
    Rectangle::new()
        .width(width)
        .height(height)
        .radius(width.min(height) / 2.0)
        .fill(ink)
        .opacity(opacity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kanade_runtime::service::Service;
    use kanade_runtime::{Size, Widget};

    #[test]
    fn audio_bars_renders_four_bars_sized_by_audio() {
        let row = audio_bars([0, 50, 100, 25], Color::rgb(255, 255, 255));
        assert_eq!(Widget::height(&row), Size::Fixed(TALL));
    }

    #[test]
    fn audio_wave_renders_seven_interpolated_bars() {
        let row = audio_wave([0, 50, 100, 25], Color::rgb(255, 255, 255));
        assert_eq!(Widget::height(&row), Size::Fixed(TALL));
    }

    #[test]
    fn audio_dots_renders_three_dots_for_bass_mids_treble() {
        let row = audio_dots([0, 50, 100, 25], Color::rgb(255, 255, 255));
        assert_eq!(Widget::height(&row), Size::Fixed(TALL));
    }

    #[test]
    fn state_paused_renders_pause_mark() {
        let row = state(false, Color::rgb(255, 255, 255), Instant::now());
        assert_eq!(Widget::height(&row), Size::Fixed(TALL));
    }

    #[test]
    fn state_fallback_synthetic_when_cava_silent() {
        cava::Spectrum::write().bars = None;
        let row = state(true, Color::rgb(255, 255, 255), Instant::now());
        assert_eq!(Widget::height(&row), Size::Fixed(TALL));
    }

    #[test]
    fn state_audio_reactive_when_cava_provides_spectrum() {
        cava::Spectrum::write().bars = Some([10, 40, 70, 100]);
        let row = state(true, Color::rgb(255, 255, 255), Instant::now());
        assert_eq!(Widget::height(&row), Size::Fixed(TALL));
        cava::Spectrum::write().bars = None;
    }
}
