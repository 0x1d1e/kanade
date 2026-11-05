use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Center, Color, Cursor, Parent, Rectangle, Row, SpaceBetween, Start, Text, Widget, children,
};

use crate::config;
use crate::icon::Icon;
use crate::settings::layout::MODULES;
use crate::settings::ring::{self, Stop};
use crate::settings::{Settings, hover, inspect, niri, umbriel, undo};
use crate::theme::{self, ThemeRoles, radius};

use super::{
    BAR, PAD, fixed, hover_container, hover_wash, humanized, left, niri_page, outlined,
    outputs_page, pill, setting, sides, umbriel_page,
};

// the footer's status line and ink
pub(crate) fn footer_status(settings: &Settings, roles: &ThemeRoles) -> (String, Color) {
    let in_search = settings.searching();
    let on_displays = !in_search && settings.page == outputs_page::PAGE;
    let on_niri = !in_search && settings.page == niri::PAGE;
    let on_umbriel = !in_search && settings.page == umbriel::PAGE;

    let in_trial = settings.outputs.trial.is_some();
    let show_niri_staged = (on_niri || in_search) && settings.niri.has_staged();
    let show_umbriel_staged = (on_umbriel || in_search) && settings.umbriel.has_staged();
    let waiting = settings.pending.len();

    if in_trial {
        outputs_page::status(settings, roles)
            .unwrap_or_else(|| (String::from("Keep display changes?"), roles.primary))
    } else if show_niri_staged {
        niri_page::status(settings, roles)
    } else if show_umbriel_staged {
        umbriel_page::status(settings, roles)
    } else if on_displays {
        if let Some(status) = outputs_page::status(settings, roles) {
            status
        } else {
            (
                config::settings_file().map_or_else(
                    || String::from("Changes are written as you make them"),
                    |file| format!("Changes are saved to {}", shortened(&file)),
                ),
                roles.on_surface_variant,
            )
        }
    } else if on_niri {
        niri_page::status(settings, roles)
    } else if on_umbriel {
        umbriel_page::status(settings, roles)
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
        let changes = if waiting == 1 { "change" } else { "changes" };

        (
            format!("{waiting} {changes} pending restart"),
            roles.primary,
        )
    } else if let Some(last) = settings.history.last() {
        let name = last.first().and_then(|saved| setting_name(&saved.path));
        let text = match name {
            Some(name) => format!("Saved automatically: {name}"),
            None => String::from("Saved automatically"),
        };
        (text, roles.on_surface_variant)
    } else {
        (
            config::settings_file().map_or_else(
                || String::from("Changes are written as you make them"),
                |file| format!("Changes are saved to {}", shortened(&file)),
            ),
            roles.on_surface_variant,
        )
    }
}

// what the files say: a refusal, a problem, a restart waiting, else the last change or where changes go; and Undo
pub(crate) fn footer(settings: &Settings, roles: &ThemeRoles) -> Rectangle {
    let in_search = settings.searching();
    let on_displays = !in_search && settings.page == outputs_page::PAGE;
    let on_niri = !in_search && settings.page == niri::PAGE;
    let on_umbriel = !in_search && settings.page == umbriel::PAGE;

    let in_trial = settings.outputs.trial.is_some();
    let show_niri_staged = (on_niri || in_search) && settings.niri.has_staged();
    let show_umbriel_staged = (on_umbriel || in_search) && settings.umbriel.has_staged();

    let (status, ink) = footer_status(settings, roles);

    let mut actions: Vec<Box<dyn Widget>> = Vec::new();

    if in_trial || on_displays {
        actions.extend(outputs_page::actions(settings, roles));
    }

    if show_niri_staged {
        let enabled = !settings.niri.is_saving();

        actions.push(pill(
            "discard",
            "Discard",
            false,
            enabled,
            settings,
            roles,
            niri::discard,
        ));
        actions.push(pill(
            "save",
            "Save",
            true,
            enabled,
            settings,
            roles,
            niri::save,
        ));
    } else if show_umbriel_staged {
        let enabled = !settings.umbriel.is_saving();

        actions.push(pill(
            "discard",
            "Discard",
            false,
            enabled,
            settings,
            roles,
            umbriel::discard,
        ));
        actions.push(pill(
            "save",
            "Save",
            true,
            enabled,
            settings,
            roles,
            umbriel::save,
        ));
    } else if !in_trial {
        let (undoable, undo_fn): (bool, fn()) = if on_niri {
            (settings.niri.undoable, niri::undo)
        } else if on_umbriel {
            (settings.umbriel.undoable, umbriel::undo)
        } else {
            (!settings.history.is_empty(), undo)
        };

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
                .on_click(left(undo_fn))
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
                ring::register(Stop::new("undo").press(undo_fn)),
                Some(roles.edge()),
                roles,
            ));
        }
    }

    // Show technical details: secondary, out of the header bar; toggles each row's key and help
    let technical_id = "show technical details";
    let technical_hovered = settings.is_hovered(technical_id);
    let technical_text = Text::new("Show technical details")
        .size(theme::text::LABEL)
        .color(roles.on_surface)
        .weight(theme::text::SEMIBOLD);
    let technical_width = 14.0 + 6.0 + fixed(Widget::width(&technical_text)) + 12.0;
    let technical_button = Rectangle::new()
        .width(technical_width)
        .height(30.0)
        .radius(radius::ROW)
        .align_child(Center, Center)
        .cursor(Cursor::Pointer)
        .on_hover(move |inside| hover(technical_id, inside))
        .semantics(|| Semantics::new(Role::Toggle, technical_id).checked(settings.inspect))
        .on_click(left(inspect))
        .child(
            Row::new(children![
                Icon::Sliders.on(14.0, roles.on_surface),
                technical_text,
            ])
            .gap(6.0)
            .align(Center),
        );

    let technical_button = if settings.inspect {
        if technical_hovered {
            technical_button.fill(hover_container(roles))
        } else {
            technical_button.fill(roles.surface_container_high)
        }
    } else if technical_hovered {
        technical_button.fill(hover_wash(roles))
    } else {
        technical_button
    };
    actions.push(outlined(
        technical_button,
        ring::register(Stop::new(technical_id).press(inspect)),
        Some(roles.edge()),
        roles,
    ));

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

// what a change to the key at `path` is called, or None if the key is unknown
fn setting_name(path: &[String]) -> Option<String> {
    if path.is_empty() {
        return None;
    }

    match path {
        [modules, name] if modules == MODULES => Some(format!("{} module", humanized(name))),
        _ => {
            let key = path.join(".");
            setting(&key)
                .or_else(|| setting(&path[..path.len().saturating_sub(1)].join(".")))
                .map(|setting| setting.label.to_owned())
        }
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
