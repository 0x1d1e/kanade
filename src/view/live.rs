//! The form of a live Activity from `kanade activity` (ADR 0042): its mark, its title, how far along
//! it is, and on Peek its detail with a bar. One with no amount shows no bar, as it cannot say.

use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Color, Column, End, Padding, Parent, Rectangle, Row, Start, Text, Widget, children,
};

use crate::icon::Icon;
use crate::island::activity::{Action, Ended, Job};
use crate::island::presentation::Presentation;
use crate::sources::live;
use crate::theme;

use super::forms::{Pills, capped_pill, pill_width, sized};
use super::satellites::label;
use super::shape::{shape, upright};
use super::standing::stacked;
use super::status::bar;

// the mark: a pulse while it runs, a check once done, a cross in the critical tone once failed
pub(super) fn mark(job: &Job) -> (Icon, Color) {
    match job.ended {
        None => (Icon::Pulse, theme::island().on_surface),
        Some(Ended::Done) => (Icon::Check, theme::island().on_surface),
        Some(Ended::Failed) => (Icon::Dismiss, theme::signals().critical),
    }
}

// how far along, while it runs and says
fn percent(job: &Job) -> Option<String> {
    match (job.ended, job.progress) {
        (None, Some(thousandths)) => Some(format!("{}%", thousandths / 10)),
        _ => None,
    }
}

// a Satellite's mark: how far along, or its pulse
pub(super) fn satellite(job: &Job) -> Box<dyn Widget> {
    match percent(job) {
        Some(percent) => label(percent, theme::island().on_surface),
        None => {
            let (icon, ink) = mark(job);
            Box::new(icon.on(14.0, ink))
        }
    }
}

// the line under the title: its detail, or how it ended
fn line(job: &Job) -> String {
    match (job.ended, job.detail.is_empty()) {
        (None, _) => job.detail.clone(),
        (Some(Ended::Done), true) => String::from("Done"),
        (Some(Ended::Failed), true) => String::from("Failed"),
        (Some(Ended::Done), false) => format!("Done - {}", job.detail),
        (Some(Ended::Failed), false) => format!("Failed - {}", job.detail),
    }
}

// the least the title keeps when pills share its Peek
const TITLE: f32 = 72.0;

/*
 * Peek replaces progress with action pills, truncating the first to preserve `TITLE` width; progress
 * keeps fixed width to avoid shifting. Presses are queued for the Activity's posting program.
 */
pub(super) fn live(
    presentation: Presentation,
    job: &Job,
    actions: &[Action],
    id: &str,
) -> Option<Rectangle> {
    let (icon, ink) = mark(job);
    let percent = percent(job);

    if presentation == Presentation::Compact && upright() {
        let mut parts = children![icon.on(18.0, ink)];
        parts.extend(percent.map(|percent| label(percent, theme::island().on_surface)));

        return Some(noted(stacked(parts), job));
    }

    let (side, size, inset, number) = match presentation {
        Presentation::Compact => (18.0, theme::text::LABEL, 15.0, 40.0),
        Presentation::Peek => (24.0, theme::text::BODY_LARGE, 20.0, 48.0),
        _ => return None,
    };

    let gap = 11.0;
    let room = shape(presentation).width - 2.0 * inset - side - gap;

    // the most a pill takes, so the title keeps `TITLE` beside even the first
    let cap = (room - gap - TITLE).max(0.0);

    let (pills, percent) = if presentation == Presentation::Peek && !actions.is_empty() {
        let mut taken = 0.0;
        let shown = actions
            .iter()
            .take_while(|action| {
                let next = taken + pill_width(action).min(cap) + gap;
                let fits = room - next >= TITLE;

                if fits {
                    taken = next;
                }

                fits
            })
            .count();

        (&actions[..shown], None)
    } else {
        (&actions[..0], percent)
    };

    let amount = if percent.is_some() { number + gap } else { 0.0 };
    let beside: f32 = pills
        .iter()
        .map(|action| pill_width(action).min(cap) + gap)
        .sum();
    let width = room - amount - beside;

    let mut words = children![
        Text::new(&job.title)
            .size(theme::text::LABEL)
            .color(theme::island().on_surface)
            .weight(theme::text::SEMIBOLD)
            .elide()
    ];

    if presentation == Presentation::Peek {
        let line = line(job);

        if !line.is_empty() {
            words.push(Box::new(
                Text::new(line)
                    .size(theme::text::LABEL_SMALL)
                    .color(theme::island().on_surface_variant)
                    .weight(theme::text::MEDIUM)
                    .elide(),
            ));
        }

        if let (None, Some(thousandths)) = (job.ended, job.progress) {
            words.push(Box::new(bar(
                width,
                f32::from(thousandths) / 1000.0,
                theme::island().on_surface,
            )));
        }
    }

    let mut row = children![icon.on(side, ink), Column::new(words).width(width).gap(4.0)];

    let subject = format!("{id}#{}", job.serial);
    let held = Pills {
        actions: pills,
        subject: &subject,
        act: live::act,
    };

    // always the neutral pill, whatever the program says: it cannot pass for a control of the shell
    row.extend(
        pills
            .iter()
            .map(|action| Box::new(capped_pill(action, &held, cap)) as Box<dyn Widget>),
    );

    if let Some(percent) = percent {
        row.push(Box::new(
            Rectangle::new()
                .width(number)
                .height(size)
                .align_child(End, Center)
                .child(
                    Text::new(percent)
                        .size(size)
                        .color(theme::island().on_surface)
                        .weight(theme::text::SEMIBOLD),
                ),
        ));
    }

    Some(noted(
        sized(presentation)
            .padding(Padding {
                top: 0.0,
                right: inset,
                bottom: 0.0,
                left: inset,
            })
            .align_child(Start, Center)
            .child(Row::new(row).width(Parent).gap(gap).align(Center)),
        job,
    ))
}

// the detail is what a program sent, so `ui.describe` redacts it unless content mode
fn noted(node: Rectangle, job: &Job) -> Rectangle {
    let line = line(job);

    if line.is_empty() {
        node
    } else {
        node.semantics(|| Semantics::new(Role::Item, "detail").content(line))
    }
}
