//! The Umbriel page's rows: each an option of Umbriel's `config.toml` (`settings::umbriel`)
//! drawn by what it takes. A change stages the edit in memory and the row says it is not saved;
//! Save writes every staged change in one transaction, and Discard drops them.

use super::*;

use crate::settings::umbriel::{self, Control, Current, Item, Number, Shown};

/// The footer's words on the page.
pub fn status(settings: &Settings, roles: &ThemeRoles) -> (String, Color) {
    match settings.umbriel.status() {
        Some((text, true)) => (text, theme::window_signals().warning),
        Some((text, false)) => (text, roles.on_surface_variant),
        None => (umbriel::STANDING.to_owned(), roles.on_surface_variant),
    }
}

/// Why the rows cannot be read or changed, in one box.
pub fn notices(settings: &Settings) -> Vec<Box<dyn Widget>> {
    let umbriel = &settings.umbriel;
    let warning = theme::window_signals().warning;
    let roles = theme::roles();

    let mut items: Vec<Box<dyn Widget>> = Vec::new();

    if !umbriel.installed {
        items.push(Box::new(super::notice(
            "Umbriel was not found on PATH. Install Umbriel or check your PATH. Changes will still be validated for valid TOML syntax.",
            warning,
        )));
    }

    if umbriel.config_missing {
        let text = Text::new("Umbriel configuration (~/.config/umbriel/config.toml) is not found")
            .size(theme::text::LABEL)
            .color(warning)
            .weight(theme::text::MEDIUM);
        let create_btn = super::link(
            "create default config".into(),
            "create-default-config",
            "Create default configuration",
            settings,
            &roles,
            umbriel::create_default,
        );
        let row = Row::new(children![text, create_btn])
            .width(super::content() - 2.0 * 12.0)
            .justify(SpaceBetween)
            .align(Center);

        items.push(Box::new(
            Rectangle::new()
                .width(super::content())
                .height(44.0)
                .radius(radius::ROW)
                .fill(super::faint(warning))
                .padding(super::sides(12.0))
                .align_child(Start, Center)
                .child(row),
        ));
    } else if let Some(why) = &umbriel.unreadable {
        items.push(Box::new(super::notice(
            &format!("Umbriel's config cannot be read: {why}"),
            warning,
        )));
    } else if let Some(why) = &umbriel.not_editable {
        items.push(Box::new(super::notice(
            &format!(
                "Umbriel's config is read-only here: {why}. The values shown are what it holds"
            ),
            warning,
        )));
    } else if !umbriel.loaded {
        items.push(Box::new(super::notice(
            "Reading Umbriel's config\u{2026}",
            roles.on_surface_variant,
        )));
    }

    items
}

/// Whether an option's name, help or path mentions the query from the start of a word.
pub fn mentions(path: &str, query: &str) -> bool {
    let mentions = |text: &str| words(text).any(|word| word.starts_with(query));

    query == "umbriel"
        || umbriel::item(path)
            .is_some_and(|item| mentions(item.label) || mentions(item.help) || mentions(item.path))
}

/// Whether a recommended rule matches the search query.
pub fn mentions_rule(id: &str, query: &str) -> bool {
    let mentions = |text: &str| words(text).any(|word| word.starts_with(query));

    query == "umbriel"
        || mentions("rule")
        || mentions("rules")
        || mentions("layer-rule")
        || mentions(id)
        || crate::umbriel_config::rules::preset(id)
            .is_some_and(|p| mentions(p.label) || mentions(p.description))
}

fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|char: char| !char.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
}

pub fn row(path: &'static str, settings: &Settings, roles: &ThemeRoles) -> Box<dyn Widget> {
    let item = umbriel::item(path).expect("the layout's umbriel rows are the page's options");
    let shown = settings.umbriel.shown(item);
    let enabled = shown.editable && !shown.busy && !settings.umbriel.is_saving();

    let control = match &item.control {
        Control::Flag => flag(item, &shown, enabled, settings, roles),
        Control::Number(spec) => number(item, spec, &shown, enabled, settings, roles),
        Control::Choice { options, .. } => {
            let chosen = match &shown.current {
                Current::Text(text) => text.as_str(),
                _ => "",
            };
            let choose = move |option: &'static str| {
                umbriel::change(path, vec![(path, Some(format!("\"{option}\"")))]);
            };

            segmented(
                format!("choice umbriel {path}"),
                options,
                chosen,
                settings,
                roles,
                enabled,
                choose,
            )
        }
        Control::Text { example } => text(item, example, &shown, enabled, roles),
    };

    labeled(
        label(item, &shown, settings, roles),
        control,
        true,
        settings,
        roles,
    )
}

pub fn rule_row(id: &'static str, settings: &Settings, roles: &ThemeRoles) -> Box<dyn Widget> {
    let preset =
        crate::umbriel_config::rules::preset(id).expect("the layout's rule rows are presets");
    let status = settings
        .umbriel
        .rule_status(id)
        .cloned()
        .unwrap_or(crate::umbriel_config::RuleStatus::Missing);

    let busy = settings.umbriel.is_busy(id);
    let on = matches!(status, crate::umbriel_config::RuleStatus::Active);
    let enabled = settings.umbriel.editable() && !busy && !settings.umbriel.is_saving();

    let switch_id = format!("switch umbriel rule {id}");
    let hovered = settings.is_hovered(&switch_id);
    let track = super::switch_track(on, hovered, roles);

    let control: Box<dyn Widget> = if !enabled {
        Box::new(track.opacity(theme::DISABLED))
    } else {
        pressed(
            track,
            switch_id,
            move || {
                if on {
                    umbriel::remove_rule(id);
                } else {
                    umbriel::apply_rule(id);
                }
            },
            TARGET / 2.0,
            roles,
        )
    };

    let mut marks: Vec<Box<dyn Widget>> = Vec::new();
    if busy {
        marks.push(Box::new(
            Text::new("Saving\u{2026}")
                .size(theme::text::LABEL_SMALL)
                .color(roles.primary)
                .weight(theme::text::SEMIBOLD),
        ));
    }

    let label = Label {
        node: id,
        hover_id: format!("row umbriel rule {id}"),
        title: preset.label.to_owned(),
        help: preset.description.to_owned(),
        marks,
        detail: None,
        keys: settings
            .inspect
            .then(|| String::from("layer_rule / overview  (umbriel config.toml)")),
        off: None,
    };

    labeled(label, control, true, settings, roles)
}

fn label(item: &'static Item, shown: &Shown, settings: &Settings, roles: &ThemeRoles) -> Label {
    let path = item.path;
    let mut marks: Vec<Box<dyn Widget>> = Vec::new();

    let mark = if shown.writing {
        Some("Saving\u{2026}")
    } else if shown.pending {
        Some("Not saved")
    } else if shown.busy {
        Some("Saving\u{2026}")
    } else {
        None
    };

    if let Some(mark) = mark {
        marks.push(Box::new(
            Text::new(mark)
                .size(theme::text::LABEL_SMALL)
                .color(roles.primary)
                .weight(theme::text::SEMIBOLD),
        ));
    } else if shown.default && settings.umbriel.loaded {
        marks.push(Box::new(
            Text::new("default")
                .size(theme::text::LABEL_SMALL)
                .color(roles.on_surface_variant)
                .weight(theme::text::MEDIUM),
        ));
    }

    if shown.set && shown.editable && !shown.busy {
        marks.push(link(
            format!("reset umbriel {path}"),
            "reset",
            "Reset",
            settings,
            roles,
            move || {
                umbriel::change(path, umbriel::reset_changes(item));
            },
        ));
    }

    let detail = match &shown.current {
        Current::Foreign(raw) => Some(format!(
            "The file has {raw}, which this control cannot show; setting it replaces that"
        )),
        _ => None,
    };
    let keys = settings
        .inspect
        .then(|| format!("{}  (umbriel config.toml)", item.paths().join("   ")));

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

fn flag(
    item: &'static Item,
    shown: &Shown,
    enabled: bool,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    let path = item.path;
    let on = matches!(shown.current, Current::Flag(true));
    let switch_id = format!("switch umbriel {path}");
    let hovered = settings.is_hovered(&switch_id);
    let track = switch_track(on, hovered, roles);

    if !enabled {
        return Box::new(track.opacity(theme::DISABLED));
    }

    pressed(
        track,
        switch_id,
        move || {
            let next_val = if !on { "true" } else { "false" };
            umbriel::change(path, vec![(path, Some(String::from(next_val)))]);
        },
        TARGET / 2.0,
        roles,
    )
}

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
        umbriel::input(item),
        example,
        roles,
        move |text| {
            let text = text.trim();
            let value = (!text.is_empty()).then(|| text.to_owned());

            umbriel::change(path, vec![(path, value)]);
        },
    ))
}

fn number(
    item: &'static Item,
    spec: &'static Number,
    shown: &Shown,
    enabled: bool,
    settings: &Settings,
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
                    format!("{} {unit}", umbriel::format_number(*value, decimals))
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

            umbriel::change(
                path,
                vec![(path, Some(umbriel::format_number(value, decimals)))],
            );
        }
    };
    let base = value.unwrap_or(least);
    let id = umbriel::input(item);

    let unit = Text::new(unit)
        .size(theme::text::LABEL)
        .color(roles.on_surface_variant)
        .weight(theme::text::MEDIUM);
    let middle = NUMBER + 4.0 + fixed(Widget::width(&unit)) + 2.0 * 8.0;
    let inside = FIELD - 2.0;

    let side = |node: &'static str, sign: &str, value: f64| {
        let side_id = format!("umbriel stepper {id} {node}");
        let hovered = settings.is_hovered(&side_id);
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
                                            umbriel::change(path, vec![(path, None)]);
                                        } else if let Some(value) = text
                                            .parse::<f64>()
                                            .ok()
                                            .filter(|value| value.is_finite())
                                        {
                                            to(value)();
                                        } else {
                                            Settings::write().umbriel.refuse(
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
