use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Column, Parent, Rectangle, Row, SpaceBetween, Start, Text, TextInput, Widget, children,
};

use toml::Value;

use crate::config::Kind;
use crate::icon::Icon;
use crate::modules::{self, CORE, Module};
use crate::settings::layout::{MODULE_GROUPS, MODULES};
use crate::settings::ring::{self, Stop};
use crate::settings::{MODULE_FILTER, Settings, change, hover, module_filter, path};
use crate::theme::{self, ThemeRoles, radius};

use super::{
    FIELD, HEADING, card_of, content, fixed, hairline, hover_wash, humanized, inner,
    module_matches, outlined, restart, setting, sides, switch, wrapped,
};

/**
 * the Modules page: each Module under its group's card, with its switch; changing one
 * takes a restart
 */
pub(crate) fn modules_view(settings: &Settings, query: Option<&str>, roles: &ThemeRoles) -> Column {
    let off = match setting(MODULES).map(|setting| &setting.kind) {
        Some(Kind::Modules(field)) => (field.get)(&settings.read),
        _ => Vec::new(),
    };

    // the search narrows them from outside, else the filter above them does
    let filter = settings.module_query.trim().to_lowercase();
    let query = query.or((!filter.is_empty()).then_some(filter.as_str()));

    let mut items: Vec<Box<dyn Widget>> = Vec::new();
    if !settings.searching() {
        items.push(module_filter_field(roles));
    }

    let core = modules::ALL.iter().find(|module| module.name == CORE);
    if let Some(core) = core.filter(|core| query.is_none_or(|query| module_matches(core, query))) {
        items.push(Box::new(card_of(
            children![module_line(
                "Island core",
                core.about,
                true,
                Box::new(
                    Text::new("Always on")
                        .size(theme::text::LABEL)
                        .color(roles.on_surface_variant)
                        .weight(theme::text::SEMIBOLD),
                ),
                Vec::new(),
                settings,
                roles,
            )],
            roles,
        )));
    }

    let mut shown = core.is_some_and(|core| query.is_none_or(|query| module_matches(core, query)));
    for (group, names) in MODULE_GROUPS {
        let mut rows: Vec<Box<dyn Widget>> = Vec::new();
        let members: Vec<&Module> = names
            .iter()
            .filter_map(|name| modules::ALL.iter().find(|module| module.name == *name))
            .collect();
        let running = members
            .iter()
            .filter(|module| !off.contains(&module.name))
            .count();

        for module in members
            .iter()
            .filter(|module| query.is_none_or(|query| module_matches(module, query)))
        {
            let name = module.name;
            let on = !off.contains(&name);

            let mut marks: Vec<Box<dyn Widget>> = Vec::new();
            if settings.pending(&format!("{MODULES}.{name}")) {
                marks.push(Box::new(restart(roles)));
            }

            let mut about = String::from(module.about);
            let requires: Vec<String> = module
                .requires
                .iter()
                .copied()
                .filter(|&required| required != CORE)
                .map(|required| humanized(required).to_lowercase())
                .collect();
            if !requires.is_empty() {
                about.push_str(&format!(" \u{00b7} needs {}", requires.join(", ")));
            }

            if !rows.is_empty() {
                rows.push(Box::new(hairline(inner(), roles)));
            }
            rows.push(Box::new(module_line(
                &humanized(name),
                &about,
                on,
                switch(
                    format!("switch {MODULES}.{name}"),
                    on,
                    settings,
                    roles,
                    move || {
                        change(path(MODULES, Some(name)), Some(Value::Boolean(!on)));
                    },
                ),
                marks,
                settings,
                roles,
            )));
        }

        if rows.is_empty() {
            continue;
        }

        shown = true;
        items.push(Box::new(
            Row::new(children![
                Text::new(*group)
                    .size(theme::text::BODY)
                    .color(roles.on_surface)
                    .weight(theme::text::SEMIBOLD),
                Text::new(format!("{running} of {} on", members.len()))
                    .size(theme::text::LABEL_SMALL)
                    .color(roles.on_surface_variant)
                    .weight(theme::text::MEDIUM),
            ])
            .gap(10.0)
            .align(Center),
        ));
        items.push(Box::new(card_of(rows, roles)));
    }

    if !shown {
        items.push(Box::new(
            Text::new("No module matches")
                .size(theme::text::BODY)
                .color(roles.on_surface_variant)
                .weight(theme::text::MEDIUM),
        ));
    }

    Column::new(items).width(content()).gap(HEADING)
}

pub(crate) fn module_filter_field(roles: &ThemeRoles) -> Box<dyn Widget> {
    let token = ring::register(Stop::new("filter").input(MODULE_FILTER));

    outlined(
        Rectangle::new()
            .width(280.0)
            .height(FIELD)
            .radius(radius::ROW - 2.0)
            .fill(roles.surface)
            .padding(sides(10.0))
            .align_child(Start, Center)
            .child(
                Row::new(children![
                    Icon::Search.on(14.0, roles.on_surface_variant),
                    TextInput::new(MODULE_FILTER)
                        .width(Parent)
                        .size(theme::text::LABEL)
                        .color(roles.on_surface)
                        .placeholder("Filter modules")
                        .on_change(module_filter),
                ])
                .width(Parent)
                .gap(8.0)
                .align(Center),
            ),
        token,
        Some(roles.edge()),
        roles,
    )
}

// a Module's name over what it does, and its switch; one that is off has a quieter name
pub(crate) fn module_line(
    name: &str,
    about: &str,
    on: bool,
    action: Box<dyn Widget>,
    marks: Vec<Box<dyn Widget>>,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Rectangle {
    let width = inner() - fixed(action.width()) - 24.0;

    let text = Column::new(children![
        Row::new(
            std::iter::once(Box::new(
                Text::new(name)
                    .size(theme::text::BODY)
                    .color(if on {
                        roles.on_surface
                    } else {
                        roles.on_surface_variant
                    })
                    .weight(theme::text::SEMIBOLD),
            ) as Box<dyn Widget>)
            .chain(marks)
            .collect(),
        )
        .height(20.0)
        .gap(10.0)
        .align(Center),
        wrapped(
            Text::new(about)
                .size(theme::text::LABEL)
                .color(roles.on_surface_variant)
                .weight(theme::text::MEDIUM),
            width,
        ),
    ])
    .width(width)
    .gap(2.0);

    let height = fixed(Widget::height(&text)).max(fixed(action.height())) + 2.0 * 10.0;
    let hover_id = format!("module {name}");
    let hovered = settings.is_hovered(&hover_id);

    let mut row = Rectangle::new()
        .width(inner())
        .height(height)
        .radius(radius::ROW)
        .semantics(|| Semantics::new(Role::Region, "module").label(name))
        .align_child(Start, Center)
        .on_hover(move |inside| hover(hover_id.clone(), inside));

    if hovered {
        row = row.fill(hover_wash(roles));
    }

    row.child(
        Row::new(vec![Box::new(text), action])
            .width(inner())
            .justify(SpaceBetween)
            .align(Center),
    )
}
