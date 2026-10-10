//! The Controls Surface's Audio sub-surface: the default speaker's level, then the outputs to play
//! on; the default microphone's level, then the inputs to record from; then each app's level, those
//! playing first. A device pressed becomes the default of its list, and says so while it switches
//! or once it failed to. A level's icon, or Enter on it, mutes it; Left and Right move it, as a
//! press or drag along its bar does. The wheel scrolls the rows, so it moves no level here.

use kanade_runtime::service::Service as _;
use kanade_runtime::{Center, Column, Cursor, End, Parent, Rectangle, Row, Text, Widget, children};

use crate::sources::pulse::Audio;

use super::WIDTH;
use super::focus::{Act, At};
use super::list::{self, GAP, ICON, ICON_GAP, ROW_INSET};
use crate::icon::Icon;
use crate::sources::audio::{Device, Direction, Level, Mixer, Stream, Switching};
use crate::surfaces::slider::Slider;
use crate::theme;
use crate::theme::space::TARGET;

// room for "100"
const NUMBER: f32 = 30.0;

// a level's bar, between its icon and its number
const BAR: f32 = WIDTH - 2.0 * ROW_INSET - ICON - NUMBER - 2.0 * ICON_GAP;

// one row, as they show
#[derive(Debug, Clone, Copy, PartialEq)]
enum Line<'a> {
    Speaker,
    Microphone,
    Device(&'a Device, Direction),
    Stream(&'a Stream),
}

impl Line<'_> {
    fn at(self) -> At {
        match self {
            Line::Speaker => At::Speaker,
            Line::Microphone => At::MicrophoneLevel,
            Line::Device(device, Direction::Output) => At::Output(device.node),
            Line::Device(device, Direction::Input) => At::Input(device.node),
            Line::Stream(stream) => At::Stream(stream.node),
        }
    }
}

// the speaker's level and the outputs, while there are any, the same for the microphone, then apps
fn lines(mixer: &Mixer) -> Vec<Line<'_>> {
    let mut lines = Vec::new();

    for (level, devices, direction) in [
        (Line::Speaker, &mixer.outputs, Direction::Output),
        (Line::Microphone, &mixer.inputs, Direction::Input),
    ] {
        if !devices.is_empty() {
            lines.push(level);
            lines.extend(devices.iter().map(|device| Line::Device(device, direction)));
        }
    }

    lines.extend(mixer.streams.iter().map(Line::Stream));
    lines
}

// its targets, one a row
pub fn rows(mixer: &Mixer) -> Vec<Vec<(At, f32)>> {
    lines(mixer)
        .into_iter()
        .map(|line| vec![(line.at(), 0.5)])
        .collect()
}

/*
 * the header, then the rows scrolled `offset` down. `ring` is what the ring is on, none while it
 * hides
 */
pub fn sound(mixer: &Mixer, switching: Switching, offset: f32, ring: Option<&At>) -> Column {
    let lines = lines(mixer);

    let list: Box<dyn Widget> = if lines.is_empty() {
        Box::new(list::state(
            Icon::Speaker(0),
            "No sound devices",
            "Or PipeWire isn\u{2019}t running",
        ))
    } else {
        let defaults = Defaults::read();

        Box::new(list::list(lines.len(), offset, |index| {
            let line = lines[index];

            Box::new(row(line, &defaults, switching, ring == Some(&line.at())))
        }))
    };

    Column::new(vec![
        Box::new(list::header("Sound", None, ring)) as Box<dyn Widget>,
        list,
    ])
    .width(WIDTH)
    .gap(GAP)
}

// the default speaker's and microphone's levels, read once for every row
struct Defaults {
    speaker: Level,
    microphone: Level,
}

impl Defaults {
    fn read() -> Defaults {
        let audio = Audio::read();

        Defaults {
            speaker: Level {
                volume: audio.volume(),
                muted: audio.muted(),
            },
            microphone: Level {
                volume: audio.microphone_volume(),
                muted: audio.microphone_muted(),
            },
        }
    }
}

fn row(line: Line, defaults: &Defaults, switching: Switching, ring: bool) -> Rectangle {
    let at = line.at();

    match line {
        Line::Speaker => level(
            at,
            speaker(defaults.speaker),
            ("Output", None),
            Slider::Speaker,
            defaults.speaker,
            ring,
        ),
        Line::Microphone => level(
            at,
            microphone(defaults.microphone),
            ("Input", None),
            Slider::Microphone,
            defaults.microphone,
            ring,
        ),
        Line::Device(device, direction) => self::device(device, direction, switching, at, ring),
        Line::Stream(stream) => {
            let icon = if stream.plays {
                speaker(stream.level)
            } else {
                microphone(stream.level)
            };

            level(
                at,
                icon,
                (&stream.app, stream.title.as_deref()),
                Slider::Stream(stream.node),
                stream.level,
                ring,
            )
        }
    }
}

fn speaker(level: Level) -> Icon {
    if level.muted {
        Icon::SpeakerMuted
    } else {
        Icon::Speaker(level.volume)
    }
}

fn microphone(level: Level) -> Icon {
    if level.muted {
        Icon::MicrophoneMuted
    } else {
        Icon::Microphone
    }
}

/*
 * its icon, which presses `at` to mute it, its name and what it plays over its bar, then its
 * number, the ring on the whole of it
 */
fn level(
    at: At,
    icon: Icon,
    (name, detail): (&str, Option<&str>),
    slider: Slider,
    Level { volume, muted }: Level,
    ring: bool,
) -> Rectangle {
    let tone = if muted {
        theme::island().on_surface_variant
    } else {
        theme::island().on_surface
    };

    // as wide as a device's icon, so the names line up
    let icon = Rectangle::new()
        .width(ICON)
        .height(TARGET)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(super::super::on_left(move || {
            super::click(Act::Press(at.clone()));
        }))
        .child(icon.on(ICON, tone));

    // only what it plays elides, so it follows the name
    let mut words = children![
        Text::new(name)
            .size(theme::text::LABEL)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD)
    ];

    if let Some(detail) = detail {
        words.push(Box::new(
            Text::new(detail)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::MEDIUM)
                .elide(),
        ));
    }

    let fraction = (f32::from(volume) / 100.0).min(1.0);

    let middle = Column::new(children![
        Row::new(words).width(Parent).gap(6.0).align(Center),
        slider.bar(BAR, fraction, tone),
    ])
    .width(BAR);

    let number = Rectangle::new()
        .width(NUMBER)
        .height(TARGET)
        .align_child(End, Center)
        .child(
            Text::new(volume.to_string())
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::SEMIBOLD),
        );

    list::frame(
        Row::new(children![icon, middle, number])
            .width(Parent)
            .gap(ICON_GAP)
            .align(Center),
        ring,
    )
}

// what a device's row says under its name, and whether that is an error
fn status(
    device: &Device,
    direction: Direction,
    switching: Switching,
) -> Option<(&'static str, bool)> {
    if switching.doing(device.node, direction) {
        Some(("Switching\u{2026}", false))
    } else if switching.failed(device.node, direction) {
        Some(("Couldn\u{2019}t switch", true))
    } else {
        None
    }
}

/*
 * its name, checked while it is the default. Pressing another makes it the default, unless one is
 * being switched to
 */
fn device(
    device: &Device,
    direction: Direction,
    switching: Switching,
    at: At,
    ring: bool,
) -> Rectangle {
    let icon = match direction {
        Direction::Output => Icon::Speaker(100),
        Direction::Input => Icon::Microphone,
    };

    let mut trailing: Vec<Box<dyn Widget>> = Vec::new();

    if device.default {
        trailing.push(Box::new(Icon::Check.on(ICON, theme::island().primary)));
    }

    list::row(
        Box::new(icon.on(ICON, theme::island().on_surface)),
        &device.name,
        status(device, direction, switching),
        trailing,
        ring,
        (!device.default && !switching.busy()).then_some(at),
    )
}
