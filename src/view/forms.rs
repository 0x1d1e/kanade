//! The forms an Activity takes in the Island: small forms, Split, and each kind's Compact and Peek.

use std::path::Path;
use std::time::Instant;

use kanade_runtime::{
    Button, Center, Color, Column, Cursor, Justify, Padding, Parent, Rectangle, Row, Size, Stack,
    Start, Text, Widget, children,
};

use crate::icon::Icon;
use crate::island::activity::{Action, Activity, Awake, Clip, Detail, Leave, Leaving, Shot, Track};
use crate::island::fade::Dissolve;
use crate::island::geometry::{self, Rect};
use crate::island::presentation::{Content, Presentation, Surface};
use crate::island::service::IslandService;
use crate::modules;
use crate::sources::{caffeine, capture, session};
use crate::surfaces;
use crate::theme;

use super::media::{Change, media_compact, media_peek, tile};
use super::rest::placeholder;
use super::satellites::satellite_mark;
use super::shape::{Room, hang, shape, upright};
use super::status::{Level, battery, battery_icon, charge_tone, level, mode, timer, workspace};
use super::toast::{critical, toast_compact, toast_peek};

/*
 * the open Surface's own content, from the view's read of the island; the Surface open now differs
 * from the content's while it fades out
 */
pub(super) fn surface(
    monitor: &str,
    content: &Content,
    island: &IslandService,
    now: Instant,
) -> Option<Rectangle> {
    let Presentation::Expanded(surface) = content.presentation else {
        return None;
    };

    let open = island.surface() == Some(surface);

    match surface {
        Surface::Media => Some(surfaces::media::surface(open, now)),
        Surface::Notifications => Some(surfaces::notifications::surface(
            monitor,
            open,
            island.visit(),
            island.held(monitor),
            island.dnd(),
        )),
        Surface::Controls => Some(surfaces::controls::surface(
            open,
            island.visit(),
            island.held(monitor),
            modules::on("notifications").then(|| island.dnd()),
        )),
        Surface::Launcher => Some(surfaces::launcher::surface(monitor, island.visit())),
        Surface::Tray => Some(surfaces::tray::surface(
            open,
            island.visit(),
            island.held(monitor),
        )),
        Surface::Clipboard => Some(surfaces::clipboard::surface(monitor, island.visit())),
        Surface::Calendar => Some(surfaces::calendar::surface(monitor, island.visit())),
        Surface::Weather => Some(surfaces::weather::surface(monitor)),
        Surface::Session => Some(surfaces::session::surface(
            open,
            island.visit(),
            island.held(monitor),
        )),
    }
}

/*
 * a Compact standing along a side edge: its parts down its length from the round end, centered
 * across it, a lead tile concentric with the end. Its words wait for the Peek
 */
pub(super) fn stacked(parts: Vec<Box<dyn Widget>>) -> Rectangle {
    sized(Presentation::Compact)
        .padding(Padding {
            top: STACK_INSET,
            bottom: STACK_INSET + 5.0,
            ..Padding::default()
        })
        .align_child(Center, Start)
        .child(
            Column::new(parts)
                .height(Parent)
                .justify(Justify::SpaceBetween)
                .align(Center),
        )
}

// a lead tile's inset from a small form's round end
const STACK_INSET: f32 = 7.0;

// the side of a lead tile in an upright Compact
pub(super) fn stacked_tile() -> f32 {
    shape(Presentation::Compact).width - 2.0 * STACK_INSET
}

// content is laid out at its Presentation's final size, which the morph reveals
pub(super) fn sized(presentation: Presentation) -> Rectangle {
    let size = shape(presentation);

    Rectangle::new().width(size.width).height(size.height)
}

/*
 * the primary's Compact form leading and the top Satellite's segment trailing; while they trade
 * places (`swap` above 0) each slides from where the other stands
 */
pub(super) fn split(
    content: &Content,
    track: Option<&Dissolve<Track>>,
    swap: f32,
    now: Instant,
) -> Option<Rectangle> {
    if content.presentation != Presentation::Split {
        return None;
    }

    let leading = Content {
        presentation: Presentation::Compact,
        activity: content.activity.clone(),
        satellite: None,
    };
    let hang = hang();
    let trailing = geometry::trailing(hang);

    // how far along the body the trailing segment starts, across the top or down a side edge
    let sideways = hang.sideways();
    let along = if sideways { trailing.y } else { trailing.x } - Room::taken();
    let at = |along: f32, inset: f32| {
        if sideways {
            (inset, along)
        } else {
            (along, inset)
        }
    };

    let mut segments: Vec<Box<dyn Widget>> = Vec::new();

    if let Some(form) = small_form(&leading, track, now).or_else(|| placeholder(&leading)) {
        let (x, y) = at(swap * along, 0.0);

        segments.push(Box::new(form.translate(x, y)));
    }

    if let Some(satellite) = &content.satellite {
        let (x, y) = at(along * (1.0 - swap), geometry::SEGMENT_INSET);

        segments.push(Box::new(segment(satellite, trailing, now).translate(x, y)));
    }

    Some(
        sized(Presentation::Split)
            .align_child(Start, Start)
            .child(Stack::new(segments)),
    )
}

// the top Satellite grown into the body's end: what it is, then its mark
pub(super) fn segment(activity: &Activity, at: Rect, now: Instant) -> Rectangle {
    let mut row = Vec::<Box<dyn Widget>>::new();

    match activity.detail() {
        Detail::Timer(_) => row.push(Box::new(Icon::Stopwatch.draw(14.0))),
        Detail::Session(Leaving::Counting { end, .. }) => {
            row.push(Box::new(
                surfaces::session::icon(Leave::Ending(*end)).draw(14.0),
            ));
        }
        Detail::Session(Leaving::Failed { leave, .. } | Leaving::Unanswered { leave, .. }) => {
            row.push(Box::new(surfaces::session::icon(*leave).draw(14.0)));
        }
        Detail::Recording(_) => row.push(Box::new(Icon::Capture.on(14.0, theme::SEMANTIC.capture))),
        Detail::Battery(charge) => row.push(Box::new(battery_icon(
            16.0,
            charge.percent,
            charge_tone(charge),
        ))),
        _ => {}
    }

    row.push(satellite_mark(activity, now));

    let segment = Rectangle::new()
        .width(at.width)
        .height(at.height)
        .radius(at.width.min(at.height) / 2.0)
        .fill(theme::island().surface_container_high)
        .align_child(Center, Center);

    if upright() {
        segment.child(Column::new(row).gap(4.0).align(Center))
    } else {
        segment.child(Row::new(row).gap(4.0).align(Center))
    }
}

// an Activity's own Compact or Peek, drawn from its Detail; none leaves it to the placeholder
pub(super) fn small_form(
    content: &Content,
    track: Option<&Dissolve<Track>>,
    now: Instant,
) -> Option<Rectangle> {
    let detail = content.activity.as_ref().map(Activity::detail);

    match (content.presentation, detail) {
        (Presentation::Compact, Some(Detail::Media(shown))) => {
            Some(media_compact(Change::of(shown, track, now), now))
        }
        (Presentation::Peek, Some(Detail::Media(shown))) => {
            Some(media_peek(Change::of(shown, track, now), now))
        }
        (Presentation::Compact, Some(Detail::Notification(toast))) => {
            Some(toast_compact(toast, critical(content)))
        }
        (Presentation::Peek, Some(Detail::Notification(toast))) => {
            Some(toast_peek(toast, critical(content)))
        }
        (presentation, Some(Detail::Volume(volume))) => level(presentation, Level::volume(volume)),
        (presentation, Some(&Detail::Brightness(percent))) => {
            level(presentation, Level::brightness(percent))
        }
        (presentation, Some(&Detail::Keyboard(percent))) => {
            level(presentation, Level::keyboard(percent))
        }
        (presentation, Some(&Detail::Mode(shown))) => self::mode(presentation, shown),
        (presentation, Some(Detail::Battery(charge))) => battery(presentation, charge),
        (presentation, Some(Detail::Workspace(workspace))) => {
            self::workspace(presentation, workspace)
        }
        (presentation, Some(Detail::Timer(countdown))) => self::timer(presentation, countdown, now),
        (presentation, Some(Detail::Screenshot(shot))) => {
            let actions = content.activity.as_ref().map_or(&[][..], Activity::actions);

            screenshot(presentation, shot, actions)
        }
        (presentation, Some(Detail::Recording(clip))) => {
            let actions = content.activity.as_ref().map_or(&[][..], Activity::actions);

            recording(presentation, clip, actions)
        }
        (presentation, Some(Detail::Caffeine(awake))) => {
            let actions = content.activity.as_ref().map_or(&[][..], Activity::actions);

            caffeine(presentation, awake, actions)
        }
        (presentation, Some(Detail::Session(leaving))) => {
            let actions = content.activity.as_ref().map_or(&[][..], Activity::actions);

            self::session(presentation, leaving, actions, now)
        }
        _ => None,
    }
}

// the picture, then what it is
fn screenshot(presentation: Presentation, shot: &Shot, actions: &[Action]) -> Option<Rectangle> {
    let title = match shot.copied {
        true => "Path copied",
        false => "Screenshot",
    };

    titled(
        presentation,
        Lead::Picture(&shot.path),
        title,
        file_name(&shot.path),
        Pills {
            actions,
            subject: &shot.path,
            act: capture::act,
        },
    )
}

// the capture mark, then what it is: recording the output named, saved to the file named, or why not
pub(super) fn recording(
    presentation: Presentation,
    clip: &Clip,
    actions: &[Action],
) -> Option<Rectangle> {
    let (title, line) = match clip {
        Clip::Recording { output, .. } => ("Recording", output.as_str()),
        Clip::Saved { path, copied } => (
            match copied {
                true => "Path copied",
                false => "Recording saved",
            },
            file_name(path),
        ),
        Clip::Failed { why, .. } => ("Recording failed", why.as_str()),
    };

    titled(
        presentation,
        Lead::Mark(Icon::Capture, theme::SEMANTIC.capture),
        title,
        line,
        Pills {
            actions,
            subject: clip.path(),
            act: capture::act,
        },
    )
}

// the cup, then that caffeine is on and for how long, or why it ended
pub(super) fn caffeine(
    presentation: Presentation,
    awake: &Awake,
    actions: &[Action],
) -> Option<Rectangle> {
    let (title, line, subject) = match awake {
        Awake::On {
            serial,
            length: Some(length),
        } => (
            "Caffeine",
            format!("Awake for {}", caffeine::length(*length)),
            serial.as_str(),
        ),
        Awake::On {
            serial,
            length: None,
        } => (
            "Caffeine",
            String::from("Awake until turned off"),
            serial.as_str(),
        ),
        Awake::Failed { why } => ("Caffeine failed", why.clone(), ""),
    };

    titled(
        presentation,
        Lead::Mark(Icon::Cup, theme::island().on_surface),
        title,
        &line,
        Pills {
            actions,
            subject,
            act: caffeine::act,
        },
    )
}

// what counts down, then how long is left, or why it was refused
pub(super) fn session(
    presentation: Presentation,
    leaving: &Leaving,
    actions: &[Action],
    now: Instant,
) -> Option<Rectangle> {
    let (leave, title, line, subject) = match leaving {
        Leaving::Counting {
            end,
            countdown,
            serial,
        } => (
            Leave::Ending(*end),
            format!(
                "{} in {} s",
                session::doing(*end),
                session::left(countdown, now)
            ),
            "Unless cancelled",
            serial.as_str(),
        ),
        Leaving::Failed { leave, why, serial } => (
            *leave,
            format!("{} failed", surfaces::session::name(*leave)),
            why.as_str(),
            serial.as_str(),
        ),
        Leaving::Unanswered { leave, serial } => (
            *leave,
            format!("{} may still happen", surfaces::session::name(*leave)),
            session::NO_ANSWER,
            serial.as_str(),
        ),
    };

    titled(
        presentation,
        Lead::Mark(surfaces::session::icon(leave), theme::island().on_surface),
        &title,
        line,
        Pills {
            actions,
            subject,
            act: session::act,
        },
    )
}

fn file_name(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
}

// what leads a titled small form
enum Lead<'a> {
    // the saved picture itself
    Picture(&'a str),

    // an icon in its ink, like the capture mark as the privacy cluster shows it
    Mark(Icon, Color),
}

// an Activity's actions, each run by its source's `act` with the key and what it acts on
struct Pills<'a> {
    actions: &'a [Action],
    subject: &'a str,
    act: fn(&str, &str),
}

/*
 * the lead, what it is and, on Peek, a line about it and a pill per action. Copying says so in
 * place of what it is, so the press shows it worked
 */
fn titled(
    presentation: Presentation,
    lead: Lead,
    title: &str,
    line: &str,
    pills: Pills,
) -> Option<Rectangle> {
    let (inset, right, radius, size, weight) = match presentation {
        Presentation::Compact => (
            7.0,
            15.0,
            theme::radius::ART_COMPACT,
            theme::text::LABEL,
            theme::text::MEDIUM,
        ),
        Presentation::Peek => (
            7.0,
            14.0,
            theme::radius::ART_PEEK,
            theme::text::BODY,
            theme::text::SEMIBOLD,
        ),
        _ => return None,
    };
    if presentation == Presentation::Compact && upright() {
        return Some(stacked(vec![lead_tile(lead, stacked_tile(), radius)]));
    }

    let shape = shape(presentation);
    let side = shape.height - 2.0 * inset;

    let mut lines = children![
        Text::new(title)
            .size(size)
            .color(theme::island().on_surface)
            .weight(weight)
    ];
    let mut row = vec![lead_tile(lead, side, radius)];

    if presentation == Presentation::Peek {
        lines.push(Box::new(
            Text::new(line)
                .size(theme::text::LABEL_SMALL)
                .color(theme::island().on_surface_variant)
                .weight(theme::text::MEDIUM)
                .elide(),
        ));
    }

    row.push(Box::new(Column::new(lines).width(Parent).gap(1.0)));

    if presentation == Presentation::Peek {
        row.extend(
            pills
                .actions
                .iter()
                .map(|action| Box::new(action_pill(action, &pills)) as Box<dyn Widget>),
        );
    }

    Some(
        sized(presentation)
            .padding(Padding {
                top: 0.0,
                right,
                bottom: 0.0,
                left: inset,
            })
            .align_child(Start, Center)
            .child(Row::new(row).width(Parent).gap(9.0).align(Center)),
    )
}

// the picture or the mark that leads a titled small form
fn lead_tile(lead: Lead, side: f32, radius: f32) -> Box<dyn Widget> {
    match lead {
        Lead::Picture(picture) => Box::new(tile(Some(picture), "", side, radius, &theme::island())),
        Lead::Mark(icon, ink) => Box::new(
            Rectangle::new()
                .width(side)
                .height(side)
                .radius(radius)
                .fill(theme::island().surface_container_high)
                .align_child(Center, Center)
                .child(icon.on((side * 0.55).round(), ink)),
        ),
    }
}

// an action, run on a left click before the click reaches the island
fn action_pill(action: &Action, pills: &Pills) -> Rectangle {
    let label = Text::new(&action.label)
        .size(theme::text::LABEL_SMALL)
        .color(theme::island().on_surface)
        .weight(theme::text::SEMIBOLD);
    let width = match label.width() {
        Size::Fixed(natural) => natural + 20.0,
        _ => theme::space::TARGET,
    };
    let (key, subject, act) = (action.key.clone(), pills.subject.to_owned(), pills.act);

    Rectangle::new()
        .width(width.ceil())
        .height(theme::space::TARGET)
        .radius(theme::space::TARGET / 2.0)
        .fill(theme::island().surface_container_high)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(move |button| {
            if button == Button::Left {
                act(&key, &subject);
            }
        })
        .child(label)
}
