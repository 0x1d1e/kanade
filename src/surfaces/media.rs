//! The Media Surface: art, title and artist, timeline, prev/play/next, the speaker volume,
//! and which player it shows. Every part keeps its place in every state, so nothing moves when a
//! player opens, art loads, or a title runs long. A player that can seek has its timeline set by a
//! press or a drag along it, or Left and Right with the ring on it.

use std::ops::Range;
use std::time::{Duration, Instant};

use crate::sources::pulse::Audio;
use kanade_runtime::service::Service;
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Button, Center, Color, Column, Cursor, End, Key, Padding, Rectangle, Row, Scroll, SpaceBetween,
    Stack, Start, Text, Widget, children, request_frame,
};

use super::grid::moved;
use super::pad::Focus;
use super::slider::Slider;
use super::{Outline, store};
use crate::icon::Icon;
use crate::island::activity::Track;
use crate::island::geometry;
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::modules;
use crate::sources::media::{self, Control};
use crate::sources::playback::{Choice, Deck, Playback};
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, DISABLED, radius};
use crate::view::{Change, bar, rise, state};

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
const TRANSPORT_GAP: f32 = 8.0;
const CONTROLS_GAP: f32 = 24.0;
const VOLUME_GAP: f32 = 10.0;

// the timeline's block: the ring's room around the bar and its thumb, then the times
const SIDE: f32 = 4.0;
const THUMB: f32 = 12.0;
const BLOCK: f32 = 36.0;

// how far Left and Right move a player
const STEP: i64 = 5;

// what the ring can be on
#[derive(Debug, Clone, PartialEq)]
enum At {
    // a player's chip, by its bus name, and the one that turns to the page after
    Chip(String),
    More(usize),
    Timeline,
    Previous,
    Play,
    Next,
    Speaker,
    Controls,
}

impl Service for Focus<At> {
    fn new() -> Self {
        Focus::default()
    }

    fn listen() {}
}

/*
 * `open` says the Surface is open rather than fading out, so only then does it ask the follower
 * to keep it fed
 */
pub fn surface(monitor: &str, open: bool, visit: u64, held: bool, now: Instant) -> Rectangle {
    if open {
        media::watch();
    }

    let playback = Playback::read();
    let shape = geometry::MEDIA;
    let width = shape.width - 2.0 * INSET;

    let deck = playback.shown.as_ref();
    let (grid, chips) = grid(&playback);
    let ring = open
        .then(|| Focus::<At>::read().of(visit, held).ring.on(&grid, chips))
        .flatten();
    let ring = ring.as_ref();
    let accent = deck.map_or(theme::island().on_surface, |deck| deck.accent.at(now));

    // the dissolve and the tint are the follower's, so the Surface asks for frames until they rest
    if deck.is_some_and(|deck| !deck.track.settled(now) || !deck.accent.settled(now)) {
        request_frame();
    }

    let nothing = Track {
        title: String::from("Nothing playing"),
        artist: String::from("Start playback in any media player"),
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
        .semantics(|| Semantics::new(Role::Region, "media"))
        .child(
            Column::new(children![
                rise(
                    0,
                    header(
                        track,
                        &playback.players,
                        deck,
                        playback.page,
                        width,
                        now,
                        ring
                    )
                ),
                rise(
                    1,
                    timeline(deck, accent, width, now, ring == Some(&At::Timeline))
                ),
                rise(2, controls(monitor, deck, accent, width, ring)),
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
    ring: Option<&At>,
) -> Row {
    let words = width - ART - GAP;

    // the visualizer at the top right, the lines short of it
    const MOVING: f32 = 36.0;

    // the artist's line is there even when it names none, like the Peek's
    let mut lines = children![
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

    // the synced lyric line, in the artwork's accent, under the artist (ADR 0063)
    if let Some((lyric, deck)) = deck.and_then(|deck| Some((deck.lyric.as_deref()?, deck))) {
        lines.push(Box::new(
            Text::new(lyric)
                .size(theme::text::LABEL_SMALL)
                .color(deck.accent.at(now))
                .weight(theme::text::MEDIUM)
                .elide(),
        ));
    }

    let shown = deck.map(|deck| deck.name.as_str());

    // no player, no visualizer: a paused one would read as a player that is paused
    let moving = Rectangle::new()
        .width(words)
        .height(ART)
        .align_child(End, Start)
        .padding(Padding {
            top: 4.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        });
    let moving = match deck {
        Some(deck) => moving.child(state(deck.track.target().playing, deck.accent.at(now), now)),
        None => moving,
    };

    Row::new(children![
        track.art(ART, radius::ROW),
        Stack::new(children![
            Column::new(lines).width(words - MOVING).gap(3.0),
            moving,
            Rectangle::new()
                .width(words)
                .height(ART)
                .align_child(Start, End)
                .child(choices(players, shown, page, words, ring)),
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
fn choices(
    players: &[Choice],
    shown: Option<&str>,
    page: Option<usize>,
    width: f32,
    ring: Option<&At>,
) -> Row {
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

        let ring = ring == Some(&At::Chip(player.name.clone()));

        chip(&player.identity, chips.width, selected, ring, select)
    });

    let more = chips.more.map(|next| {
        let hidden = players.len() - chips.page.len();

        chip(
            &format!("+{hidden}"),
            chips.width,
            false,
            ring == Some(&At::More(next)),
            Control::Page(next),
        )
    });

    Row::new(page.chain(more).collect()).gap(CHIP_GAP)
}

fn chip(label: &str, width: f32, selected: bool, ring: bool, control: Control) -> Box<dyn Widget> {
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
        .semantics(|| {
            // a player is called as it names itself, "+N" turns the page
            match control {
                Control::Page(_) => Semantics::new(Role::Button, "more").label(label),
                _ => Semantics::new(Role::Button, "player")
                    .content(label)
                    .selected(selected),
            }
        })
        .on_click(pointed(move || {
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

    let chip = if selected {
        chip.fill(theme::island().surface_container_high)
    } else {
        chip.border(1.0, theme::island().surface_container_high)
    };

    Box::new(chip.border_if(ring))
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

/*
 * how far along, with the time played and the time left; times only once the player says them. One
 * that can seek has a thumb on the bar, and the whole block takes a press or a drag, `SIDE` being
 * the room for the ring around the track, whose ends are the bar's
 */
fn timeline(deck: Option<&Deck>, accent: Color, width: f32, now: Instant, ring: bool) -> Rectangle {
    let timeline = deck.map(|deck| deck.timeline);
    let seek = deck.filter(|deck| deck.can_seek);
    let inner = width - 2.0 * SIDE;

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

    // the thumb's room is there with or without one, so nothing moves when a player can seek
    let mut track = children![
        Rectangle::new()
            .width(inner)
            .height(THUMB)
            .align_child(Start, Center)
            .child(bar(inner, fraction, accent)),
    ];

    if seek.is_some() {
        let left = (inner * fraction - THUMB / 2.0).clamp(0.0, inner - THUMB);

        track.push(Box::new(
            Rectangle::new()
                .width(inner)
                .height(THUMB)
                .padding(Padding {
                    top: 0.0,
                    right: 0.0,
                    bottom: 0.0,
                    left,
                })
                .align_child(Start, Center)
                .child(
                    Rectangle::new()
                        .width(THUMB)
                        .height(THUMB)
                        .radius(THUMB / 2.0)
                        .fill(accent),
                ),
        ));
    }

    let block = Rectangle::new()
        .width(width)
        .height(BLOCK)
        .radius(8.0)
        .padding(Padding {
            top: 2.0,
            right: SIDE,
            bottom: 2.0,
            left: SIDE,
        })
        .align_child(Start, Start)
        .border_if(ring)
        .semantics(|| {
            Semantics::new(Role::Slider, "timeline")
                .level(fraction)
                .disabled(seek.is_none())
        })
        .child(
            Column::new(children![
                Stack::new(track).width(inner).height(THUMB),
                Row::new(children![time(played), time(left)])
                    .width(inner)
                    .height(16.0)
                    .justify(SpaceBetween)
                    .align(Center),
            ])
            .gap(4.0),
        );

    let Some(deck) = seek else {
        return block;
    };

    let (name, length) = (deck.name.clone(), deck.timeline.length);

    block.cursor(Cursor::Pointer).on_drag(move |point| {
        crate::view::claim();
        hide();
        media::seek(name.clone(), place(length, (point.x - SIDE) / inner));
    })
}

// where along a track `length` long a press at `fraction` of the bar is, the ends being its own
fn place(length: Duration, fraction: f32) -> Duration {
    length.mul_f64(f64::from(fraction.clamp(0.0, 1.0)))
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

// previous, play or pause, next; then the speaker, pressed to mute, its level, and Controls
fn controls(
    monitor: &str,
    deck: Option<&Deck>,
    accent: Color,
    width: f32,
    ring: Option<&At>,
) -> Row {
    let transport = Row::new(children![
        button(
            Transport::Previous,
            BUTTON,
            None,
            able(deck, |deck| deck.can_previous).map(Control::Previous),
            ring == Some(&At::Previous),
        ),
        {
            let (glyph, control) = play_pause(deck);

            button(
                glyph,
                PLAY,
                Some(theme::island().primary),
                control,
                ring == Some(&At::Play),
            )
        },
        button(
            Transport::Next,
            BUTTON,
            None,
            able(deck, |deck| deck.can_next).map(Control::Next),
            ring == Some(&At::Next),
        ),
    ])
    .gap(TRANSPORT_GAP)
    .align(Center);

    // Controls is where the rest of the machine is, one press from the player
    let controls = modules::on("controls");
    let layout = controls_layout(width, controls);
    let mut row: Vec<Box<dyn Widget>> = vec![
        Box::new(transport),
        Box::new(volume(
            layout.volume_bar,
            accent,
            ring == Some(&At::Speaker),
        )),
    ];
    if controls {
        row.push(Box::new(to_controls(monitor, ring == Some(&At::Controls))));
    }

    Row::new(row).width(width).gap(CONTROLS_GAP).align(Center)
}

#[derive(Debug, Clone, Copy)]
struct ControlsLayout {
    volume_bar: f32,
    speaker_center: f32,
    controls_center: f32,
}

fn controls_layout(width: f32, controls: bool) -> ControlsLayout {
    let transport = 2.0 * BUTTON + PLAY + 2.0 * TRANSPORT_GAP;
    let group_gaps = CONTROLS_GAP * if controls { 2.0 } else { 1.0 };
    let controls_width = if controls { TARGET } else { 0.0 };
    let volume_bar = width - transport - group_gaps - controls_width - TARGET - VOLUME_GAP;

    ControlsLayout {
        volume_bar,
        speaker_center: transport + CONTROLS_GAP + TARGET / 2.0,
        controls_center: width - TARGET / 2.0,
    }
}

// opens Controls in place of the player
fn to_controls(monitor: &str, ring: bool) -> Rectangle {
    let monitor = monitor.to_owned();

    Rectangle::new()
        .width(TARGET)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .align_child(Center, Center)
        .border_if(ring)
        .cursor(Cursor::Pointer)
        .semantics(|| Semantics::new(Role::Button, "controls"))
        .on_click(pointed(move || {
            crate::view::open(&monitor, Surface::Controls, false);
        }))
        .child(Icon::Sliders.on(20.0, theme::island().on_surface_variant))
}

// the shown player's bus name when it can do what `can` says
fn able(deck: Option<&Deck>, can: fn(&Deck) -> bool) -> Option<String> {
    deck.filter(|deck| can(deck)).map(|deck| deck.name.clone())
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
 * gpu a canvas of its own every frame. The body-colored glyph on a fill stays solid, it
 * shows the body through either way
 */
fn button(
    glyph: Transport,
    side: f32,
    fill: Option<Color>,
    control: Option<Control>,
    ring: bool,
) -> Rectangle {
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
        .semantics(|| {
            let name = match glyph {
                Transport::Previous => "previous",
                Transport::Play => "play",
                Transport::Pause => "pause",
                Transport::Next => "next",
            };

            Semantics::new(Role::Button, name).disabled(control.is_none())
        })
        .child(glyph.draw(side * 0.5, tone));

    let button = match fill {
        Some(fill) => button.fill(fade(fill)),
        None => button,
    }
    .border_if(ring);

    match control {
        Some(control) => button.cursor(Cursor::Pointer).on_click(pointed(move || {
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
fn volume(width: f32, accent: Color, ring: bool) -> Rectangle {
    // the speaker and its bar, in the ring's room
    let level = |row: Row| {
        Rectangle::new()
            .width(TARGET + 10.0 + width)
            .height(TARGET)
            .radius(TARGET / 2.0)
            .align_child(Start, Center)
            .child(row.gap(VOLUME_GAP).align(Center))
    };

    if !modules::on("audio") {
        return level(Row::new(children![
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
        ]));
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
        .semantics(|| Semantics::new(Role::Toggle, "mute").checked(muted))
        .on_click(pointed(|| {
            Audio::toggle_mute();
        }))
        .child(icon.draw(20.0));

    let tone = if muted {
        theme::island().on_surface_variant
    } else {
        accent
    };

    let bar = Slider::Speaker.bar(width, f32::from(percent) / 100.0, tone);

    level(Row::new(children![speaker, bar])).border_if(ring)
}

/*
 * the targets, row by row, each with where its middle is across the Surface from 0 to 1: the
 * player chips when there are several, then the timeline of a player that can seek, then the
 * transport, the speaker and Controls. The ring starts on the row after the chips, which are the
 * `header` rows
 */
fn grid(playback: &Playback) -> (Vec<Vec<(At, f32)>>, usize) {
    let width = geometry::MEDIA.width - 2.0 * INSET;
    let deck = playback.shown.as_ref();
    let mut rows = Vec::new();

    if playback.players.len() > 1 {
        let index = deck.and_then(|deck| {
            playback
                .players
                .iter()
                .position(|player| player.name == deck.name)
        });
        let chips = Chips::of(
            playback.players.len(),
            index,
            playback.page,
            width - ART - GAP,
        );

        let ats = playback.players[chips.page.clone()]
            .iter()
            .map(|player| At::Chip(player.name.clone()))
            .chain(chips.more.map(At::More));

        rows.push(
            ats.enumerate()
                .map(|(place, at)| {
                    let middle = ART + GAP + (place as f32 + 0.5) * (chips.width + CHIP_GAP);

                    (at, middle / width)
                })
                .collect(),
        );
    }

    let header = rows.len();

    if deck.is_some_and(|deck| deck.can_seek) {
        rows.push(vec![(At::Timeline, 0.5)]);
    }

    // where each button's middle is along the transport
    let mut row = Vec::new();

    if able(deck, |deck| deck.can_previous).is_some() {
        row.push((At::Previous, BUTTON / 2.0 / width));
    }

    if play_pause(deck).1.is_some() {
        row.push((At::Play, (BUTTON + 8.0 + PLAY / 2.0) / width));
    }

    if able(deck, |deck| deck.can_next).is_some() {
        row.push((At::Next, (BUTTON + 8.0 + PLAY + 8.0 + BUTTON / 2.0) / width));
    }

    if modules::on("audio") {
        let layout = controls_layout(width, modules::on("controls"));
        row.push((At::Speaker, layout.speaker_center / width));
    }

    if modules::on("controls") {
        let layout = controls_layout(width, true);
        row.push((At::Controls, layout.controls_center / width));
    }

    if !row.is_empty() {
        rows.push(row);
    }

    (rows, header)
}

// what a key asks for beyond moving the ring
#[derive(Debug, PartialEq)]
enum Act {
    Press(At),

    // Left or Right on a level: -1 or 1
    Step(At, i64),
}

/*
 * the focus after a key and what it asks for, none when the key is not the Surface's, as Escape,
 * which closes it. A key while the ring hides only shows it, so nothing is pressed that it was not
 * seen on. On the timeline and the speaker Left and Right move the level, not the ring
 */
fn step(
    focus: Focus<At>,
    key: Key,
    grid: &[Vec<(At, f32)>],
    header: usize,
) -> Option<(Focus<At>, Option<Act>)> {
    let place = focus.ring.place(grid, header)?;
    let at = grid[place.row][place.column].0.clone();
    let level = matches!(at, At::Timeline | At::Speaker);

    let act = match key {
        Key::Enter | Key::Space if at != At::Timeline => Some(Act::Press(at.clone())),
        Key::Left if level => Some(Act::Step(at.clone(), -1)),
        Key::Right if level => Some(Act::Step(at.clone(), 1)),
        _ => None,
    };

    let to = match key {
        Key::Left | Key::Right if level => None,
        _ => moved(place, key, grid),
    };

    if act.is_none() && to.is_none() {
        return None;
    }

    if let Some(ring) = focus.ring.revealed(grid, header) {
        return Some((Focus { ring }, None));
    }

    let ring = match to {
        Some(to) => focus.ring.onto(grid[to.row][to.column].0.clone()),
        None => focus.ring,
    };

    Some((Focus { ring }, act))
}

/*
 * a key while this island shows the Media Surface; false for one it does not use, as Escape,
 * which the window's own keys then get to close it
 */
pub fn key(monitor: &str, key: Key) -> bool {
    let (visit, held) = {
        let island = IslandService::read();

        if island.presentation(monitor) != Presentation::Expanded(Surface::Media) {
            return false;
        }

        (island.visit(), island.held(monitor))
    };

    let playback = Playback::read().clone();
    let (grid, header) = grid(&playback);

    let Some((focus, act)) = step(Focus::<At>::read().of(visit, held), key, &grid, header) else {
        return false;
    };

    if let Some(act) = act {
        press(monitor, playback.shown.as_ref(), act);
    }

    store(focus);

    IslandService::write().attend(monitor, Instant::now());

    true
}

// what a key's act does, as the pointer would on the same target; what the player cannot do is nothing
fn press(monitor: &str, deck: Option<&Deck>, act: Act) {
    match act {
        Act::Press(At::Chip(name)) => media::control(Control::Select(name)),
        Act::Press(At::More(page)) => media::control(Control::Page(page)),
        Act::Press(At::Previous) => {
            if let Some(name) = able(deck, |deck| deck.can_previous) {
                media::control(Control::Previous(name));
            }
        }
        Act::Press(At::Play) => {
            if let Some(control) = play_pause(deck).1 {
                media::control(control);
            }
        }
        Act::Press(At::Next) => {
            if let Some(name) = able(deck, |deck| deck.can_next) {
                media::control(Control::Next(name));
            }
        }
        Act::Press(At::Speaker) => {
            if modules::on("audio") {
                Audio::toggle_mute();
            }
        }
        Act::Press(At::Controls) => crate::view::open(monitor, Surface::Controls, false),
        Act::Press(At::Timeline) => {}
        Act::Step(At::Timeline, direction) => {
            if let Some(deck) = deck.filter(|deck| deck.can_seek) {
                media::control(Control::SeekBy(deck.name.clone(), direction * STEP));
            }
        }

        // the wheel counts down as lower
        Act::Step(At::Speaker, direction) => Slider::Speaker.wheel(-direction as f32),
        Act::Step(..) => {}
    }
}

// a press or drag with the pointer: the ring hides, since the pointer is what moves now
fn pointed(press: impl Fn() + 'static) -> impl Fn(Button) + 'static {
    super::on_left(move || {
        hide();
        press();
    })
}

fn hide() {
    let visit = IslandService::read().visit();
    // bound first: a read guard left in `store`'s argument would block its write
    let ring = Focus::<At>::read().of(visit, false).ring.hidden();

    store(Focus { ring });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn grid() -> Vec<Vec<(At, f32)>> {
        vec![
            vec![(At::Timeline, 0.5)],
            vec![(At::Play, 0.2), (At::Controls, 0.9)],
        ]
    }

    // a press hides the ring through the Focus Service: a read guard held across its write once froze
    // the shell at the first click
    #[test]
    fn hiding_the_ring_does_not_wait_on_itself() {
        let (done, ended) = std::sync::mpsc::channel();

        // shown, so that hiding it is a write, which `store` skips when nothing changes
        let visit = IslandService::read().visit();
        *Focus::<At>::write() = Focus {
            ring: crate::surfaces::ring::Ring::start(visit, true),
        };

        std::thread::spawn(move || {
            hide();
            let _ = done.send(());
        });

        assert!(
            ended
                .recv_timeout(std::time::Duration::from_secs(5))
                .is_ok()
        );
    }

    // a ring that hides is shown by the first key, which seeks nothing
    #[test]
    fn a_hidden_ring_is_shown_before_a_key_seeks() {
        let hidden = Focus::<At>::default().of(1, false);

        let (shown, act) = step(hidden, Key::Right, &grid(), 0).unwrap();

        assert_eq!(act, None);
        assert_eq!(shown.ring.on(&grid(), 0), Some(At::Timeline));

        let (_, act) = step(shown, Key::Left, &grid(), 0).unwrap();

        assert_eq!(act, Some(Act::Step(At::Timeline, -1)));
    }

    // Left and Right move a level, so the ring leaves the timeline only by Down or Tab
    #[test]
    fn arrows_on_the_timeline_do_not_move_the_ring() {
        let focus = Focus::<At>::default().of(1, true);

        let (after, _) = step(focus.clone(), Key::Right, &grid(), 0).unwrap();

        assert_eq!(after.ring.on(&grid(), 0), Some(At::Timeline));
        assert_eq!(step(focus.clone(), Key::Enter, &grid(), 0), None);

        let (after, _) = step(focus, Key::Down, &grid(), 0).unwrap();

        assert_eq!(after.ring.on(&grid(), 0), Some(At::Play));
    }

    #[test]
    fn a_press_beyond_the_bar_lands_on_the_track_s_ends() {
        let length = Duration::from_secs(200);

        assert_eq!(place(length, -0.3), Duration::ZERO);
        assert_eq!(place(length, 0.5), Duration::from_secs(100));
        assert_eq!(place(length, 1.4), length);
    }
}
