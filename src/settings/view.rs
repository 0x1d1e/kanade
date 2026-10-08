//! The Settings window's view: the pages down the side, the page's settings as cards beside them.
//! Each card names its key as a config file does, says what it does, and draws the control its
//! Kind takes; one the settings file sets has a Reset, which hands the key back to the layers below.

use amane::{
    Align, Button, Center, Column, Cursor, End, Padding, Parent, Rectangle, Row, ScrollArea,
    Service, Size, SpaceBetween, Start, Text, TextInput, Widget, Window, children,
};
use toml::Value;

use crate::config::{self, Kind, Setting};
use crate::modules;
use crate::theme::space::{INSET, TARGET};
use crate::theme::{self, ThemeRoles, radius};

use super::{Settings, change, input, pages, path, turn_to};

// fixed, so niri floats it as the dialog it is
const WIDTH: f32 = 760.0;
const HEIGHT: f32 = 560.0;

const SIDEBAR: f32 = 180.0;

// the cards' column, a gutter short of the page so a scrolled card clears the edge
const CARDS: f32 = WIDTH - SIDEBAR - 2.0 * INSET - 8.0;
const CARD_INSET: f32 = 16.0;
const INNER: f32 = CARDS - 2.0 * CARD_INSET;

const PAGE_ROW: f32 = 36.0;
const FIELD: f32 = 36.0;
const LINE: f32 = 32.0;

// a press of `-` or `+` on a duration
const STEP: u64 = 10;

pub fn window() -> Window {
    let settings = Settings::read();
    let roles = theme::roles();

    Window::new()
        .title("Kanade Settings")
        .size(WIDTH, HEIGHT)
        .resizable(false)
        .child(
            Rectangle::new()
                .width(Parent)
                .height(Parent)
                .fill(roles.surface)
                .child(
                    Row::new(children![
                        sidebar(&settings, &roles),
                        page(&settings, &roles)
                    ])
                    .width(Parent)
                    .height(Parent),
                ),
        )
}

fn sidebar(settings: &Settings, roles: &ThemeRoles) -> Rectangle {
    let rows = pages()
        .map(|module| {
            let here = module.name == settings.page;
            let name = module.name;

            let row = Rectangle::new()
                .width(Parent)
                .height(PAGE_ROW)
                .radius(radius::ROW)
                .padding(sides(12.0))
                .align_child(Start, Center)
                .cursor(Cursor::Pointer)
                .on_click(move |button| {
                    if button == Button::Left {
                        turn_to(name);
                    }
                })
                .child(
                    Text::new(title(name))
                        .size(theme::text::BODY)
                        .color(roles.on_surface)
                        .weight(if here {
                            theme::text::SEMIBOLD
                        } else {
                            theme::text::MEDIUM
                        }),
                );

            let row = if here {
                row.fill(roles.surface_container_high)
            } else {
                row
            };

            Box::new(row) as Box<dyn Widget>
        })
        .collect();

    Rectangle::new()
        .width(SIDEBAR)
        .height(Parent)
        .fill(roles.surface_container)
        .padding(12.0)
        .align_child(Start, Start)
        .child(Column::new(rows).width(Parent).gap(4.0))
}

fn page(settings: &Settings, roles: &ThemeRoles) -> Rectangle {
    let module = pages()
        .find(|module| module.name == settings.page)
        .or_else(|| pages().next())
        .expect("a Module owns settings");

    let mut top = children![
        Text::new(title(module.name))
            .size(theme::text::TITLE_LARGE)
            .color(roles.on_surface)
            .weight(theme::text::SEMIBOLD),
    ];

    let notices = [
        (!modules::on(module.name)).then(|| {
            format!(
                "The {} module is off: these apply once it is turned on, after a restart",
                module.name
            )
        }),
        settings.unreadable.clone(),
        settings.refused.clone(),
        settings
            .error
            .as_ref()
            .map(|error| format!("The config was not reloaded: {error}"))
            .or_else(|| {
                (!settings.skipped.is_empty())
                    .then(|| format!("Skipped in the config: {}", settings.skipped.join("; ")))
            }),
    ];
    for notice in notices.into_iter().flatten() {
        top.push(Box::new(wrapped(
            Text::new(notice)
                .size(theme::text::LABEL)
                .color(theme::SEMANTIC.warning)
                .weight(theme::text::MEDIUM),
            CARDS,
        )));
    }

    let cards = module
        .settings
        .iter()
        .map(|setting| Box::new(card(settings, setting, roles)) as Box<dyn Widget>)
        .collect();

    top.push(Box::new(
        ScrollArea::new("settings cards", Column::new(cards).width(CARDS).gap(12.0))
            .width(Parent)
            .height(Parent),
    ));

    Rectangle::new()
        .width(Parent)
        .height(Parent)
        .padding(INSET)
        .align_child(Start, Start)
        .child(Column::new(top).width(Parent).height(Parent).gap(12.0))
}

/*
 * a setting: its key, Reset while the settings file sets it, and whether its new value waits for a
 * restart; what it does, then its control
 */
fn card(settings: &Settings, setting: &'static Setting, roles: &ThemeRoles) -> Rectangle {
    let key_path = path(setting.key, None);

    let mut marks: Vec<Box<dyn Widget>> = Vec::new();
    if setting.restart && settings.pending(setting.key) {
        marks.push(Box::new(restart(roles)));
    }
    if settings.set_here(&key_path) {
        marks.push(Box::new(button("Reset", roles, move || {
            change(key_path.clone(), None)
        })));
    }

    let mut help = String::from(setting.help);
    if setting.restart {
        help.push_str("; takes a restart");
    }

    let mut items = children![
        Row::new(children![
            Text::new(setting.key)
                .size(theme::text::BODY)
                .color(roles.on_surface)
                .weight(theme::text::SEMIBOLD),
            Row::new(marks).gap(8.0).align(Center),
        ])
        .width(INNER)
        .height(TARGET)
        .justify(SpaceBetween)
        .align(Center),
        wrapped(
            Text::new(help)
                .size(theme::text::LABEL)
                .color(roles.on_surface_variant)
                .weight(theme::text::MEDIUM),
            INNER,
        ),
    ];
    items.push(control(settings, setting, roles));

    let column = Column::new(items).width(INNER).gap(8.0);
    let height = fixed(Widget::height(&column)) + 2.0 * CARD_INSET;

    Rectangle::new()
        .width(CARDS)
        .height(height)
        .radius(radius::CARD)
        .fill(roles.surface_container)
        .padding(CARD_INSET)
        .align_child(Start, Start)
        .child(column)
}

// what sets the key, by its Kind, showing the value the files give
fn control(settings: &Settings, setting: &'static Setting, roles: &ThemeRoles) -> Box<dyn Widget> {
    let read = &settings.read;
    let key = setting.key;

    match &setting.kind {
        Kind::Switch(field) => {
            let on = (field.get)(read);

            Box::new(switch(on, roles, move || {
                change(path(key, None), Some(Value::Boolean(!on)));
            }))
        }
        Kind::Choice(options, field) => {
            let chosen = (field.get)(read);

            Box::new(
                Row::new(
                    options
                        .iter()
                        .map(|&option| {
                            Box::new(choice(option, option == chosen, roles, move || {
                                change(path(key, None), Some(Value::String(option.to_owned())));
                            })) as Box<dyn Widget>
                        })
                        .collect(),
                )
                .gap(8.0),
            )
        }
        Kind::Millis(field) => {
            let ms = u64::try_from((field.get)(read).as_millis()).unwrap_or(config::LONGEST);
            let to = |ms: u64| {
                move || {
                    let ms = ms.clamp(config::SHORTEST, config::LONGEST);
                    change(path(key, None), Some(Value::Integer(ms as i64)));
                }
            };

            Box::new(
                Row::new(children![
                    button("\u{2212}", roles, to(ms.saturating_sub(STEP))),
                    Rectangle::new()
                        .width(88.0)
                        .height(TARGET)
                        .align_child(Center, Center)
                        .child(
                            Text::new(format!("{ms} ms"))
                                .size(theme::text::BODY)
                                .color(roles.on_surface)
                                .weight(theme::text::MEDIUM),
                        ),
                    button("+", roles, to(ms + STEP)),
                ])
                .gap(8.0)
                .align(Center),
            )
        }
        Kind::Path(_) => Box::new(
            Column::new(children![
                text_field(
                    input(setting),
                    setting.example.unwrap_or("").trim_matches('"'),
                    roles,
                    move |text| {
                        let text = text.trim();
                        let value = (!text.is_empty()).then(|| Value::String(text.to_owned()));

                        change(path(key, None), value);
                    },
                ),
                hint("Enter saves; an empty field removes this override", roles),
            ])
            .width(INNER)
            .gap(6.0),
        ),
        Kind::DesktopIds(field) => {
            let ids = (field.get)(read);

            let mut rows: Vec<Box<dyn Widget>> = ids
                .iter()
                .enumerate()
                .map(|(index, id)| {
                    let list = ids.clone();
                    let up = (index > 0).then(|| {
                        let mut list = list.clone();
                        list.swap(index - 1, index);
                        move || change(path(key, None), Some(strings(&list)))
                    });

                    let mut without = list;
                    without.remove(index);

                    let mut actions: Vec<Box<dyn Widget>> = Vec::new();
                    if let Some(up) = up {
                        actions.push(Box::new(button("Up", roles, up)));
                    }
                    actions.push(Box::new(button("Remove", roles, move || {
                        change(path(key, None), Some(strings(&without)));
                    })));

                    Box::new(line(id, actions, roles)) as Box<dyn Widget>
                })
                .collect();

            rows.push(Box::new(text_field(
                input(setting),
                "Add a .desktop file id",
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

            Box::new(Column::new(rows).width(INNER).gap(6.0))
        }
        Kind::AppIds(field) => {
            let mut rows: Vec<Box<dyn Widget>> = (field.get)(read)
                .into_iter()
                .map(|(app_id, id)| {
                    let entry = path(key, Some(&app_id));

                    // one from the user's own config is theirs to remove
                    let actions = if settings.set_here(&entry) {
                        children![button("Remove", roles, move || change(entry.clone(), None))]
                    } else {
                        children![]
                    };

                    Box::new(line(&format!("{app_id} \u{2192} {id}"), actions, roles))
                        as Box<dyn Widget>
                })
                .collect();

            rows.push(Box::new(text_field(
                input(setting),
                "app id = .desktop file id",
                roles,
                move |text| match text.split_once('=') {
                    Some((app_id, id)) if !app_id.trim().is_empty() => change(
                        path(key, Some(app_id.trim())),
                        Some(Value::String(id.trim().to_owned())),
                    ),
                    _ => {
                        Settings::write().refused =
                            Some(format!("{key}: expected app id = .desktop file id"));
                    }
                },
            )));

            Box::new(Column::new(rows).width(INNER).gap(6.0))
        }
        // every Module but the core, which cannot be turned off
        Kind::Modules(field) => {
            let off = (field.get)(read);

            Box::new(
                Column::new(
                    modules::ALL
                        .iter()
                        .filter(|module| module.name != modules::CORE)
                        .map(|module| {
                            let name = module.name;
                            let on = !off.contains(&name);

                            let mut actions: Vec<Box<dyn Widget>> = Vec::new();
                            if settings.pending(&format!("{key}.{name}")) {
                                actions.push(Box::new(restart(roles)));
                            }
                            actions.push(Box::new(switch(on, roles, move || {
                                change(path(key, Some(name)), Some(Value::Boolean(!on)));
                            })));

                            Box::new(line(name, actions, roles)) as Box<dyn Widget>
                        })
                        .collect(),
                )
                .width(INNER)
                .gap(2.0),
            )
        }
    }
}

// a list's text with its actions at the end
fn line(text: &str, actions: Vec<Box<dyn Widget>>, roles: &ThemeRoles) -> Row {
    Row::new(children![
        Text::new(text)
            .size(theme::text::BODY)
            .color(roles.on_surface)
            .weight(theme::text::MEDIUM)
            .elide(),
        Row::new(actions).gap(8.0).align(Center),
    ])
    .width(INNER)
    .height(LINE)
    .gap(12.0)
    .align(Center)
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

// one option of a Choice, filled while chosen
fn choice(option: &str, chosen: bool, roles: &ThemeRoles, press: impl Fn() + 'static) -> Rectangle {
    let pill = Rectangle::new()
        .width(72.0)
        .height(32.0)
        .radius(16.0)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(left(press));

    let (pill, color) = if chosen {
        (pill.fill(roles.primary), roles.on_primary)
    } else {
        (pill.border(1.0, roles.outline), roles.on_surface)
    };

    pill.child(
        Text::new(option)
            .size(theme::text::BODY)
            .color(color)
            .weight(theme::text::SEMIBOLD),
    )
}

// an outlined pill with a word or sign, as wide as it needs
fn button(label: &str, roles: &ThemeRoles, press: impl Fn() + 'static) -> Rectangle {
    let text = Text::new(label)
        .size(theme::text::LABEL)
        .color(roles.on_surface)
        .weight(theme::text::SEMIBOLD);

    let width = (fixed(Widget::width(&text)) + 24.0).max(TARGET + 8.0);

    Rectangle::new()
        .width(width)
        .height(TARGET)
        .radius(TARGET / 2.0)
        .border(1.0, roles.outline)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_click(left(press))
        .child(text)
}

// a text input, saved by Enter
fn text_field(
    id: &'static str,
    placeholder: &str,
    roles: &ThemeRoles,
    submit: impl Fn(String) + 'static,
) -> Rectangle {
    Rectangle::new()
        .width(INNER)
        .height(FIELD)
        .radius(radius::ROW)
        .fill(roles.surface_container_high)
        .padding(sides(12.0))
        .align_child(Start, Center)
        .child(
            TextInput::new(id)
                .width(Parent)
                .size(theme::text::BODY)
                .color(roles.on_surface)
                .placeholder(placeholder)
                .on_submit(submit),
        )
}

// text wrapped to `width`, as tall as its lines
fn wrapped(text: Text, width: f32) -> Rectangle {
    let text = text.wrap();
    let height = text.height_in(width);

    Rectangle::new().width(width).height(height).child(text)
}

fn hint(text: &str, roles: &ThemeRoles) -> Text {
    Text::new(text)
        .size(theme::text::LABEL_SMALL)
        .color(roles.on_surface_variant)
        .weight(theme::text::MEDIUM)
}

fn restart(roles: &ThemeRoles) -> Text {
    Text::new("Restart to apply")
        .size(theme::text::LABEL_SMALL)
        .color(roles.primary)
        .weight(theme::text::SEMIBOLD)
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

// a Module's name as a page's title
fn title(name: &str) -> String {
    let mut letters = name.chars();

    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
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
