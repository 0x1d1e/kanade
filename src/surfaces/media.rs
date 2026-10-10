//! The Media Surface (plan 7): art, title and artist, timeline, prev/play/next, the speaker volume,
//! and which player it shows. Every part keeps its place in every state, so nothing moves when a
//! player opens, art loads, or a title runs long.

use std::ops::Range;
use std::time::{Duration, Instant};

use crate::sources::pulse::Audio;
use kanade_runtime::service::Service;
use kanade_runtime::{
    Center, Color, Column, Cursor, End, Padding, Rectangle, Row, Scroll, SpaceBetween, Stack,
    Start, Text, Widget, children, request_frame,
};

use super::slider::Slider;
use crate::icon::Icon;
use crate::island::activity::Track;
use crate::island::geometry;
use crate::modules;
use crate::sources::media::{self, Control};
use crate::sources::playback::{Choice, Deck, Playback};
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, DISABLED, radius};
use crate::view::{Change, bar, state};

// with INSET, concentric with the body's corner
const ART: f32 = 80.0;

const GAP: f32 = 14.0;

const CHIP: f32 = 24.0;
const CHIP_WIDTH: f32 = 110.0;
const CHIP_GAP: f32 = 6.0;

// as narrow as a chip gets and still reads a short name like "mpv 2"
const CHIP_MIN: f32 = 64.0;

const BUTTON: f32 = 36.0;
const PLAY: f32 = 44.0;

/*
 * `open` says the Surface is open rather than fading out, so only then does it ask the follower
 * to keep it fed
 */
pub fn surface(open: bool, now: Instant) -> Rectangle {
    if open {
        media::watch();
    }

    let playback = Playback::read();
    let shape = geometry::MEDIA;
    let width = shape.width - 2.0 * INSET;

    let deck = playback.shown.as_ref();
    let accent = deck.map_or(theme::island().on_surface, |deck| deck.accent.at(now));

    // the dissolve and the tint are the follower's, so the Surface asks for frames until they rest
    if deck.is_some_and(|deck| !deck.track.settled(now) || !deck.accent.settled(now)) {
        request_frame();
    }

    let nothing = Track {
        title: String::from("Nothing playing"),
        ..Track::default()
    };

    let track = deck.map_or(Change::of(&nothing, None, now), |deck| {
        Change::of(deck.track.target(), Some(&deck.track), now)
    });

    let surface = Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .child(
            Column::new(children![
                header(track, &playback.players, deck, playback.page, width, now),
                timeline(deck, accent, width, now),
                controls(deck, accent, width),
            ])
            .width(width)
            .gap(12.0),
        );

    // the wheel anywhere on it sets the volume, unless `audio` is off
    if modules::on("audio") {
        surface.on_scroll(|Scroll { y, .. }| Slider::Speaker.wheel(y))
    } else {
        surface
    }
}

// art, then title and artist, then the players to choose from
fn header(
    track: Change,
    players: &[Choice],
    deck: Option<&Deck>,
    page: Option<usize>,
    width: f32,
    now: Instant,
) -> Row {
    let words = width - ART - GAP;

    // the visualizer at the top right, the lines short of it
    const MOVING: f32 = 36.0;

    // the artist's line is there even when it names none, like the Peek's
    let lines = children![
        track
            .line(|track| &track.title, theme::island().on_surface)
            .size(theme::text::TITLE)
            .weight(theme::text::SEMIBOLD)
            .elide(),
        track
            .line(|track| &track.artist, theme::island().on_surface_variant)
            .size(theme::text::LABEL)
            .weight(theme::text::MEDIUM)
            .elide(),
    ];

    let shown = deck.map(|deck| deck.name.as_str());

    Row::new(children![
        track.art(ART, radius::ROW),
        Stack::new(children![
            Column::new(lines).width(words - MOVING).gap(3.0),
            Rectangle::new()
                .width(words)
                .height(ART)
                .align_child(End, Start)
                .padding(Padding {
                    top: 4.0,
                    right: 0.0,
                    bottom: 0.0,
                    left: 0.0,
                })
                .child(state(
                    deck.is_some_and(|deck| deck.track.target().playing),
                    now
                )),
            Rectangle::new()
                .width(words)
                .height(ART)
                .align_child(Start, End)
                .child(choices(players, shown, page, words)),
        ])
        .width(words)
        .height(ART),
    ])
    .gap(GAP)
}

/*
 * one player says its name; more each get a chip, the shown one filled, pressed to show that one
 * until the Surface closes. Past what fits, a page of them, first the one with the shown player,
 * and a "+N" chip that only turns to the next page
 */
fn choices(players: &[Choice], shown: Option<&str>, page: Option<usize>, width: f32) -> Row {
    if let [only] = players {
        return Row::new(children![
            Text::new(&only.identity)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::MEDIUM)
                .elide()
        ])
        .width(width);
    }

    let index = players
        .iter()
        .position(|player| shown == Some(player.name.as_str()));
    let chips = Chips::of(players.len(), index, page, width);

    let page = players[chips.page.clone()].iter().map(|player| {
        let selected = shown == Some(player.name.as_str());

        let select = Control::Select(player.name.clone());

        chip(&player.identity, chips.width, selected, select)
    });

    let more = chips.more.map(|next| {
        let hidden = players.len() - chips.page.len();

        chip(
            &format!("+{hidden}"),
            chips.width,
            false,
            Control::Page(next),
        )
    });

    Row::new(page.chain(more).collect()).gap(CHIP_GAP)
}

fn chip(label: &str, width: f32, selected: bool, control: Control) -> Box<dyn Widget> {
    let chip = Rectangle::new()
        .width(width)
        .height(CHIP)
        .radius(CHIP / 2.0)
        .padding(Padding {
            top: 0.0,
            right: 10.0,
            bottom: 0.0,
            left: 10.0,
        })
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(super::on_left(move || {
            media::control(control.clone());
        }))
        .child(
            Text::new(label)
                .size(theme::text::LABEL_SMALL)
                .color(if selected {
                    theme::island().on_surface
                } else {
                    theme::island().on_surface_variant
                })
                .weight(theme::text::SEMIBOLD)
                .elide(),
        );

    Box::new(if selected {
        chip.fill(theme::island().surface_container_high)
    } else {
        chip.border(1.0, theme::island().surface_container_high)
    })
}

// which player chips show, and how wide each is
#[derive(Debug, PartialEq)]
struct Chips {
    page: Range<usize>,

    // the next page, the last turning back to the first
    more: Option<usize>,

    width: f32,
}

impl Chips {
    // on `page` when turned to one, else on the page with the shown player
    fn of(count: usize, shown: Option<usize>, page: Option<usize>, width: f32) -> Chips {
        let slots = (((width + CHIP_GAP) / (CHIP_MIN + CHIP_GAP)) as usize).max(2);

        if count <= slots {
            return Chips {
                page: 0..count,
                more: None,
                width: chip_width(width, count),
            };
        }

        let per = slots - 1;
        let pages = count.div_ceil(per);

        // a page left behind by players closing is the last one there still is
        let page = page.unwrap_or(shown.unwrap_or(0) / per).min(pages - 1);
        let start = page * per;
        let end = (start + per).min(count);

        // every page as wide as the full ones, so turning the last one moves nothing
        Chips {
            page: start..end,
            more: Some((page + 1) % pages),
            width: chip_width(width, slots),
        }
    }
}

fn chip_width(width: f32, slots: usize) -> f32 {
    let slots = slots.max(1) as f32;

    ((width - CHIP_GAP * (slots - 1.0)) / slots).min(CHIP_WIDTH)
}

// how far along, with the time played and the time left; times only once the player says them
fn timeline(deck: Option<&Deck>, accent: Color, width: f32, now: Instant) -> Column {
    let timeline = deck.map(|deck| deck.timeline);

    let fraction = timeline.map_or(0.0, |timeline| timeline.fraction(now));
    let (played, left) = timeline.map_or((String::new(), String::new()), |timeline| {
        let position = timeline.position(now);

        if timeline.length.is_zero() {
            (clock(position), String::new())
        } else {
            let left = Duration::from_secs(timeline.length.as_secs() - position.as_secs());

            (clock(position), format!("-{}", clock(left)))
        }
    });

    let time = |text: String| {
        Text::new(text)
            .size(theme::text::LABEL_SMALL)
            .color(theme::island().on_surface_variant)
            .weight(theme::text::MEDIUM)
    };

    Column::new(children![
        bar(width, fraction, accent),
        Row::new(children![time(played), time(left)])
            .width(width)
            .height(16.0)
            .justify(SpaceBetween)
            .align(Center),
    ])
    .gap(6.0)
}

// 3:07, or 1:02:07 past an hour
fn clock(time: Duration) -> String {
    let seconds = time.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);

    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

// previous, play or pause, next; then the speaker, pressed to mute, and its level
fn controls(deck: Option<&Deck>, accent: Color, width: f32) -> Row {
    let able = |can: fn(&Deck) -> bool| deck.filter(|deck| can(deck)).map(|deck| deck.name.clone());

    let transport = Row::new(children![
        button(
            Transport::Previous,
            BUTTON,
            None,
            able(|deck| deck.can_previous).map(Control::Previous)
        ),
        {
            let (glyph, control) = play_pause(deck);

            button(glyph, PLAY, Some(theme::island().primary), control)
        },
        button(
            Transport::Next,
            BUTTON,
            None,
            able(|deck| deck.can_next).map(Control::Next)
        ),
    ])
    .gap(8.0)
    .align(Center);

    let gap = 24.0;
    let level = width - (2.0 * BUTTON + PLAY + 2.0 * 8.0) - gap - TARGET - 10.0;

    Row::new(children![transport, volume(level, accent)])
        .width(width)
        .gap(gap)
        .align(Center)
}

// a playing player pauses, any other plays; each only when the player says it can
fn play_pause(deck: Option<&Deck>) -> (Transport, Option<Control>) {
    match deck {
        Some(deck) if deck.track.target().playing => (
            Transport::Pause,
            deck.can_pause.then(|| Control::Pause(deck.name.clone())),
        ),
        deck => (
            Transport::Play,
            deck.filter(|deck| deck.can_play)
                .map(|deck| Control::Play(deck.name.clone())),
        ),
    }
}

/*
 * a round button; filled ones carry their glyph in the body's color. None to do leaves it faded,
 * at the same place, not pressable. Faded part by part, not as a whole: a faded group costs the
 * gpu a canvas of its own every frame (#36). The body-colored glyph on a fill stays solid, it
 * shows the body through either way
 */
fn button(glyph: Transport, side: f32, fill: Option<Color>, control: Option<Control>) -> Rectangle {
    let fade = |color: Color| match control {
        Some(_) => color,
        None => faded(color),
    };

    let tone = if fill.is_some() {
        theme::island().on_primary
    } else {
        fade(theme::island().on_surface)
    };

    let button = Rectangle::new()
        .width(side)
        .height(side)
        .radius(side / 2.0)
        .align_child(Center, Center)
        .child(glyph.draw(side * 0.5, tone));

    let button = match fill {
        Some(fill) => button.fill(fade(fill)),
        None => button,
    };

    match control {
        Some(control) => button
            .cursor(Cursor::Pointer)
            .on_click(super::on_left(move || {
                media::control(control.clone());
            })),
        None => button,
    }
}

// as if drawn at DISABLED opacity
fn faded(color: Color) -> Color {
    theme::faded(color, DISABLED)
}

/*
 * the speaker's icon and its level `width` wide, set by a press or a drag along it; with `audio`
 * off Audio is never read, and both are faded and empty
 */
fn volume(width: f32, accent: Color) -> Row {
    if !modules::on("audio") {
        return Row::new(children![
            Rectangle::new()
                .width(TARGET)
                .height(TARGET)
                .align_child(Center, Center)
                .opacity(DISABLED)
                .child(Icon::Speaker(0).draw(20.0)),
            Rectangle::new()
                .width(width)
                .height(TARGET)
                .align_child(Start, Center)
                .opacity(DISABLED)
                .child(bar(width, 0.0, accent)),
        ])
        .gap(10.0)
        .align(Center);
    }

    let audio = Audio::read();
    let (percent, muted) = (audio.volume(), audio.muted());
    drop(audio);

    let icon = if muted {
        Icon::SpeakerMuted
    } else {
        Icon::Speaker(percent)
    };

    let speaker = Rectangle::new()
        .width(TARGET)
        .height(TARGET)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(super::on_left(|| {
            Audio::toggle_mute();
        }))
        .child(icon.draw(20.0));

    let tone = if muted {
        theme::island().on_surface_variant
    } else {
        accent
    };

    let level = Slider::Speaker.bar(width, f32::from(percent) / 100.0, tone);

    Row::new(children![speaker, level]).gap(10.0).align(Center)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Transport {
    Previous,
    Play,
    Pause,
    Next,
}

impl Transport {
    // in `tone`, so it can sit on a filled button
    fn draw(self, side: f32, tone: Color) -> Rectangle {
        let icon = match self {
            Transport::Previous => Icon::Previous,
            Transport::Play => Icon::Play,
            Transport::Pause => Icon::Pause,
            Transport::Next => Icon::Next,
        };

        icon.on(side, tone)
    }
}
