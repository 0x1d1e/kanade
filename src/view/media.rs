//! The Media forms, and the tiles, art and visualizer other forms borrow.

use std::sync::OnceLock;
use std::time::Instant;

use kanade_runtime::service::Service;
use kanade_runtime::{
    Center, Color, Column, End, Image, Padding, Parent, Rectangle, Row, Stack, Start, Text, Widget,
    children, request_frame,
};

use crate::config;
use crate::island::activity::Track;
use crate::island::fade::{Dissolve, InPlace, swap};
use crate::island::presentation::Presentation;
use crate::look::Visualizer;
use crate::sources::playback::Playback;
use crate::theme::{self, ThemeRoles};

use super::forms::{sized, stacked, stacked_tile};
use super::shape::{shape, upright};

/*
 * art, title, then whether it plays. The art's inset matches top, left and bottom, so it sits
 * concentric with the body's round end; the state mark keeps clear of the other end
 */
pub(super) fn media_compact(change: Change, now: Instant) -> Rectangle {
    if upright() {
        return stacked(children![
            change.art(stacked_tile(), theme::radius::ART_COMPACT),
            state(change.to.playing, now),
        ]);
    }

    let shape = shape(Presentation::Compact);
    let inset = 7.0;

    sized(Presentation::Compact)
        .padding(Padding {
            top: 0.0,
            right: 15.0,
            bottom: 0.0,
            left: inset,
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
                state(change.to.playing, now),
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

    sized(Presentation::Peek)
        .padding(Padding {
            top: 0.0,
            right: 20.0,
            bottom: 0.0,
            left: inset,
        })
        .align_child(Start, Center)
        .child(
            Row::new(children![
                change.art(shape.height - 2.0 * inset, theme::radius::ART_PEEK),
                peek_lines(change),
                state(change.to.playing, now),
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
 * a track as it dissolves where it stands (#37): the one it replaces still beneath, and how far
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

/*
 * what moves beside a playing track, as `media.visualizer` says, in the artwork's accent: the
 * motion is drawn, not heard, as the Dynamic Island's. A pause mark while it does not play, and
 * three still bars with `off`, so only a moving one draws frames
 */
pub(crate) fn state(playing: bool, now: Instant) -> Row {
    const TALL: f32 = 14.0;

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

    request_frame();

    let ink = Playback::read()
        .shown
        .as_ref()
        .map_or(theme::island().on_surface, |deck| deck.accent.at(now));
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
        Visualizer::Off => unreachable!("drawn still above"),
    };

    Row::new(marks).height(TALL).gap(gap).align(Center)
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
