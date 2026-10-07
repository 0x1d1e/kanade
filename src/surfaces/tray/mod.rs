//! The Tray (#135): at Rest the pointer on the island raises the strip, the clock then a slot per
//! tray item, its icon taking the item's left, middle and right clicks and its wheel. The Tray
//! Surface lists them all, each with the chevron to its menu, a sub-surface a submenu deep. Past
//! the strip's slots the last opens it. Every target is a key away, a ring on the one the arrows
//! reach (`focus`).

use std::time::Instant;

use amane::{
    Button, Center, Column, Cursor, Image, Key, Parent, Rectangle, Row, Scroll, Service, Stack,
    Start, Text, Widget, children,
};

use self::focus::{Act, At, Focus};
use super::Ring;
use super::controls::list;
use crate::clock;
use crate::config;
use crate::icon::Icon;
use crate::island::geometry::{self, REST, TRAY_SLOT, TRAY_SLOTS};
use crate::island::presentation::{Presentation, Surface};
use crate::island::service::IslandService;
use crate::sources::tray::menu::{self, Entry, Menu, Toggle};
use crate::sources::tray::{self, Item, Orientation, Status, Tray};
use crate::theme::space::INSET;
use crate::theme::{self, DISABLED, radius};
use crate::view;

mod focus;

const WIDTH: f32 = geometry::CONTROLS.width - 2.0 * INSET;

// an icon in a strip slot
const SLOT_ICON: f32 = 20.0;

// a wheel line, in the deltas items take: a notch of a mouse wheel is 120
const NOTCH: f32 = 120.0;

// the items shown: a passive one has nothing to say right now
fn items() -> Vec<Item> {
    Tray::read()
        .items
        .iter()
        .filter(|item| item.status != Status::Passive)
        .cloned()
        .collect()
}

/*
 * the strip at Presentation::Tray: the clock, as at Rest, then the items' icons. Past the slots
 * the last says how many more and opens the Surface with them all
 */
pub fn strip(monitor: &str, presentation: Presentation) -> Option<Rectangle> {
    let Presentation::Tray(slots) = presentation else {
        return None;
    };

    let items = items();
    let slots = usize::from(slots);
    let over = items.len() > TRAY_SLOTS;

    let mut row = children![
        Rectangle::new()
            .width(REST.width)
            .height(Parent)
            .align_child(Center, Center)
            .child(
                Text::new(clock::now(config::get().clock))
                    .size(theme::text::LABEL)
                    .color(theme::ISLAND.on_surface)
                    .weight(theme::text::SEMIBOLD),
            )
    ];

    for (index, item) in items.iter().take(slots).enumerate() {
        if over && index == slots - 1 {
            row.push(Box::new(more(monitor, items.len() - index)));
        } else {
            row.push(Box::new(slot(monitor, item)));
        }
    }

    let shape = geometry::shape(presentation);

    Some(
        Rectangle::new()
            .width(shape.width)
            .height(shape.height)
            .align_child(Start, Center)
            .child(Row::new(row).height(shape.height).align(Center)),
    )
}

// an item's icon, which its clicks and wheel reach
fn slot(monitor: &str, item: &Item) -> Rectangle {
    let clicked = (monitor.to_owned(), item.clone());
    let scrolled = item.clone();

    Rectangle::new()
        .width(TRAY_SLOT)
        .height(TRAY_SLOT)
        .radius(radius::ICON)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(move |button| {
            let (monitor, item) = &clicked;

            match button {
                Button::Left if item.is_menu && item.has_menu() => open(monitor, item),
                Button::Left => tray::activate(item, 0, 0),
                Button::Middle => tray::secondary_activate(item, 0, 0),
                Button::Right if item.has_menu() => open(monitor, item),
                Button::Right => tray::context_menu(item, 0, 0),
            }
        })
        .on_scroll(move |Scroll { x, y }| {
            // a wheel down is a negative delta, as for a mouse wheel's notch
            if y != 0.0 {
                tray::scroll(
                    &scrolled,
                    (-y * NOTCH).round() as i32,
                    Orientation::Vertical,
                );
            }

            if x != 0.0 {
                tray::scroll(
                    &scrolled,
                    (-x * NOTCH).round() as i32,
                    Orientation::Horizontal,
                );
            }
        })
        .child(Row::new(vec![icon(item, SLOT_ICON)]))
}

// the slot past the others, "+N", which opens the Surface with them all
fn more(monitor: &str, count: usize) -> Rectangle {
    let monitor = monitor.to_owned();

    Rectangle::new()
        .width(TRAY_SLOT)
        .height(TRAY_SLOT)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(move |button| {
            if button == Button::Left {
                view::open(&monitor, Surface::Tray, false);
            }
        })
        .child(
            Text::new(format!("+{count}"))
                .size(theme::text::LABEL_SMALL)
                .color(theme::ISLAND.on_surface_variant)
                .weight(theme::text::SEMIBOLD),
        )
}

// the Surface open on `item`'s menu, pinned, as its menu would stay up until dismissed
fn open(monitor: &str, item: &Item) {
    view::open(monitor, Surface::Tray, true);

    let visit = IslandService::read().visit();
    let focus = Focus::read().of(visit, false);

    set(focus.into_menu(item.key()));
    menu::open(item);
}

// the icon an item gives: its file, else its own pixels, else its title's initial
fn icon(item: &Item, side: f32) -> Box<dyn Widget> {
    let Some(path) = item.icon.file.as_ref().or(item.icon.pixmap.as_ref()) else {
        let initial = item
            .title
            .chars()
            .next()
            .map_or_else(String::new, |initial| initial.to_uppercase().collect());

        return Box::new(view::tile(
            None,
            &initial,
            side,
            radius::ICON,
            &theme::ISLAND,
        ));
    };

    // decoded at twice its size, crisp at scale 2
    let pixels = (side * 2.0) as u32;

    Box::new(
        Rectangle::new()
            .width(side)
            .height(side)
            .fill(Image::contain(path.clone()).thumbnail(pixels, pixels)),
    )
}

/*
 * the items, or the menu of one a submenu deep. `open` says the Surface is open rather than fading
 * out, so only then does the ring show
 */
pub fn surface(open: bool, visit: u64, held: bool) -> Rectangle {
    let shape = geometry::CONTROLS;
    let items = items();
    let menu = Menu::read().clone();

    let focus = level(Focus::read().of(visit, held), &items, &menu);
    let grid = focus.grid(rows(&focus, &items, &menu));
    let focus = settled(focus, &grid);
    let ring = open.then(|| focus.ring(&grid)).flatten();

    let content = match focus.item {
        None => listing(&items, &focus, ring.as_ref()),
        Some(key) => {
            let item = items.iter().find(|item| item.key() == key);
            let title = item.map_or("", |item| item.title.as_str());

            menu_listing(title, node(&focus, &menu), &menu, &focus, ring.as_ref())
        }
    };

    Rectangle::new()
        .width(shape.width)
        .height(shape.height)
        .padding(INSET)
        .align_child(Start, Start)
        .child(content)
}

fn listing(items: &[Item], focus: &Focus, ring: Option<&At>) -> Column {
    let header = Row::new(children![
        Text::new("Tray")
            .size(theme::text::TITLE)
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::SEMIBOLD)
    ])
    .width(WIDTH)
    .height(list::HEADER)
    .align(Center);

    let body: Box<dyn Widget> = if items.is_empty() {
        Box::new(list::state(Icon::Check, "Nothing in the tray", ""))
    } else {
        let items = items.to_vec();
        let ring = ring.cloned();

        Box::new(list::scrolling(
            items.len(),
            focus.offset,
            scroll,
            move |index| Box::new(item_row(&items[index], ring.as_ref())),
        ))
    };

    Column::new(vec![Box::new(header), body])
        .width(WIDTH)
        .gap(list::GAP)
}

// an item: its icon and title, pressed to activate it, then the chevron to its menu
fn item_row(item: &Item, ring: Option<&At>) -> Rectangle {
    let key = item.key();
    let mut parts = vec![
        icon(item, list::ICON),
        Box::new(
            Column::new(children![
                Text::new(&item.title)
                    .size(theme::text::BODY)
                    .color(theme::ISLAND.on_surface)
                    .weight(theme::text::SEMIBOLD)
                    .elide()
            ])
            .width(Parent),
        ),
    ];

    if item.has_menu() {
        parts.push(Box::new(
            Rectangle::new()
                .width(theme::space::TARGET)
                .height(theme::space::TARGET)
                .radius(theme::space::TARGET / 2.0)
                .align_child(Center, Center)
                .cursor(Cursor::Pointer)
                .border_if(ring == Some(&At::Menu(key)))
                .on_click(super::on_left(move || click(Act::Press(At::Menu(key)))))
                .child(Icon::Forward.draw(18.0)),
        ));
    }

    list::frame(
        Row::new(parts)
            .width(Parent)
            .gap(list::ICON_GAP)
            .align(Center),
        ring == Some(&At::Item(key)),
    )
    .cursor(Cursor::Pointer)
    .on_click(super::on_left(move || click(Act::Press(At::Item(key)))))
}

// a menu level: the back chevron and the item's title, then its entries
fn menu_listing(
    title: &str,
    node: Option<&Entry>,
    menu: &Menu,
    focus: &Focus,
    ring: Option<&At>,
) -> Column {
    let title = node
        .filter(|_| !focus.entered.is_empty())
        .map_or(title, |node| node.label.as_str());

    let header = Row::new(children![
        Rectangle::new()
            .width(theme::space::TARGET)
            .height(theme::space::TARGET)
            .radius(theme::space::TARGET / 2.0)
            .align_child(Center, Center)
            .cursor(Cursor::Pointer)
            .border_if(ring == Some(&At::Back))
            .on_click(super::on_left(|| click(Act::Press(At::Back))))
            .child(Icon::Back.draw(18.0)),
        Text::new(title)
            .size(theme::text::TITLE)
            .color(theme::ISLAND.on_surface)
            .weight(theme::text::SEMIBOLD)
            .elide()
    ])
    .width(WIDTH)
    .height(list::HEADER)
    .gap(8.0)
    .align(Center);

    let entries = node.map(shown).unwrap_or_default();

    let body: Box<dyn Widget> = match node {
        None if menu.failed => Box::new(list::state(Icon::Back, "The menu did not open", "")),
        None => Box::new(list::state(Icon::Forward, "Opening", "")),
        Some(_) if entries.is_empty() => Box::new(list::state(Icon::Check, "Nothing in it", "")),
        Some(_) => {
            let ring = ring.cloned();

            Box::new(list::scrolling(
                entries.len(),
                focus.offset,
                scroll,
                move |index| {
                    let (entry, ruled) = &entries[index];

                    Box::new(entry_row(entry, *ruled, ring.as_ref()))
                },
            ))
        }
    };

    Column::new(vec![Box::new(header), body])
        .width(WIDTH)
        .gap(list::GAP)
}

/*
 * a level's entries as rows, each with whether a separator stood above it, which then draws as a
 * rule at its top
 */
fn shown(node: &Entry) -> Vec<(Entry, bool)> {
    let mut rows = Vec::new();
    let mut ruled = false;

    for entry in &node.children {
        if entry.separator {
            ruled = !rows.is_empty();
        } else {
            rows.push((entry.clone(), ruled));
            ruled = false;
        }
    }

    rows
}

// an entry: its mark if it is checked, its label, and a chevron if it opens a submenu
fn entry_row(entry: &Entry, ruled: bool, ring: Option<&At>) -> Rectangle {
    let id = entry.id;

    let checked = matches!(entry.toggle, Toggle::Check(true) | Toggle::Radio(true));
    let mark: Box<dyn Widget> = if checked {
        Box::new(Icon::Check.draw(list::ICON))
    } else {
        Box::new(Rectangle::new().width(list::ICON).height(list::ICON))
    };

    let mut parts = vec![
        mark,
        Box::new(
            Column::new(children![
                Text::new(&entry.label)
                    .size(theme::text::BODY)
                    .color(theme::ISLAND.on_surface)
                    .weight(theme::text::MEDIUM)
                    .elide()
            ])
            .width(Parent),
        ),
    ];

    if entry.submenu {
        parts.push(Box::new(Icon::Forward.draw(18.0)));
    }

    let mut layers = children![list::frame(
        Row::new(parts)
            .width(Parent)
            .gap(list::ICON_GAP)
            .align(Center),
        ring == Some(&At::Entry(id)),
    )];

    // the separator, in the middle of the gap above
    if ruled {
        layers.push(Box::new(
            Rectangle::new()
                .width(WIDTH - 2.0 * list::ROW_INSET)
                .height(1.0)
                .fill(theme::ISLAND.surface_container_high)
                .translate(list::ROW_INSET, -(list::ROW_GAP + 1.0) / 2.0),
        ));
    }

    let row = Rectangle::new()
        .width(WIDTH)
        .height(list::ROW)
        .align_child(Start, Start)
        .child(Stack::new(layers).width(WIDTH).height(list::ROW));

    if !entry.enabled {
        return row.opacity(DISABLED);
    }

    row.cursor(Cursor::Pointer)
        .on_click(super::on_left(move || click(Act::Press(At::Entry(id)))))
}

// the level `focus` is on, its menu's item, or a submenu, since gone leaving the levels above
fn level(focus: Focus, items: &[Item], menu: &Menu) -> Focus {
    let Some(key) = focus.item else {
        return focus;
    };

    if !items.iter().any(|item| item.key() == key) {
        return focus.without_menu();
    }

    let Some(root) = menu.of(key) else {
        return focus;
    };

    let depth = focus
        .entered
        .iter()
        .scan(root, |node, id| {
            let entry = node
                .children
                .iter()
                .find(|entry| entry.id == *id && entry.submenu)?;
            *node = entry;
            Some(())
        })
        .count();

    focus.within(depth)
}

// the entry the focus's level lists, none while the menu is not read
fn node<'a>(focus: &Focus, menu: &'a Menu) -> Option<&'a Entry> {
    let mut node = menu.of(focus.item?)?;

    for id in &focus.entered {
        node = node.children.iter().find(|entry| entry.id == *id)?;
    }

    Some(node)
}

// the targets the level lists under its header, read now
fn rows(focus: &Focus, items: &[Item], menu: &Menu) -> Vec<Vec<(At, f32)>> {
    if focus.item.is_none() {
        return items
            .iter()
            .map(|item| {
                if item.has_menu() {
                    vec![(At::Item(item.key()), 0.45), (At::Menu(item.key()), 0.97)]
                } else {
                    vec![(At::Item(item.key()), 0.45)]
                }
            })
            .collect();
    }

    node(focus, menu)
        .map(|node| {
            shown(node)
                .into_iter()
                .map(|(entry, _)| vec![(At::Entry(entry.id), 0.5)])
                .collect()
        })
        .unwrap_or_default()
}

/*
 * a key while this island shows the Tray Surface; false for one it does not use, as Escape at the
 * items, which the window's own keys then get to close it
 */
pub fn key(monitor: &str, key: Key) -> bool {
    let (visit, held) = {
        let island = IslandService::read();

        if island.presentation(monitor) != Presentation::Expanded(Surface::Tray) {
            return false;
        }

        (island.visit(), island.held(monitor))
    };

    let items = items();
    let menu = Menu::read().clone();
    let focus = level(Focus::read().of(visit, held), &items, &menu);
    let grid = focus.grid(rows(&focus, &items, &menu));
    let focus = settled(focus, &grid);

    let Some((focus, act)) = focus.step(key, &grid) else {
        return false;
    };

    let focus = match act {
        Some(act) => self::act(focus, act, &items, &menu),
        None => focus,
    };

    set(focus);

    IslandService::write().attend(monitor, Instant::now());

    true
}

fn click(act: Act) {
    let visit = IslandService::read().visit();
    let items = items();
    let menu = Menu::read().clone();
    let focus = level(Focus::read().of(visit, false), &items, &menu).hidden();

    set(self::act(focus, act, &items, &menu));
}

// what a key or click asks for done, giving where that leaves the focus
fn act(focus: Focus, act: Act, items: &[Item], menu: &Menu) -> Focus {
    let item = |key: u64| items.iter().find(|item| item.key() == key);

    match act {
        Act::Press(At::Back) => focus.out(),
        Act::Press(At::Item(key)) => {
            let Some(item) = item(key) else {
                return focus;
            };

            if item.is_menu && item.has_menu() {
                menu::open(item);
                return focus.into_menu(key);
            }

            tray::activate(item, 0, 0);
            close();
            focus
        }
        Act::Press(At::Menu(key)) => {
            let Some(item) = item(key) else {
                return focus;
            };

            menu::open(item);
            focus.into_menu(key)
        }
        Act::Press(At::Entry(id)) | Act::Enter(id) => {
            let (Some(item), Some(entry)) = (
                focus.item.and_then(item),
                node(&focus, menu).and_then(|node| node.find(id)),
            ) else {
                return focus;
            };

            if !entry.enabled {
                return focus;
            }

            if entry.submenu {
                menu::enter(item, id);
                return focus.into_submenu(id);
            }

            // Right only enters; an action runs on Enter or a press
            if matches!(act, Act::Enter(_)) {
                return focus;
            }

            menu::click(item, id);
            close();
            focus
        }
    }
}

// the Surface done with, as a menu closes once an action in it runs
fn close() {
    let monitor = IslandService::read().expanded_on().map(str::to_owned);

    if let Some(monitor) = monitor {
        view::collapse(&monitor);
    }
}

// the rows scrolled by `pixels`, down further down; the ring hides, the pointer moving now
fn scroll(pixels: f32) {
    let visit = IslandService::read().visit();
    let items = items();
    let menu = Menu::read().clone();
    let focus = level(Focus::read().of(visit, false), &items, &menu);
    let grid = focus.grid(rows(&focus, &items, &menu));
    let mut focus = settled(focus, &grid).hidden();

    let most = list::most(grid.len().saturating_sub(focus.header()));
    focus.offset = (focus.offset + pixels).clamp(0.0, most);

    set(focus);
}

// `focus` scrolled to where its rows show, the ringed one whole
fn settled(mut focus: Focus, grid: &[Vec<(At, f32)>]) -> Focus {
    let count = grid.len().saturating_sub(focus.header());

    focus.offset = list::scrolled(focus.offset, count, focus.row(grid));
    focus
}

// a write wakes the window even when nothing changed, so only write a real change
fn set(focus: Focus) {
    if *Focus::read() != focus {
        *Focus::write() = focus;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: i32, separator: bool) -> Entry {
        Entry {
            id,
            label: format!("{id}"),
            enabled: true,
            separator,
            toggle: Toggle::None,
            submenu: false,
            children: Vec::new(),
        }
    }

    #[test]
    fn a_separator_rules_the_entry_under_it_only_between_entries() {
        let node = Entry {
            children: vec![
                entry(1, true),
                entry(2, false),
                entry(3, true),
                entry(4, true),
                entry(5, false),
                entry(6, true),
            ],
            ..entry(0, false)
        };

        let rows: Vec<(i32, bool)> = shown(&node)
            .into_iter()
            .map(|(entry, ruled)| (entry.id, ruled))
            .collect();

        assert_eq!(rows, [(2, false), (5, true)]);
    }
}
