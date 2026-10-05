//! The Media Surface (plan 7): art, title and artist, timeline, prev/play/next, the speaker volume,
//! and which player it shows. Every part keeps its place in every state, so nothing moves when a
//! player opens, art loads, or a title runs long.

use std::ops::Range;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use amane::{
    Audio, Button, Canvas, Center, Color, Column, Cursor, End, Padding, Path, Rectangle, Row,
    Scroll, Service, Shape as _, SpaceBetween, Stack, Start, Text, Widget, children, shapes,
};

use crate::island::activity::Track;
use crate::island::geometry;
use crate::sources::media::{self, Control};
use crate::sources::playback::{Choice, Deck, Playback};
use crate::theme;
use crate::view::{Icon, art, bar};

const INSET: f32 = 20.0;

// with INSET, concentric with the body's corner
const ART: f32 = 80.0;
const ART_RADIUS: f32 = 12.0;

const GAP: f32 = 14.0;

// clear of the queued badge in the body's top right corner
const BADGE: f32 = 36.0;

const CHIP: f32 = 24.0;
const CHIP_WIDTH: f32 = 110.0;
const CHIP_GAP: f32 = 6.0;

// as narrow as a chip gets and still reads a short name like "mpv 2"
const CHIP_MIN: f32 = 64.0;

const BUTTON: f32 = 36.0;
const PLAY: f32 = 44.0;

// a pressable target never smaller than plan 7's 24 px
const TARGET: f32 = 24.0;

// a control the player cannot do now
const DISABLED: f32 = 0.35;

// percent per wheel line
const WHEEL: f32 = 5.0;

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
    let accent = deck.and_then(|deck| deck.accent).unwrap_or(theme::FG);

    let nothing = Track {
        title: String::from("Nothing playing"),
        ..Track::default()
    };

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .on_scroll(|Scroll { y, .. }| wheel(y))
        .child(
            Column::new(children![
                header(
                    deck.map_or(&nothing, |deck| &deck.track),
                    &playback.players,
                    deck,
                    playback.page,
                    width
                ),
                timeline(deck, accent, width, now),
                controls(deck, accent, width),
            ])
            .width(width)
            .gap(12.0),
        )
}

// art, then title and artist, then the players to choose from
fn header(
    track: &Track,
    players: &[Choice],
    deck: Option<&Deck>,
    page: Option<usize>,
    width: f32,
) -> Row {
    let words = width - ART - GAP;

    let mut lines = children![
        Text::new(&track.title)
            .size(16.0)
            .color(theme::FG)
            .weight(600)
            .elide()
    ];

    if !track.artist.is_empty() {
        lines.push(Box::new(
            Text::new(&track.artist)
                .size(13.0)
                .color(theme::MUTED)
                .weight(500)
                .elide(),
        ));
    }

    let shown = deck.map(|deck| deck.name.as_str());

    Row::new(children![
        art(track, ART, ART_RADIUS),
        Stack::new(children![
            Column::new(lines).width(words - BADGE).gap(3.0),
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
                .size(12.0)
                .color(theme::MUTED)
                .weight(500)
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
        .on_click(move |button| {
            if button == Button::Left {
                media::control(control.clone());
            }
        })
        .child(
            Text::new(label)
                .size(12.0)
                .color(if selected { theme::FG } else { theme::MUTED })
                .weight(600)
                .elide(),
        );

    Box::new(if selected {
        chip.fill(theme::DOT)
    } else {
        chip.border(1.0, theme::DOT)
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

    let time = |text: String| Text::new(text).size(12.0).color(theme::MUTED).weight(500);

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

            button(glyph, PLAY, Some(theme::FG), control)
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
        Some(deck) if deck.track.playing => (
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
 * at the same place, not pressable
 */
fn button(glyph: Transport, side: f32, fill: Option<Color>, control: Option<Control>) -> Rectangle {
    let tone = if fill.is_some() {
        theme::BODY
    } else {
        theme::FG
    };

    let button = Rectangle::new()
        .width(side)
        .height(side)
        .radius(side / 2.0)
        .align_child(Center, Center)
        .child(glyph.draw(side * 0.5, tone));

    let button = match fill {
        Some(fill) => button.fill(fill),
        None => button,
    };

    match control {
        Some(control) => button.cursor(Cursor::Pointer).on_click(move |pressed| {
            if pressed == Button::Left {
                media::control(control.clone());
            }
        }),
        None => button.opacity(DISABLED),
    }
}

// the speaker's icon and its level `width` wide, set by a press or a drag along it
fn volume(width: f32, accent: Color) -> Row {
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
        .on_click(|button| {
            if button == Button::Left {
                Audio::toggle_mute();
            }
        })
        .child(icon.draw(20.0));

    let tone = if muted { theme::MUTED } else { accent };

    let level = Rectangle::new()
        .width(width)
        .height(TARGET)
        .align_child(Start, Center)
        .cursor(Cursor::Pointer)
        .on_drag(move |point| set_volume((point.x / width * 100.0).round().clamp(0.0, 100.0) as u8))
        .child(bar(width, f32::from(percent) / 100.0, tone));

    Row::new(children![speaker, level]).gap(10.0).align(Center)
}

/*
 * the level last asked for and when, since the speaker reports it only after a moment: a wheel
 * spun faster than that steps on from what it asked, not from the old level. And the part of a
 * line too small to move a percent, which a touchpad scrolls in, kept for the next one
 */
struct Asked {
    percent: u8,
    at: Instant,
    carry: f32,
}

static ASKED: Mutex<Option<Asked>> = Mutex::new(None);

// how long an asked level stands for the speaker's own
const ASKED_FOR: Duration = Duration::from_millis(300);

// down is quieter
fn wheel(lines: f32) {
    let mut asked = ASKED.lock().unwrap_or_else(PoisonError::into_inner);
    let now = Instant::now();

    let (from, carry) = match asked.as_ref() {
        Some(asked) if now.duration_since(asked.at) < ASKED_FOR => (asked.percent, asked.carry),
        _ => (Audio::read().volume(), 0.0),
    };

    let steps = carry - lines * WHEEL;
    let whole = steps.trunc();
    let percent = (f32::from(from) + whole).clamp(0.0, 100.0) as u8;

    *asked = Some(Asked {
        percent,
        at: now,
        carry: steps - whole,
    });
    drop(asked);

    if percent != from {
        Audio::set_volume(percent);
    }
}

fn set_volume(percent: u8) {
    *ASKED.lock().unwrap_or_else(PoisonError::into_inner) = Some(Asked {
        percent,
        at: Instant::now(),
        carry: 0.0,
    });

    Audio::set_volume(percent);
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Transport {
    Previous,
    Play,
    Pause,
    Next,
}

impl Transport {
    // on a 20 unit grid like the island's icons, in `tone` so it can sit on a filled button
    fn draw(self, side: f32, tone: Color) -> Canvas {
        let u = side / 20.0;

        let triangle = |from: f32, to: f32| {
            Path::new()
                .move_to(from * u, 4.5 * u)
                .line_to(to * u, 10.0 * u)
                .line_to(from * u, 15.5 * u)
                .close()
                .fill(tone)
        };

        let pause = |x: f32| {
            Path::new()
                .move_to(x * u, 4.0 * u)
                .line_to((x + 3.0) * u, 4.0 * u)
                .line_to((x + 3.0) * u, 16.0 * u)
                .line_to(x * u, 16.0 * u)
                .close()
                .fill(tone)
        };

        let shapes = match self {
            Transport::Previous => shapes![triangle(10.0, 1.5), triangle(18.5, 10.0)],

            // a triangle's weight sits left of its box, so it moves right to look centered
            Transport::Play => shapes![triangle(6.0, 16.5)],
            Transport::Pause => shapes![pause(5.0), pause(12.0)],
            Transport::Next => shapes![triangle(1.5, 10.0), triangle(10.0, 18.5)],
        };

        Canvas::new().width(side).height(side).shapes(shapes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // the room choices() gets on the Surface
    const WORDS: f32 = geometry::MEDIA.width - 2.0 * INSET - ART - GAP;

    #[test]
    fn every_player_chip_stays_pressable_however_many_players() {
        for count in 2..=24 {
            for shown in 0..count {
                let chips = Chips::of(count, Some(shown), None, WORDS);
                let shown_chips = chips.page.len() + usize::from(chips.more.is_some());
                let used = shown_chips as f32 * chips.width + (shown_chips - 1) as f32 * CHIP_GAP;

                assert!(chips.width >= CHIP_MIN.max(TARGET), "{count} players");
                assert!(used <= WORDS, "{count} players overflow");
                assert!(chips.page.contains(&shown), "{count} players hide {shown}");
            }
        }
    }

    #[test]
    fn the_more_chip_turns_the_page_and_back() {
        let first = Chips::of(11, Some(0), None, WORDS);
        assert_eq!(first.page, 0..3);
        assert_eq!(first.more, Some(1));

        let last = Chips::of(11, Some(10), None, WORDS);
        assert_eq!(last.page, 9..11);
        assert_eq!(last.more, Some(0));
        assert_eq!(last.width, first.width);

        // what fits shows whole
        assert_eq!(Chips::of(3, Some(2), None, WORDS).more, None);
    }

    #[test]
    fn a_turned_page_shows_whatever_player_is_shown() {
        // the shown player is on the first page, the second is turned to
        let turned = Chips::of(11, Some(0), Some(1), WORDS);
        assert_eq!(turned.page, 3..6);
        assert_eq!(turned.more, Some(2));

        // players closed under a page past the end leave the last one
        assert_eq!(Chips::of(5, Some(0), Some(3), WORDS).page, 3..5);
    }

    #[test]
    fn a_playing_player_pauses_and_a_paused_one_plays_only_if_it_can() {
        let deck = |playing: bool, can_play: bool, can_pause: bool| Deck {
            name: String::from("mpv"),
            track: Track {
                playing,
                ..Track::default()
            },
            timeline: crate::sources::playback::Timeline {
                position: Duration::ZERO,
                at: Instant::now(),
                length: Duration::ZERO,
                rate: 0.0,
            },
            accent: None,
            can_play,
            can_pause,
            can_previous: false,
            can_next: false,
        };
        let pause = Some(Control::Pause(String::from("mpv")));
        let play = Some(Control::Play(String::from("mpv")));

        assert_eq!(
            play_pause(Some(&deck(true, false, true))),
            (Transport::Pause, pause)
        );
        assert_eq!(
            play_pause(Some(&deck(true, true, false))),
            (Transport::Pause, None)
        );
        assert_eq!(
            play_pause(Some(&deck(false, true, false))),
            (Transport::Play, play)
        );
        assert_eq!(
            play_pause(Some(&deck(false, false, true))),
            (Transport::Play, None)
        );
        assert_eq!(play_pause(None), (Transport::Play, None));
    }

    #[test]
    fn clocks_read_like_a_player() {
        assert_eq!(clock(Duration::from_secs(0)), "0:00");
        assert_eq!(clock(Duration::from_millis(187_900)), "3:07");
        assert_eq!(clock(Duration::from_secs(3727)), "1:02:07");
    }
}
