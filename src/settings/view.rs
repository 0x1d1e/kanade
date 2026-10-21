//! The Settings window's view: the pages down the side under a search field, the page's sections
//! beside them, a footer with what the files say and Undo. Each row names a setting and says what
//! it does, with its control beside it or, for a list, under it; one the settings file sets has a
//! Reset, which hands the key back to the layers below. Inspect shows each row's key.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use kanade_runtime::service::Service;
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Align, Button, Canvas, Center, Color, Column, Cursor, End, Gradient, Image, Key, Padding,
    Parent, Path, Rectangle, Reveal, Row, ScrollArea, Shape, Size, SpaceBetween, Stack, Start,
    Text, TextInput, Widget, Window, children,
};
use toml::Value;

use crate::config::{self, Choices, DayTime, Kind, Location, Setting};
use crate::doctor::Verdict;
use crate::icon::Icon;
use crate::look::{Along, Edge, ShellMode};
use crate::modules::{self, CORE, Module};
use crate::pane;
use crate::sources::icons;
use crate::theme::space::TARGET;
use crate::theme::{self, ThemeRoles, radius};
use crate::tiles::{self, Placed};

use super::health;
use super::layout::{MODULE_GROUPS, MODULES, PAGES, Page, Row as Line};
use super::login;
use super::niri;
use super::outputs;
use super::ring::{self, Stop};
use super::{
    FONT_SEARCH, MODULE_FILTER, SEARCH, Settings, change as super_change,
    change_all as super_change_all, font_search, hover, input, inspect, module_filter, path,
    search, select_font, turn_to, undo,
};

fn change(path: Vec<String>, value: Option<Value>) {
    let is_login =
        Settings::read().page == login::PAGE || path.first().is_some_and(|p| p == "sddm");
    super_change(path, value);
    if is_login {
        login::load();
    }
}

fn change_all(changes: Vec<(Vec<String>, Option<Value>)>) {
    let is_login = Settings::read().page == login::PAGE
        || changes
            .iter()
            .any(|(p, _)| p.first().is_some_and(|k| k == "sddm"));
    super_change_all(changes);
    if is_login {
        login::load();
    }
}

pub(super) fn hover_wash(roles: &ThemeRoles) -> Color {
    theme::faded(roles.on_surface, 0.08)
}

pub(super) fn hover_primary(roles: &ThemeRoles) -> Color {
    theme::mix(roles.primary, roles.on_primary, 0.12)
}

pub(super) fn hover_container(roles: &ThemeRoles) -> Color {
    theme::mix(roles.surface_container_high, roles.on_surface, 0.08)
}

// the size it opens at, and the smallest it can be made
mod niri_page;
mod outputs_page;

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
// a row's label and help, about 70 characters a line
const MOST_MEASURE: f32 = 460.0;
const CARD_INSET: f32 = 16.0;

const BAR: f32 = 52.0;

// a gap and a ring around a control that has no edge of its own to take the ring
const HALO: f32 = 4.0;
const RING: f32 = 2.0;
const ROW_INSET: f32 = 14.0;
const FIELD: f32 = 34.0;
const ENTRY: f32 = 36.0;
const PAGE_TITLE: f32 = 22.0;

// between a page's sections, and from a heading to its card
const SECTIONS: f32 = 24.0;
const HEADING: f32 = 8.0;

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
const PERCENT_STEP: u64 = 10;
// an idle stage steps by half a minute
const SECONDS_STEP: u64 = 30;
// a lead or a staleness steps by five minutes, as calendars offer them
const MINUTE_STEP: u64 = 5;
// a warmth steps by about what the eye tells apart
const KELVIN_STEP: u64 = 100;

pub fn window() -> Window {
    ring::begin();

    let settings = Settings::read();
    let roles = theme::roles();

    // the niri page's inputs hold what the worker last read
    niri::sync_inputs(&settings.niri);
    outputs::sync_inputs(&settings.outputs);

    Window::new()
        .title("Kanade Settings")
        .size(WIDTH, HEIGHT)
        .min_size(MIN_WIDTH, MIN_HEIGHT)
        .on_key(key)
        .child(
            Row::new(children![
                sidebar(&settings, &roles),
                Rectangle::new()
                    .width(Parent)
                    .height(Parent)
                    .fill(roles.surface)
                    .padding(Padding {
                        top: crate::island::geometry::REST.height + 8.0,
                        right: 0.0,
                        bottom: 0.0,
                        left: 0.0,
                    })
                    .child(main(&settings, &roles)),
            ])
            .width(Parent)
            .height(Parent),
        )
}

// Esc puts the monitors back while a change waits for Keep, then goes to the ring as any key
fn key(key: Key) {
    if key == Key::Escape && Settings::read().outputs.trial.is_some() {
        outputs::revert();
    }

    ring::key(key);
}

// the brand, the search field, then the pages under their groups' headings, scrolled in a short window
fn sidebar(settings: &Settings, roles: &ThemeRoles) -> Rectangle {
    let search = search_field(roles);

    // the pages are one stop: Up and Down turn to the one before or after
    let at = PAGES
        .iter()
        .position(|page| page.name == settings.page)
        .unwrap_or(0);
    let pages = ring::register(
        Stop::new("pages")
            .press(move || turn_to(PAGES[at].name))
            .step(move |direction| {
                let next = (at as i32 + direction).clamp(0, PAGES.len() as i32 - 1);

                turn_to(PAGES[next as usize].name);
            }),
    );

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

        items.push(nav(page, settings, pages, roles));
    }

    Rectangle::new()
        .width(SIDEBAR)
        .height(Parent)
        .fill(theme::over(roles.surface_container, roles.surface))
        .padding(SIDE_INSET)
        .align_child(Start, Start)
        .child(
            Column::new(children![
                brand(roles),
                search,
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

    // Kanade's icon, else the gradient and Island mark while it is not installed
    let mark = Rectangle::new().width(MARK).height(MARK).radius(9.0);
    let mark = match icons::own_icon() {
        Some(path) => mark.fill(Image::contain(path).thumbnail(MARK as u32 * 2, MARK as u32 * 2)),
        None => mark
            .fill(backdrop())
            .align_child(Center, Center)
            .child(Icon::Island.on(16.0, theme::ON_SCENE)),
    };

    Row::new(children![
        mark,
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

fn search_field(roles: &ThemeRoles) -> Box<dyn Widget> {
    let token = ring::register(Stop::new("search").input(SEARCH));

    outlined(
        Rectangle::new()
            .width(NAV)
            .height(FIELD)
            .radius(radius::ROW)
            .fill(roles.surface)
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
            ),
        token,
        Some(roles.edge()),
        roles,
    )
}

// a page down the side, filled while it shows; the ring is on the one that shows
fn nav(
    page: &'static Page,
    settings: &Settings,
    ring: Option<u64>,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    let shown = page.name == settings.page;
    let here = shown && settings.query.trim().is_empty();
    let name = page.name;
    let hovered = settings.is_hovered(name);

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
        .on_hover(move |inside| hover(name, inside))
        .semantics(|| {
            Semantics::new(Role::Tab, name)
                .label(page.title)
                .selected(shown)
        })
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
    let row = if here {
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

    outlined(row, ring.filter(|_| shown), None, roles)
}

// the header, the page or the search's results, the footer
fn main(settings: &Settings, roles: &ThemeRoles) -> Column {
    let page = PAGES
        .iter()
        .find(|page| page.name == settings.page)
        .unwrap_or(&PAGES[0]);

    let searching = !settings.query.trim().is_empty();
    let title = if searching { "Search" } else { page.title };

    // built in the order the keyboard goes through them
    let header = header(title, settings, roles);
    let body = if searching {
        results(settings, roles)
    } else {
        page_body(page, settings, roles)
    };
    let footer = footer(settings, roles);

    Column::new(children![
        header,
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
        footer,
    ])
    .width(Parent)
    .height(Parent)
}

// the page's title, and Inspect
fn header(title: &str, settings: &Settings, roles: &ThemeRoles) -> Rectangle {
    let hovered = settings.is_hovered("inspect");
    let inspect_button = Rectangle::new()
        .width(96.0)
        .height(30.0)
        .radius(radius::ROW)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_hover(move |inside| hover("inspect", inside))
        .semantics(|| Semantics::new(Role::Toggle, "inspect").checked(settings.inspect))
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
        if hovered {
            inspect_button.fill(hover_container(roles))
        } else {
            inspect_button.fill(roles.surface_container_high)
        }
    } else if hovered {
        inspect_button.fill(hover_wash(roles))
    } else {
        inspect_button
    };
    let inspect_button = outlined(
        inspect_button,
        ring::register(Stop::new("inspect").press(inspect)),
        Some(roles.edge()),
        roles,
    );

    Rectangle::new()
        .width(Parent)
        .height(BAR)
        .padding(sides(PAD))
        .align_child(Start, Center)
        .child(
            Row::new(children![
                Text::new(title)
                    .size(PAGE_TITLE)
                    .color(roles.on_surface)
                    .weight(theme::text::SEMIBOLD),
                inspect_button,
            ])
            .width(Parent)
            .justify(SpaceBetween)
            .align(Center),
        )
}

// what the files say: a refusal, a problem, a restart waiting, else the last change or where changes go; and Undo
fn footer(settings: &Settings, roles: &ThemeRoles) -> Rectangle {
    // the niri page answers for niri's file, and has its own Undo
    let niri_page = !settings.searching() && settings.page == niri::PAGE
        || settings.searching()
            && (settings.niri.touched
                || settings.niri.undoable
                || settings.niri.status().is_some()
                || ring::on_niri_setting());
    let waiting = settings.pending.len();
    let (status, ink) = if niri_page {
        niri_page::status(settings, roles)
    } else if let Some(refused) = &settings.refused {
        (refused.clone(), theme::window_signals().warning)
    } else if settings.unreadable.is_some()
        || settings.error.is_some()
        || !settings.skipped.is_empty()
    {
        (
            String::from("The config has problems, shown above"),
            theme::window_signals().warning,
        )
    } else if waiting > 0 {
        let changes = if waiting == 1 {
            "change waits"
        } else {
            "changes wait"
        };

        (format!("{waiting} {changes} for a restart"), roles.primary)
    } else if let Some(saved) = settings.history.last().and_then(|last| last.first()) {
        (
            format!("Saved: {}", changed(&saved.path)),
            roles.on_surface_variant,
        )
    } else {
        (
            config::settings_file().map_or_else(
                || String::from("Changes are written as you make them"),
                |file| format!("Changes are saved to {}", shortened(&file)),
            ),
            roles.on_surface_variant,
        )
    };

    let mut actions: Vec<Box<dyn Widget>> = Vec::new();
    let undoable = if niri_page {
        settings.niri.undoable
    } else {
        !settings.history.is_empty()
    };
    let undo: fn() = if niri_page { niri::undo } else { undo };
    if niri_page {
        actions.extend(outputs_page::actions(settings, roles));
    }
    if undoable {
        let hovered = settings.is_hovered("undo");
        let undo_button = Rectangle::new()
            .width(84.0)
            .height(30.0)
            .radius(radius::ROW)
            .align_child(Center, Center)
            .cursor(Cursor::Pointer)
            .on_hover(move |inside| hover("undo", inside))
            .semantics(|| Semantics::new(Role::Button, "undo"))
            .on_click(left(undo))
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

        let undo_button = if hovered {
            undo_button.fill(hover_wash(roles))
        } else {
            undo_button
        };

        actions.push(outlined(
            undo_button,
            ring::register(Stop::new("undo").press(undo)),
            Some(roles.edge()),
            roles,
        ));
    }

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
                Row::new(actions),
            ])
            .width(Parent)
            .gap(16.0)
            .justify(SpaceBetween)
            .align(Center),
        )
}

// what a change to the key at `path` is called
fn changed(path: &[String]) -> String {
    let key = path.join(".");

    match path {
        [modules, name] if modules == MODULES => format!("{} module", humanized(name)),
        _ => setting(&key)
            .or_else(|| setting(&path[..path.len().saturating_sub(1)].join(".")))
            .map_or(key, |setting| setting.label.to_owned()),
    }
}

// a path with the home directory as ~
fn shortened(path: &std::path::Path) -> String {
    match std::env::var_os("HOME")
        .and_then(|home| path.strip_prefix(home).ok().map(|rest| rest.to_owned()))
    {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

// a page's summary, what is wrong with the files, then its sections
fn page_body(page: &'static Page, settings: &Settings, roles: &ThemeRoles) -> Column {
    let mut items = children![
        space(14.0),
        wrapped(
            Text::new(page.summary)
                .size(theme::text::BODY)
                .color(roles.on_surface_variant)
                .weight(theme::text::MEDIUM),
            content(),
        ),
    ];
    if page.name == niri::PAGE {
        items.extend(niri_page::notices(settings));
    } else {
        items.extend(notices(settings));
    }

    for section in page.sections {
        items.push(Box::new(section_view(
            Heading::Page(section.title),
            section.rows.iter().collect(),
            settings,
            None,
            roles,
        )));
    }

    items.push(Box::new(space(12.0)));

    Column::new(items).width(content()).gap(SECTIONS)
}

// every row matching the search, under its page's and section's titles
fn results(settings: &Settings, roles: &ThemeRoles) -> Column {
    let query = settings.query.trim().to_lowercase();

    let mut items = children![
        space(14.0),
        Text::new(format!(
            "Results for \u{201c}{}\u{201d}",
            settings.query.trim()
        ))
        .size(theme::text::BODY)
        .color(roles.on_surface_variant)
        .weight(theme::text::MEDIUM),
    ];
    items.extend(notices(settings));
    if PAGES
        .iter()
        .flat_map(|page| page.sections)
        .flat_map(|section| section.rows)
        .any(|row| matches!(row, Line::Niri(_)) && matches(row, &query))
    {
        items.extend(niri_page::notices(settings));
    }

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
            items.push(Box::new(section_view(
                Heading::Found(page, section.title),
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

    Column::new(items).width(content()).gap(SECTIONS)
}

// whether a row's settings mention the query; the Modules row, whether a Module does, as it shows those
fn matches(row: &Line, query: &str) -> bool {
    let mentions = |text: &str| text.to_lowercase().contains(query);

    if let Line::Niri(path) = row {
        return niri_page::mentions(path, query);
    }

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
        .map(|problem| {
            Box::new(notice(&problem, theme::window_signals().warning)) as Box<dyn Widget>
        })
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

// what a section is titled in: on its page, or found by a search under the page's name too
enum Heading {
    Page(&'static str),
    Found(&'static Page, &'static str),
}

/*
 * a section's rows on a card, hairlines between them, under a heading when it has several rows:
 * one row is titled by its own label. The Modules row draws its own cards. `query` narrows the
 * Modules to those it mentions
 */
fn section_view(
    heading: Heading,
    rows: Vec<&Line>,
    settings: &Settings,
    query: Option<&str>,
    roles: &ThemeRoles,
) -> Column {
    let mut items: Vec<Box<dyn Widget>> = Vec::new();

    match heading {
        Heading::Page(title)
            if (rows.len() > 1 || rows.iter().any(|row| matches!(row, Line::Outputs)))
                && !title.is_empty() =>
        {
            items.push(Box::new(
                Text::new(title)
                    .size(theme::text::TITLE)
                    .color(roles.on_surface)
                    .weight(theme::text::SEMIBOLD),
            ));
        }
        Heading::Page(_) => {}
        Heading::Found(page, title) => items.push(found_heading(page, title, settings, roles)),
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

// "Page  ›  Section" over a result, pressed to open the page
fn found_heading(
    page: &'static Page,
    title: &str,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    let name = page.name;
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
        .on_click(left(move || turn_to(name)))
        .child(row);
    let control = if hovered {
        control.fill(hover_wash(roles))
    } else {
        control
    };

    outlined(
        control,
        ring::register(Stop::new(id).press(move || turn_to(name))),
        None,
        roles,
    )
}

/*
 * a card's rows, the same `CARD_INSET` in from its edge on every side, so a row's hover wash stays
 * clear of the card's corner instead of meeting it. The inset and `radius::CARD` are the same, so a
 * row's straight edge begins where the card's corner ends and the row keeps its own smaller
 * `radius::ROW`: a nested radius of `radius::CARD - CARD_INSET` would be square here. The height
 * grows by what the padding takes, so the column still fits exactly
 */
fn card_of(rows: Vec<Box<dyn Widget>>, roles: &ThemeRoles) -> Rectangle {
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

fn row_view(row: &Line, settings: &Settings, roles: &ThemeRoles) -> Box<dyn Widget> {
    match *row {
        Line::Key(key) => {
            let setting = setting(key).expect("the layout's keys are the schema's");
            let (control, beside) = control(settings, setting, roles);

            labeled(
                label(settings, &[setting], setting.label, &help(setting), roles),
                control,
                beside,
                settings,
                roles,
            )
        }
        Line::Health => health_view(settings, roles),
        Line::Login => login_view(settings, roles),
        Line::Niri(path) => niri_page::row(path, settings, roles),
        Line::Outputs => Box::new(outputs_page::section(settings, roles)),
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

            labeled(
                label(settings, &[edge, align], title, description, roles),
                Box::new(position(settings, edge, align, roles)),
                true,
                settings,
                roles,
            )
        }
    }
}

// the width of a check's verdict, "warn" at the label size
const VERDICT: f32 = 40.0;

/*
 * the Health page's checks as doctor words them, each with its verdict and detail lines under it,
 * over how many need attention and Check again. Read-only: the checks ran off the draw thread
 */
fn health_view(settings: &Settings, roles: &ThemeRoles) -> Box<dyn Widget> {
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

fn is_status_warning(text: &str) -> bool {
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
fn login_view(settings: &Settings, roles: &ThemeRoles) -> Box<dyn Widget> {
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

// a row's label over its help, with its marks, and what Inspect shows
pub(super) struct Label {
    // the setting it is for, which names the row: the first of them when it sets more than one
    pub(super) node: &'static str,
    pub(super) hover_id: String,
    pub(super) title: String,
    pub(super) help: String,
    pub(super) marks: Vec<Box<dyn Widget>>,

    // under Inspect, the schema's whole help where Settings shows a shorter line
    pub(super) detail: Option<String>,
    pub(super) keys: Option<String>,
    pub(super) off: Option<String>,
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
        .find(|setting| setting.hint.is_some())
        .filter(|_| settings.inspect)
        .map(|setting| capitalized(setting.help));

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
        node: shown[0].key,
        hover_id: shown[0].key.to_string(),
        title: title.to_owned(),
        help: help.to_owned(),
        marks,
        detail,
        keys,
        off,
    }
}

// a label with its control beside it, or under it when the control is wide
pub(super) fn labeled(
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
        Kind::Path(_) | Kind::Text(_) | Kind::Node(_) => (
            Box::new(field_with_hint(
                input(setting),
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
        Kind::Argv(_) => unreachable!("hooks are set in config files only"),
        Kind::Names(..) => unreachable!("focus.auto is set in config files only"),
    }
}

// the font picker's width
const FONT_FIELD: f32 = 320.0;

fn font_picker(settings: &Settings, roles: &ThemeRoles) -> Column {
    const SUGGESTIONS: usize = 8;

    let families = settings.fonts.clone();
    let matching = super::fonts::matching(&families, &settings.font_query, SUGGESTIONS);
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
                        if let Some(family) = super::fonts::matching(&families, &query, 1)
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
            "Using the system default: Inter, then sans-serif"
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

fn font_choice(
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

fn font_note(text: &str, roles: &ThemeRoles) -> Rectangle {
    wrapped(
        Text::new(text)
            .size(theme::text::LABEL_SMALL)
            .color(roles.on_surface_variant)
            .weight(theme::text::MEDIUM),
        FONT_FIELD,
    )
}

/*
 * the Modules, a card per group under its name and how many of it run, each with what it does, what
 * it needs and a switch; the core first, which is always on. A filter narrows them. Turning one
 * takes a restart
 */
fn modules_view(settings: &Settings, query: Option<&str>, roles: &ThemeRoles) -> Column {
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

fn module_filter_field(roles: &ThemeRoles) -> Box<dyn Widget> {
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
fn module_line(
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

// a small screen with the places an edge and a side make, the chosen one filled
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
fn spot((edge, along): (&str, &str)) -> &'static str {
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
fn list(
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
fn entry_line(
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
pub(super) fn switch(
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

pub(super) fn switch_track(on: bool, hovered: bool, roles: &ThemeRoles) -> Rectangle {
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
pub(super) fn segmented(
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
fn cards(
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
fn tone_preview(option: &str) -> Box<dyn Widget> {
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

fn theme_tone(option: &str) -> crate::look::Tone {
    crate::look::Tone::named(option)
}

// the Island in a finish over the wallpaper's colors; Blur's is translucent over a band it blurs
fn finish_preview(option: &str) -> Box<dyn Widget> {
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
fn spring_preview(option: &str, roles: &ThemeRoles) -> Box<dyn Widget> {
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

// a number: `-`, the number to type over and its unit, `+`, in one outlined capsule; Up and Down step it
fn stepper(
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
    let id = input(setting);

    let unit = Text::new(unit)
        .size(theme::text::LABEL)
        .color(roles.on_surface_variant)
        .weight(theme::text::MEDIUM);
    let middle = NUMBER + 4.0 + fixed(Widget::width(&unit)) + 2.0 * 8.0;
    let inside = FIELD - 2.0;

    let side = |node: &'static str, sign: &str, value: u64| {
        let side_id = format!("stepper {id} {node}");
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
                    side("less", "\u{2212}", value.saturating_sub(step)),
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
                                    .on_submit(move |text| match text.trim().parse::<u64>() {
                                        Ok(value) => to(value)(),
                                        Err(_) => {
                                            Settings::write().refused = Some(format!(
                                                "Not saved: {key}: expected {expected}"
                                            ));
                                        }
                                    }),
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
fn field_with_hint(
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
fn text_button(
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

fn icon_button(
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
pub(super) fn link(
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
fn text_field(
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
fn outlined(
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
fn halo(
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
pub(super) fn button(
    control: Rectangle,
    id: impl Into<String>,
    press: impl Fn() + 'static,
    resting: Option<Color>,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    button_styled(control, id, press, resting, true, settings, roles)
}

pub(super) fn button_styled(
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

// a pressable that is its own edge, in a halo
pub(super) fn pressed(
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

fn restart(roles: &ThemeRoles) -> Text {
    Text::new("Restart to apply")
        .size(theme::text::LABEL_SMALL)
        .color(roles.primary)
        .weight(theme::text::SEMIBOLD)
}

// the line a setting shows under its name, saying when it takes a restart
fn help(setting: &Setting) -> String {
    let mut help = capitalized(setting.hint.unwrap_or(setting.help));
    if setting.restart {
        help.push_str(". Takes a restart");
    }

    help
}

// an option as the window names it: the schema's label, else its name with dashes as spaces
fn humanize(options: &Choices, option: &str) -> String {
    options
        .label(option)
        .map_or_else(|| humanized(option), str::to_owned)
}

// a name with dashes as spaces, as a sentence
fn humanized(name: &str) -> String {
    capitalized(&name.replace('-', " "))
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

// the gradient of Settings' mark without Kanade's icon
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
