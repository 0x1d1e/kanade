//! The forms of the status Activities: timer, battery, workspace, level and mode.

use std::time::Instant;

use kanade_runtime::{
    Center, Color, Column, End, Padding, Parent, Rectangle, Row, Stack, Start, Text, Widget,
    children,
};

use crate::icon::Icon;
use crate::island::activity::{Charge, Countdown, Device, Mode, Volume, Workspace};
use crate::island::presentation::Presentation;
use crate::sources::timer;
use crate::theme::{self, ThemeRoles};

use super::forms::{sized, stacked};
use super::shape::{shape, upright};

// what is left, muted while it stands still
pub(super) fn timer_tone(countdown: &Countdown) -> Color {
    match countdown.paused {
        Some(_) => theme::island().on_surface_variant,
        None => theme::island().on_surface,
    }
}

/*
 * the stopwatch, what it is, then what is left; Peek also says how long it was started for. The
 * clock has a fixed width, so a second ticking by redraws it in place
 */
pub(super) fn timer(
    presentation: Presentation,
    countdown: &Countdown,
    now: Instant,
) -> Option<Rectangle> {
    if presentation == Presentation::Compact && upright() {
        return Some(stacked(children![
            Icon::Stopwatch.draw(18.0),
            Text::new(timer::short(countdown, now))
                .size(theme::text::LABEL_SMALL)
                .color(timer_tone(countdown))
                .weight(theme::text::SEMIBOLD),
        ]));
    }

    // h:mm:ss once the timer was started for an hour or more, m:ss below
    let hours = countdown.length.as_secs() >= 3600;

    let (icon, size, inset, clock) = match (presentation, hours) {
        (Presentation::Compact, false) => (18.0, theme::text::LABEL, 15.0, 40.0),
        (Presentation::Compact, true) => (18.0, theme::text::LABEL, 15.0, 56.0),
        (Presentation::Peek, false) => (24.0, theme::text::TITLE_LARGE, 20.0, 52.0),
        (Presentation::Peek, true) => (24.0, theme::text::TITLE_LARGE, 20.0, 72.0),
        _ => return None,
    };

    let gap = 11.0;
    let width = shape(presentation).width - 2.0 * inset - icon - clock - 2.0 * gap;

    let title = match countdown.paused {
        Some(_) => "Paused",
        None => "Timer",
    };
    let mut words = children![
        Text::new(title)
            .size(theme::text::LABEL)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD)
    ];

    if presentation == Presentation::Peek {
        words.push(Box::new(
            Text::new(format!("{} timer", timer::length(countdown)))
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::MEDIUM),
        ));
    }

    Some(
        sized(presentation)
            .padding(Padding {
                top: 0.0,
                right: inset,
                bottom: 0.0,
                left: inset,
            })
            .align_child(Start, Center)
            .child(
                Row::new(children![
                    Icon::Stopwatch.draw(icon),
                    Column::new(words).width(width).gap(3.0),
                    Rectangle::new()
                        .width(clock)
                        .height(size)
                        .align_child(End, Center)
                        .child(
                            Text::new(timer::clock(countdown, now))
                                .size(size)
                                .color(timer_tone(countdown))
                                .weight(theme::text::SEMIBOLD),
                        ),
                ])
                .gap(gap)
                .align(Center),
            ),
    )
}

// amber while low, red once critical, and never color alone: the number and the words say it too
pub(super) fn charge_tone(charge: &Charge) -> Color {
    if charge.critical {
        theme::SEMANTIC.critical
    } else {
        theme::SEMANTIC.warning
    }
}

/*
 * battery, what it means, the number; Peek says what to do about it. Fixed widths, so a number
 * that ticks down redraws in place
 */
pub(super) fn battery(presentation: Presentation, charge: &Charge) -> Option<Rectangle> {
    if presentation == Presentation::Compact && upright() {
        let tone = charge_tone(charge);

        return Some(stacked(children![
            battery_icon(22.0, charge.percent, tone),
            Text::new(format!("{}%", charge.percent))
                .size(theme::text::LABEL_SMALL)
                .color(tone)
                .weight(theme::text::SEMIBOLD),
        ]));
    }

    let (icon, number, size, inset) = match presentation {
        Presentation::Compact => (22.0, 40.0, theme::text::LABEL, 15.0),
        Presentation::Peek => (26.0, 48.0, theme::text::TITLE_LARGE, 20.0),
        _ => return None,
    };

    let gap = 11.0;
    let width = shape(presentation).width - 2.0 * inset - icon - number - 2.0 * gap;
    let tone = charge_tone(charge);

    let title = if charge.critical {
        "Battery Critical"
    } else {
        "Low Battery"
    };

    let mut words = children![
        Text::new(title)
            .size(theme::text::LABEL)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD)
    ];

    if presentation == Presentation::Peek {
        words.push(Box::new(
            Text::new("Plug in to charge")
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::MEDIUM),
        ));
    }

    Some(
        sized(presentation)
            .padding(Padding {
                top: 0.0,
                right: inset,
                bottom: 0.0,
                left: inset,
            })
            .align_child(Start, Center)
            .child(
                Row::new(vec![
                    Box::new(battery_icon(icon, charge.percent, tone)) as Box<dyn Widget>,
                    Box::new(Column::new(words).width(width).gap(3.0)),
                    Box::new(
                        Rectangle::new()
                            .width(number)
                            .height(size)
                            .align_child(End, Center)
                            .child(
                                Text::new(format!("{}%", charge.percent))
                                    .size(size)
                                    .color(tone)
                                    .weight(theme::text::SEMIBOLD),
                            ),
                    ),
                ])
                .gap(gap)
                .align(Center),
            ),
    )
}

// a battery on its side, filled as far as it is charged, a sliver at least so empty still reads
pub(super) fn battery_icon(width: f32, percent: u8, tone: Color) -> Row {
    let nub = width / 11.0;
    let shell = width - nub - 1.0;
    let height = width / 2.0;
    let border = 1.5;
    let inner = shell - 2.0 * border - 2.0;
    let filled = (inner * f32::from(percent.min(100)) / 100.0).max(2.0);

    let body = Rectangle::new()
        .width(shell)
        .height(height)
        .radius(height / 3.5)
        .border(border, tone)
        .padding(border + 1.0)
        .align_child(Start, Center)
        .child(
            Rectangle::new()
                .width(filled)
                .height(height - 2.0 * border - 2.0)
                .radius(theme::radius::HAIRLINE)
                .fill(tone),
        );

    let tip = Rectangle::new()
        .width(nub)
        .height(height / 2.5)
        .radius(nub / 2.0)
        .fill(tone);

    Row::new(children![body, tip])
        .width(width)
        .gap(1.0)
        .align(Center)
}

// past this many workspaces on an output the pager would not fit, so the numbers say it instead
const PAGER: u32 = 10;

/*
 * the workspace's name, then where it is among its output's workspaces; Peek says its number
 * under a name. The pager has a fixed width, so a switch moves its mark and nothing else
 */
pub(super) fn workspace(presentation: Presentation, workspace: &Workspace) -> Option<Rectangle> {
    if presentation == Presentation::Compact && upright() {
        return Some(stacked(vec![
            Box::new(
                Text::new(workspace.index.to_string())
                    .size(theme::text::LABEL)
                    .color(theme::island().on_surface)
                    .weight(theme::text::SEMIBOLD),
            ),
            pager(workspace, 6.0, true),
        ]));
    }

    let (size, dot, inset) = match presentation {
        Presentation::Compact => (theme::text::LABEL, 6.0, 17.0),
        Presentation::Peek => (theme::text::BODY_LARGE, 7.0, 22.0),
        _ => return None,
    };

    let number = format!("Workspace {}", workspace.index);

    let mut words = children![
        Text::new(workspace.name.as_deref().unwrap_or(&number))
            .size(size)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD)
            .elide()
    ];

    if presentation == Presentation::Peek && workspace.name.is_some() {
        words.push(Box::new(
            Text::new(number)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::MEDIUM),
        ));
    }

    Some(
        sized(presentation)
            .padding(Padding {
                top: 0.0,
                right: inset,
                bottom: 0.0,
                left: inset,
            })
            .align_child(Start, Center)
            .child(
                Row::new(vec![
                    Box::new(Column::new(words).width(Parent).gap(1.0)) as Box<dyn Widget>,
                    pager(workspace, dot, false),
                ])
                .width(Parent)
                .gap(12.0)
                .align(Center),
            ),
    )
}

/*
 * a dot per workspace and a longer mark for the focused one, or its number past `PAGER`; down
 * the form standing `upright`, where the number above it already says which
 */
fn pager(workspace: &Workspace, dot: f32, upright: bool) -> Box<dyn Widget> {
    let (mark, gap) = (dot * 8.0 / 3.0, dot * 5.0 / 6.0);

    if workspace.count > PAGER {
        let count = match upright {
            true => format!("of {}", workspace.count),
            false => format!("{} / {}", workspace.index, workspace.count),
        };

        return Box::new(
            Text::new(count)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::SEMIBOLD),
        );
    }

    let dots = (1..=workspace.count)
        .map(|index| {
            let focused = index == workspace.index;

            let long = if focused { mark } else { dot };
            let (width, height) = if upright { (dot, long) } else { (long, dot) };

            Box::new(
                Rectangle::new()
                    .width(width)
                    .height(height)
                    .radius(dot / 2.0)
                    .fill(if focused {
                        theme::island().on_surface
                    } else {
                        theme::island().on_surface_variant
                    }),
            ) as Box<dyn Widget>
        })
        .collect();

    if upright {
        Box::new(Column::new(dots).gap(gap).align(Center))
    } else {
        Box::new(Row::new(dots).gap(gap).align(Center))
    }
}

// a Volume or Brightness as one bar, the same layout for both
pub(super) struct Level {
    icon: Icon,
    label: &'static str,
    percent: u8,

    // muted: the bar and the number go quiet, the icon says why
    quiet: bool,
}

impl Level {
    pub(super) fn volume(volume: &Volume) -> Level {
        let (icon, label) = match volume.device {
            Device::Speaker => (Icon::Speaker(volume.percent), "Volume"),
            Device::Microphone => (Icon::Microphone, "Microphone"),
        };

        Level {
            icon: if volume.muted { icon.muted() } else { icon },
            label,
            percent: volume.percent,
            quiet: volume.muted,
        }
    }

    pub(super) fn brightness(percent: u8) -> Level {
        Level {
            icon: Icon::Sun,
            label: "Brightness",
            percent,
            quiet: false,
        }
    }

    pub(super) fn keyboard(percent: u8) -> Level {
        Level {
            icon: Icon::Keyboard,
            label: "Keyboard",
            percent,
            quiet: false,
        }
    }
}

/*
 * icon, bar, number; Peek names the level above its bar. Every part has a fixed width, so a level
 * that moves slides the bar and nothing else
 */
pub(super) fn level(presentation: Presentation, level: Level) -> Option<Rectangle> {
    let (icon, number, size, inset) = match presentation {
        Presentation::Compact => (20.0, 26.0, theme::text::LABEL, 15.0),
        Presentation::Peek => (24.0, 30.0, theme::text::BODY_LARGE, 20.0),
        _ => return None,
    };

    let gap = 11.0;
    let width = shape(presentation).width - 2.0 * inset - icon - number - 2.0 * gap;

    let tone = if level.quiet {
        theme::island().on_surface_variant
    } else {
        theme::island().on_surface
    };

    if presentation == Presentation::Compact && upright() {
        let length = shape(presentation).height - 100.0;

        return Some(stacked(children![
            level.icon.draw(icon),
            rising(length, f32::from(level.percent) / 100.0, tone),
            Text::new(level.percent.to_string())
                .size(theme::text::LABEL_SMALL)
                .color(tone)
                .weight(theme::text::SEMIBOLD),
        ]));
    }

    let bar = bar(width, f32::from(level.percent) / 100.0, tone);

    let middle: Box<dyn Widget> = match presentation {
        Presentation::Peek => Box::new(
            Column::new(children![
                Text::new(level.label)
                    .size(theme::text::LABEL_SMALL)
                    .color(theme::island().on_surface_variant)
                    .weight(theme::text::MEDIUM),
                bar,
            ])
            .gap(6.0),
        ),
        _ => Box::new(bar),
    };

    Some(
        sized(presentation)
            .padding(Padding {
                top: 0.0,
                right: inset,
                bottom: 0.0,
                left: inset,
            })
            .align_child(Start, Center)
            .child(
                Row::new(vec![
                    Box::new(level.icon.draw(icon)) as Box<dyn Widget>,
                    middle,
                    Box::new(
                        Rectangle::new()
                            .width(number)
                            .height(size)
                            .align_child(End, Center)
                            .child(
                                Text::new(level.percent.to_string())
                                    .size(size)
                                    .color(tone)
                                    .weight(theme::text::SEMIBOLD),
                            ),
                    ),
                ])
                .gap(gap)
                .align(Center),
            ),
    )
}

/*
 * a lamp, lit in the primary color while the mode is on, then its name and the word for which way
 * it went; Peek names it larger, the word under it. Every part has a fixed width, so a mode turned
 * back only relights the lamp and flips the word
 */
pub(super) fn mode(presentation: Presentation, mode: Mode) -> Option<Rectangle> {
    let (lamp, size, inset) = match presentation {
        Presentation::Compact => (26.0, theme::text::LABEL, 8.0),
        Presentation::Peek => (34.0, theme::text::BODY_LARGE, 12.0),
        _ => return None,
    };

    let (icon, name) = match mode {
        Mode::CapsLock(_) => (Icon::CapsLock, "Caps Lock"),
        Mode::NumLock(_) => (Icon::Keypad, "Num Lock"),
        Mode::Airplane(_) => (Icon::Airplane, "Airplane Mode"),
    };

    let roles = theme::island();
    let on = mode.on();
    let word = if on { "On" } else { "Off" };
    let glyph = lamp * 0.6;

    let lit = Rectangle::new()
        .width(lamp)
        .height(lamp)
        .radius(lamp / 2.0)
        .fill(if on {
            roles.primary
        } else {
            roles.surface_container_high
        })
        .align_child(Center, Center)
        .child(if on {
            icon.on(glyph, roles.on_primary)
        } else {
            icon.on(glyph, roles.on_surface_variant)
        });

    let title = Text::new(name)
        .size(size)
        .color(roles.on_surface)
        .weight(theme::text::SEMIBOLD);

    let state = Text::new(word)
        .size(theme::text::LABEL_SMALL)
        .color(roles.on_surface_variant)
        .weight(theme::text::MEDIUM);

    if presentation == Presentation::Compact && upright() {
        return Some(stacked(children![lit, state]));
    }

    let right = match presentation {
        Presentation::Peek => 20.0,
        _ => 15.0,
    };
    let gap = 10.0;
    let word_width = 28.0;
    let width = shape(presentation).width - inset - right - lamp - gap;

    let words: Box<dyn Widget> = match presentation {
        Presentation::Peek => Box::new(Column::new(children![title, state]).gap(3.0)),
        _ => Box::new(
            Row::new(children![
                Rectangle::new()
                    .width(width - word_width)
                    .height(size)
                    .align_child(Start, Center)
                    .child(title),
                Rectangle::new()
                    .width(word_width)
                    .height(size)
                    .align_child(End, Center)
                    .child(state),
            ])
            .align(Center),
        ),
    };

    Some(
        sized(presentation)
            .padding(Padding {
                top: 0.0,
                right,
                bottom: 0.0,
                left: inset,
            })
            .align_child(Start, Center)
            .child(
                Row::new(vec![Box::new(lit) as Box<dyn Widget>, words])
                    .gap(gap)
                    .align(Center),
            ),
    )
}

// `bar` standing, filled from the bottom, for a level along a side edge
fn rising(length: f32, fraction: f32, tone: Color) -> Rectangle {
    let width = 6.0;
    let filled = length * fraction.clamp(0.0, 1.0);

    let track = Rectangle::new()
        .width(width)
        .height(length)
        .radius(width / 2.0)
        .fill(theme::island().surface_container_high)
        .align_child(Center, End);

    // shorter than its round ends it would draw as a misshapen dot
    if filled >= width {
        track.child(
            Rectangle::new()
                .width(width)
                .height(filled)
                .radius(width / 2.0)
                .fill(tone),
        )
    } else {
        track
    }
}

// `fraction` of it filled, 0 to 1
pub(crate) fn bar(width: f32, fraction: f32, tone: Color) -> Stack {
    bar_on(width, fraction, tone, &theme::island())
}

// `bar`, its track in `roles`, for what draws beside the island
pub(crate) fn bar_on(width: f32, fraction: f32, tone: Color, roles: &ThemeRoles) -> Stack {
    let height = 6.0;
    let filled = width * fraction.clamp(0.0, 1.0);

    let track = Rectangle::new()
        .width(width)
        .height(height)
        .radius(height / 2.0)
        .fill(roles.surface_container_high);

    let mut layers = children![track];

    // narrower than its round ends it would draw as a misshapen dot
    if filled >= height {
        layers.push(Box::new(
            Rectangle::new()
                .width(filled)
                .height(height)
                .radius(height / 2.0)
                .fill(tone),
        ));
    }

    Stack::new(layers).width(width).height(height)
}
