//! The niri page's rows: each an option of niri's `config.kdl` (`settings::niri`) drawn by what it
//! takes. A press posts the change to the worker and the row says it is saving until the result
//! comes back; nothing here reads the file or calls the engine.

use super::*;

use crate::settings::niri::{Control, Current, Item, Number, Shown};

// the footer's words on the page
pub fn status(settings: &Settings, roles: &ThemeRoles) -> (String, Color) {
    let said = |(text, warns): (String, bool)| {
        (
            text,
            if warns {
                theme::window_signals().warning
            } else {
                roles.primary
            },
        )
    };

    // a change waiting for Keep comes first, then niri's own words, then what the monitors last did
    if let Some(trial) = settings.outputs.countdown() {
        return said(trial);
    }

    match settings.niri.status() {
        Some((text, true)) => (text, theme::window_signals().warning),
        Some((text, false)) => (text, roles.on_surface_variant),
        None => match settings.outputs.notice.clone() {
            Some((text, true)) => (text, theme::window_signals().warning),
            Some((text, false)) => (text, roles.on_surface_variant),
            None => (niri::STANDING.to_owned(), roles.on_surface_variant),
        },
    }
}

// why the rows cannot be read or changed, in one box
pub fn notices(settings: &Settings) -> Vec<Box<dyn Widget>> {
    let niri = &settings.niri;
    let warning = theme::window_signals().warning;

    let notice = if let Some(why) = &niri.unreadable {
        Some((format!("niri's config cannot be read: {why}"), warning))
    } else if let Some(why) = &niri.not_editable {
        Some((
            format!("niri's config is read-only here: {why}. The values shown are what it holds"),
            warning,
        ))
    } else if !niri.loaded {
        Some((
            String::from("Reading niri's config\u{2026}"),
            theme::roles().on_surface_variant,
        ))
    } else {
        None
    };

    notice
        .map(|(text, ink)| Box::new(super::notice(&text, ink)) as Box<dyn Widget>)
        .into_iter()
        .collect()
}

// whether an option's name, help or path mentions the query
pub fn mentions(path: &str, query: &str) -> bool {
    let mentions = |text: &str| text.to_lowercase().contains(query);

    "niri".contains(query)
        || niri::item(path)
            .is_some_and(|item| mentions(item.label) || mentions(item.help) || mentions(item.path))
}

pub fn row(path: &'static str, settings: &Settings, roles: &ThemeRoles) -> Box<dyn Widget> {
    let item = niri::item(path).expect("the layout's niri rows are the page's options");
    let shown = settings.niri.shown(item);
    let enabled = shown.editable && !shown.busy;

    let control = match &item.control {
        Control::Flag | Control::Drawn { .. } => flag(item, &shown, enabled, roles),
        Control::Number(spec) => number(item, spec, &shown, enabled, roles),
        Control::Choice { options, .. } => {
            let chosen = match &shown.current {
                Current::Text(text) => text.as_str(),
                _ => "",
            };
            let choose = move |option: &'static str| {
                niri::change(path, vec![(path, Some(niri::quoted(option)))]);
            };

            segmented(
                format!("choice niri {path}"),
                options,
                chosen,
                roles,
                enabled,
                choose,
            )
        }
        Control::Text { example } => text(item, example, &shown, enabled, roles),
    };

    labeled(label(item, &shown, settings, roles), control, true, roles)
}

fn label(item: &'static Item, shown: &Shown, settings: &Settings, roles: &ThemeRoles) -> Label {
    let path = item.path;
    let mut marks: Vec<Box<dyn Widget>> = Vec::new();

    if shown.busy {
        marks.push(Box::new(
            Text::new("Saving\u{2026}")
                .size(theme::text::LABEL_SMALL)
                .color(roles.primary)
                .weight(theme::text::SEMIBOLD),
        ));
    } else if shown.default && settings.niri.loaded && shown.owner.is_none() {
        marks.push(Box::new(
            Text::new("default")
                .size(theme::text::LABEL_SMALL)
                .color(roles.on_surface_variant)
                .weight(theme::text::MEDIUM),
        ));
    }

    if shown.set && shown.editable && !shown.busy {
        marks.push(link(
            format!("reset niri {path}"),
            "reset",
            "Reset",
            roles,
            move || {
                niri::change(path, niri::reset_changes(item));
            },
        ));
    }

    let detail = shown.owner.clone().or_else(|| match &shown.current {
        Current::Foreign(raw) => Some(format!(
            "The file has {raw}, which this control cannot show; setting it replaces that"
        )),
        _ => None,
    });
    let keys = settings
        .inspect
        .then(|| format!("{}  (niri config.kdl)", item.paths().join("   ")));

    Label {
        node: path,
        hover_id: path.to_string(),
        title: item.label.to_owned(),
        help: item.help.to_owned(),
        marks,
        detail,
        keys,
        off: shown.refused.clone(),
    }
}

// a switch for a flag, or for a ring, border or shadow
fn flag(item: &'static Item, shown: &Shown, enabled: bool, roles: &ThemeRoles) -> Box<dyn Widget> {
    let path = item.path;
    let on = matches!(shown.current, Current::Flag(true));
    let switch_id = format!("switch niri {path}");
    let hovered = Settings::read().is_hovered(&switch_id);
    let track = switch_track(on, hovered, roles);

    if !enabled {
        return Box::new(track.opacity(theme::DISABLED));
    }

    pressed(
        track,
        switch_id,
        move || match item.control {
            Control::Drawn { .. } => niri::change(path, niri::drawn_changes(item, !on)),
            _ => niri::change(path, vec![(path, (!on).then(|| String::from("true")))]),
        },
        TARGET / 2.0,
        roles,
    )
}

// a value that cannot be changed now, as text
fn readonly(text: String, roles: &ThemeRoles) -> Box<dyn Widget> {
    Box::new(
        Text::new(text)
            .size(theme::text::LABEL)
            .color(roles.on_surface_variant)
            .weight(theme::text::MEDIUM),
    )
}

fn text(
    item: &'static Item,
    example: &'static str,
    shown: &Shown,
    enabled: bool,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    let path = item.path;

    if !enabled {
        return readonly(
            match &shown.current {
                Current::Text(text) | Current::Foreign(text) if !text.is_empty() => text.clone(),
                _ => String::from("Not set"),
            },
            roles,
        );
    }

    Box::new(field_with_hint(
        niri::input(item),
        example,
        roles,
        move |text| {
            let text = text.trim();
            let value = (!text.is_empty()).then(|| niri::quoted(text));

            niri::change(path, vec![(path, value)]);
        },
    ))
}

// a number: `-`, the number to type over and its unit, `+`, in one outlined capsule; Up and Down step it
fn number(
    item: &'static Item,
    spec: &'static Number,
    shown: &Shown,
    enabled: bool,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    const SIDE: f32 = 30.0;
    const NUMBER: f32 = 56.0;

    let path = item.path;
    let label = item.label;
    let &Number {
        least,
        most,
        step,
        decimals,
        unit,
        ..
    } = spec;
    let value = match shown.current {
        Current::Number(value) => Some(value),
        _ => None,
    };

    if !enabled {
        return readonly(
            match &shown.current {
                Current::Number(value) => {
                    format!("{} {unit}", niri::format_number(*value, decimals))
                }
                Current::Foreign(raw) => raw.clone(),
                _ => String::from("Not set"),
            },
            roles,
        );
    }

    let to = move |value: f64| {
        move || {
            let value = value.clamp(least, most);

            niri::change(
                path,
                vec![(path, Some(niri::format_number(value, decimals)))],
            );
        }
    };
    let base = value.unwrap_or(least);
    let id = niri::input(item);

    let unit = Text::new(unit)
        .size(theme::text::LABEL)
        .color(roles.on_surface_variant)
        .weight(theme::text::MEDIUM);
    let middle = NUMBER + 4.0 + fixed(Widget::width(&unit)) + 2.0 * 8.0;
    let inside = FIELD - 2.0;

    let side = |node: &'static str, sign: &str, value: f64| {
        let side_id = format!("niri stepper {id} {node}");
        let hovered = Settings::read().is_hovered(&side_id);
        let hover_id = side_id.clone();
        let mut btn = Rectangle::new()
            .width(SIDE)
            .height(inside)
            .semantics(|| Semantics::new(Role::Button, node))
            .align_child(Center, Center)
            .cursor(Cursor::Pointer)
            .on_hover(move |inside| hover(hover_id.clone(), inside))
            .on_click(left(to(value)));
        if hovered {
            btn = btn.fill(hover_wash(roles));
        }
        btn.child(
            Text::new(sign)
                .size(theme::text::BODY)
                .color(roles.on_surface)
                .weight(theme::text::SEMIBOLD),
        )
    };
    let divider = || {
        Rectangle::new()
            .width(1.0)
            .height(inside)
            .fill(faint(roles.outline))
    };

    let token = ring::register(
        Stop::new(format!("field {id}"))
            .input(id)
            .step(move |direction| {
                // Up raises the number, as the arrows of a number field do
                if direction < 0 {
                    to(base + step)();
                } else {
                    to(base - step)();
                }
            }),
    );

    outlined(
        Rectangle::new()
            .width(2.0 * SIDE + middle + 2.0 + 2.0)
            .height(FIELD)
            .radius(radius::ROW - 2.0)
            .fill(roles.surface)
            .align_child(Center, Center)
            .child(
                Row::new(children![
                    side("less", "\u{2212}", base - step),
                    divider(),
                    Rectangle::new()
                        .width(middle)
                        .height(inside)
                        .padding(sides(8.0))
                        .align_child(Start, Center)
                        .child(
                            Row::new(children![
                                TextInput::new(id)
                                    .width(NUMBER)
                                    .size(theme::text::LABEL)
                                    .color(roles.on_surface)
                                    .on_submit(move |text| {
                                        let text = text.trim();
                                        if text.is_empty() {
                                            niri::change(path, vec![(path, None)]);
                                        } else if let Some(value) = text
                                            .parse::<f64>()
                                            .ok()
                                            .filter(|value| value.is_finite())
                                        {
                                            to(value)();
                                        } else {
                                            Settings::write().niri.refuse(
                                                path,
                                                format!("Not saved: {label}: expected a number"),
                                            );
                                        }
                                    }),
                                unit,
                            ])
                            .gap(4.0)
                            .align(Center),
                        ),
                    divider(),
                    side("more", "+", base + step),
                ])
                .align(Center),
            ),
        token,
        Some(roles.edge()),
        roles,
    )
}
