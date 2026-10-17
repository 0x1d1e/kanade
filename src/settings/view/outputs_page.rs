//! The Outputs section of the niri page: one card for each connected monitor, its controls trying a
//! change in niri at once and the footer asking to keep it (`settings::outputs`). A press posts to
//! the worker; nothing here asks niri or touches the config.

use super::*;

use crate::settings::outputs::{self, Edit, Monitor, SCALES, Transform, Vrr};

// the section: a heading and a card for each monitor, or why there are none
pub fn section(settings: &Settings, roles: &ThemeRoles) -> Column {
    let state = &settings.outputs;
    let mut items: Vec<Box<dyn Widget>> = Vec::new();

    if let Some(why) = &state.error {
        items.push(Box::new(notice(why, theme::window_signals().warning)));
    } else if !state.loaded {
        items.push(plain("Asking niri for its outputs\u{2026}", roles));
    } else if state.monitors.is_empty() {
        items.push(plain("niri lists no outputs", roles));
    }

    for monitor in &state.monitors {
        items.push(monitor_view(monitor, settings, roles));
    }

    Column::new(items).width(content()).gap(HEADING * 2.0)
}

// Revert and Keep, while a change waits for one of them
pub fn actions(settings: &Settings, roles: &ThemeRoles) -> Vec<Box<dyn Widget>> {
    if settings.outputs.trial.is_none() {
        return Vec::new();
    }

    vec![
        pill("revert", "Revert", false, roles, outputs::revert),
        pill("keep", "Keep", true, roles, outputs::keep),
    ]
}

fn pill(
    node: &'static str,
    word: &str,
    primary: bool,
    roles: &ThemeRoles,
    press: fn(),
) -> Box<dyn Widget> {
    let text = Text::new(word)
        .size(theme::text::LABEL)
        .color(if primary {
            roles.on_primary
        } else {
            roles.on_surface
        })
        .weight(theme::text::SEMIBOLD);
    let control = Rectangle::new()
        .width(84.0)
        .height(30.0)
        .radius(radius::ROW)
        .semantics(|| Semantics::new(Role::Button, node))
        .align_child(Center, Center)
        .child(text);

    button(
        if primary {
            control.fill(roles.primary)
        } else {
            control
        },
        format!("outputs {node}"),
        press,
        (!primary).then(|| roles.edge()),
        roles,
    )
}

fn plain(text: &str, roles: &ThemeRoles) -> Box<dyn Widget> {
    Box::new(
        Text::new(text)
            .size(theme::text::BODY)
            .color(roles.on_surface_variant)
            .weight(theme::text::MEDIUM),
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

// why a monitor's controls do nothing, if they do not
fn blocker(settings: &Settings, monitor: &Monitor) -> Option<String> {
    let state = &settings.outputs;

    if let Some(why) = &state.blocked {
        return Some(format!("niri's config is read-only here: {why}"));
    }

    if let Some(owner) = &monitor.config.owner {
        return Some(format!("{owner}; Kanade does not change it"));
    }

    state
        .trial
        .as_ref()
        .filter(|trial| trial.output != monitor.output.name)
        .map(|trial| format!("Keep or revert the change to {} first", trial.output))
}

fn monitor_view(monitor: &Monitor, settings: &Settings, roles: &ThemeRoles) -> Box<dyn Widget> {
    let output = &monitor.output;
    let name = output.name.clone();
    let state = &settings.outputs;
    let trial = state
        .trial
        .as_ref()
        .is_some_and(|trial| trial.output == name);
    let blocked = blocker(settings, monitor);
    let acts = blocked.is_none();
    let on = output.enabled();

    // the only one that is on stays on
    let alone = on
        && state
            .monitors
            .iter()
            .all(|other| other.output.name == name || !other.output.enabled());

    let mut marks: Vec<Box<dyn Widget>> = vec![Box::new(
        Text::new(name.clone())
            .size(theme::text::LABEL)
            .color(roles.on_surface_variant)
            .weight(theme::text::MEDIUM),
    )];

    if trial {
        marks.push(Box::new(
            Text::new("Trying")
                .size(theme::text::LABEL_SMALL)
                .color(roles.primary)
                .weight(theme::text::SEMIBOLD),
        ));
    }

    if acts && monitor.config.set && state.trial.is_none() {
        let reset = name.clone();

        marks.push(link(
            format!("reset output {name}"),
            "reset",
            "Reset",
            roles,
            move || outputs::reset(&reset),
        ));
    }

    let heading = Row::new(
        std::iter::once(Box::new(
            Text::new(output.product())
                .size(theme::text::TITLE)
                .color(roles.on_surface)
                .weight(theme::text::SEMIBOLD),
        ) as Box<dyn Widget>)
        .chain(marks)
        .collect(),
    )
    .gap(10.0)
    .align(Center);

    let block = format!(
        "Saved in the block output \"{}\" of niri's config",
        monitor.config.block
    );
    let detail = [blocked.clone(), monitor.config.note.clone()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(". ");
    let mut head = children![heading];

    head.push(Box::new(wrapped(
        Text::new(if detail.is_empty() { block } else { detail })
            .size(theme::text::LABEL_SMALL)
            .color(if blocked.is_some() {
                theme::window_signals().warning
            } else {
                roles.on_surface_variant
            })
            .weight(theme::text::MEDIUM),
        content(),
    )));

    let mut rows: Vec<Box<dyn Widget>> = vec![
        enabled_row(monitor, acts, alone, roles),
        mode_row(monitor, acts && on, settings, roles),
        scale_row(monitor, acts && on, roles),
        transform_row(monitor, acts && on, roles),
    ];
    if output.vrr_supported {
        rows.push(vrr_row(monitor, acts && on, roles));
    }
    rows.push(position_row(monitor, acts && on, roles));

    let mut card = Vec::new();
    for row in rows {
        if !card.is_empty() {
            card.push(Box::new(hairline(inner(), roles)) as Box<dyn Widget>);
        }
        card.push(row);
    }

    Box::new(
        Column::new(vec![
            Box::new(Column::new(head).width(content()).gap(2.0)),
            Box::new(card_of(card, roles)),
        ])
        .width(content())
        .gap(HEADING),
    )
}

fn row_label(node: &'static str, title: &str, help: &str, off: Option<String>) -> Label {
    Label {
        node,
        title: title.to_owned(),
        help: help.to_owned(),
        marks: Vec::new(),
        detail: None,
        keys: None,
        off,
    }
}

fn enabled_row(monitor: &Monitor, acts: bool, alone: bool, roles: &ThemeRoles) -> Box<dyn Widget> {
    let output = &monitor.output;
    let name = output.name.clone();
    let on = output.enabled();
    let track = switch_track(on, roles);
    let control: Box<dyn Widget> = if !acts || alone {
        Box::new(track.opacity(theme::DISABLED))
    } else {
        pressed(
            track,
            format!("switch output {name} enabled"),
            move || outputs::edit(&name, Edit::Enabled(!on)),
            TARGET / 2.0,
            roles,
        )
    };

    labeled(
        row_label(
            "output-enabled",
            "Enabled",
            "Turns the monitor on or off. The last one that is on cannot be turned off",
            (alone && acts).then(|| String::from("It is the only monitor that is on")),
        ),
        control,
        true,
        roles,
    )
}

fn mode_row(
    monitor: &Monitor,
    acts: bool,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    let output = &monitor.output;
    let name = output.name.clone();
    let open = settings.outputs.mode_open.as_deref() == Some(name.as_str());
    let current = output.current_mode();
    let shown = current.map_or_else(
        || {
            String::from(if output.enabled() {
                "Custom mode"
            } else {
                "Off"
            })
        },
        |mode| mode.label(),
    );
    let help = "The resolution and refresh rate, from what the monitor reports";

    if !acts {
        return labeled(
            row_label("output-mode", "Mode", help, None),
            readonly(shown, roles),
            true,
            roles,
        );
    }

    let toggle = {
        let name = name.clone();

        move || {
            let mut settings = Settings::write();

            settings.outputs.mode_open = if open { None } else { Some(name.clone()) };
        }
    };
    let text = Text::new(format!(
        "{shown}  {}",
        if open { "\u{25b4}" } else { "\u{25be}" }
    ))
    .size(theme::text::LABEL)
    .color(roles.on_surface)
    .weight(theme::text::SEMIBOLD);
    let width = fixed(Widget::width(&text)) + 2.0 * 12.0;
    let chooser = button(
        Rectangle::new()
            .width(width)
            .height(FIELD)
            .radius(radius::ROW - 2.0)
            .fill(roles.surface)
            .semantics(|| Semantics::new(Role::Button, "mode"))
            .align_child(Center, Center)
            .child(text),
        format!("output {name} mode"),
        toggle,
        Some(roles.edge()),
        roles,
    );

    if !open {
        return labeled(
            row_label("output-mode", "Mode", help, None),
            chooser,
            true,
            roles,
        );
    }

    let mut lines: Vec<Box<dyn Widget>> = vec![chooser];

    for mode in &output.modes {
        let here = current.is_some_and(|current| current.text() == mode.text());
        let mode = *mode;
        let pick = {
            let name = name.clone();

            move || {
                Settings::write().outputs.mode_open = None;
                outputs::edit(&name, Edit::Mode(mode));
            }
        };
        let mut words = children![
            Text::new(mode.label())
                .size(theme::text::LABEL)
                .color(roles.on_surface)
                .weight(if here {
                    theme::text::SEMIBOLD
                } else {
                    theme::text::MEDIUM
                }),
        ];
        for (mark, shown) in [(here, "current"), (mode.preferred, "preferred")] {
            if mark {
                words.push(Box::new(
                    Text::new(shown)
                        .size(theme::text::LABEL_SMALL)
                        .color(roles.on_surface_variant)
                        .weight(theme::text::MEDIUM),
                ));
            }
        }

        let line = Rectangle::new()
            .width(inner())
            .height(32.0)
            .radius(radius::ROW)
            .padding(sides(12.0))
            .semantics(|| Semantics::new(Role::Item, "mode").content(mode.text()))
            .align_child(Start, Center)
            .child(Row::new(words).gap(10.0).align(Center));
        let line = if here {
            line.fill(faint(roles.primary))
        } else {
            line.fill(roles.surface)
        };

        lines.push(button(
            line,
            format!("output {name} mode {}", mode.text()),
            pick,
            None,
            roles,
        ));
    }

    labeled(
        row_label("output-mode", "Mode", help, None),
        Box::new(Column::new(lines).width(inner()).gap(4.0)),
        false,
        roles,
    )
}

fn scale_row(monitor: &Monitor, acts: bool, roles: &ThemeRoles) -> Box<dyn Widget> {
    let output = &monitor.output;
    let name = output.name.clone();
    let scale = output.logical.map_or(1.0, |logical| logical.scale);
    let help = "Makes everything larger: 2 is twice the size. niri rounds it to a scale the screen can show";
    let (_, _, step) = SCALES;

    let control: Box<dyn Widget> = if acts {
        // the next multiple of the step, from one niri rounded to something else
        let by = |direction: f64| {
            let name = name.clone();
            let to = ((scale / step).round() + direction) * step;

            move || outputs::edit(&name, Edit::Scale(to))
        };
        let submit = {
            let name = name.clone();

            move |text: String| match text.trim().parse::<f64>() {
                Ok(scale) if scale.is_finite() => outputs::edit(&name, Edit::Scale(scale)),
                _ => {
                    Settings::write().outputs.notice =
                        Some((String::from("Scale: expected a number, as 1.25"), true));
                }
            }
        };

        capsule(
            outputs::input(&name, 's'),
            format!("output {name} scale"),
            by(-1.0),
            by(1.0),
            submit,
            roles,
        )
    } else {
        readonly(format!("{}\u{d7}", niri::format_number(scale, 4)), roles)
    };

    labeled(
        row_label("output-scale", "Scale", help, None),
        control,
        true,
        roles,
    )
}

fn transform_row(monitor: &Monitor, acts: bool, roles: &ThemeRoles) -> Box<dyn Widget> {
    let output = &monitor.output;
    let name = output.name.clone();
    let current = output
        .logical
        .map_or(Transform::Normal, |logical| logical.transform);
    let help = "Turns or flips the picture, counter-clockwise";

    if !acts {
        return labeled(
            row_label("output-transform", "Transform", help, None),
            readonly(current.label().to_owned(), roles),
            true,
            roles,
        );
    }

    let rows: Vec<Box<dyn Widget>> = Transform::ALL
        .chunks(4)
        .enumerate()
        .map(|(at, group)| {
            let group = group.to_vec();
            let chosen = group.iter().position(|&transform| transform == current);
            let name = name.clone();

            chips(
                format!("output {name} transform {at}"),
                group.iter().map(|t| t.label().to_owned()).collect(),
                chosen,
                move |index| outputs::edit(&name, Edit::Transform(group[index])),
                roles,
            )
        })
        .collect();

    labeled(
        row_label("output-transform", "Transform", help, None),
        Box::new(Column::new(rows).gap(6.0)),
        true,
        roles,
    )
}

fn vrr_row(monitor: &Monitor, acts: bool, roles: &ThemeRoles) -> Box<dyn Widget> {
    let output = &monitor.output;
    let name = output.name.clone();
    let hint = monitor.config.vrr;
    let current = Vrr::of(output.vrr_enabled, hint);
    let help = "Follows the frame rate of what is drawn. On demand turns it on only while a window that asks for it is shown";

    let control: Box<dyn Widget> = if acts {
        chips(
            format!("output {name} vrr"),
            Vrr::ALL.iter().map(|vrr| vrr.label().to_owned()).collect(),
            Vrr::ALL.iter().position(|&vrr| vrr == current),
            move |index| outputs::edit(&name, Edit::Vrr(Vrr::ALL[index])),
            roles,
        )
    } else {
        readonly(current.label().to_owned(), roles)
    };

    labeled(
        row_label("output-vrr", "Variable refresh rate", help, None),
        control,
        true,
        roles,
    )
}

fn position_row(monitor: &Monitor, acts: bool, roles: &ThemeRoles) -> Box<dyn Widget> {
    let output = &monitor.output;
    let name = output.name.clone();
    let help = "Where the monitor sits among the others, in logical pixels from the top left. Both are set together";

    let control: Box<dyn Widget> = if acts {
        let apply = {
            let name = name.clone();

            move || match outputs::typed_position(&name) {
                Ok((x, y)) => outputs::edit(&name, Edit::Position(x, y)),
                Err(why) => Settings::write().outputs.notice = Some((why, true)),
            }
        };
        let field = |axis: char| {
            let id = outputs::input(&name, axis);
            let apply = apply.clone();

            Row::new(children![
                Text::new(axis.to_ascii_uppercase().to_string())
                    .size(theme::text::LABEL)
                    .color(roles.on_surface_variant)
                    .weight(theme::text::SEMIBOLD),
                position_field(id, roles, apply),
            ])
            .gap(6.0)
            .align(Center)
        };

        Box::new(
            Row::new(children![
                field('x'),
                field('y'),
                link(
                    format!("apply output {name} position"),
                    "apply",
                    "Apply",
                    roles,
                    apply.clone()
                ),
            ])
            .gap(12.0)
            .align(Center),
        )
    } else {
        let (x, y) = output
            .logical
            .map_or((0, 0), |logical| (logical.x, logical.y));

        readonly(format!("{x}, {y}"), roles)
    };

    labeled(
        row_label("output-position", "Position", help, None),
        control,
        true,
        roles,
    )
}

// a whole number, kept as typed for Apply; Enter applies
fn position_field(
    id: &'static str,
    roles: &ThemeRoles,
    apply: impl Fn() + Clone + 'static,
) -> Box<dyn Widget> {
    let token = ring::register(Stop::new(format!("field {id}")).input(id));

    outlined(
        Rectangle::new()
            .width(84.0)
            .height(FIELD)
            .radius(radius::ROW - 2.0)
            .fill(roles.surface)
            .padding(sides(10.0))
            .align_child(Start, Center)
            .child(
                TextInput::new(id)
                    .width(Parent)
                    .size(theme::text::LABEL)
                    .color(roles.on_surface)
                    .on_change(move |text| outputs::typed(id, text))
                    .on_submit(move |text| {
                        outputs::typed(id, text);
                        apply();
                    }),
            ),
        token,
        Some(roles.edge()),
        roles,
    )
}

// options side by side in a trough, the chosen one filled; one stop, Left and Right choose
fn chips(
    id: String,
    labels: Vec<String>,
    chosen: Option<usize>,
    pick: impl Fn(usize) + Clone + 'static,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    const INSET: f32 = 3.0;
    const HEIGHT: f32 = 28.0;

    let count = labels.len();
    let segments: Vec<Box<dyn Widget>> = labels
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            let here = chosen == Some(index);
            let text = Text::new(label.clone())
                .size(theme::text::LABEL)
                .color(if here {
                    roles.on_primary
                } else {
                    roles.on_surface
                })
                .weight(theme::text::SEMIBOLD);
            let width = fixed(Widget::width(&text)) + 24.0;
            let pick = pick.clone();
            let segment = Rectangle::new()
                .width(width)
                .height(HEIGHT)
                .radius(HEIGHT / 2.0)
                .semantics(move || {
                    Semantics::new(Role::Tab, "option")
                        .label(label)
                        .selected(here)
                })
                .align_child(Center, Center)
                .cursor(Cursor::Pointer)
                .on_click(left(move || pick(index)))
                .child(text);

            Box::new(if here {
                segment.fill(roles.primary)
            } else {
                segment
            }) as Box<dyn Widget>
        })
        .collect();

    let row = Row::new(segments).gap(2.0);
    let width = fixed(Widget::width(&row)) + 2.0 * INSET;
    let trough = Rectangle::new()
        .width(width)
        .height(HEIGHT + 2.0 * INSET)
        .radius(HEIGHT / 2.0 + INSET)
        .fill(roles.track())
        .border(1.0, roles.edge())
        .padding(INSET)
        .child(row);

    let at = chosen.unwrap_or(0);
    let token = ring::register(Stop::new(id).step(move |direction| {
        let next = (at as i32 + direction).clamp(0, count as i32 - 1);

        pick(next as usize);
    }));

    halo(trough, token, HEIGHT / 2.0 + INSET, roles)
}

// `-`, a number to type over, `+`, in one outlined capsule; Up and Down step it
fn capsule(
    input: &'static str,
    id: String,
    less: impl Fn() + Clone + 'static,
    more: impl Fn() + Clone + 'static,
    submit: impl Fn(String) + 'static,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    const SIDE: f32 = 30.0;
    const NUMBER: f32 = 64.0;

    let middle = NUMBER + 2.0 * 8.0;
    let inside = FIELD - 2.0;
    let side = |node: &'static str, sign: &str, press: Box<dyn Fn()>| {
        Rectangle::new()
            .width(SIDE)
            .height(inside)
            .semantics(|| Semantics::new(Role::Button, node))
            .align_child(Center, Center)
            .cursor(Cursor::Pointer)
            .on_click(left(press))
            .child(
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
    let (down, up) = (less.clone(), more.clone());
    let token = ring::register(Stop::new(id).input(input).step(move |direction| {
        // Up raises the number, as the arrows of a number field do
        if direction < 0 {
            up();
        } else {
            down();
        }
    }));

    outlined(
        Rectangle::new()
            .width(2.0 * SIDE + middle + 2.0 + 2.0)
            .height(FIELD)
            .radius(radius::ROW - 2.0)
            .fill(roles.surface)
            .align_child(Center, Center)
            .child(
                Row::new(children![
                    side("less", "\u{2212}", Box::new(less)),
                    divider(),
                    Rectangle::new()
                        .width(middle)
                        .height(inside)
                        .padding(sides(8.0))
                        .align_child(Start, Center)
                        .child(
                            TextInput::new(input)
                                .width(NUMBER)
                                .size(theme::text::LABEL)
                                .color(roles.on_surface)
                                .on_change(move |text| outputs::typed(input, text))
                                .on_submit(submit)
                        ),
                    divider(),
                    side("more", "+", Box::new(more)),
                ])
                .align(Center),
            ),
        token,
        Some(roles.edge()),
        roles,
    )
}
