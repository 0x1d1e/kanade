//! The Outputs section of the niri page: one card for each connected monitor, its controls trying a
//! change in niri at once and the footer asking to keep it (`settings::outputs`). A press posts to
//! the worker; nothing here asks niri or touches the config.

use super::*;

use crate::settings::outputs::{
    self, BoundingBox, CanvasFit, Disconnected, Edit, Monitor, Rect, SCALES, SNAP_THRESHOLD,
    Transform, Vrr, snap,
};

const CANVAS_HEIGHT: f32 = 220.0;
const CANVAS_PADDING: f32 = 20.0;

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

    if !state.monitors.is_empty() {
        items.push(canvas_view(settings, roles));
    }

    for monitor in &state.monitors {
        items.push(monitor_view(monitor, settings, roles));
    }

    if !state.disconnected.is_empty() {
        items.push(Box::new(
            Text::new("Disconnected monitors")
                .size(theme::text::TITLE)
                .color(roles.on_surface)
                .weight(theme::text::SEMIBOLD),
        ));

        for disc in &state.disconnected {
            items.push(disconnected_view(disc, settings, roles));
        }
    }

    Column::new(items).width(content()).gap(HEADING * 2.0)
}

// Revert and Keep, while a change waits for one of them
pub fn actions(settings: &Settings, roles: &ThemeRoles) -> Vec<Box<dyn Widget>> {
    if settings.outputs.trial.is_none() {
        return Vec::new();
    }

    vec![
        pill("revert", "Revert", false, settings, roles, outputs::revert),
        pill("keep", "Keep", true, settings, roles, outputs::keep),
    ]
}

fn pill(
    node: &'static str,
    word: &str,
    primary: bool,
    settings: &Settings,
    roles: &ThemeRoles,
    press: fn(),
) -> Box<dyn Widget> {
    let id = format!("outputs {node}");
    let hovered = settings.is_hovered(&id);
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

    let control = if primary {
        if hovered {
            control.fill(hover_primary(roles))
        } else {
            control.fill(roles.primary)
        }
    } else if hovered {
        control.fill(hover_wash(roles))
    } else {
        control
    };

    button_styled(
        control,
        id,
        press,
        (!primary).then(|| roles.edge()),
        false,
        settings,
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
            settings,
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
        enabled_row(monitor, acts, alone, settings, roles),
        mode_row(monitor, acts && on, settings, roles),
        scale_row(monitor, acts && on, settings, roles),
        transform_row(monitor, acts && on, settings, roles),
    ];
    if output.vrr_supported {
        rows.push(vrr_row(monitor, acts && on, settings, roles));
    }
    rows.push(position_row(monitor, acts && on, settings, roles));

    let mut card = Vec::new();
    for row in rows {
        if !card.is_empty() {
            card.push(Box::new(hairline(inner(), roles)) as Box<dyn Widget>);
        }
        card.push(row);
    }

    let selected = settings.outputs.selected.as_deref() == Some(&name);
    let card_rect = card_of(card, roles);
    let card_rect = if selected {
        card_rect.border(1.5, roles.primary)
    } else {
        card_rect
    };

    Box::new(
        Column::new(vec![
            Box::new(Column::new(head).width(content()).gap(2.0)),
            Box::new(card_rect),
        ])
        .width(content())
        .gap(HEADING),
    )
}

fn row_label(
    monitor_name: &str,
    node: &'static str,
    title: &str,
    help: &str,
    off: Option<String>,
) -> Label {
    let suffix = node.strip_prefix("output-").unwrap_or(node);
    Label {
        node,
        hover_id: format!("{monitor_name}-{suffix}"),
        title: title.to_owned(),
        help: help.to_owned(),
        marks: Vec::new(),
        detail: None,
        keys: None,
        off,
    }
}

fn enabled_row(
    monitor: &Monitor,
    acts: bool,
    alone: bool,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    let output = &monitor.output;
    let name = output.name.clone();
    let on = output.enabled();
    let switch_id = format!("switch output {name} enabled");
    let hovered = settings.is_hovered(&switch_id);
    let track = switch_track(on, hovered, roles);
    let control: Box<dyn Widget> = if !acts || alone {
        Box::new(track.opacity(theme::DISABLED))
    } else {
        pressed(
            track,
            switch_id,
            move || outputs::edit(&name, Edit::Enabled(!on)),
            TARGET / 2.0,
            roles,
        )
    };

    labeled(
        row_label(
            &output.name,
            "output-enabled",
            "Enabled",
            "Turns the monitor on or off. The last one that is on cannot be turned off",
            (alone && acts).then(|| String::from("It is the only monitor that is on")),
        ),
        control,
        true,
        settings,
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
            row_label(&output.name, "output-mode", "Mode", help, None),
            readonly(shown, roles),
            true,
            settings,
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
        settings,
        roles,
    );

    if !open {
        return labeled(
            row_label(&output.name, "output-mode", "Mode", help, None),
            chooser,
            true,
            settings,
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
        let mode_id = format!("output {name} mode {}", mode.text());
        let hovered = settings.is_hovered(&mode_id);
        let line = if here {
            if hovered {
                line.fill(theme::mix(faint(roles.primary), roles.on_surface, 0.08))
            } else {
                line.fill(faint(roles.primary))
            }
        } else if hovered {
            line.fill(theme::mix(roles.surface, roles.on_surface, 0.08))
        } else {
            line.fill(roles.surface)
        };

        lines.push(button_styled(
            line, mode_id, pick, None, false, settings, roles,
        ));
    }

    labeled(
        row_label(&output.name, "output-mode", "Mode", help, None),
        Box::new(Column::new(lines).width(inner()).gap(4.0)),
        false,
        settings,
        roles,
    )
}

fn scale_row(
    monitor: &Monitor,
    acts: bool,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
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
            settings,
            roles,
        )
    } else {
        readonly(format!("{}\u{d7}", niri::format_number(scale, 4)), roles)
    };

    labeled(
        row_label(&output.name, "output-scale", "Scale", help, None),
        control,
        true,
        settings,
        roles,
    )
}

fn transform_row(
    monitor: &Monitor,
    acts: bool,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    let output = &monitor.output;
    let name = output.name.clone();
    let current = output
        .logical
        .map_or(Transform::Normal, |logical| logical.transform);
    let help = "Turns or flips the picture, counter-clockwise";

    if !acts {
        return labeled(
            row_label(&output.name, "output-transform", "Transform", help, None),
            readonly(current.label().to_owned(), roles),
            true,
            settings,
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
                settings,
                roles,
            )
        })
        .collect();

    labeled(
        row_label(&output.name, "output-transform", "Transform", help, None),
        Box::new(Column::new(rows).gap(6.0)),
        true,
        settings,
        roles,
    )
}

fn vrr_row(
    monitor: &Monitor,
    acts: bool,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
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
            settings,
            roles,
        )
    } else {
        readonly(current.label().to_owned(), roles)
    };

    labeled(
        row_label(
            &output.name,
            "output-vrr",
            "Variable refresh rate",
            help,
            None,
        ),
        control,
        true,
        settings,
        roles,
    )
}

fn position_row(
    monitor: &Monitor,
    acts: bool,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
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
                    settings,
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
        row_label(&output.name, "output-position", "Position", help, None),
        control,
        true,
        settings,
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
    settings: &Settings,
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
            let seg_id = format!("{id} {index}");
            let hovered = settings.is_hovered(&seg_id);
            let hover_seg = seg_id.clone();
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
                .align_child(Center, Center);
            let segment = segment
                .cursor(Cursor::Pointer)
                .on_hover(move |inside| hover(hover_seg.clone(), inside))
                .on_click(left(move || pick(index)))
                .child(text);

            Box::new(if here {
                if hovered {
                    segment.fill(hover_primary(roles))
                } else {
                    segment.fill(roles.primary)
                }
            } else if hovered {
                segment.fill(hover_wash(roles))
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
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    const SIDE: f32 = 30.0;
    const NUMBER: f32 = 64.0;

    let middle = NUMBER + 2.0 * 8.0;
    let inside = FIELD - 2.0;
    let side = |node: &'static str, sign: &str, press: Box<dyn Fn()>| {
        let side_id = format!("capsule {id} {node}");
        let hovered = settings.is_hovered(&side_id);
        let hover_id = side_id.clone();
        let mut btn = Rectangle::new()
            .width(SIDE)
            .height(inside)
            .semantics(|| Semantics::new(Role::Button, node))
            .align_child(Center, Center)
            .cursor(Cursor::Pointer)
            .on_hover(move |inside| hover(hover_id.clone(), inside))
            .on_click(left(press));
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
    let (down, up) = (less.clone(), more.clone());
    let token = ring::register(Stop::new(id.clone()).input(input).step(move |direction| {
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

fn canvas_view(settings: &Settings, roles: &ThemeRoles) -> Box<dyn Widget> {
    let state = &settings.outputs;
    let canvas_w = content();
    let canvas_h = CANVAS_HEIGHT;

    let enabled: Vec<&Monitor> = state
        .monitors
        .iter()
        .filter(|m| m.output.enabled())
        .collect();

    if enabled.is_empty() {
        let empty_box = Rectangle::new()
            .width(canvas_w)
            .height(canvas_h)
            .radius(radius::CARD)
            .fill(roles.surface_container)
            .border(1.0, roles.edge())
            .semantics(|| Semantics::new(Role::Region, "outputs-canvas"))
            .align_child(Center, Center)
            .child(plain("All monitors are currently disabled", roles));
        return Box::new(empty_box);
    }

    let rects: Vec<Rect> = enabled
        .iter()
        .map(|m| m.output.logical.unwrap().rect())
        .collect();
    let bbox = BoundingBox::of(&rects);
    let fit = CanvasFit::new(bbox, canvas_w, canvas_h, CANVAS_PADDING);

    let mut layers: Vec<Box<dyn Widget>> = Vec::new();

    let dragged_output = state.drag.as_ref().map(|d| d.output.as_str());
    let mut dragged_layer: Option<Box<dyn Widget>> = None;

    for monitor in &enabled {
        let name = monitor.output.name.clone();
        let logical = monitor.output.logical.unwrap();
        let selected = state.selected.as_deref() == Some(&name);
        let is_dragged = dragged_output == Some(&name);
        let (cx, cy, cw, ch) = fit.to_canvas(logical.x, logical.y, logical.width, logical.height);

        let others: Vec<Rect> = enabled
            .iter()
            .filter(|m| m.output.name != name)
            .map(|m| m.output.logical.unwrap().rect())
            .collect();

        let blocked = blocker(settings, monitor).is_some();

        let mut box_rect = monitor_box(monitor, cw, ch, selected && !is_dragged, is_dragged, roles)
            .identity(("canvas-monitor", &name))
            .semantics(move || Semantics::new(Role::Button, format!("canvas-{name}")))
            .translate(cx, cy);

        let n_click = monitor.output.name.clone();
        let n_drag = monitor.output.name.clone();
        let n_end = monitor.output.name.clone();

        if !blocked {
            let snap_threshold = ((12.0 / fit.scale).round() as i32).max(SNAP_THRESHOLD);
            box_rect = box_rect
                .cursor(if is_dragged {
                    Cursor::Grabbing
                } else {
                    Cursor::Grab
                })
                .on_drag_end(move || {
                    outputs::drag_end(&n_end);
                })
                .on_drag(move |point| {
                    let current_drag = Settings::read().outputs.drag.clone();
                    if let Some(drag) = current_drag.filter(|d| d.output == n_drag) {
                        let dx = point.x - drag.start_offset.0;
                        let dy = point.y - drag.start_offset.1;
                        let d_log_x = (dx / fit.scale).round() as i32;
                        let d_log_y = (dy / fit.scale).round() as i32;
                        let cand_x = drag.original_logical.0 + d_log_x;
                        let cand_y = drag.original_logical.1 + d_log_y;
                        let (sx, sy) = snap(
                            cand_x,
                            cand_y,
                            logical.width,
                            logical.height,
                            &others,
                            snap_threshold,
                        );
                        outputs::drag_update(&n_drag, (sx, sy));
                        return;
                    }
                    outputs::drag_start(&n_drag, (point.x, point.y), (logical.x, logical.y));
                })
                .on_click(move |_| {
                    outputs::select(&n_click);
                });
        } else {
            box_rect = box_rect.on_click(move |_| {
                outputs::select(&n_click);
            });
        }

        layers.push(Box::new(box_rect));

        if let Some(drag) = state.drag.as_ref().filter(|_| is_dragged) {
            let (px, py, pw, ph) = fit.to_canvas(
                drag.current_logical.0,
                drag.current_logical.1,
                logical.width,
                logical.height,
            );
            let preview_box = monitor_box(monitor, pw, ph, true, false, roles)
                .inert(true)
                .translate(px, py);
            dragged_layer = Some(Box::new(preview_box));
        }
    }

    if let Some(dragged) = dragged_layer {
        layers.push(dragged);
    }

    let container = Rectangle::new()
        .width(canvas_w)
        .height(canvas_h)
        .radius(radius::CARD)
        .fill(roles.surface_container)
        .border(1.0, roles.edge())
        .semantics(|| Semantics::new(Role::Region, "outputs-canvas"))
        .clip()
        .child(Stack::new(layers).width(canvas_w).height(canvas_h));

    Box::new(container)
}

fn monitor_box(
    monitor: &Monitor,
    width: f32,
    height: f32,
    selected: bool,
    ghost: bool,
    roles: &ThemeRoles,
) -> Rectangle {
    let output = &monitor.output;
    let logical = output.logical.unwrap();

    let connector = &output.name;
    let label = Text::new(connector.clone())
        .size(theme::text::LABEL)
        .weight(theme::text::SEMIBOLD)
        .color(if ghost {
            roles.on_surface_variant
        } else if selected {
            roles.primary
        } else {
            roles.on_surface
        });

    let res_scale = format!(
        "{} \u{d7} {} \u{b7} {}x",
        logical.width,
        logical.height,
        crate::settings::niri::format_number(logical.scale, 2)
    );
    let sub = Text::new(res_scale)
        .size(theme::text::LABEL_SMALL)
        .color(roles.on_surface_variant);

    let items: Vec<Box<dyn Widget>> = if height >= 40.0 {
        vec![Box::new(label), Box::new(sub)]
    } else {
        vec![Box::new(label)]
    };
    let content = Column::new(items).align(Center).gap(2.0);

    let rect = Rectangle::new()
        .width(width)
        .height(height)
        .radius(radius::ROW)
        .align_child(Center, Center)
        .child(content);

    if ghost {
        rect.fill(roles.surface_container_high)
            .opacity(0.4)
            .border(1.0, roles.edge())
    } else if selected {
        rect.fill(roles.surface_container_high)
            .border(2.0, roles.primary)
    } else {
        rect.fill(roles.surface).border(1.0, roles.edge())
    }
}

fn disconnected_view(
    disconnected: &Disconnected,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    let name = disconnected.name.clone();
    let state = &settings.outputs;
    let blocked = state.blocked.is_some();
    let acts = !blocked && state.trial.is_none();

    let mut marks: Vec<Box<dyn Widget>> = vec![Box::new(
        Text::new("Disconnected")
            .size(theme::text::LABEL_SMALL)
            .color(roles.on_surface_variant)
            .weight(theme::text::MEDIUM),
    )];

    if acts && disconnected.config.set {
        let reset_name = name.clone();
        marks.push(link(
            format!("reset disconnected output {name}"),
            "reset",
            "Reset",
            settings,
            roles,
            move || outputs::reset(&reset_name),
        ));
    }

    let heading = Row::new(
        std::iter::once(Box::new(
            Text::new(name.clone())
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
        disconnected.config.block
    );
    let detail = [
        state
            .blocked
            .clone()
            .map(|w| format!("niri's config is read-only here: {w}")),
        disconnected.config.owner.clone(),
        disconnected.config.note.clone(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(". ");

    let head = vec![
        Box::new(heading) as Box<dyn Widget>,
        Box::new(wrapped(
            Text::new(if detail.is_empty() { block } else { detail })
                .size(theme::text::LABEL_SMALL)
                .color(if state.blocked.is_some() {
                    theme::window_signals().warning
                } else {
                    roles.on_surface_variant
                })
                .weight(theme::text::MEDIUM),
            content(),
        )),
    ];

    let mut rows: Vec<Box<dyn Widget>> = Vec::new();

    let keys = [
        (
            "mode",
            "Mode",
            "The display mode configured for this monitor",
        ),
        ("scale", "Scale", "The UI scale configured for this monitor"),
        (
            "position",
            "Position",
            "The desktop position configured for this monitor",
        ),
        (
            "transform",
            "Transform",
            "The rotation configured for this monitor",
        ),
        (
            "variable-refresh-rate",
            "Variable refresh rate",
            "VRR setting configured for this monitor",
        ),
        (
            "off",
            "Off",
            "Whether the monitor is configured to be disabled",
        ),
    ];

    for (key, title, help) in keys {
        if let Some(reading) = disconnected.config.readings.get(key) {
            let val = match reading {
                crate::niri_config::Reading::Own(v)
                | crate::niri_config::Reading::Included(v, _)
                | crate::niri_config::Reading::Overridden(v, _) => Some(v.clone()),
                crate::niri_config::Reading::Unset => None,
            };

            if let Some(val) = val {
                rows.push(labeled(
                    row_label(&name, key, title, help, None),
                    readonly(val, roles),
                    false,
                    settings,
                    roles,
                ));
            }
        }
    }

    if rows.is_empty() {
        rows.push(plain("No managed properties set in this block", roles));
    }

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
