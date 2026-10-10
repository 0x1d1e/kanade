//! The Settings window's view: the pages down the side under a search field, the page's sections
//! beside them, a footer with what the files say and Undo. Each row names a setting and says what
//! it does, with its control beside it or, for a list, under it; one the settings file sets has a
//! Reset, which hands the key back to the layers below. Inspect shows each row's key.

use std::cell::RefCell;
use std::collections::HashMap;

use kanade_runtime::service::Service;
use kanade_runtime::{
    Align, Button, Center, Color, Column, Cursor, End, Gradient, Padding, Parent, Rectangle, Row,
    ScrollArea, Size, SpaceBetween, Start, Text, TextInput, Widget, Window, children,
};
use toml::Value;

use crate::config::{self, Kind, Location, Setting};
use crate::glass;
use crate::icon::Icon;
use crate::look::{Along, Edge, Material};
use crate::modules::{self, CORE, Module};
use crate::theme::space::TARGET;
use crate::theme::{self, ThemeRoles, radius};

use super::layout::{self, MODULE_GROUPS, MODULES, PAGES, Page, Row as Line};
use super::{SEARCH, Settings, change, change_all, input, inspect, path, search, turn_to, undo};

// the size it opens at, and the smallest it can be made
const WIDTH: f32 = 920.0;
const HEIGHT: f32 = 640.0;
const MIN_WIDTH: f32 = 680.0;
const MIN_HEIGHT: f32 = 420.0;

const SIDEBAR: f32 = 232.0;
const SIDE_INSET: f32 = 16.0;
const NAV: f32 = SIDEBAR - 2.0 * SIDE_INSET;

// the page's column, a gutter short of the window's edge so a scrolled row clears it, no wider than reads well
const PAD: f32 = 28.0;
const GUTTER: f32 = 8.0;
const MOST_CONTENT: f32 = 780.0;
const CARD_INSET: f32 = 16.0;

const BAR: f32 = 52.0;
const ROW_INSET: f32 = 14.0;
const FIELD: f32 = 34.0;
const ENTRY: f32 = 36.0;
const PAGE_TITLE: f32 = 24.0;

// the area right of the sidebar, as wide as the window now is
fn main_width() -> f32 {
    let (width, _) = kanade_runtime::window_size();
    let width = if width > 0.0 { width } else { WIDTH };

    width.max(MIN_WIDTH) - SIDEBAR
}

// the page's column
fn content() -> f32 {
    (main_width() - 2.0 * PAD - GUTTER).min(MOST_CONTENT)
}

// a card's rows
fn inner() -> f32 {
    content() - 2.0 * CARD_INSET
}

// a control wider than this goes under its label
fn most_beside() -> f32 {
    inner() * 0.55
}

// a whole number stepped by `-` and `+` or typed, kept from `least` to `most`
struct Steps {
    least: u64,
    most: u64,
    step: u64,

    // after the number, and what a typed one must be
    unit: &'static str,
    expected: &'static str,
}

const MILLIS: Steps = Steps {
    least: config::SHORTEST,
    most: config::LONGEST,
    step: 10,
    unit: "ms",
    expected: "whole milliseconds",
};

// a size steps by as much as a list row or two
const PIXEL_STEP: u64 = 20;

pub fn window() -> Window {
    let settings = Settings::read();
    let roles = theme::roles();

    Window::new()
        .title("Kanade Settings")
        .size(WIDTH, HEIGHT)
        .min_size(MIN_WIDTH, MIN_HEIGHT)
        .child(
            Rectangle::new()
                .width(Parent)
                .height(Parent)
                .fill(roles.surface)
                .child(
                    Row::new(children![
                        sidebar(&settings, &roles),
                        main(&settings, &roles)
                    ])
                    .width(Parent)
                    .height(Parent),
                ),
        )
}

// the brand, the search field, then the pages under their groups' headings, scrolled in a short window
fn sidebar(settings: &Settings, roles: &ThemeRoles) -> Rectangle {
    let mut items: Vec<Box<dyn Widget>> = Vec::new();

    let mut group = "";
    for page in PAGES {
        if page.group != group {
            group = page.group;
            items.push(Box::new(
                Rectangle::new()
                    .width(NAV)
                    .height(30.0)
                    .padding(Padding {
                        top: 10.0,
                        right: 0.0,
                        bottom: 0.0,
                        left: 10.0,
                    })
                    .align_child(Start, Center)
                    .child(
                        Text::new(group)
                            .size(theme::text::LABEL_SMALL)
                            .color(roles.on_surface_variant)
                            .weight(theme::text::SEMIBOLD),
                    ),
            ));
        }

        items.push(Box::new(nav(page, settings, roles)));
    }

    Rectangle::new()
        .width(SIDEBAR)
        .height(Parent)
        .fill(roles.surface_container)
        .padding(SIDE_INSET)
        .align_child(Start, Start)
        .child(
            Column::new(children![
                brand(roles),
                search_field(roles),
                ScrollArea::new(scroll_id("pages"), Column::new(items).width(NAV).gap(4.0))
                    .width(Parent)
                    .height(Parent),
            ])
            .width(NAV)
            .height(Parent)
            .gap(4.0),
        )
}

fn brand(roles: &ThemeRoles) -> Row {
    const MARK: f32 = 30.0;

    Row::new(children![
        Rectangle::new()
            .width(MARK)
            .height(MARK)
            .radius(9.0)
            .fill(backdrop())
            .align_child(Center, Center)
            .child(Icon::Island.on(16.0, theme::ON_SCENE)),
        Column::new(children![
            Text::new("kanade")
                .size(theme::text::BODY)
                .color(roles.on_surface)
                .weight(theme::text::SEMIBOLD),
            Text::new("Settings")
                .size(theme::text::LABEL_SMALL)
                .color(roles.on_surface_variant)
                .weight(theme::text::MEDIUM),
        ])
        .gap(1.0),
    ])
    .width(NAV)
    .height(44.0)
    .gap(10.0)
    .align(Center)
}

fn search_field(roles: &ThemeRoles) -> Rectangle {
    Rectangle::new()
        .width(NAV)
        .height(FIELD)
        .radius(radius::ROW)
        .fill(roles.surface_container_high)
        .padding(sides(10.0))
        .align_child(Start, Center)
        .child(
            Row::new(children![
                Icon::Search.on(14.0, roles.on_surface_variant),
                TextInput::new(SEARCH)
                    .width(Parent)
                    .size(theme::text::LABEL)
                    .color(roles.on_surface)
                    .placeholder("Find a setting")
                    .on_change(search),
            ])
            .width(Parent)
            .gap(8.0)
            .align(Center),
        )
}

// a page down the side, filled while it shows
fn nav(page: &'static Page, settings: &Settings, roles: &ThemeRoles) -> Rectangle {
    let here = page.name == settings.page && settings.query.trim().is_empty();
    let name = page.name;

    let (ink, weight) = if here {
        (roles.on_surface, theme::text::SEMIBOLD)
    } else {
        (roles.on_surface_variant, theme::text::MEDIUM)
    };

    let row = Rectangle::new()
        .width(NAV)
        .height(34.0)
        .radius(radius::ROW)
        .padding(sides(10.0))
        .align_child(Start, Center)
        .cursor(Cursor::Pointer)
        .on_click(left(move || turn_to(name)))
        .child(
            Row::new(children![
                page.icon.on(16.0, ink),
                Text::new(page.title)
                    .size(theme::text::LABEL)
                    .color(roles.on_surface)
                    .weight(weight),
            ])
            .gap(10.0)
            .align(Center),
        );

    if here {
        row.fill(roles.surface_container_high)
    } else {
        row
    }
}

// the header, the page or the search's results, the footer
fn main(settings: &Settings, roles: &ThemeRoles) -> Column {
    let page = PAGES
        .iter()
        .find(|page| page.name == settings.page)
        .unwrap_or(&PAGES[0]);

    let searching = !settings.query.trim().is_empty();
    let (title, body) = if searching {
        ("Search", results(settings, roles))
    } else {
        (page.title, page_body(page, settings, roles))
    };

    Column::new(children![
        header(title, settings, roles),
        hairline(Parent, roles),
        Rectangle::new()
            .width(Parent)
            .height(Parent)
            .padding(Padding {
                top: 0.0,
                right: 0.0,
                bottom: 0.0,

                // a window wider than the column reads well at centers it
                left: PAD + ((main_width() - 2.0 * PAD - GUTTER - content()) / 2.0).max(0.0),
            })
            .child(
                ScrollArea::new(
                    scroll_id(if searching { "search" } else { page.name }),
                    body
                )
                .width(Parent)
                .height(Parent),
            ),
        hairline(Parent, roles),
        footer(settings, roles),
    ])
    .width(Parent)
    .height(Parent)
}

// where in the window this is, and Inspect
fn header(title: &str, settings: &Settings, roles: &ThemeRoles) -> Rectangle {
    let inspect_button = Rectangle::new()
        .width(96.0)
        .height(30.0)
        .radius(radius::ROW)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(left(inspect))
        .child(
            Row::new(children![
                Icon::Sliders.on(14.0, roles.on_surface),
                Text::new("Inspect")
                    .size(theme::text::LABEL)
                    .color(roles.on_surface)
                    .weight(theme::text::SEMIBOLD),
            ])
            .gap(6.0)
            .align(Center),
        );

    let inspect_button = if settings.inspect {
        inspect_button.fill(roles.surface_container_high)
    } else {
        inspect_button.border(1.0, roles.outline)
    };

    Rectangle::new()
        .width(Parent)
        .height(BAR)
        .padding(sides(PAD))
        .align_child(Start, Center)
        .child(
            Row::new(children![
                Row::new(children![
                    Text::new("Settings  /")
                        .size(theme::text::LABEL)
                        .color(roles.on_surface_variant)
                        .weight(theme::text::MEDIUM),
                    Text::new(title)
                        .size(theme::text::LABEL)
                        .color(roles.on_surface)
                        .weight(theme::text::SEMIBOLD),
                ])
                .gap(6.0)
                .align(Center),
                inspect_button,
            ])
            .width(Parent)
            .justify(SpaceBetween)
            .align(Center),
        )
}

// what the files say, and Undo
fn footer(settings: &Settings, roles: &ThemeRoles) -> Rectangle {
    let waiting = settings.pending.len();
    let (status, ink) = if let Some(refused) = &settings.refused {
        (refused.clone(), theme::SEMANTIC.warning)
    } else if settings.unreadable.is_some()
        || settings.error.is_some()
        || !settings.skipped.is_empty()
    {
        (
            String::from("The config has problems, shown above"),
            theme::SEMANTIC.warning,
        )
    } else if waiting > 0 {
        let changes = if waiting == 1 {
            "change waits"
        } else {
            "changes wait"
        };

        (format!("{waiting} {changes} for a restart"), roles.primary)
    } else {
        (
            String::from("Changes apply as you make them"),
            roles.on_surface_variant,
        )
    };

    let undo_button = Rectangle::new()
        .width(84.0)
        .height(30.0)
        .radius(radius::ROW)
        .border(1.0, roles.outline)
        .align_child(Center, Center)
        .child(
            Row::new(children![
                Icon::Undo.on(14.0, roles.on_surface),
                Text::new("Undo")
                    .size(theme::text::LABEL)
                    .color(roles.on_surface)
                    .weight(theme::text::SEMIBOLD),
            ])
            .gap(6.0)
            .align(Center),
        );

    let undo_button = if settings.history.is_empty() {
        undo_button.opacity(theme::DISABLED)
    } else {
        undo_button.cursor(Cursor::Pointer).on_click(left(undo))
    };

    Rectangle::new()
        .width(Parent)
        .height(BAR)
        .padding(sides(PAD))
        .align_child(Start, Center)
        .child(
            Row::new(children![
                Text::new(status)
                    .size(theme::text::LABEL)
                    .color(ink)
                    .weight(theme::text::MEDIUM)
                    .elide(),
                undo_button,
            ])
            .width(Parent)
            .gap(16.0)
            .justify(SpaceBetween)
            .align(Center),
        )
}

// a page's title and summary, what is wrong with the files, then its sections
fn page_body(page: &'static Page, settings: &Settings, roles: &ThemeRoles) -> Column {
    let mut items = children![
        space(20.0),
        Text::new(page.title)
            .size(PAGE_TITLE)
            .color(roles.on_surface)
            .weight(theme::text::SEMIBOLD),
        wrapped(
            Text::new(page.summary)
                .size(theme::text::BODY)
                .color(roles.on_surface_variant)
                .weight(theme::text::MEDIUM),
            content(),
        ),
    ];
    items.extend(notices(settings));

    for section in page.sections {
        items.push(Box::new(section_view(
            section.title,
            section.rows.iter().collect(),
            settings,
            None,
            roles,
        )));
    }

    items.push(Box::new(space(12.0)));

    Column::new(items).width(content()).gap(14.0)
}

// every row matching the search, under its page's and section's titles
fn results(settings: &Settings, roles: &ThemeRoles) -> Column {
    let query = settings.query.trim().to_lowercase();

    let mut items = children![
        space(20.0),
        Text::new(format!(
            "Results for \u{201c}{}\u{201d}",
            settings.query.trim()
        ))
        .size(PAGE_TITLE)
        .color(roles.on_surface)
        .weight(theme::text::SEMIBOLD),
    ];
    items.extend(notices(settings));

    let mut found = false;
    for page in PAGES {
        for section in page.sections {
            let rows: Vec<&Line> = section
                .rows
                .iter()
                .filter(|row| matches(row, &query))
                .collect();
            if rows.is_empty() {
                continue;
            }

            found = true;
            let title = if section.title.is_empty() {
                page.title.to_owned()
            } else {
                format!("{}  \u{203a}  {}", page.title, section.title)
            };
            items.push(Box::new(section_view(
                &title,
                rows,
                settings,
                Some(&query),
                roles,
            )));
        }
    }

    if !found {
        items.push(Box::new(
            Text::new("Nothing matches. Try a word from a setting's name or what it does")
                .size(theme::text::BODY)
                .color(roles.on_surface_variant)
                .weight(theme::text::MEDIUM),
        ));
    }

    items.push(Box::new(space(12.0)));

    Column::new(items).width(content()).gap(14.0)
}

// whether a row's settings mention the query; the Modules row, whether a Module does, as it shows those
fn matches(row: &Line, query: &str) -> bool {
    let mentions = |text: &str| text.to_lowercase().contains(query);

    row.keys().into_iter().any(|key| {
        if key == MODULES {
            return modules::ALL
                .iter()
                .any(|module| module_matches(module, query));
        }

        setting(key).is_some_and(|setting| {
            mentions(setting.label) || mentions(setting.help) || mentions(setting.key)
        })
    })
}

fn module_matches(module: &Module, query: &str) -> bool {
    module.name.to_lowercase().contains(query) || module.about.to_lowercase().contains(query)
}

// what in the files keeps a change from applying, each in a box
fn notices(settings: &Settings) -> Vec<Box<dyn Widget>> {
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
        .map(|problem| Box::new(notice(&problem, theme::SEMANTIC.warning)) as Box<dyn Widget>)
        .collect()
}

fn notice(text: &str, ink: Color) -> Rectangle {
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
 * a section's heading and its rows on a card, hairlines between them; the Modules row draws its
 * own cards. `query` narrows the Modules to those it mentions
 */
fn section_view(
    title: &str,
    rows: Vec<&Line>,
    settings: &Settings,
    query: Option<&str>,
    roles: &ThemeRoles,
) -> Column {
    let mut items: Vec<Box<dyn Widget>> = Vec::new();

    if !title.is_empty() {
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

        if !card.is_empty() {
            card.push(Box::new(hairline(inner(), roles)));
        }
        card.push(row_view(row, settings, roles));
    }

    if !card.is_empty() {
        items.push(Box::new(card_of(card, roles)));
    }

    Column::new(items).width(content()).gap(10.0)
}

fn card_of(rows: Vec<Box<dyn Widget>>, roles: &ThemeRoles) -> Rectangle {
    let column = Column::new(rows).width(inner());
    let height = fixed(Widget::height(&column));

    Rectangle::new()
        .width(content())
        .height(height)
        .radius(radius::CARD)
        .fill(roles.surface_container)
        .padding(sides(CARD_INSET))
        .child(column)
}

fn row_view(row: &Line, settings: &Settings, roles: &ThemeRoles) -> Box<dyn Widget> {
    match *row {
        Line::Key(key) => {
            let setting = setting(key).expect("the layout's keys are the schema's");
            let (control, beside) = control(settings, setting, roles);

            labeled(
                label(settings, &[setting], setting.label, &help(setting), roles),
                control,
                beside,
                roles,
            )
        }
        Line::Material => {
            let setting = setting(layout::MATERIAL).expect("material is in the schema");

            labeled(
                label(settings, &[setting], setting.label, &help(setting), roles),
                Box::new(materials(settings, setting, roles)),
                false,
                roles,
            )
        }
        Line::Position { edge, align } => {
            let edge = setting(edge).expect("the layout's keys are the schema's");
            let align = setting(align).expect("the layout's keys are the schema's");

            labeled(
                label(
                    settings,
                    &[edge, align],
                    "Position",
                    "Which edge of the screen, and where along it",
                    roles,
                ),
                Box::new(position(settings, edge, align, roles)),
                true,
                roles,
            )
        }
    }
}

// a row's label over its help, with its marks, and what Inspect shows
struct Label {
    title: String,
    help: String,
    marks: Vec<Box<dyn Widget>>,
    keys: Option<String>,
    off: Option<String>,
}

fn label(
    settings: &Settings,
    shown: &[&'static Setting],
    title: &str,
    help: &str,
    roles: &ThemeRoles,
) -> Label {
    let mut marks: Vec<Box<dyn Widget>> = Vec::new();

    if shown
        .iter()
        .any(|setting| setting.restart && settings.pending(setting.key))
    {
        marks.push(Box::new(restart(roles)));
    }

    let paths: Vec<Vec<String>> = shown
        .iter()
        .map(|setting| path(setting.key, None))
        .filter(|path| settings.set_here(path))
        .collect();
    if !paths.is_empty() {
        marks.push(Box::new(link("Reset", roles, move || {
            change_all(paths.iter().map(|path| (path.clone(), None)).collect());
        })));
    }

    let keys = settings.inspect.then(|| {
        shown
            .iter()
            .map(|setting| {
                if settings.set_here(&path(setting.key, None)) {
                    format!("{}  (settings.toml)", setting.key)
                } else {
                    setting.key.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("   ")
    });

    let off = shown.iter().find_map(|setting| {
        let owner = owner(setting.key)?;

        (owner.name != CORE && !modules::on(owner.name)).then(|| {
            format!(
                "The {} module is off: this applies once it is on, after a restart",
                owner.name
            )
        })
    });

    Label {
        title: title.to_owned(),
        help: help.to_owned(),
        marks,
        keys,
        off,
    }
}

// a label with its control beside it, or under it when the control is wide
fn labeled(
    label: Label,
    control: Box<dyn Widget>,
    beside: bool,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    let width = fixed(control.width());
    let beside = beside && width <= most_beside();
    let text_width = if beside {
        inner() - width - 24.0
    } else {
        inner()
    };

    let mut lines = children![
        Row::new(
            std::iter::once(Box::new(
                Text::new(label.title)
                    .size(theme::text::BODY)
                    .color(roles.on_surface)
                    .weight(theme::text::SEMIBOLD),
            ) as Box<dyn Widget>)
            .chain(label.marks)
            .collect(),
        )
        .height(22.0)
        .gap(10.0)
        .align(Center),
        wrapped(
            Text::new(label.help)
                .size(theme::text::LABEL)
                .color(roles.on_surface_variant)
                .weight(theme::text::MEDIUM),
            text_width,
        ),
    ];
    if let Some(off) = label.off {
        lines.push(Box::new(wrapped(
            Text::new(off)
                .size(theme::text::LABEL_SMALL)
                .color(theme::SEMANTIC.warning)
                .weight(theme::text::MEDIUM),
            text_width,
        )));
    }
    if let Some(keys) = label.keys {
        lines.push(Box::new(
            Text::new(keys)
                .size(theme::text::LABEL_SMALL)
                .font("monospace")
                .color(roles.primary)
                .weight(theme::text::MEDIUM)
                .elide(),
        ));
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

    Box::new(
        Rectangle::new()
            .width(inner())
            .height(height)
            .padding(Padding {
                top: ROW_INSET,
                right: 0.0,
                bottom: ROW_INSET,
                left: 0.0,
            })
            .align_child(Start, Center)
            .child(Column::new(vec![content]).width(inner())),
    )
}

// what sets the key, by its Kind, showing the value the files give, and whether it fits beside
fn control(
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
                Box::new(switch(on, roles, move || {
                    change(path(key, None), Some(Value::Boolean(!on)));
                })),
                true,
            )
        }
        Kind::Choice(options, field) => {
            let chosen = (field.get)(read);

            (
                Box::new(segmented(key, options, chosen, roles, move |option| {
                    change(path(key, None), Some(Value::String(option.to_owned())));
                })),
                true,
            )
        }
        Kind::Millis(field) => {
            let ms = u64::try_from((field.get)(read).as_millis()).unwrap_or(config::LONGEST);

            (Box::new(stepper(setting, ms, MILLIS, roles)), true)
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

            (Box::new(stepper(setting, px, steps, roles)), true)
        }
        Kind::Path(_) | Kind::Text(_) => (
            Box::new(field_with_hint(
                setting,
                setting.example.unwrap_or("").trim_matches('"'),
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
                setting,
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
                roles,
            )),
            false,
        ),
        Kind::Paths(field) => (
            Box::new(list(
                setting,
                (field.get)(read),
                "Add a file or directory, then Enter",
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
                        children![icon_button(Icon::Dismiss, roles, move || {
                            change(entry.clone(), None)
                        })]
                    } else {
                        children![]
                    };

                    Box::new(entry_line(
                        &format!("{app_id}  \u{2192}  {id}"),
                        actions,
                        roles,
                    )) as Box<dyn Widget>
                })
                .collect();

            rows.push(Box::new(text_field(
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
            )));

            (Box::new(Column::new(rows).width(inner()).gap(6.0)), false)
        }
        Kind::Modules(_) => (Box::new(modules_view(settings, None, roles)), false),
    }
}

/*
 * the Modules, a card per group, each with what it does, what it needs and a switch; the core
 * first, which is always on. Turning one takes a restart
 */
fn modules_view(settings: &Settings, query: Option<&str>, roles: &ThemeRoles) -> Column {
    let off = match setting(MODULES).map(|setting| &setting.kind) {
        Some(Kind::Modules(field)) => (field.get)(&settings.read),
        _ => Vec::new(),
    };

    let mut items = children![notice(
        "Turning a Module on or off takes effect after a restart",
        roles.primary
    )];

    let core = modules::ALL.iter().find(|module| module.name == CORE);
    if let Some(core) = core.filter(|core| query.is_none_or(|query| module_matches(core, query))) {
        items.push(Box::new(card_of(
            children![module_line(
                "Island core",
                core.about,
                Box::new(
                    Text::new("Always on")
                        .size(theme::text::LABEL)
                        .color(roles.on_surface_variant)
                        .weight(theme::text::SEMIBOLD),
                ),
                Vec::new(),
                roles,
            )],
            roles,
        )));
    }

    for (group, names) in MODULE_GROUPS {
        let mut rows: Vec<Box<dyn Widget>> = Vec::new();

        for module in names
            .iter()
            .filter_map(|name| modules::ALL.iter().find(|module| module.name == *name))
            .filter(|module| query.is_none_or(|query| module_matches(module, query)))
        {
            let name = module.name;
            let on = !off.contains(&name);

            let mut marks: Vec<Box<dyn Widget>> = Vec::new();
            if settings.pending(&format!("{MODULES}.{name}")) {
                marks.push(Box::new(restart(roles)));
            }

            let mut about = String::from(module.about);
            let requires: Vec<&str> = module
                .requires
                .iter()
                .copied()
                .filter(|&required| required != CORE)
                .collect();
            if !requires.is_empty() {
                about.push_str(&format!(" \u{00b7} requires {}", requires.join(", ")));
            }

            if !rows.is_empty() {
                rows.push(Box::new(hairline(inner(), roles)));
            }
            rows.push(Box::new(module_line(
                name,
                &about,
                Box::new(switch(on, roles, move || {
                    change(path(MODULES, Some(name)), Some(Value::Boolean(!on)));
                })),
                marks,
                roles,
            )));
        }

        if rows.is_empty() {
            continue;
        }

        items.push(Box::new(
            Text::new(*group)
                .size(theme::text::LABEL)
                .color(roles.on_surface_variant)
                .weight(theme::text::SEMIBOLD),
        ));
        items.push(Box::new(card_of(rows, roles)));
    }

    Column::new(items).width(content()).gap(10.0)
}

fn module_line(
    name: &str,
    about: &str,
    action: Box<dyn Widget>,
    marks: Vec<Box<dyn Widget>>,
    roles: &ThemeRoles,
) -> Rectangle {
    let width = inner() - fixed(action.width()) - 24.0;

    let text = Column::new(children![
        Row::new(
            std::iter::once(Box::new(
                Text::new(name)
                    .size(theme::text::BODY)
                    .color(roles.on_surface)
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

    let height = fixed(Widget::height(&text)).max(TARGET) + 2.0 * 12.0;

    Rectangle::new()
        .width(inner())
        .height(height)
        .align_child(Start, Center)
        .child(
            Row::new(vec![Box::new(text), action])
                .width(inner())
                .justify(SpaceBetween)
                .align(Center),
        )
}

// a tile of each material over a bright backdrop, in the glass tone set now
fn materials(settings: &Settings, setting: &'static Setting, roles: &ThemeRoles) -> Row {
    const GAP: f32 = 10.0;
    let tile = (inner() - 4.0 * GAP) / 5.0;

    let Kind::Choice(options, field) = &setting.kind else {
        return Row::new(Vec::new());
    };
    let chosen = (field.get)(&settings.read);
    let tone = settings.read.appearance.tone;
    let key = setting.key;

    let tiles = options
        .iter()
        .map(|&option| {
            let here = option == chosen;

            let scene = Rectangle::new()
                .width(tile)
                .height(72.0)
                .radius(radius::ROW)
                .fill(backdrop())
                .align_child(Center, Center)
                .child(
                    Rectangle::new()
                        .width(64.0)
                        .height(22.0)
                        .radius(11.0)
                        .fill(glass::swatch(Material::named(option), tone))
                        .border(1.0, theme::faded(theme::ON_SCENE, 0.3)),
                );
            let scene = if here {
                scene.border(2.0, roles.primary)
            } else {
                scene
            };

            let (ink, weight) = if here {
                (roles.on_surface, theme::text::SEMIBOLD)
            } else {
                (roles.on_surface_variant, theme::text::MEDIUM)
            };

            Box::new(
                Rectangle::new()
                    .width(tile)
                    .height(98.0)
                    .cursor(Cursor::Pointer)
                    .on_click(left(move || {
                        change(path(key, None), Some(Value::String(option.to_owned())));
                    }))
                    .child(
                        Column::new(children![
                            scene,
                            Rectangle::new()
                                .width(tile)
                                .height(20.0)
                                .align_child(Center, Center)
                                .child(
                                    Text::new(humanize(key, option))
                                        .size(theme::text::LABEL)
                                        .color(ink)
                                        .weight(weight),
                                ),
                        ])
                        .gap(6.0),
                    ),
            ) as Box<dyn Widget>
        })
        .collect();

    Row::new(tiles).width(inner()).gap(GAP)
}

// a small screen with the six places an edge and a side make, the chosen one filled
fn position(
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

        Box::new(
            Rectangle::new()
                .width(cell_width)
                .height(cell_height)
                .align_child(Center, Center)
                .cursor(Cursor::Pointer)
                .on_click(left(press))
                .child(
                    Rectangle::new()
                        .width(width)
                        .height(height)
                        .radius(5.0)
                        .fill(if here {
                            roles.primary
                        } else {
                            faint(roles.on_surface_variant)
                        }),
                ),
        ) as Box<dyn Widget>
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

    let sides = Row::new(
        [Edge::Left, Edge::Right]
            .into_iter()
            .map(|side| place((side.name(), read.1), (10.0, 30.0)))
            .collect(),
    )
    .width(Parent)
    .justify(SpaceBetween);

    Rectangle::new()
        .width(176.0)
        .height(116.0)
        .radius(radius::ROW)
        .fill(roles.surface_container_high)
        .border(1.0, roles.outline)
        .padding(6.0)
        .child(
            Column::new(children![line(Edge::Top), sides, line(Edge::Bottom)])
                .width(Parent)
                .height(Parent)
                .justify(SpaceBetween),
        )
}

// a list key's entries, each with Up, Down and Remove, and a field that adds one at the end
fn list(
    setting: &'static Setting,
    ids: Vec<String>,
    placeholder: &str,
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
                actions.push(Box::new(text_button("\u{2191}", roles, to(list))));
            }
            if index + 1 < ids.len() {
                let mut list = ids.clone();
                list.swap(index, index + 1);
                actions.push(Box::new(text_button("\u{2193}", roles, to(list))));
            }

            let mut without = ids.clone();
            without.remove(index);
            actions.push(Box::new(icon_button(Icon::Dismiss, roles, to(without))));

            Box::new(entry_line(id, actions, roles)) as Box<dyn Widget>
        })
        .collect();

    rows.push(Box::new(text_field(
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
    )));

    Column::new(rows).width(inner()).gap(6.0)
}

// a list's entry with its actions at the end
fn entry_line(text: &str, actions: Vec<Box<dyn Widget>>, roles: &ThemeRoles) -> Rectangle {
    Rectangle::new()
        .width(inner())
        .height(ENTRY)
        .radius(radius::ROW)
        .fill(roles.surface_container_high)
        .padding(sides(12.0))
        .align_child(Start, Center)
        .child(
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

// on is filled with the knob at the end, off outlined with it at the start
fn switch(on: bool, roles: &ThemeRoles, press: impl Fn() + 'static) -> Rectangle {
    const KNOB: f32 = 16.0;

    let track = Rectangle::new()
        .width(44.0)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .padding(4.0)
        .cursor(Cursor::Pointer)
        .on_click(left(press));

    let (track, knob, end) = if on {
        (
            track.fill(roles.primary),
            roles.on_primary,
            Align::from(End),
        )
    } else {
        (
            track
                .fill(roles.surface_container_high)
                .border(1.0, roles.outline),
            roles.on_surface_variant,
            Align::from(Start),
        )
    };

    track.align_child(end, Center).child(
        Rectangle::new()
            .width(KNOB)
            .height(KNOB)
            .radius(KNOB / 2.0)
            .fill(knob),
    )
}

// a Choice's options side by side in a trough, the chosen one filled
fn segmented(
    key: &'static str,
    options: &'static [&'static str],
    chosen: &str,
    roles: &ThemeRoles,
    choose: impl Fn(&'static str) + Clone + 'static,
) -> Rectangle {
    const INSET: f32 = 3.0;
    const HEIGHT: f32 = 28.0;

    let segments: Vec<Box<dyn Widget>> = options
        .iter()
        .map(|&option| {
            let here = option == chosen;
            let text = Text::new(humanize(key, option))
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
                .align_child(Center, Center)
                .cursor(Cursor::Pointer)
                .on_click(left(move || choose(option)))
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

    Rectangle::new()
        .width(width)
        .height(HEIGHT + 2.0 * INSET)
        .radius(HEIGHT / 2.0 + INSET)
        .fill(roles.surface_container_high)
        .padding(INSET)
        .child(row)
}

// a number: `-`, the number to type over and its unit, `+`
fn stepper(setting: &'static Setting, value: u64, steps: Steps, roles: &ThemeRoles) -> Row {
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

    Row::new(children![
        text_button("\u{2212}", roles, to(value.saturating_sub(step))),
        text_field(input(setting), 64.0, "", roles, move |text| {
            match text.trim().parse::<u64>() {
                Ok(value) => to(value)(),
                Err(_) => {
                    Settings::write().refused =
                        Some(format!("Not saved: {key}: expected {expected}"));
                }
            }
        }),
        Text::new(unit)
            .size(theme::text::LABEL)
            .color(roles.on_surface_variant)
            .weight(theme::text::MEDIUM),
        text_button("+", roles, to(value + step)),
    ])
    .gap(6.0)
    .align(Center)
}

// a text field beside its label, saved by Enter, an empty one handing the key back
fn field_with_hint(
    setting: &'static Setting,
    placeholder: &str,
    roles: &ThemeRoles,
    submit: impl Fn(String) + 'static,
) -> Column {
    const WIDE: f32 = 260.0;

    Column::new(children![
        text_field(input(setting), WIDE, placeholder, roles, submit),
        Text::new("Enter saves; empty resets")
            .size(theme::text::LABEL_SMALL)
            .color(roles.on_surface_variant)
            .weight(theme::text::MEDIUM),
    ])
    .width(WIDE)
    .gap(4.0)
}

// a square button with a sign
fn text_button(label: &str, roles: &ThemeRoles, press: impl Fn() + 'static) -> Rectangle {
    Rectangle::new()
        .width(28.0)
        .height(28.0)
        .radius(radius::ROW - 4.0)
        .border(1.0, roles.outline)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(left(press))
        .child(
            Text::new(label)
                .size(theme::text::BODY)
                .color(roles.on_surface)
                .weight(theme::text::SEMIBOLD),
        )
}

fn icon_button(icon: Icon, roles: &ThemeRoles, press: impl Fn() + 'static) -> Rectangle {
    Rectangle::new()
        .width(28.0)
        .height(28.0)
        .radius(radius::ROW - 4.0)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(left(press))
        .child(icon.on(14.0, roles.on_surface_variant))
}

// a word in the primary color that acts when pressed
fn link(label: &str, roles: &ThemeRoles, press: impl Fn() + 'static) -> Rectangle {
    let text = Text::new(label)
        .size(theme::text::LABEL_SMALL)
        .color(roles.primary)
        .weight(theme::text::SEMIBOLD);
    let width = fixed(Widget::width(&text)) + 4.0;

    Rectangle::new()
        .width(width)
        .height(20.0)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(left(press))
        .child(text)
}

// a text input, saved by Enter
fn text_field(
    id: &'static str,
    width: f32,
    placeholder: &str,
    roles: &ThemeRoles,
    submit: impl Fn(String) + 'static,
) -> Rectangle {
    Rectangle::new()
        .width(width)
        .height(FIELD)
        .radius(radius::ROW - 2.0)
        .fill(roles.surface_container_high)
        .border(1.0, faint(roles.outline))
        .padding(sides(10.0))
        .align_child(Start, Center)
        .child(
            TextInput::new(id)
                .width(Parent)
                .size(theme::text::LABEL)
                .color(roles.on_surface)
                .placeholder(placeholder)
                .on_submit(submit),
        )
}

fn restart(roles: &ThemeRoles) -> Text {
    Text::new("Restart to apply")
        .size(theme::text::LABEL_SMALL)
        .color(roles.primary)
        .weight(theme::text::SEMIBOLD)
}

// a setting's help as a sentence, saying when it takes a restart
fn help(setting: &Setting) -> String {
    let mut help = capitalized(setting.help);
    if setting.restart {
        help.push_str(". Takes a restart");
    }

    help
}

// an option as the window names it
fn humanize(key: &str, option: &str) -> String {
    match (key, option) {
        ("clock", "24h") => String::from("14:05"),
        ("clock", "12h") => String::from("2:05 PM"),
        ("weather.units", "metric") => String::from("\u{00b0}C \u{00b7} km/h"),
        ("weather.units", "imperial") => String::from("\u{00b0}F \u{00b7} mph"),
        _ => capitalized(&option.replace('-', " ")),
    }
}

fn capitalized(text: &str) -> String {
    let mut letters = text.chars();

    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

fn setting(key: &str) -> Option<&'static Setting> {
    owner(key)?
        .settings
        .iter()
        .find(|setting| setting.key == key)
}

// the Module owning a key
fn owner(key: &str) -> Option<&'static Module> {
    modules::ALL
        .iter()
        .find(|module| module.settings.iter().any(|setting| setting.key == key))
}

// the scene glass shows over
fn backdrop() -> Gradient {
    let [from, middle, to] = theme::SCENE;

    Gradient::linear(120.0, [(0.0, from), (0.5, middle), (1.0, to)])
}

// a color faint enough for a box's fill or a quiet line
fn faint(color: Color) -> Color {
    theme::faded(color, 0.2)
}

fn hairline(width: impl Into<Size>, roles: &ThemeRoles) -> Rectangle {
    Rectangle::new()
        .width(width)
        .height(1.0)
        .fill(faint(roles.outline))
}

fn space(height: f32) -> Rectangle {
    Rectangle::new().width(1.0).height(height)
}

// text wrapped to `width`, as tall as its lines
fn wrapped(text: Text, width: f32) -> Rectangle {
    let text = text.wrap();
    let height = text.height_in(width);

    Rectangle::new().width(width).height(height).child(text)
}

fn left(press: impl Fn() + 'static) -> impl Fn(Button) + 'static {
    move |button| {
        if button == Button::Left {
            press();
        }
    }
}

fn strings(list: &[String]) -> Value {
    Value::Array(list.iter().cloned().map(Value::String).collect())
}

fn sides(inset: f32) -> Padding {
    Padding {
        top: 0.0,
        right: inset,
        bottom: 0.0,
        left: inset,
    }
}

fn fixed(size: Size) -> f32 {
    match size {
        Size::Fixed(pixels) => pixels,
        Size::Parent => 0.0,
    }
}

thread_local! {
    // each page's scroll id, so turning to a page keeps where it was scrolled to
    static SCROLLS: RefCell<HashMap<&'static str, &'static str>> = RefCell::new(HashMap::new());
}

fn scroll_id(page: &'static str) -> &'static str {
    SCROLLS.with_borrow_mut(|scrolls| {
        *scrolls
            .entry(page)
            .or_insert_with(|| Box::leak(format!("settings {page}").into_boxed_str()))
    })
}
