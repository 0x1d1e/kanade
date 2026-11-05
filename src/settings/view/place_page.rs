//! The Weather place finder's row (ADR 0136, `settings::place`): a name field with Find, the
//! matches to pick from, and Use my location, with what each sends and where said beside it.
//! Nothing here asks the network; a press posts to the worker, and the row shows it is working.

use super::*;

use crate::settings::place::{self, Busy, Said};

// the name field's width, as the other text fields
const WIDE: f32 = 260.0;

pub fn row(settings: &Settings, roles: &ThemeRoles) -> Box<dyn Widget> {
    let finder = &settings.place;
    let body = inner();
    let free = finder.busy.is_none();

    let find = |typed: String| {
        if !typed.trim().is_empty() {
            place::search(&typed);
        }
    };

    let mut items = children![
        Text::new("Find a place")
            .size(theme::text::BODY)
            .color(roles.on_surface)
            .weight(theme::text::SEMIBOLD),
        Row::new(children![
            text_field(place::INPUT, WIDE, "Phnom Penh, Cambodia", roles, find),
            action("Find", "find place", settings, roles, free, || {
                place::search(&TextInput::text(place::INPUT))
            },),
        ])
        .gap(8.0)
        .align(Center),
        hairline(inner(), roles),
        Row::new(children![action(
            "Use my location",
            "use my location",
            settings,
            roles,
            free,
            place::locate,
        )])
        .align(Center),
    ];

    let (line, ink) = match (&finder.busy, &finder.said) {
        (Some(Busy::Searching), _) => (Some(String::from("Searching\u{2026}")), roles.on_surface),
        (Some(Busy::Locating), _) => (Some(String::from("Locating\u{2026}")), roles.on_surface),
        (
            None,
            Some(Said::Set {
                label,
                from_address,
            }),
        ) => (
            Some(if *from_address {
                format!("Location set to {label}")
            } else {
                format!("Weather set to {label}")
            }),
            roles.primary,
        ),
        (None, Some(Said::Failed(why))) => (Some(why.clone()), theme::window_signals().warning),
        (None, None) => (None, roles.on_surface),
    };

    if let Some(line) = line {
        items.push(Box::new(space(6.0)));
        items.push(Box::new(wrapped(
            Text::new(line)
                .size(theme::text::LABEL)
                .color(ink)
                .weight(theme::text::SEMIBOLD),
            body.min(MOST_MEASURE),
        )));
    }

    if !finder.results.is_empty() {
        items.push(Box::new(space(6.0)));

        for (index, found) in finder.results.iter().enumerate() {
            let label = found.label();
            let chosen = found.clone();
            let name = label.clone();

            items.push(button(
                Rectangle::new()
                    .width(body.min(MOST_MEASURE))
                    .height(32.0)
                    .radius(radius::ROW - 4.0)
                    .padding(sides(10.0))
                    .align_child(Start, Center)
                    .semantics(move || Semantics::new(Role::Button, "place").label(name.clone()))
                    .child(
                        Text::new(label)
                            .size(theme::text::LABEL)
                            .color(roles.on_surface)
                            .weight(theme::text::MEDIUM)
                            .elide(),
                    ),
                format!("pick place {index}"),
                move || place::pick(&chosen),
                Some(roles.edge()),
                settings,
                roles,
            ));
        }
    }

    Box::new(Column::new(items).width(body).gap(8.0))
}

// a labelled button that does nothing while the finder works
fn action(
    label: &'static str,
    node: &'static str,
    settings: &Settings,
    roles: &ThemeRoles,
    enabled: bool,
    press: impl Fn() + 'static,
) -> Box<dyn Widget> {
    let text = Text::new(label)
        .size(theme::text::LABEL)
        .color(if enabled {
            roles.on_surface
        } else {
            roles.on_surface_variant
        })
        .weight(theme::text::SEMIBOLD);
    let width = fixed(Widget::width(&text)) + 24.0;

    button(
        Rectangle::new()
            .width(width)
            .height(FIELD)
            .radius(radius::ROW - 2.0)
            .semantics(move || Semantics::new(Role::Button, node).disabled(!enabled))
            .align_child(Center, Center)
            .child(text),
        format!("place {node}"),
        move || {
            if enabled {
                press();
            }
        },
        Some(roles.edge()),
        settings,
        roles,
    )
}
