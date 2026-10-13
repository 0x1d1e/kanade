//! The Satellites beside the body.

use std::time::Instant;

use kanade_runtime::{Center, Color, Rectangle, Row, Text, Widget};

use crate::icon::Icon;
use crate::island::activity::{Activity, Detail, Kind, Leaving};
use crate::island::geometry::{self, Canvas, Hang, Rect, Shape};
use crate::island::satellites::{Mark, Satellites};
use crate::sources::{session, timer};
use crate::theme;

use super::status::{charge_tone, timer_tone};

// the Satellites, then the ones past the cap as one "+N"
pub(super) fn satellites(
    satellites: &Satellites,
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

    satellites
        .shown(now)
        .into_iter()
        .map(|shown| {
            let mark = match shown.mark {
                Mark::Activity(activity) => satellite_mark(activity, now),
                Mark::Overflow(count) => label(format!("+{count}"), theme::island().on_surface),
            };
            let at = geometry::satellite(body, hang, canvas, shown.slot, shown.presence);

            Box::new(dot(at, mark).opacity(opacity * shown.opacity)) as Box<dyn Widget>
        })
        .collect()
}

/*
 * a battery shows its number in its tone, a timer, countdown or reminder what is left, a
 * recording that it records in the capture tone, caffeine its cup, a live Activity how far along
 * or its mark, the night light its sunset, a refusal or no answer a mark in the critical tone, the
 * rest their Kind
 */
pub(super) fn satellite_mark(activity: &Activity, now: Instant) -> Box<dyn Widget> {
    match activity.detail() {
        Detail::Recording(_) => label(String::from("Rec"), theme::signals().capture),
        Detail::Caffeine(_) => Box::new(Icon::Cup.draw(14.0)),
        Detail::Live(job) => super::live::satellite(job),
        Detail::NightLight(_) => Box::new(Icon::Sunset.draw(14.0)),
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

pub(super) fn label(text: String, tone: Color) -> Box<dyn Widget> {
    Box::new(
        Text::new(text)
            .size(theme::text::LABEL_SMALL)
            .color(tone)
            .weight(theme::text::SEMIBOLD),
    )
}

pub(super) fn dot(at: Rect, mark: Box<dyn Widget>) -> Rectangle {
    Rectangle::new()
        .width(at.width)
        .height(at.height)
        .radius(at.width / 2.0)
        .fill(theme::island().surface_container_high)
        .align_child(Center, Center)
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
        Kind::Screenshot => "Sc",
        Kind::Recording => "Rec",
        Kind::Caffeine => "Cf",
        Kind::Session => "Ss",
        Kind::Mode => "Md",
        Kind::Live => "Lv",
        Kind::NightLight => "Nl",
        Kind::Reminder => "Rm",
    }
}
