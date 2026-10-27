//! The Satellites beside the body.

use std::time::Instant;

use kanade_runtime::{Center, Color, Rectangle, Row, Text, Widget};

use crate::icon::Icon;
use crate::island::activity::{Activity, Class, Clip, Detail, Kind, Leaving};
use crate::island::geometry::{self, Canvas, Hang, Rect, Shape};
use crate::island::satellites::{Mark, Satellites};
use crate::pane;
use crate::sources::{session, timer};
use crate::theme;

use super::status::{charge_tone, timer_tone};

// the Satellites, then the ones past the cap as one "+N"
pub(super) fn satellites(
    satellites: &Satellites,
    output: &str,
    body: Rect,
    hang: Hang,
    canvas: Canvas,
    shape: Shape,
    now: Instant,
) -> Vec<Box<dyn Widget>> {
    let opacity = geometry::satellite_opacity(shape, hang);

    if opacity == 0.0 {
        return Vec::new();
    }

    let placed: Vec<_> = satellites
        .shown(now)
        .into_iter()
        .map(|shown| {
            let mark = match shown.mark {
                Mark::Activity(activity) => satellite_mark(activity, output, now),
                Mark::Overflow(count) => label(format!("+{count}"), theme::island().on_surface),
            };
            let at = geometry::satellite(body, hang, canvas, shown.slot, shown.presence);

            (shown.slot, at, mark, opacity * shown.opacity)
        })
        .collect();

    /*
     * one liquid shape with the body, whose neck stretches and snaps as a dot comes out, where the
     * finish and motion allow it; else each dot its own fill
     */
    let mut by_slot: Vec<_> = placed
        .iter()
        .map(|(slot, at, _, opacity)| (*slot, *at, *opacity))
        .collect();

    by_slot.sort_by(|a, b| a.0.total_cmp(&b.0));

    let blob = pane::blob(
        pane::Body {
            x: body.x,
            y: body.y,
            width: body.width,
            height: body.height,
            radius: shape.radius,
            join: None,
        },
        geometry::SATELLITE_GAP,
        &by_slot
            .into_iter()
            .map(|(_, at, opacity)| pane::Dot {
                x: at.x,
                y: at.y,
                diameter: at.width,
                opacity,
            })
            .collect::<Vec<_>>(),
    );
    let joined = blob.is_some();

    blob.map(|blob| Box::new(blob) as Box<dyn Widget>)
        .into_iter()
        .chain(placed.into_iter().map(|(_, at, mark, opacity)| {
            let dot = dot(at, mark, !joined);

            Box::new(dot.opacity(opacity)) as Box<dyn Widget>
        }))
        .collect()
}

/*
 * a battery shows its number in its tone, a timer, countdown or reminder what is left, a
 * recording that it records in the capture tone, caffeine its cup, media its cover as a disc, turning while it plays, a live Activity how far along
 * or its mark, the night light its sunset, a refusal or no answer a mark in the critical tone, the
 * rest their Kind
 */
pub(super) fn satellite_mark(activity: &Activity, output: &str, now: Instant) -> Box<dyn Widget> {
    match activity.detail() {
        // the mark of a paused recording is the pause, as a label that long would not fit
        Detail::Recording(Clip::Recording { paused: true, .. }) => {
            Box::new(Icon::Pause.on(14.0, theme::signals().capture))
        }
        Detail::Recording(_) => label(String::from("Rec"), theme::signals().capture),
        Detail::Caffeine(_) => Box::new(Icon::Cup.draw(14.0)),
        Detail::Dictation(_) => Box::new(Icon::Microphone.draw(14.0)),
        Detail::Media(track) => Box::new(super::media::disc(track, output, 20.0, now)),
        Detail::Live(job) => super::live::satellite(job),
        Detail::NightLight(_) => Box::new(Icon::Sunset.draw(14.0)),
        Detail::Hardware(event) => Box::new(mark(event.class).draw(14.0)),
        Detail::Reminder(reminder) => label(reminder.short.clone(), theme::island().on_surface),
        Detail::Battery(charge) => label(charge.percent.to_string(), charge_tone(charge)),
        Detail::Timer(countdown, _) => label(timer::short(countdown, now), timer_tone(countdown)),
        Detail::Session(Leaving::Counting { countdown, .. }) => label(
            format!("{}s", session::left(countdown, now)),
            theme::island().on_surface,
        ),
        Detail::Session(Leaving::Failed { .. } | Leaving::Unanswered { .. }) => {
            label(String::from("!"), theme::signals().critical)
        }
        _ => label(
            abbreviation(activity.kind()).to_owned(),
            theme::island().on_surface,
        ),
    }
}

// the mark of a device event's class
pub(super) fn mark(class: Class) -> Icon {
    match class {
        Class::Bluetooth => Icon::Bluetooth,
        Class::Charger => Icon::Bolt,
        Class::Output => Icon::Speaker(1),
        Class::Wifi => Icon::Wifi,
    }
}

pub(super) fn label(text: String, tone: Color) -> Box<dyn Widget> {
    Box::new(
        Text::new(text)
            .size(theme::text::LABEL_SMALL)
            .color(tone)
            .weight(theme::text::SEMIBOLD),
    )
}

// a Satellite's dot; without its own fill when the liquid shape under it draws that
pub(super) fn dot(at: Rect, mark: Box<dyn Widget>, filled: bool) -> Rectangle {
    let dot = Rectangle::new()
        .width(at.width)
        .height(at.height)
        .radius(at.width / 2.0);

    let dot = if filled {
        // the body's own fill and rim, so a dot standing apart is the same pane
        dot.fill(theme::island().surface)
            .border(1.0, theme::pane_rim())
    } else {
        dot
    };

    dot.align_child(Center, Center)
        .translate(at.x, at.y)
        .child(Row::new(vec![mark]))
}

// stand-in for each Kind's glyph, distinct per Kind
pub(super) fn abbreviation(kind: Kind) -> &'static str {
    match kind {
        Kind::Media => "M",
        Kind::Notification => "N",
        Kind::Volume => "V",
        Kind::Brightness => "Br",
        Kind::Workspace => "W",
        Kind::Battery => "Ba",
        Kind::Network => "Nw",
        Kind::Bluetooth => "Bt",
        Kind::Timer => "T",
        Kind::Recording => "Rec",
        Kind::Caffeine => "Cf",
        Kind::Dictation => "Dc",
        Kind::Session => "Ss",
        Kind::Mode => "Md",
        Kind::Live => "Lv",
        Kind::NightLight => "Nl",
        Kind::Reminder => "Rm",
    }
}
