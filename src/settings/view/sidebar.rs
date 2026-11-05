use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Column, Cursor, Image, Padding, Parent, Rectangle, Row, ScrollArea, Stack, Start, Text,
    TextInput, Widget, children,
};

use crate::icon::Icon;
use crate::settings::layout::{Page, visible_pages};
use crate::settings::ring::{self, Stop};
use crate::settings::{SEARCH, Settings, hover, search, turn_to};
use crate::sources::icons;
use crate::theme::{self, ThemeRoles, radius};

use super::{
    FIELD, backdrop, hairline, hover_container, hover_wash, left, outlined, scroll_id, sides,
};

pub(crate) const COMPACT_BREAKPOINT: f32 = 820.0;
pub(crate) const RAIL: f32 = 64.0;
pub(crate) const SIDEBAR: f32 = 256.0;
pub(crate) const SIDE_INSET: f32 = 16.0;
pub(crate) const NAV: f32 = SIDEBAR - 2.0 * SIDE_INSET;
pub(crate) const RAIL_NAV: f32 = 44.0;

pub(crate) fn is_compact() -> bool {
    let (width, _) = kanade_runtime::window_size();
    width > 0.0 && width < COMPACT_BREAKPOINT
}

pub(crate) fn sidebar_width() -> f32 {
    if is_compact() { RAIL } else { SIDEBAR }
}

// the brand, the search field, then the pages under their groups' headings, scrolled in a short window
pub(crate) fn sidebar(settings: &Settings, roles: &ThemeRoles) -> Rectangle {
    let search = search_field(settings, roles);

    // the pages are one stop: Up and Down turn to the one before or after
    let visible = visible_pages(crate::sources::compositor::active());
    let current = visible.clone();
    let adjacent = visible.clone();
    let at = visible
        .iter()
        .position(|page| page.name == settings.page)
        .unwrap_or(0);
    let pages = ring::register(
        Stop::new("pages")
            .press(move || turn_to(current[at].name))
            .step(move |direction| {
                let next = (at as i32 + direction).clamp(0, adjacent.len() as i32 - 1);

                turn_to(adjacent[next as usize].name);
            }),
    );

    let compact = is_compact();
    let mut items: Vec<Box<dyn Widget>> = Vec::new();

    let mut group = "";
    for page in visible_pages(crate::sources::compositor::active()) {
        if page.group != group {
            let prev = group;
            group = page.group;

            if compact {
                if !prev.is_empty() {
                    items.push(Box::new(
                        Rectangle::new()
                            .width(RAIL_NAV)
                            .height(9.0)
                            .padding(Padding {
                                top: 4.0,
                                right: 6.0,
                                bottom: 4.0,
                                left: 6.0,
                            })
                            .align_child(Center, Center)
                            .child(hairline(32.0, roles)),
                    ));
                }
            } else {
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
        }

        items.push(nav(page, settings, pages, roles));
    }

    let (sidebar_w, nav_w, padding) = if compact {
        (
            RAIL,
            RAIL_NAV,
            Padding {
                top: SIDE_INSET,
                right: (RAIL - RAIL_NAV) / 2.0,
                bottom: SIDE_INSET,
                left: (RAIL - RAIL_NAV) / 2.0,
            },
        )
    } else {
        (
            SIDEBAR,
            NAV,
            Padding {
                top: SIDE_INSET,
                right: SIDE_INSET,
                bottom: SIDE_INSET,
                left: SIDE_INSET,
            },
        )
    };

    Rectangle::new()
        .width(sidebar_w)
        .height(Parent)
        .fill(theme::over(roles.surface_container, roles.surface))
        .padding(padding)
        .align_child(Center, Start)
        .child(
            Column::new(children![
                brand(roles),
                search,
                ScrollArea::new(scroll_id("pages"), Column::new(items).width(nav_w).gap(4.0))
                    .width(Parent)
                    .height(Parent),
            ])
            .width(nav_w)
            .height(Parent)
            .gap(4.0),
        )
}

fn brand(roles: &ThemeRoles) -> Box<dyn Widget> {
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

    if is_compact() {
        Box::new(
            Rectangle::new()
                .width(RAIL_NAV)
                .height(44.0)
                .align_child(Center, Center)
                .child(mark),
        )
    } else {
        Box::new(
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
            .align(Center),
        )
    }
}

pub(crate) fn search_field(settings: &Settings, roles: &ThemeRoles) -> Box<dyn Widget> {
    let token = ring::register(Stop::new("search").input(SEARCH).press(ring::focus_search));

    if is_compact() {
        let hovered = settings.is_hovered("search");
        let fill = if hovered {
            hover_wash(roles)
        } else {
            roles.surface
        };

        let btn = Rectangle::new()
            .width(RAIL_NAV)
            .height(RAIL_NAV)
            .radius(radius::ROW)
            .fill(fill)
            .align_child(Center, Center)
            .cursor(Cursor::Pointer)
            .on_hover(|inside| hover("search", inside))
            .semantics(|| Semantics::new(Role::Button, "Search").label("Find a setting"))
            .on_click(left(ring::focus_search))
            .child(
                Stack::new(children![
                    Icon::Search.on(16.0, roles.on_surface_variant),
                    Rectangle::new().width(0.0).height(0.0).child(
                        TextInput::new(SEARCH)
                            .width(0.0)
                            .size(theme::text::LABEL)
                            .color(roles.surface)
                            .on_change(search),
                    ),
                ])
                .width(16.0)
                .height(16.0),
            );

        outlined(btn, token, Some(roles.edge()), roles)
    } else {
        outlined(
            Rectangle::new()
                .width(NAV)
                .height(FIELD)
                .radius(radius::ROW)
                .fill(roles.surface)
                .padding(sides(10.0))
                .align_child(Start, Center)
                .cursor(Cursor::Text)
                .on_click(left(ring::focus_search))
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
}

// a page down the side, filled while it shows; the ring is on the one that shows
pub(crate) fn nav(
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

    let (row, child): (Rectangle, Box<dyn Widget>) = if is_compact() {
        let row = Rectangle::new()
            .width(RAIL_NAV)
            .height(RAIL_NAV)
            .radius(radius::ROW)
            .align_child(Center, Center)
            .cursor(Cursor::Pointer)
            .on_hover(move |inside| hover(name, inside))
            .semantics(|| {
                Semantics::new(Role::Tab, name)
                    .label(page.title)
                    .selected(shown)
            })
            .on_click(left(move || turn_to(name)));
        (row, Box::new(page.icon.on(18.0, ink)))
    } else {
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
            .on_click(left(move || turn_to(name)));
        (
            row,
            Box::new(
                Row::new(children![
                    page.icon.on(16.0, ink),
                    Text::new(page.title)
                        .size(theme::text::LABEL)
                        .color(roles.on_surface)
                        .weight(weight),
                ])
                .gap(10.0)
                .align(Center),
            ),
        )
    };

    let mut row = row.child(child);

    if here {
        if hovered {
            row = row.fill(hover_container(roles));
        } else {
            row = row.fill(roles.surface_container_high);
        }
    } else if hovered {
        row = row.fill(hover_wash(roles));
    }

    outlined(row, ring.filter(|_| shown), None, roles)
}
