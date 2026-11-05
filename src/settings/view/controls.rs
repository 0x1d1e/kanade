use std::rc::Rc;

use kanade_runtime::service::Service;
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Align, Button, Canvas, Center, Color, Column, Cursor, End, Gradient, Padding, Parent, Path,
    Rectangle, Reveal, Row, Shape, Size, SpaceBetween, Stack, Start, Text, TextInput, Widget,
    children,
};
use toml::Value;

use crate::config::{self, Choices, DayTime, Kind, Location, Setting};
use crate::doctor::Verdict;
use crate::icon::Icon;
use crate::look::{Along, Edge, ShellMode};
use crate::modules::{self, CORE, Module};
use crate::pane;
use crate::settings::health;
use crate::settings::layout::{MODULES, Page, Row as Line};
use crate::settings::login;
use crate::settings::ring::{self, Stop};
use crate::settings::{
    FONT_SEARCH, Origin, Settings, change as super_change, change_all as super_change_all,
    font_search, hover, input, path, select_font, turn_to_setting,
};
use crate::theme::space::TARGET;
use crate::theme::{self, ThemeRoles, radius};
use crate::tiles::{self, Placed};

use super::{
    MatchScore, content, inner, modules_view, most_beside, niri_page, outputs_page, place_page,
    row_stop_id, umbriel_page,
};

pub(crate) const BAR: f32 = 52.0;
pub(crate) const CARD_INSET: f32 = 16.0;
pub(crate) const ENTRY: f32 = 36.0;
pub(crate) const FIELD: f32 = 34.0;
pub(crate) const FONT_FIELD: f32 = 320.0;
pub(crate) const HALO: f32 = 4.0;
pub(crate) const HEADING: f32 = 8.0;
pub(crate) const MOST_MEASURE: f32 = 460.0;
pub(crate) const RING: f32 = 2.0;
pub(crate) const ROW_INSET: f32 = 14.0;
pub(crate) const SECTIONS: f32 = 24.0;
pub(crate) const VERDICT: f32 = 40.0;

// a whole number stepped by `-` and `+` or typed, kept from `least` to `most`
pub(crate) struct Steps {
    pub(crate) least: u64,
    pub(crate) most: u64,
    pub(crate) step: u64,
    pub(crate) unit: &'static str,
    pub(crate) expected: &'static str,
}

pub(crate) const MILLIS: Steps = Steps {
    least: config::SHORTEST,
    most: config::LONGEST,
    step: 10,
    unit: "ms",
    expected: "whole milliseconds",
};

// a size steps by as much as a list row or two
pub(crate) const PIXEL_STEP: u64 = 20;
pub(crate) const PERCENT_STEP: u64 = 10;
// an idle stage steps by half a minute
pub(crate) const SECONDS_STEP: u64 = 30;
// a lead or a staleness steps by five minutes, as calendars offer them
pub(crate) const MINUTE_STEP: u64 = 5;
// a warmth steps by about what the eye tells apart
pub(crate) const KELVIN_STEP: u64 = 100;

pub(crate) fn change(path: Vec<String>, value: Option<Value>) {
    let is_login =
        Settings::read().page == login::PAGE || path.first().is_some_and(|p| p == "sddm");
    super_change(path, value);
    if is_login {
        login::load();
    }
}

pub(crate) fn change_all(changes: Vec<(Vec<String>, Option<Value>)>) {
    let is_login = Settings::read().page == login::PAGE
        || changes
            .iter()
            .any(|(p, _)| p.first().is_some_and(|k| k == "sddm"));
    super_change_all(changes);
    if is_login {
        login::load();
    }
}

pub(crate) fn hover_wash(roles: &ThemeRoles) -> Color {
    theme::faded(roles.on_surface, 0.08)
}

pub(crate) fn hover_primary(roles: &ThemeRoles) -> Color {
    theme::mix(roles.primary, roles.on_primary, 0.12)
}

pub(crate) fn hover_container(roles: &ThemeRoles) -> Color {
    theme::mix(roles.surface_container_high, roles.on_surface, 0.08)
}

// what in the files keeps a change from applying, each in a box
pub(crate) fn notices(settings: &Settings) -> Vec<Box<dyn Widget>> {
    let problems = [
        settings.unreadable.clone(),
        settings
            .error
            .as_ref()
            .map(|error| format!("The config was not reloaded: {error}")),
        (!settings.skipped.is_empty())
            .then(|| format!("Skipped in the config: {}", settings.skipped.join("; "))),
    ];

    problems
        .into_iter()
        .flatten()
        .map(|problem| {
            Box::new(notice(&problem, theme::window_signals().warning)) as Box<dyn Widget>
        })
        .collect()
}

pub(crate) fn notice(text: &str, ink: Color) -> Rectangle {
    let text = Text::new(text)
        .size(theme::text::LABEL)
        .color(ink)
        .weight(theme::text::MEDIUM)
        .wrap();
    let height = text.height_in(content() - 2.0 * 12.0);

    Rectangle::new()
        .width(content())
        .height(height + 2.0 * 10.0)
        .radius(radius::ROW)
        .fill(faint(ink))
        .padding(Padding {
            top: 10.0,
            right: 12.0,
            bottom: 10.0,
            left: 12.0,
        })
        .child(text)
}

/*
 * a section's rows on a card, hairlines between them, under a heading when it has several rows:
 * one row is titled by its own label. The Modules row draws its own cards. `query` narrows the
 * Modules to those it mentions
 */
pub(crate) fn section_view(
    title: &'static str,
    rows: Vec<&Line>,
    settings: &Settings,
    query: Option<&str>,
    roles: &ThemeRoles,
) -> Column {
    let mut items: Vec<Box<dyn Widget>> = Vec::new();

    if (rows.len() > 1 || rows.iter().any(|row| matches!(row, Line::Outputs))) && !title.is_empty()
    {
        items.push(Box::new(
            Text::new(title)
                .size(theme::text::TITLE)
                .color(roles.on_surface)
                .weight(theme::text::SEMIBOLD),
        ));
    }

    let mut card: Vec<Box<dyn Widget>> = Vec::new();
    for row in rows {
        if let Line::Key(MODULES) = row {
            items.push(Box::new(modules_view(settings, query, roles)));
            continue;
        }

        if let Line::Outputs = row {
            items.push(Box::new(outputs_page::section(settings, roles)));
            continue;
        }

        if !card.is_empty() {
            card.push(Box::new(hairline(inner(), roles)));
        }
        card.push(row_view(row, settings, roles));
    }

    if !card.is_empty() {
        items.push(Box::new(card_of(card, roles)));
    }

    Column::new(items).width(content()).gap(HEADING)
}

pub(crate) fn section_view_matched(
    page: &'static Page,
    title: &'static str,
    first_stop: Option<&str>,
    rows: Vec<(&'static Line, MatchScore)>,
    settings: &Settings,
    query: Option<&str>,
    roles: &ThemeRoles,
) -> Column {
    let mut items: Vec<Box<dyn Widget>> = Vec::new();
    items.push(found_heading(page, title, first_stop, settings, roles));

    let mut card: Vec<Box<dyn Widget>> = Vec::new();
    for (row, match_score) in rows {
        if let Line::Key(MODULES) = row {
            items.push(Box::new(modules_view(settings, query, roles)));
            continue;
        }

        if let Line::Outputs = row {
            items.push(Box::new(outputs_page::section(settings, roles)));
            continue;
        }

        if !card.is_empty() {
            card.push(Box::new(hairline(inner(), roles)));
        }
        card.push(row_view_detailed(
            row,
            Some((page.name, match_score)),
            settings,
            roles,
        ));
    }

    if !card.is_empty() {
        items.push(Box::new(card_of(card, roles)));
    }

    Column::new(items).width(content()).gap(HEADING)
}

// "Page  ›  Section" over a result, pressed to open the page
pub(crate) fn found_heading(
    page: &'static Page,
    title: &str,
    first_stop: Option<&str>,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    let name = page.name;
    let target_stop = first_stop.unwrap_or("").to_string();
    let target_stop_clone = target_stop.clone();
    let mut path = children![
        page.icon.on(14.0, roles.on_surface_variant),
        Text::new(page.title)
            .size(theme::text::LABEL)
            .color(roles.on_surface_variant)
            .weight(theme::text::SEMIBOLD),
    ];
    if !title.is_empty() {
        path.push(Box::new(
            Text::new("\u{203a}")
                .size(theme::text::LABEL)
                .color(roles.on_surface_variant)
                .weight(theme::text::MEDIUM),
        ));
        path.push(Box::new(
            Text::new(title)
                .size(theme::text::LABEL)
                .color(roles.on_surface)
                .weight(theme::text::SEMIBOLD),
        ));
    }

    let row = Row::new(path).gap(8.0).align(Center);
    let width = fixed(Widget::width(&row)) + 2.0 * 8.0;
    let id = format!("open {name} {title}");
    let hovered = settings.is_hovered(&id);
    let hover_id = id.clone();

    let press = move || turn_to_setting(name, &target_stop_clone);
    let press_stop = move || turn_to_setting(name, &target_stop);

    let control = Rectangle::new()
        .width(width)
        .height(28.0)
        .radius(radius::ROW - 4.0)
        .padding(sides(8.0))
        .align_child(Start, Center)
        .cursor(Cursor::Pointer)
        .on_hover(move |inside| hover(hover_id.clone(), inside))
        .semantics(|| {
            Semantics::new(Role::Button, "open")
                .label(format!("{} {title}", page.title).trim().to_owned())
        })
        .on_click(left(press))
        .child(row);
    let control = if hovered {
        control.fill(hover_wash(roles))
    } else {
        control
    };

    outlined(
        control,
        ring::register(Stop::new(id).press(press_stop)),
        None,
        roles,
    )
}

/*
 * a card's rows, the same `CARD_INSET` in from its edge on every side, so a row's hover wash stays
 * clear of the card's corner instead of meeting it. The inset and `radius::CARD` are the same, so a
 * row's straight edge begins where the card's corner ends and the row keeps its own smaller
 * `radius::ROW`: a nested radius of `radius::CARD - CARD_INSET` would be square here. The height
 * adds the inset back, so the column still fits
 */
pub(crate) fn card_of(rows: Vec<Box<dyn Widget>>, roles: &ThemeRoles) -> Rectangle {
    let column = Column::new(rows).width(inner());
    let height = fixed(Widget::height(&column)) + 2.0 * CARD_INSET;

    Rectangle::new()
        .width(content())
        .height(height)
        .radius(radius::CARD)
        .fill(roles.card())
        .padding(CARD_INSET)
        .child(column)
}

pub(crate) fn row_view(row: &Line, settings: &Settings, roles: &ThemeRoles) -> Box<dyn Widget> {
    row_view_detailed(row, None, settings, roles)
}

pub(crate) fn row_view_detailed(
    row: &Line,
    match_info: Option<(&'static str, MatchScore)>,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    let (direct_nav, hint) = match match_info {
        Some((page_name, _score)) => {
            let nav = row_stop_id(row).map(|stop_id| (page_name, stop_id));
            (nav, None)
        }
        None => (None, None),
    };

    match *row {
        Line::Key(key) => {
            let setting = setting(key).expect("the layout's keys are the schema's");
            let (control, beside) = control(settings, setting, roles);

            let mut label_widget =
                label(settings, &[setting], setting.label, &help(setting), roles);
            label_widget.hint = hint;
            label_widget.direct_nav = direct_nav;

            labeled(label_widget, control, beside, settings, roles)
        }
        Line::Health => health_view(settings, roles),
        Line::Login => login_view(settings, roles),
        Line::Place => place_page::row(settings, roles),
        Line::Niri(path) => niri_page::row(path, settings, roles),
        Line::Outputs => Box::new(outputs_page::section(settings, roles)),
        Line::NiriRule(id) => niri_page::rule_row(id, settings, roles),
        Line::Umbriel(path) => umbriel_page::row(path, settings, roles),
        Line::UmbrielRule(id) => umbriel_page::rule_row(id, settings, roles),
        Line::Position { edge, align } => {
            let edge = setting(edge).expect("the layout's keys are the schema's");
            let align = setting(align).expect("the layout's keys are the schema's");

            let (title, description) = match edge.key {
                "shell.edge" => (
                    "Combined position",
                    "The combined shell uses a top or bottom edge; the separate placements stay untouched",
                ),
                "island.edge" if settings.read.shell.mode == ShellMode::Combined => (
                    "Separate Island position",
                    "Saved for Separate mode. Combined uses its own position",
                ),
                "dock.edge" if settings.read.shell.mode == ShellMode::Combined => (
                    "Separate Dock position",
                    "Saved for Separate mode. Combined uses its own position",
                ),
                "island.edge" => (
                    "Separate Island position",
                    "Used only in Separate mode; Combined has its own position",
                ),
                "dock.edge" => (
                    "Separate Dock position",
                    "Used only in Separate mode; Combined has its own position",
                ),
                _ => ("Position", "Which edge of the screen, and where along it"),
            };

            let mut label_widget = label(settings, &[edge, align], title, description, roles);
            label_widget.hint = hint;
            label_widget.direct_nav = direct_nav;

            labeled(
                label_widget,
                Box::new(position(settings, edge, align, roles)),
                true,
                settings,
                roles,
            )
        }
    }
}

/*
 * the Health page's checks as doctor words them, each with its verdict and detail lines under it,
 * over how many need attention and Check again. Read-only: the checks ran off the draw thread
 */
pub(crate) fn health_view(settings: &Settings, roles: &ThemeRoles) -> Box<dyn Widget> {
    let signals = theme::window_signals();
    let waiting = health::needing(&settings.health);

    let summary = if settings.health_loading {
        String::from("Checking\u{2026}")
    } else if !settings.health_loaded {
        String::from("Not checked yet")
    } else if waiting == 0 {
        format!("All {} checks pass", settings.health.len())
    } else {
        format!(
            "{waiting} of {} checks need attention",
            settings.health.len()
        )
    };
    let ink = if waiting > 0 && !settings.health_loading {
        signals.warning
    } else {
        roles.on_surface
    };

    let again = link(
        "check again".into(),
        "check-again",
        "Check again",
        settings,
        roles,
        health::load,
    );
    let mut items = children![
        Row::new(children![
            Text::new(summary)
                .size(theme::text::BODY)
                .color(ink)
                .weight(theme::text::SEMIBOLD),
            again,
        ])
        .width(inner())
        .justify(SpaceBetween)
        .align(Center),
    ];

    let body = inner() - VERDICT - 10.0;
    for check in &settings.health {
        let (word, ink) = match check.verdict {
            Verdict::Ok => ("ok", roles.on_surface_variant),
            Verdict::Warn => ("warn", signals.warning),
            Verdict::Fail => ("fail", signals.critical),
        };

        let mut lines = children![wrapped(
            Text::new(check.text.clone())
                .size(theme::text::LABEL)
                .color(roles.on_surface)
                .weight(theme::text::MEDIUM),
            body,
        )];
        for line in &check.detail {
            lines.push(Box::new(wrapped(
                Text::new(line.clone())
                    .size(theme::text::LABEL_SMALL)
                    .color(roles.on_surface_variant)
                    .weight(theme::text::MEDIUM),
                body,
            )));
        }

        items.push(Box::new(hairline(inner(), roles)));
        items.push(Box::new(
            Row::new(children![
                Rectangle::new().width(VERDICT).height(18.0).child(
                    Text::new(word)
                        .size(theme::text::LABEL)
                        .color(ink)
                        .weight(theme::text::SEMIBOLD),
                ),
                Column::new(lines).width(body).gap(2.0),
            ])
            .width(inner())
            .gap(10.0),
        ));
    }

    Box::new(Column::new(items).width(inner()).gap(10.0))
}

pub(crate) fn is_status_warning(text: &str) -> bool {
    text.contains("not ready")
        || text.contains("missing")
        || text.contains("WARNING")
        || text.contains("cannot read")
        || text.contains("stale")
}

/*
 * the Login screen page's status as `kanade sddm status` words it, over whether SDDM reads the
 * overlay, and the root commands still to run, each to copy. Read-only: Kanade never runs them
 */
pub(crate) fn login_view(settings: &Settings, roles: &ThemeRoles) -> Box<dyn Widget> {
    let signals = theme::window_signals();
    let body = inner();

    let (summary, ink) = match &settings.login {
        _ if settings.login_loading => (String::from("Checking\u{2026}"), roles.on_surface),
        None => (String::from("Not checked yet"), roles.on_surface),
        Some(Err(_)) => (String::from("Cannot be checked"), signals.warning),
        Some(Ok(status))
            if status.enabled
                && status.ready
                && !status.lines.iter().any(|line| is_status_warning(line)) =>
        {
            (
                String::from("Enabled: SDDM reads the ready overlay"),
                roles.on_surface,
            )
        }
        Some(Ok(status)) if status.enabled => (
            String::from("Enabled: SDDM reads the overlay, and a row below is not current"),
            signals.warning,
        ),
        Some(Ok(_)) => (String::from("Not enabled yet"), signals.warning),
    };

    let again = link(
        "refresh login".into(),
        "refresh",
        "Refresh",
        settings,
        roles,
        login::load,
    );
    let mut items = children![
        Row::new(children![
            Text::new(summary)
                .size(theme::text::BODY)
                .color(ink)
                .weight(theme::text::SEMIBOLD),
            again,
        ])
        .width(inner())
        .justify(SpaceBetween)
        .align(Center),
    ];

    let line = |text: String, ink| {
        Box::new(wrapped(
            Text::new(text)
                .size(theme::text::LABEL_SMALL)
                .color(ink)
                .weight(theme::text::MEDIUM),
            body,
        )) as Box<dyn Widget>
    };

    match &settings.login {
        Some(Err(error)) => items.push(line(error.clone(), roles.on_surface_variant)),
        Some(Ok(status)) => {
            for text in &status.lines {
                let ink = if is_status_warning(text) {
                    signals.warning
                } else {
                    roles.on_surface_variant
                };
                items.push(line(text.clone(), ink));
            }

            items.push(Box::new(hairline(inner(), roles)));
            items.push(line(
                if !status.steps.is_empty() {
                    String::from(
                        "In this order (Kanade never runs the commands for root; the last one only acts once the theme is in place):",
                    )
                } else if status.blocked {
                    String::from("As root: edit the file named above, which no command of Kanade's can outdo")
                } else {
                    String::from("Nothing to do as root; `kanade sddm sync` writes the look")
                },
                roles.on_surface,
            ));

            for (index, step) in status.steps.iter().enumerate() {
                let copied = step.command().to_owned();
                let shown = match step {
                    crate::sddm::Step::Root(command) => format!("as root: {command}"),
                    crate::sddm::Step::Sync => String::from("in a terminal: kanade sddm sync"),
                };
                items.push(Box::new(
                    Row::new(children![
                        wrapped(
                            Text::new(shown)
                                .size(theme::text::LABEL_SMALL)
                                .color(roles.on_surface)
                                .weight(theme::text::MEDIUM),
                            body - 70.0,
                        ),
                        link(
                            format!("copy login step {index}"),
                            "copy",
                            "Copy",
                            settings,
                            roles,
                            move || {
                                let text = copied.clone();
                                std::thread::spawn(move || {
                                    let _ = crate::sources::clipboard::put(&text);
                                });
                            },
                        ),
                    ])
                    .width(inner())
                    .justify(SpaceBetween)
                    .align(Center),
                ));
            }
        }
        None => {}
    }

    Box::new(Column::new(items).width(inner()).gap(8.0))
}

// a row's label over its help, with its marks, and what Show technical details shows
pub(crate) struct Label {
    pub(crate) node: &'static str,
    pub(crate) hover_id: String,
    pub(crate) title: String,
    pub(crate) help: String,
    pub(crate) marks: Vec<Box<dyn Widget>>,
    pub(crate) detail: Option<String>,
    pub(crate) keys: Option<Vec<String>>,
    pub(crate) value: Option<String>,
    pub(crate) off: Option<String>,
    pub(crate) hint: Option<String>,
    pub(crate) direct_nav: Option<(&'static str, String)>,
}

// a setting's effective value as a short line of text, for Show technical details; none for a key
// that sets nothing (an empty list, for example)
pub(crate) fn value_text(value: &Value) -> String {
    match value {
        Value::Boolean(b) => b.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(list) => list.iter().map(value_text).collect::<Vec<_>>().join(", "),
        Value::Table(_) | Value::Datetime(_) => String::from("{…}"),
    }
}

pub(crate) fn label(
    settings: &Settings,
    shown: &[&'static Setting],
    title: &str,
    help: &str,
    roles: &ThemeRoles,
) -> Label {
    let mut marks: Vec<Box<dyn Widget>> = Vec::new();

    if shown
        .iter()
        .any(|setting| setting.tail.restart && settings.pending(setting.key))
    {
        marks.push(Box::new(restart(roles)));
    }

    let paths: Vec<Vec<String>> = shown
        .iter()
        .map(|setting| path(setting.key, None))
        .filter(|path| settings.set_here(path))
        .collect();
    if !paths.is_empty() {
        marks.push(link(
            format!("reset {}", shown[0].key),
            "reset",
            "Reset",
            settings,
            roles,
            move || change_all(paths.iter().map(|path| (path.clone(), None)).collect()),
        ));
    }

    let detail = shown
        .iter()
        .find(|setting| setting.tail.hint.is_some())
        .filter(|_| settings.inspect)
        .map(|setting| capitalized(setting.help));

    let keys = settings.inspect.then(|| {
        shown
            .iter()
            .map(|setting| {
                let tag = match settings.origin(setting.key) {
                    Origin::SettingsOverride => "(Settings override)",
                    Origin::ConfigFile => "(config file)",
                    Origin::Default => "(default)",
                };
                format!("{}  {tag}", setting.key)
            })
            .collect::<Vec<_>>()
    });
    let value = settings
        .inspect
        .then(|| {
            shown
                .iter()
                .filter_map(|setting| setting.kind.value(&settings.read).as_ref().map(value_text))
                .collect::<Vec<_>>()
                .join("   ")
        })
        .filter(|value| !value.is_empty());

    let off = shown.iter().find_map(|setting| {
        let owner = owner(setting.key)?;

        (owner.name != CORE && !modules::on(owner.name))
            .then(|| format!("Requires {} module", owner.name))
    });

    Label {
        node: shown[0].key,
        hover_id: shown[0].key.to_string(),
        title: title.to_owned(),
        help: help.to_owned(),
        marks,
        detail,
        keys,
        value,
        off,
        hint: None,
        direct_nav: None,
    }
}

// a label with its control beside it, or under it when the control is wide
pub(crate) fn labeled(
    label: Label,
    control: Box<dyn Widget>,
    beside: bool,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    let width = fixed(control.width());
    let beside = beside && width <= most_beside();
    let text_width = if beside {
        inner() - width - 24.0
    } else {
        inner()
    }
    .min(MOST_MEASURE);

    let title_text = Text::new(&label.title)
        .size(theme::text::BODY)
        .color(roles.on_surface)
        .weight(theme::text::SEMIBOLD);

    let title_widget: Box<dyn Widget> = if let Some((page, stop_id)) = label.direct_nav {
        let target_stop = stop_id.clone();
        let press = move || turn_to_setting(page, &target_stop);
        let id = format!("jump to {page} {stop_id}");
        let hovered = settings.is_hovered(&id);
        let hover_id = id.clone();

        let title_row = Row::new(children![
            title_text,
            Icon::Forward.on(12.0, roles.on_surface_variant),
        ])
        .gap(6.0)
        .align(Center);

        let mut btn = Rectangle::new()
            .width(fixed(Widget::width(&title_row)) + 8.0)
            .height(24.0)
            .radius(5.0)
            .padding(sides(4.0))
            .align_child(Start, Center)
            .cursor(Cursor::Pointer)
            .on_hover(move |inside| hover(hover_id.clone(), inside))
            .semantics(move || Semantics::new(Role::Button, "jump to setting"))
            .on_click(left(press))
            .child(title_row);

        if hovered {
            btn = btn.fill(hover_wash(roles));
        }

        Box::new(btn)
    } else {
        Box::new(title_text)
    };

    let mut title_items: Vec<Box<dyn Widget>> = vec![title_widget];
    if let Some(hint) = label.hint {
        title_items.push(Box::new(
            Text::new(hint)
                .size(theme::text::LABEL_SMALL)
                .color(roles.on_surface_variant)
                .weight(theme::text::MEDIUM),
        ));
    }
    title_items.extend(label.marks);

    let mut lines = children![
        Row::new(title_items).height(24.0).gap(10.0).align(Center),
        wrapped(
            Text::new(label.help)
                .size(theme::text::LABEL)
                .color(roles.on_surface_variant)
                .weight(theme::text::MEDIUM),
            text_width,
        ),
    ];
    if let Some(detail) = label.detail {
        lines.push(Box::new(wrapped(
            Text::new(detail)
                .size(theme::text::LABEL_SMALL)
                .color(roles.on_surface_variant)
                .weight(theme::text::MEDIUM),
            text_width,
        )));
    }
    if let Some(off) = label.off {
        lines.push(Box::new(wrapped(
            Text::new(off)
                .size(theme::text::LABEL_SMALL)
                .color(theme::window_signals().warning)
                .weight(theme::text::MEDIUM),
            text_width,
        )));
    }
    if let Some(keys) = &label.keys {
        for key in keys {
            lines.push(copy_key_row(key, text_width, settings, roles));
        }
    }
    if let Some(value) = &label.value {
        lines.push(Box::new(wrapped(
            Text::new(value)
                .size(theme::text::LABEL_SMALL)
                .font("monospace")
                .color(roles.on_surface_variant)
                .weight(theme::text::MEDIUM),
            text_width,
        )));
    }
    let text = Column::new(lines).width(text_width).gap(4.0);

    let content: Box<dyn Widget> = if beside {
        let height = fixed(Widget::height(&text)).max(fixed(control.height()));

        Box::new(
            Row::new(vec![Box::new(text), control])
                .width(inner())
                .height(height)
                .justify(SpaceBetween)
                .align(Center),
        )
    } else {
        Box::new(
            Column::new(vec![Box::new(text), control])
                .width(inner())
                .gap(12.0),
        )
    };

    let height = fixed(content.height()) + 2.0 * ROW_INSET;

    let node = label.node;
    let hover_id = label.hover_id;
    let hovered = settings.is_hovered(&hover_id);

    let mut row = Rectangle::new()
        .width(inner())
        .height(height)
        .radius(radius::ROW)
        .padding(Padding {
            top: ROW_INSET,
            right: 0.0,
            bottom: ROW_INSET,
            left: 0.0,
        })
        .semantics(|| Semantics::new(Role::Region, node))
        .align_child(Start, Center)
        .on_hover(move |inside| hover(hover_id.clone(), inside));

    if hovered {
        row = row.fill(hover_wash(roles));
    }

    Box::new(row.child(Column::new(vec![content]).width(inner())))
}

// one config key shown in technical mode, with a Copy action that puts the key on the clipboard
pub(crate) fn copy_key_row(
    key: &str,
    width: f32,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    let id = format!("copy {key}");
    let hovered = settings.is_hovered(&id);
    let hover_id = id.clone();
    // the row shows the key with an origin marker, e.g. "island.autohide  (Settings override)"; copy the bare key
    let copy = key.split_once("  (").map(|(key, _)| key).unwrap_or(key);
    let copy_owned = copy.to_owned();
    let press: Rc<dyn Fn()> = Rc::new(move || {
        let text = copy_owned.clone();
        std::thread::spawn(move || {
            let _ = crate::sources::clipboard::put(&text);
        });
    });
    let press_click = press.clone();

    let text = Text::new("Copy")
        .size(theme::text::LABEL_SMALL)
        .color(roles.primary)
        .weight(theme::text::SEMIBOLD);
    let button_width = 14.0 + 6.0 + fixed(Widget::width(&text)) + 12.0;

    let control = Rectangle::new()
        .width(button_width)
        .height(20.0)
        .radius(5.0)
        .cursor(Cursor::Pointer)
        .on_hover(move |inside| hover(hover_id.clone(), inside))
        .semantics(|| Semantics::new(Role::Button, "copy"))
        .on_click(left(move || press_click()))
        .child(
            Row::new(children![Icon::Clipboard.on(12.0, roles.primary), text,])
                .gap(6.0)
                .align(Center),
        );

    let control = if hovered {
        control.fill(hover_wash(roles))
    } else {
        control
    };

    let row = Row::new(children![
        Text::new(key)
            .size(theme::text::LABEL_SMALL)
            .font("monospace")
            .color(roles.primary)
            .weight(theme::text::MEDIUM)
            .elide(),
        outlined(
            control,
            ring::register(Stop::new(id).press(move || press())),
            None,
            roles,
        ),
    ])
    .width(width)
    .gap(8.0)
    .justify(SpaceBetween)
    .align(Center);

    Box::new(row)
}

// what sets the key, by its Kind, showing the value the files give, and whether it fits beside
pub(crate) fn control(
    settings: &Settings,
    setting: &'static Setting,
    roles: &ThemeRoles,
) -> (Box<dyn Widget>, bool) {
    let read = &settings.read;
    let key = setting.key;

    match &setting.kind {
        Kind::Switch(field) => {
            let on = (field.get)(read);

            (
                switch(format!("switch {key}"), on, settings, roles, move || {
                    change(path(key, None), Some(Value::Boolean(!on)));
                }),
                true,
            )
        }
        Kind::Choice(options, field) => {
            let chosen = (field.get)(read);
            let enabled = key != "dock.merge" || read.shell.mode == ShellMode::Separate;

            let choose = move |option: &'static str| {
                change(path(key, None), Some(Value::String(option.to_owned())));
            };
            let id = format!("choice {key}");

            let control = match key {
                "appearance.tone" => {
                    cards(id, options, chosen, settings, roles, tone_preview, choose)
                }
                "appearance.finish" => {
                    cards(id, options, chosen, settings, roles, finish_preview, choose)
                }
                "appearance.motion" => cards(
                    id,
                    options,
                    chosen,
                    settings,
                    roles,
                    |option| spring_preview(option, roles),
                    choose,
                ),
                _ => segmented(id, options, chosen, settings, roles, enabled, choose),
            };

            (control, true)
        }
        Kind::Millis(field) => {
            let ms = u64::try_from((field.get)(read).as_millis()).unwrap_or(config::LONGEST);

            (stepper(setting, ms, MILLIS, settings, roles), true)
        }
        Kind::Pixels(range, field) => {
            let px = u64::from((field.get)(read));
            let steps = Steps {
                least: u64::from(range.least),
                most: u64::from(range.most),
                step: PIXEL_STEP,
                unit: "px",
                expected: "whole pixels",
            };

            (stepper(setting, px, steps, settings, roles), true)
        }
        Kind::Percent(range, field) => {
            let percent = u64::from((field.get)(read));
            let steps = Steps {
                least: u64::from(range.least),
                most: u64::from(range.most),
                step: PERCENT_STEP,
                unit: "%",
                expected: "whole percent",
            };

            (stepper(setting, percent, steps, settings, roles), true)
        }
        Kind::Seconds(range, field) => {
            let seconds = u64::from((field.get)(read));
            let steps = Steps {
                least: u64::from(range.least),
                most: u64::from(range.most),
                step: SECONDS_STEP,
                unit: "s",
                expected: "whole seconds",
            };

            (stepper(setting, seconds, steps, settings, roles), true)
        }
        Kind::Minutes(range, field) => {
            let minutes = (field.get)(read).as_secs() / 60;
            let steps = Steps {
                least: u64::from(range.least),
                most: u64::from(range.most),
                step: MINUTE_STEP,
                unit: "min",
                expected: "whole minutes",
            };

            (stepper(setting, minutes, steps, settings, roles), true)
        }
        Kind::Kelvin(range, field) => {
            let kelvin = u64::from((field.get)(read));
            let steps = Steps {
                least: u64::from(range.least),
                most: u64::from(range.most),
                step: KELVIN_STEP,
                unit: "K",
                expected: "whole kelvin",
            };

            (stepper(setting, kelvin, steps, settings, roles), true)
        }
        Kind::DayTime(_) => (
            Box::new(field_with_hint(
                input(setting),
                "HH:MM",
                roles,
                move |text| {
                    let text = text.trim();
                    if text.is_empty() {
                        change(path(key, None), None);
                        return;
                    }

                    match DayTime::parse(text) {
                        Ok(time) => change(path(key, None), Some(Value::String(time.to_string()))),
                        Err(why) => {
                            Settings::write().refused = Some(format!("Not saved: {key}: {why}"))
                        }
                    }
                },
            )),
            true,
        ),
        Kind::Count(count, field) => {
            let number = u64::from((field.get)(read));
            let steps = Steps {
                least: u64::from(count.range.least),
                most: u64::from(count.range.most),
                step: 1,
                unit: count.unit,
                expected: count.expected,
            };

            (stepper(setting, number, steps, settings, roles), true)
        }
        Kind::Text(_) if key == "appearance.font" => {
            (Box::new(font_picker(settings, roles)), false)
        }
        Kind::Path(_) | Kind::Text(_) | Kind::Node(_) | Kind::PamService(_) => (
            Box::new(field_with_hint(
                input(setting),
                setting.tail.example.unwrap_or("").trim_matches('"'),
                roles,
                move |text| {
                    let text = text.trim();
                    let value = (!text.is_empty()).then(|| Value::String(text.to_owned()));

                    change(path(key, None), value);
                },
            )),
            true,
        ),
        Kind::Location(_) => (
            Box::new(field_with_hint(
                input(setting),
                "latitude, longitude",
                roles,
                move |text| {
                    let text = text.trim();
                    if text.is_empty() {
                        change(path(key, None), None);
                        return;
                    }

                    match Location::parse(text) {
                        Ok(value) => change(path(key, None), Some(value)),
                        Err(why) => {
                            Settings::write().refused = Some(format!("Not saved: {key}: {why}"))
                        }
                    }
                },
            )),
            true,
        ),
        Kind::DesktopIds(field) => (
            Box::new(list(
                setting,
                (field.get)(read),
                "Add a .desktop file id, then Enter",
                None,
                settings,
                roles,
            )),
            false,
        ),
        Kind::Words(field) => (
            Box::new(list(
                setting,
                (field.get)(read),
                "Add an app id, then Enter",
                None,
                settings,
                roles,
            )),
            false,
        ),
        Kind::Paths(field) => (
            Box::new(list(
                setting,
                (field.get)(read),
                "Add a file or directory, then Enter",
                None,
                settings,
                roles,
            )),
            false,
        ),
        Kind::Urls(field) => (
            Box::new(list(
                setting,
                (field.get)(read),
                "Add a feed address, then Enter",
                None,
                settings,
                roles,
            )),
            false,
        ),
        Kind::Tiles(field) => (
            Box::new(list(
                setting,
                (field.get)(read).iter().map(Placed::to_string).collect(),
                "Add a tile, like focus or wifi:full, then Enter",
                Some(tiles::cycled),
                settings,
                roles,
            )),
            false,
        ),
        Kind::AppIds(field) => {
            let mut rows: Vec<Box<dyn Widget>> = (field.get)(read)
                .into_iter()
                .map(|(app_id, id)| {
                    let entry = path(key, Some(&app_id));

                    // one from the user's own config is theirs to remove
                    let actions = if settings.set_here(&entry) {
                        children![icon_button(
                            format!("remove {key} {app_id}"),
                            "remove",
                            Icon::Dismiss,
                            settings,
                            roles,
                            move || change(entry.clone(), None)
                        )]
                    } else {
                        children![]
                    };

                    Box::new(entry_line(
                        &format!("{app_id}  \u{2192}  {id}"),
                        actions,
                        settings,
                        roles,
                    )) as Box<dyn Widget>
                })
                .collect();

            rows.push(text_field(
                input(setting),
                inner(),
                "app id = .desktop file id, then Enter",
                roles,
                move |text| match text.split_once('=') {
                    Some((app_id, id)) if !app_id.trim().is_empty() => change(
                        path(key, Some(app_id.trim())),
                        Some(Value::String(id.trim().to_owned())),
                    ),
                    _ => {
                        Settings::write().refused = Some(format!(
                            "Not saved: {key}: expected app id = .desktop file id"
                        ));
                    }
                },
            ));

            (Box::new(Column::new(rows).width(inner()).gap(6.0)), false)
        }
        Kind::Modules(_) => unreachable!("section_view draws the Modules row itself"),
        Kind::Gestures(_) => unreachable!("gestures are set in config files only"),
        Kind::Rules(_) => unreachable!("notifications.rules is set in config files only"),
        Kind::Quicklinks(_) => unreachable!("launcher.quicklinks is set in config files only"),
        Kind::Argv(_) => unreachable!("hooks are set in config files only"),
        Kind::Names(..) => unreachable!("focus.auto is set in config files only"),
        Kind::DeskPlaces(_) => {
            unreachable!("desk.place is set in config files and by dragging a face's handle")
        }
    }
}

pub(crate) fn font_picker(settings: &Settings, roles: &ThemeRoles) -> Column {
    const SUGGESTIONS: usize = 8;

    let families = settings.fonts.clone();
    let matching = crate::settings::fonts::matching(&families, &settings.font_query, SUGGESTIONS);
    let chosen = settings.read.appearance.font.as_deref();
    let query = settings.font_query.clone();
    let field = outlined(
        Rectangle::new()
            .width(FONT_FIELD)
            .height(FIELD)
            .radius(radius::ROW - 2.0)
            .fill(roles.surface)
            .padding(sides(10.0))
            .align_child(Start, Center)
            .child(
                TextInput::new(FONT_SEARCH)
                    .width(Parent)
                    .size(theme::text::LABEL)
                    .color(roles.on_surface)
                    .placeholder("Search installed font families")
                    .on_change(font_search)
                    .on_submit(move |query| {
                        if let Some(family) = crate::settings::fonts::matching(&families, &query, 1)
                            .into_iter()
                            .next()
                        {
                            select_font(Some(family));
                        }
                    }),
            ),
        ring::register(Stop::new("font").input(FONT_SEARCH)),
        Some(roles.edge()),
        roles,
    );

    let mut items = children![field];
    if settings.fonts_loading {
        items.push(Box::new(font_note(
            "Listing installed font families…",
            roles,
        )));
    } else if let Some(error) = &settings.fonts_error {
        items.push(Box::new(font_note(error, roles)));
    } else if settings.fonts_loaded && settings.fonts.is_empty() {
        items.push(Box::new(font_note(
            "Fontconfig returned no installed families",
            roles,
        )));
    } else if query.trim().is_empty() {
        let system = if chosen.is_some() {
            "Use the system default"
        } else {
            "Using the default: Open Sauce One"
        };
        if chosen.is_some() {
            items.push(font_choice(system, false, settings, roles, || {
                select_font(None)
            }));
        } else {
            items.push(Box::new(font_note(system, roles)));
        }
    } else if matching.is_empty() {
        items.push(Box::new(font_note("No installed family matches", roles)));
    } else {
        for family in matching {
            let selected = chosen.is_some_and(|chosen| chosen.eq_ignore_ascii_case(&family));
            let choice = family.clone();
            items.push(font_choice(&family, selected, settings, roles, move || {
                select_font(Some(choice.clone()))
            }));
        }
        items.push(Box::new(font_note(
            "Type to narrow the list. Enter chooses the first match.",
            roles,
        )));
    }

    Column::new(items).width(FONT_FIELD).gap(6.0)
}

pub(crate) fn font_choice(
    name: &str,
    selected: bool,
    settings: &Settings,
    roles: &ThemeRoles,
    press: impl Fn() + 'static,
) -> Box<dyn Widget> {
    let id = format!("font {name}");
    let hovered = settings.is_hovered(&id);
    let text = Text::new(name.to_owned())
        .size(theme::text::LABEL)
        .color(roles.on_surface)
        .weight(if selected {
            theme::text::SEMIBOLD
        } else {
            theme::text::MEDIUM
        })
        .elide();
    let row = Rectangle::new()
        .width(320.0)
        .height(34.0)
        .radius(radius::ROW - 4.0)
        .padding(sides(10.0))
        .semantics(|| {
            Semantics::new(Role::Button, "font")
                .content(name)
                .selected(selected)
        })
        .align_child(Start, Center)
        .child(text);
    let row = if selected {
        if hovered {
            row.fill(hover_container(roles))
        } else {
            row.fill(roles.surface_container_high)
        }
    } else if hovered {
        row.fill(hover_wash(roles))
    } else {
        row
    };

    button_styled(row, id, press, None, false, settings, roles)
}

pub(crate) fn font_note(text: &str, roles: &ThemeRoles) -> Rectangle {
    wrapped(
        Text::new(text)
            .size(theme::text::LABEL_SMALL)
            .color(roles.on_surface_variant)
            .weight(theme::text::MEDIUM),
        FONT_FIELD,
    )
}

// a small screen with the places an edge and a side make, the chosen one filled
pub(crate) fn position(
    settings: &Settings,
    edge: &'static Setting,
    align: &'static Setting,
    roles: &ThemeRoles,
) -> Rectangle {
    let (Kind::Choice(_, edge_field), Kind::Choice(_, align_field)) = (&edge.kind, &align.kind)
    else {
        return Rectangle::new().width(0.0).height(0.0);
    };
    let read = (
        (edge_field.get)(&settings.read),
        (align_field.get)(&settings.read),
    );
    let (edge_key, align_key) = (edge.key, align.key);
    let active = match edge.key {
        "shell.edge" => settings.read.shell.mode == ShellMode::Combined,
        "island.edge" | "dock.edge" => settings.read.shell.mode == ShellMode::Separate,
        _ => true,
    };
    let combined_position = edge.key == "shell.edge";

    // a place to press: the pill as the bar would lie there, lit where it is now
    let place = move |to: (&'static str, &'static str), (width, height): (f32, f32)| {
        let sideways = Edge::named(to.0) != Edge::Top && Edge::named(to.0) != Edge::Bottom;

        // on a side edge the side does not count, and is kept for going back to the top or bottom
        let here = to.0 == read.0 && (sideways || to.1 == read.1);

        let press = move || {
            let mut changes = Vec::new();
            if to.0 != read.0 {
                changes.push((path(edge_key, None), Some(Value::String(to.0.into()))));
            }
            if !sideways && to.1 != read.1 {
                changes.push((path(align_key, None), Some(Value::String(to.1.into()))));
            }
            change_all(changes);
        };

        let (cell_width, cell_height) = if sideways { (24.0, 44.0) } else { (48.0, 24.0) };
        let selectable = active && !(combined_position && sideways);
        let id = format!("place {edge_key} {} {}", to.0, to.1);
        let hovered = settings.is_hovered(&id);

        let mut cell = Rectangle::new()
            .width(cell_width)
            .height(cell_height)
            .radius(7.0)
            .semantics(|| {
                Semantics::new(Role::Button, spot(to))
                    .selected(here)
                    .disabled(!selectable)
            })
            .align_child(Center, Center);

        if hovered && selectable {
            cell = cell.fill(hover_wash(roles));
        }

        let cell = cell.child(
            Rectangle::new()
                .width(width)
                .height(height)
                .radius(5.0)
                .fill(if here { roles.primary } else { roles.edge() }),
        );

        if selectable {
            button(cell, id, press, None, settings, roles)
        } else {
            Box::new(cell.opacity(theme::DISABLED))
        }
    };

    let line = |side: Edge| {
        Row::new(
            [Along::Left, Along::Center, Along::Right]
                .into_iter()
                .map(|along| place((side.name(), along.name()), (34.0, 10.0)))
                .collect(),
        )
        .width(Parent)
        .justify(SpaceBetween)
    };

    let top = line(Edge::Top);
    let sides = Row::new(
        [Edge::Left, Edge::Right]
            .into_iter()
            .map(|side| place((side.name(), read.1), (10.0, 30.0)))
            .collect(),
    )
    .width(Parent)
    .justify(SpaceBetween);
    let bottom = line(Edge::Bottom);

    Rectangle::new()
        .width(176.0)
        .height(116.0)
        .radius(radius::ROW)
        .fill(roles.track())
        .border(1.0, roles.edge())
        .padding(6.0)
        .child(
            Column::new(children![top, sides, bottom])
                .width(Parent)
                .height(Parent)
                .justify(SpaceBetween),
        )
}

// a place's name: the corner or side of the screen it stands for
pub(crate) fn spot((edge, along): (&str, &str)) -> &'static str {
    match (edge, along) {
        ("top", "left") => "top-left",
        ("top", "center") => "top-center",
        ("top", "right") => "top-right",
        ("bottom", "left") => "bottom-left",
        ("bottom", "center") => "bottom-center",
        ("bottom", "right") => "bottom-right",
        ("left", _) => "left",
        _ => "right",
    }
}

// a list key's entries, each with Up, Down and Remove, and a field that adds one at the end
pub(crate) fn list(
    setting: &'static Setting,
    ids: Vec<String>,
    placeholder: &str,
    cycle: Option<fn(&str) -> Option<String>>,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Column {
    let key = setting.key;
    let to = |list: Vec<String>| move || change(path(key, None), Some(strings(&list)));

    let mut rows: Vec<Box<dyn Widget>> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let mut actions: Vec<Box<dyn Widget>> = Vec::new();

            if index > 0 {
                let mut list = ids.clone();
                list.swap(index - 1, index);
                actions.push(text_button(
                    format!("up {key} {id}"),
                    "up",
                    "\u{2191}",
                    settings,
                    roles,
                    to(list),
                ));
            }
            if index + 1 < ids.len() {
                let mut list = ids.clone();
                list.swap(index, index + 1);
                actions.push(text_button(
                    format!("down {key} {id}"),
                    "down",
                    "\u{2193}",
                    settings,
                    roles,
                    to(list),
                ));
            }

            if let Some(next) = cycle.and_then(|cycle| cycle(id)) {
                let mut list = ids.clone();
                list[index] = next;
                actions.push(text_button(
                    format!("cycle {key} {id}"),
                    "cycle",
                    "\u{21c4}",
                    settings,
                    roles,
                    to(list),
                ));
            }

            let mut without = ids.clone();
            without.remove(index);
            actions.push(icon_button(
                format!("remove {key} {id}"),
                "remove",
                Icon::Dismiss,
                settings,
                roles,
                to(without),
            ));

            Box::new(entry_line(id, actions, settings, roles)) as Box<dyn Widget>
        })
        .collect();

    rows.push(text_field(
        input(setting),
        inner(),
        placeholder,
        roles,
        move |text| {
            let text = text.trim();
            if text.is_empty() {
                return;
            }

            let mut list = ids.clone();
            list.push(text.to_owned());
            change(path(key, None), Some(strings(&list)));
        },
    ));

    Column::new(rows).width(inner()).gap(6.0)
}

// a list's entry with its actions at the end
pub(crate) fn entry_line(
    text: &str,
    actions: Vec<Box<dyn Widget>>,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Rectangle {
    let hover_id = format!("entry {text}");
    let hovered = settings.is_hovered(&hover_id);

    let entry = Rectangle::new()
        .width(inner())
        .height(ENTRY)
        .radius(radius::ROW)
        .fill(if hovered {
            theme::mix(roles.surface, roles.on_surface, 0.08)
        } else {
            roles.surface
        })
        .padding(sides(12.0))
        .semantics(|| Semantics::new(Role::Item, "entry").content(text))
        .align_child(Start, Center)
        .on_hover(move |inside| hover(hover_id.clone(), inside));

    entry.child(
        Row::new(children![
            Text::new(text)
                .size(theme::text::LABEL)
                .color(roles.on_surface)
                .weight(theme::text::MEDIUM)
                .elide(),
            Row::new(actions).gap(4.0).align(Center),
        ])
        .width(Parent)
        .gap(12.0)
        .justify(SpaceBetween)
        .align(Center),
    )
}

// on is filled with the knob at the end, off in a trough with an edge and the knob at the start
pub(crate) fn switch(
    id: String,
    on: bool,
    settings: &Settings,
    roles: &ThemeRoles,
    press: impl Fn() + 'static,
) -> Box<dyn Widget> {
    let hovered = settings.is_hovered(&id);
    pressed(
        switch_track(on, hovered, roles),
        id,
        press,
        TARGET / 2.0,
        roles,
    )
}

pub(crate) fn switch_track(on: bool, hovered: bool, roles: &ThemeRoles) -> Rectangle {
    const KNOB: f32 = 16.0;

    let track = Rectangle::new()
        .width(44.0)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .padding(4.0);

    let (track, knob, end) = if on {
        let fill = if hovered {
            hover_primary(roles)
        } else {
            roles.primary
        };
        (track.fill(fill), roles.on_primary, Align::from(End))
    } else {
        let fill = if hovered {
            theme::mix(roles.track(), roles.on_surface, 0.12)
        } else {
            roles.track()
        };
        let edge = if hovered {
            roles.on_surface_variant
        } else {
            roles.edge()
        };
        (
            track.fill(fill).border(1.0, edge),
            if hovered {
                roles.on_surface_variant
            } else {
                roles.edge()
            },
            Align::from(Start),
        )
    };

    track
        .semantics(|| Semantics::new(Role::Toggle, "switch").checked(on))
        .align_child(end, Center)
        .child(
            Rectangle::new()
                .width(KNOB)
                .height(KNOB)
                .radius(KNOB / 2.0)
                .fill(knob),
        )
}

// a Choice's options side by side in a trough, the chosen one filled; one stop, Left and Right choose
pub(crate) fn segmented(
    id: String,
    options: &'static Choices,
    chosen: &str,
    settings: &Settings,
    roles: &ThemeRoles,
    enabled: bool,
    choose: impl Fn(&'static str) + Clone + 'static,
) -> Box<dyn Widget> {
    const INSET: f32 = 3.0;
    const HEIGHT: f32 = 28.0;

    let segments: Vec<Box<dyn Widget>> = options
        .names
        .iter()
        .map(|&option| {
            let here = option == chosen;
            let seg_id = format!("{id} {option}");
            let hovered = settings.is_hovered(&seg_id);
            let hover_seg = seg_id.clone();
            let text = Text::new(humanize(options, option))
                .size(theme::text::LABEL)
                .color(if here {
                    roles.on_primary
                } else {
                    roles.on_surface
                })
                .weight(theme::text::SEMIBOLD);
            let width = fixed(Widget::width(&text)) + 24.0;
            let choose = choose.clone();

            let segment = Rectangle::new()
                .width(width)
                .height(HEIGHT)
                .radius(HEIGHT / 2.0)
                .semantics(|| {
                    Semantics::new(Role::Tab, option)
                        .selected(here)
                        .disabled(!enabled)
                })
                .align_child(Center, Center);
            let segment = if enabled {
                segment
                    .cursor(Cursor::Pointer)
                    .on_hover(move |inside| hover(hover_seg.clone(), inside))
                    .on_click(left(move || choose(option)))
            } else {
                segment
            };
            let segment = segment.child(text);
            let segment = if here {
                if hovered {
                    segment.fill(hover_primary(roles))
                } else {
                    segment.fill(roles.primary)
                }
            } else if hovered && enabled {
                segment.fill(hover_wash(roles))
            } else {
                segment
            };
            let segment = if enabled {
                segment
            } else {
                segment.opacity(theme::DISABLED)
            };

            Box::new(segment) as Box<dyn Widget>
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

    if !enabled {
        return Box::new(trough);
    }

    let at = options
        .names
        .iter()
        .position(|&option| option == chosen)
        .unwrap_or(0);
    let names = options.names;
    let token = ring::register(Stop::new(id).step(move |direction| {
        let next = (at as i32 + direction).clamp(0, names.len() as i32 - 1);

        choose(names[next as usize]);
    }));

    halo(trough, token, HEIGHT / 2.0 + INSET, roles)
}

// a Choice as cards that each show what choosing it does; one stop, the arrows choose
pub(crate) fn cards(
    id: String,
    options: &'static Choices,
    chosen: &str,
    settings: &Settings,
    roles: &ThemeRoles,
    preview: impl Fn(&'static str) -> Box<dyn Widget>,
    choose: impl Fn(&'static str) + Clone + 'static,
) -> Box<dyn Widget> {
    const THUMB: (f32, f32) = (96.0, 56.0);
    const CARD: f32 = 8.0;
    const GAP: f32 = 8.0;

    let cards: Vec<Box<dyn Widget>> = options
        .names
        .iter()
        .map(|&option| {
            let here = option == chosen;
            let card_id = format!("{id} {option}");
            let hovered = settings.is_hovered(&card_id);
            let hover_card = card_id.clone();
            let choose = choose.clone();
            let card = Rectangle::new()
                .width(THUMB.0 + 2.0 * CARD)
                .height(THUMB.1 + 2.0 * CARD + 22.0)
                .radius(radius::ROW + 2.0)
                .padding(CARD)
                .semantics(|| Semantics::new(Role::Tab, option).selected(here))
                .cursor(Cursor::Pointer)
                .on_hover(move |inside| hover(hover_card.clone(), inside))
                .on_click(left(move || choose(option)))
                .child(
                    Column::new(children![
                        Rectangle::new()
                            .width(THUMB.0)
                            .height(THUMB.1)
                            .radius(radius::ROW - 2.0)
                            .child(preview(option)),
                        Rectangle::new()
                            .width(THUMB.0)
                            .height(22.0)
                            .align_child(Center, Center)
                            .child(
                                Text::new(humanize(options, option))
                                    .size(theme::text::LABEL)
                                    .color(roles.on_surface)
                                    .weight(if here {
                                        theme::text::SEMIBOLD
                                    } else {
                                        theme::text::MEDIUM
                                    }),
                            ),
                    ])
                    .gap(0.0),
                );

            Box::new(if here {
                if hovered {
                    card.fill(theme::mix(roles.track(), roles.on_surface, 0.08))
                        .border(2.0, hover_primary(roles))
                } else {
                    card.fill(roles.track()).border(2.0, roles.primary)
                }
            } else if hovered {
                card.fill(hover_wash(roles))
                    .border(1.0, roles.on_surface_variant)
            } else {
                card.border(1.0, roles.edge())
            }) as Box<dyn Widget>
        })
        .collect();

    let count = cards.len() as f32;
    let row = Row::new(cards).gap(GAP);
    let (width, height) = (
        count * (THUMB.0 + 2.0 * CARD) + (count - 1.0) * GAP,
        THUMB.1 + 2.0 * CARD + 22.0,
    );
    let group = Rectangle::new().width(width).height(height).child(row);

    let at = options
        .names
        .iter()
        .position(|&option| option == chosen)
        .unwrap_or(0);
    let names = options.names;
    let token = ring::register(Stop::new(id).step(move |direction| {
        let next = (at as i32 + direction).clamp(0, names.len() as i32 - 1);

        choose(names[next as usize]);
    }));

    halo(group, token, radius::ROW + 2.0, roles)
}

// the tone's surface with a card and a line of text on it, as Settings and apps draw it
pub(crate) fn tone_preview(option: &str) -> Box<dyn Widget> {
    let tone = theme_tone(option);
    let roles = crate::appearance::roles(tone);

    Box::new(
        Rectangle::new()
            .width(Parent)
            .height(Parent)
            .radius(radius::ROW - 2.0)
            .fill(roles.surface)
            .padding(8.0)
            .child(
                Column::new(children![
                    Rectangle::new()
                        .width(40.0)
                        .height(6.0)
                        .radius(3.0)
                        .fill(roles.on_surface),
                    Rectangle::new()
                        .width(Parent)
                        .height(24.0)
                        .radius(6.0)
                        .fill(roles.card())
                        .padding(6.0)
                        .align_child(Start, Center)
                        .child(
                            Rectangle::new()
                                .width(28.0)
                                .height(8.0)
                                .radius(4.0)
                                .fill(roles.primary),
                        ),
                ])
                .gap(6.0),
            ),
    )
}

pub(crate) fn theme_tone(option: &str) -> crate::look::Tone {
    crate::look::Tone::named(option)
}

// the Island in a finish over the wallpaper's colors; Blur's is translucent over a band it blurs
pub(crate) fn finish_preview(option: &str) -> Box<dyn Widget> {
    let finish = crate::look::Finish::named(option);
    let roles = crate::appearance::island_roles(finish);
    let blurs = finish == crate::look::Finish::Blur;

    let pill = Rectangle::new()
        .width(64.0)
        .height(24.0)
        .radius(12.0)
        .fill(if blurs {
            theme::faded(roles.surface, pane::BLUR_FILL)
        } else {
            roles.surface
        })
        .border(1.0, roles.outline)
        .padding(sides(8.0))
        .align_child(Start, Center)
        .child(
            Row::new(children![
                Rectangle::new()
                    .width(8.0)
                    .height(8.0)
                    .radius(4.0)
                    .fill(roles.primary),
                Rectangle::new()
                    .width(24.0)
                    .height(5.0)
                    .radius(2.5)
                    .fill(roles.on_surface),
            ])
            .gap(6.0)
            .align(Center),
        );

    let mut layers: Vec<Box<dyn Widget>> = Vec::new();

    // what lies behind, which a blurred pane softens
    if blurs {
        layers.push(Box::new(
            Rectangle::new()
                .width(Parent)
                .height(Parent)
                .align_child(Center, Center)
                .child(
                    Rectangle::new()
                        .width(Parent)
                        .height(10.0)
                        .fill(theme::faded(roles.primary, 0.7)),
                ),
        ));
    }

    layers.push(Box::new(
        Rectangle::new()
            .width(Parent)
            .height(Parent)
            .align_child(Center, Center)
            .child(pill),
    ));

    Box::new(
        Rectangle::new()
            .width(Parent)
            .height(Parent)
            .radius(radius::ROW - 2.0)
            .fill(backdrop())
            .clip()
            .child(Stack::new(layers)),
    )
}

// a spring settling on its target: the curve of its position over time, the target a guide line
pub(crate) fn spring_preview(option: &str, roles: &ThemeRoles) -> Box<dyn Widget> {
    const WIDTH: f32 = 96.0;
    const HEIGHT: f32 = 56.0;
    const PAD_X: f32 = 10.0;
    const PAD_Y: f32 = 8.0;
    const SPAN: f32 = 1.25;
    const STEPS: usize = 40;

    // the natural frequency, so a spring settles in about 0.6 s
    const OMEGA: f32 = 14.0;
    const SECONDS: f32 = 0.6;

    let zeta = crate::look::Bounce::named(option).damping();
    let position = |t: f32| {
        if zeta >= 1.0 {
            1.0 - (-OMEGA * t).exp() * (1.0 + OMEGA * t)
        } else {
            let damped = OMEGA * (1.0 - zeta * zeta).sqrt();

            1.0 - (-zeta * OMEGA * t).exp()
                * ((damped * t).cos() + zeta / (1.0 - zeta * zeta).sqrt() * (damped * t).sin())
        }
    };
    let at = |t: f32, value: f32| {
        (
            PAD_X + t / SECONDS * (WIDTH - 2.0 * PAD_X),
            HEIGHT - PAD_Y - value / SPAN * (HEIGHT - 2.0 * PAD_Y),
        )
    };

    let (x, y) = at(0.0, 0.0);
    let mut curve = Path::new().move_to(x, y);
    for step in 1..=STEPS {
        let t = SECONDS * step as f32 / STEPS as f32;
        let (x, y) = at(t, position(t));

        curve = curve.line_to(x, y);
    }
    let (start, target) = (at(0.0, 1.0), at(SECONDS, 1.0));
    let guide = Path::new()
        .move_to(start.0, start.1)
        .line_to(target.0, target.1)
        .stroke(1.0, roles.edge());

    Box::new(Canvas::new().width(WIDTH).height(HEIGHT).shapes(vec![
        Box::new(guide),
        Box::new(curve.stroke(2.0, roles.primary)),
    ]))
}

// a number: `-`, the number to type over and its unit, `+`, in one outlined capsule; Up and Down step it, and Tab reaches `-` and `+`
pub(crate) fn stepper(
    setting: &'static Setting,
    value: u64,
    steps: Steps,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    const SIDE: f32 = 30.0;
    const NUMBER: f32 = 52.0;

    let key = setting.key;
    let Steps {
        least,
        most,
        step,
        unit,
        expected,
    } = steps;
    let to = move |value: u64| {
        move || {
            let value = value.clamp(least, most);
            change(path(key, None), Some(Value::Integer(value as i64)));
        }
    };
    // a typed number is the user's own, so one outside the range is refused rather than quietly
    // becoming the nearest end: the config registry refuses it with the same words
    let typed = move |text: String| match text.trim().parse::<u64>() {
        Ok(value) if (least..=most).contains(&value) => to(value)(),
        Ok(_) => {
            Settings::write().refused = Some(format!(
                "Not saved: {key}: expected {expected}, from {least} to {most}"
            ));
        }
        Err(_) => {
            Settings::write().refused = Some(format!("Not saved: {key}: expected {expected}"));
        }
    };
    let id = input(setting);

    let unit = Text::new(unit)
        .size(theme::text::LABEL)
        .color(roles.on_surface_variant)
        .weight(theme::text::MEDIUM);
    let middle = NUMBER + 4.0 + fixed(Widget::width(&unit)) + 2.0 * 8.0;
    let inside = FIELD - 2.0;

    // a side button's outer corners follow the capsule's, so its ring does not poke out of them
    let corner = radius::ROW - 3.0;
    let side = |node: &'static str, sign: &str, value: u64| {
        let side_id = format!("stepper {id} {node}");
        let outer = node == "less";
        let btn = Rectangle::new()
            .width(SIDE)
            .height(inside)
            .radius_top_left(if outer { corner } else { 0.0 })
            .radius_bottom_left(if outer { corner } else { 0.0 })
            .radius_top_right(if outer { 0.0 } else { corner })
            .radius_bottom_right(if outer { 0.0 } else { corner })
            .semantics(|| Semantics::new(Role::Button, node))
            .align_child(Center, Center)
            .child(
                Text::new(sign)
                    .size(theme::text::BODY)
                    .color(roles.on_surface)
                    .weight(theme::text::SEMIBOLD),
            );

        // Tab reaches it and Enter or Space steps it, as a click does
        button(btn, side_id, to(value), None, settings, roles)
    };
    let divider = || {
        Rectangle::new()
            .width(1.0)
            .height(inside)
            .fill(faint(roles.outline))
    };

    let less = side("less", "\u{2212}", value.saturating_sub(step));
    let token = ring::register(
        Stop::new(format!("field {id}"))
            .input(id)
            .step(move |direction| {
                // Up raises the number, as the arrows of a number field do
                if direction < 0 {
                    to(value + step)();
                } else {
                    to(value.saturating_sub(step))();
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
                    less,
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
                                    .on_submit(typed),
                                unit,
                            ])
                            .gap(4.0)
                            .align(Center),
                        ),
                    divider(),
                    side("more", "+", value + step),
                ])
                .align(Center),
            ),
        token,
        Some(roles.edge()),
        roles,
    )
}

// a text field beside its label, saved by Enter, an empty one handing the key back
pub(crate) fn field_with_hint(
    id: &'static str,
    placeholder: &str,
    roles: &ThemeRoles,
    submit: impl Fn(String) + 'static,
) -> Column {
    const WIDE: f32 = 260.0;

    Column::new(children![
        text_field(id, WIDE, placeholder, roles, submit),
        Text::new("Enter saves; empty resets")
            .size(theme::text::LABEL_SMALL)
            .color(roles.on_surface_variant)
            .weight(theme::text::MEDIUM),
    ])
    .width(WIDE)
    .gap(4.0)
}

// a square button with a sign
pub(crate) fn text_button(
    id: String,
    node: &'static str,
    label: &str,
    settings: &Settings,
    roles: &ThemeRoles,
    press: impl Fn() + 'static,
) -> Box<dyn Widget> {
    button(
        Rectangle::new()
            .width(28.0)
            .height(28.0)
            .radius(radius::ROW - 4.0)
            .semantics(|| Semantics::new(Role::Button, node))
            .align_child(Center, Center)
            .child(
                Text::new(label)
                    .size(theme::text::BODY)
                    .color(roles.on_surface)
                    .weight(theme::text::SEMIBOLD),
            ),
        id,
        press,
        Some(roles.edge()),
        settings,
        roles,
    )
}

pub(crate) fn icon_button(
    id: String,
    node: &'static str,
    icon: Icon,
    settings: &Settings,
    roles: &ThemeRoles,
    press: impl Fn() + 'static,
) -> Box<dyn Widget> {
    button(
        Rectangle::new()
            .width(28.0)
            .height(28.0)
            .radius(radius::ROW - 4.0)
            .semantics(|| Semantics::new(Role::Button, node))
            .align_child(Center, Center)
            .child(icon.on(14.0, roles.on_surface_variant)),
        id,
        press,
        None,
        settings,
        roles,
    )
}

// a word in the primary color that acts when pressed
pub(crate) fn link(
    id: String,
    node: &'static str,
    label: &str,
    settings: &Settings,
    roles: &ThemeRoles,
    press: impl Fn() + 'static,
) -> Box<dyn Widget> {
    let text = Text::new(label)
        .size(theme::text::LABEL_SMALL)
        .color(roles.primary)
        .weight(theme::text::SEMIBOLD);
    let width = fixed(Widget::width(&text)) + 8.0;

    button(
        Rectangle::new()
            .width(width)
            .height(20.0)
            .radius(5.0)
            .semantics(|| Semantics::new(Role::Button, node))
            .align_child(Center, Center)
            .child(text),
        id,
        press,
        None,
        settings,
        roles,
    )
}

// a text input, saved by Enter
pub(crate) fn text_field(
    id: &'static str,
    width: f32,
    placeholder: &str,
    roles: &ThemeRoles,
    submit: impl Fn(String) + 'static,
) -> Box<dyn Widget> {
    let token = ring::register(Stop::new(format!("field {id}")).input(id));

    outlined(
        Rectangle::new()
            .width(width)
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
                    .placeholder(placeholder)
                    .on_submit(submit),
            ),
        token,
        Some(roles.edge()),
        roles,
    )
}

/*
 * a control with an edge of its own, which the ring takes the place of: 2 px in the ring's color
 * where `resting` is a 1 px edge, or none. Moved into view when the ring arrives
 */
pub(crate) fn outlined(
    control: Rectangle,
    ring: Option<u64>,
    resting: Option<Color>,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    match (ring, resting) {
        (Some(token), _) => Box::new(Reveal::new(control.border(RING, roles.ring()), token)),
        (None, Some(edge)) => Box::new(control.border(1.0, edge)),
        (None, None) => Box::new(control),
    }
}

/*
 * a control whose own color is its edge, with a gap and a ring around it while the ring is on it.
 * The gap is always there, so the ring arriving moves nothing
 */
pub(crate) fn halo(
    control: impl Widget + 'static,
    ring: Option<u64>,
    corner: f32,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    let (width, height) = (fixed(control.width()), fixed(control.height()));
    let around = Rectangle::new()
        .width(width + 2.0 * HALO)
        .height(height + 2.0 * HALO)
        .radius(corner + HALO)
        .padding(HALO)
        .child(control);

    match ring {
        Some(token) => Box::new(Reveal::new(around.border(RING, roles.ring()), token)),
        None => Box::new(around),
    }
}

// a pressable of its own edge or none, that Enter and Space press as well
pub(crate) fn button(
    control: Rectangle,
    id: impl Into<String>,
    press: impl Fn() + 'static,
    resting: Option<Color>,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    button_styled(control, id, press, resting, true, settings, roles)
}

pub(crate) fn button_styled(
    control: Rectangle,
    id: impl Into<String>,
    press: impl Fn() + 'static,
    resting: Option<Color>,
    wash: bool,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    let id = id.into();
    let hover_id = id.clone();
    let hovered = settings.is_hovered(&id);
    let press: Rc<dyn Fn()> = Rc::new(press);
    let click = press.clone();
    let token = ring::register(Stop::new(id).press(move || press()));

    let mut control = control
        .cursor(Cursor::Pointer)
        .on_hover(move |inside| hover(hover_id.clone(), inside))
        .on_click(left(move || click()));

    if hovered && wash {
        control = control.fill(hover_wash(roles));
    }

    outlined(control, token, resting, roles)
}

// a word pressed as a whole, filled when it is the one to take; dimmed and unreachable when not
pub(crate) fn pill(
    node: &'static str,
    word: &str,
    primary: bool,
    enabled: bool,
    settings: &Settings,
    roles: &ThemeRoles,
    press: fn(),
) -> Box<dyn Widget> {
    let id = format!("pill {node}");
    let hovered = enabled && settings.is_hovered(&id);
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
        .semantics(move || Semantics::new(Role::Button, node).disabled(!enabled))
        .align_child(Center, Center)
        .child(text);

    let control = if !enabled {
        let control = if primary {
            control.fill(roles.primary)
        } else {
            control
        };

        control.opacity(theme::DISABLED)
    } else if primary {
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

    if !enabled {
        return Box::new(control);
    }

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

// a pressable that is its own edge, in a halo
pub(crate) fn pressed(
    control: Rectangle,
    id: String,
    press: impl Fn() + 'static,
    corner: f32,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    let hover_id = id.clone();
    let press: Rc<dyn Fn()> = Rc::new(press);
    let click = press.clone();
    let token = ring::register(Stop::new(id).press(move || press()));

    halo(
        control
            .cursor(Cursor::Pointer)
            .on_hover(move |inside| hover(hover_id.clone(), inside))
            .on_click(left(move || click())),
        token,
        corner,
        roles,
    )
}

pub(crate) fn restart(roles: &ThemeRoles) -> Text {
    Text::new("Restart to apply")
        .size(theme::text::LABEL_SMALL)
        .color(roles.primary)
        .weight(theme::text::SEMIBOLD)
}

// the line a setting shows under its name, saying when it takes a restart
pub(crate) fn help(setting: &Setting) -> String {
    let mut help = capitalized(setting.tail.hint.unwrap_or(setting.help));
    if setting.tail.restart {
        help.push_str(". Takes a restart");
    }

    help
}

// an option as the window names it: the schema's label, else its name with dashes as spaces
pub(crate) fn humanize(options: &Choices, option: &str) -> String {
    options
        .label(option)
        .map_or_else(|| humanized(option), str::to_owned)
}

// a name with dashes as spaces, as a sentence
pub(crate) fn humanized(name: &str) -> String {
    capitalized(&name.replace('-', " "))
}

pub(crate) fn capitalized(text: &str) -> String {
    let mut letters = text.chars();

    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

pub(crate) fn setting(key: &str) -> Option<&'static Setting> {
    owner(key)?
        .settings
        .iter()
        .find(|setting| setting.key == key)
}

// the Module owning a key
pub(crate) fn owner(key: &str) -> Option<&'static Module> {
    modules::ALL
        .iter()
        .find(|module| module.settings.iter().any(|setting| setting.key == key))
}

// the gradient of Settings' mark without Kanade's icon
pub(crate) fn backdrop() -> Gradient {
    let [from, middle, to] = theme::SCENE;

    Gradient::linear(120.0, [(0.0, from), (0.5, middle), (1.0, to)])
}

// a color faint enough for a box's fill or a quiet line
pub(crate) fn faint(color: Color) -> Color {
    theme::faded(color, 0.2)
}

pub(crate) fn hairline(width: impl Into<Size>, roles: &ThemeRoles) -> Rectangle {
    Rectangle::new()
        .width(width)
        .height(1.0)
        .fill(faint(roles.outline))
}

pub(crate) fn space(height: f32) -> Rectangle {
    Rectangle::new().width(1.0).height(height)
}

// text wrapped to `width`, as tall as its lines
pub(crate) fn wrapped(text: Text, width: f32) -> Rectangle {
    let text = text.wrap();
    let height = text.height_in(width);

    Rectangle::new().width(width).height(height).child(text)
}

pub(crate) fn left(press: impl Fn() + 'static) -> impl Fn(Button) + 'static {
    move |button| {
        if button == Button::Left {
            press();
        }
    }
}

pub(crate) fn strings(list: &[String]) -> Value {
    Value::Array(list.iter().cloned().map(Value::String).collect())
}

pub(crate) fn sides(inset: f32) -> Padding {
    Padding {
        top: 0.0,
        right: inset,
        bottom: 0.0,
        left: inset,
    }
}

pub(crate) fn fixed(size: Size) -> f32 {
    match size {
        Size::Fixed(pixels) => pixels,
        Size::Parent => 0.0,
    }
}
