use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{Center, Column, Cursor, Rectangle, Row, Start, Text, Widget, children};

use crate::config::Kind;
use crate::icon::Icon;
use crate::modules::{self, Module};
use crate::settings::layout::{MODULES, Page, Row as Line, visible_pages};
use crate::settings::ring::{self, Stop};
use crate::settings::{Settings, hover, niri, turn_to, umbriel};
use crate::sources::compositor::Compositor;
use crate::theme::{self, ThemeRoles, radius};

use super::{
    CARD_INSET, PAD, SECTIONS, content, fixed, help, hover_wash, inner, left, niri_page, notices,
    outlined, section_view_matched, setting, sides, space, umbriel_page,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MatchReason {
    Key(String),
    Alias(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MatchScore {
    pub(crate) score: u32,
    pub(crate) reason: Option<MatchReason>,
}

pub(crate) const SCORE_EXACT_LABEL: u32 = 100;
pub(crate) const SCORE_PREFIX_LABEL: u32 = 80;
pub(crate) const SCORE_SUBSTRING_LABEL: u32 = 60;
pub(crate) const SCORE_SECTION_OR_PAGE: u32 = 50;
pub(crate) const SCORE_ALIAS: u32 = 40;
pub(crate) const SCORE_DESCRIPTION: u32 = 20;
pub(crate) const SCORE_CONFIG_KEY: u32 = 10;

pub(crate) fn is_word_start(text: &str, query: &str) -> bool {
    let text = text.to_lowercase();
    let query = query.to_lowercase();
    if text.starts_with(&query) {
        return true;
    }
    for part in text.split(|c: char| !c.is_alphanumeric()) {
        if !part.is_empty() && part.starts_with(&query) {
            return true;
        }
    }
    false
}

fn row_metadata(row: &Line) -> (Vec<String>, Vec<String>, Vec<String>, Vec<&'static str>) {
    let mut labels = Vec::new();
    let mut descriptions = Vec::new();
    let mut keys = Vec::new();
    let mut aliases = Vec::new();

    match row {
        Line::Key(MODULES) => {
            for module in modules::ALL {
                labels.push(module.name.to_string());
                descriptions.push(module.about.to_string());
            }
            keys.push(MODULES.to_string());
            aliases.extend(&[
                "module",
                "modules",
                "extension",
                "extensions",
                "plugins",
                "widget",
                "widgets",
            ]);
        }
        Line::Key(key) => {
            keys.push(key.to_string());
            if let Some(s) = setting(key) {
                labels.push(s.label.to_string());
                descriptions.push(help(s));
            }
            match *key {
                "appearance.tone" => aliases.extend(&["dark mode", "light mode", "theme", "color"]),
                "appearance.finish" => aliases.extend(&["transparency", "blur", "matte", "glass"]),
                "appearance.font" => aliases.extend(&["font", "typography", "text", "typeface"]),
                "island.autohide" => aliases.extend(&["hide", "autohide", "collapse"]),
                "lock.lock-after" => {
                    aliases.extend(&["lock", "security", "screen lock", "auth", "password"])
                }
                "power.profile" => aliases.extend(&["battery", "power", "performance", "saving"]),
                _ => {}
            }
        }
        Line::Position { edge, align } => {
            keys.push(edge.to_string());
            keys.push(align.to_string());
            labels.extend(vec![
                "Position".to_string(),
                "Combined position".to_string(),
                "Separate Island position".to_string(),
                "Separate Dock position".to_string(),
            ]);
            descriptions.push("Which edge of the screen, and where along it".to_string());
            aliases.extend(&[
                "edge",
                "alignment",
                "top",
                "bottom",
                "left",
                "right",
                "dock placement",
                "bar",
            ]);
        }
        Line::Health => {
            labels.push("System health".to_string());
            descriptions.push(
                "Checks for required components, daemons, and system integration".to_string(),
            );
            aliases.extend(&["health", "doctor", "diagnostics", "status", "system check"]);
        }
        Line::Login => {
            labels.push("Login screen".to_string());
            descriptions.push(
                "Display manager themes, greeter configuration, and login appearance".to_string(),
            );
            aliases.extend(&["login", "sddm", "greeter", "theme", "lock screen"]);
        }
        Line::Place => {
            labels.push("Weather location".to_string());
            descriptions.push("Look up coordinates from a city name or IP address".to_string());
            aliases.extend(&[
                "weather",
                "place",
                "city",
                "location",
                "find",
                "ip address",
                "geocod",
            ]);
        }
        Line::Outputs => {
            labels.push("Displays".to_string());
            descriptions
                .push("Resolution, refresh rate, scaling, arrangements, and VRR".to_string());
            aliases.extend(&[
                "display",
                "displays",
                "monitor",
                "monitors",
                "resolution",
                "scale",
                "scaling",
                "refresh rate",
                "vrr",
                "outputs",
            ]);
        }
        Line::Niri(path) => {
            keys.push(path.to_string());
            if let Some(item) = niri::item(path) {
                labels.push(item.label.to_string());
                descriptions.push(item.help.to_string());
                descriptions.push(item.path.to_string());
            }
            aliases.extend(&[
                "niri",
                "window manager",
                "compositor",
                "shortcut",
                "shortcuts",
                "keybinding",
                "keybindings",
            ]);
        }
        Line::NiriRule(id) => {
            keys.push(id.to_string());
            if let Some(preset) = crate::niri_config::preset(id) {
                labels.push(preset.label.to_string());
                descriptions.push(preset.description.to_string());
            }
            aliases.extend(&["niri", "window rule", "rules", "preset", "layer-rule"]);
        }
        Line::Umbriel(path) => {
            keys.push(path.to_string());
            if let Some(item) = umbriel::item(path) {
                labels.push(item.label.to_string());
                descriptions.push(item.help.to_string());
                descriptions.push(item.path.to_string());
            }
            aliases.extend(&[
                "umbriel",
                "window manager",
                "compositor",
                "shortcut",
                "shortcuts",
                "keybinding",
                "keybindings",
            ]);
        }
        Line::UmbrielRule(id) => {
            keys.push(id.to_string());
            if let Some(preset) = crate::umbriel_config::rules::preset(id) {
                labels.push(preset.label.to_string());
                descriptions.push(preset.description.to_string());
            }
            aliases.extend(&["umbriel", "window rule", "rules", "preset", "layer-rule"]);
        }
    }

    (labels, descriptions, keys, aliases)
}

pub(crate) fn score_row(
    row: &Line,
    section_title: &str,
    page_title: &str,
    query: &str,
    comp: Compositor,
) -> MatchScore {
    if (matches!(row, Line::Niri(_) | Line::NiriRule(_)) && comp != Compositor::Niri)
        || (matches!(row, Line::Umbriel(_) | Line::UmbrielRule(_)) && comp != Compositor::Umbriel)
    {
        return MatchScore {
            score: 0,
            reason: None,
        };
    }

    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return MatchScore {
            score: 0,
            reason: None,
        };
    }

    let (labels, descriptions, config_keys, aliases) = row_metadata(row);

    // 1. Exact label match (100)
    for label in &labels {
        if label.to_lowercase() == query {
            return MatchScore {
                score: SCORE_EXACT_LABEL,
                reason: None,
            };
        }
    }

    // 2. Prefix / word start match in label (80)
    for label in &labels {
        if is_word_start(label, &query) {
            return MatchScore {
                score: SCORE_PREFIX_LABEL,
                reason: None,
            };
        }
    }

    // 3. Substring in label / module name (60)
    for label in &labels {
        if label.to_lowercase().contains(&query) {
            return MatchScore {
                score: SCORE_SUBSTRING_LABEL,
                reason: None,
            };
        }
    }

    // Check secondary match dimensions
    let section_or_page_match = (!section_title.is_empty()
        && section_title.to_lowercase().contains(&query))
        || (!page_title.is_empty() && page_title.to_lowercase().contains(&query));

    let alias_match = aliases.into_iter().find(|alias| {
        let a = alias.to_lowercase();
        a.contains(&query) || query.contains(&a)
    });

    let desc_match = descriptions
        .iter()
        .any(|desc| desc.to_lowercase().contains(&query));

    let key_match = config_keys.into_iter().find(|key| {
        let k = key.to_lowercase();
        k.contains(&query)
    });

    // 4. Section / page title match (50)
    if section_or_page_match {
        let reason = alias_match
            .map(|a| MatchReason::Alias(a.to_string()))
            .or_else(|| key_match.map(|k| MatchReason::Key(k.to_string())));
        return MatchScore {
            score: SCORE_SECTION_OR_PAGE,
            reason,
        };
    }

    // 5. Keyword / alias match (40)
    if let Some(alias) = alias_match {
        return MatchScore {
            score: SCORE_ALIAS,
            reason: Some(MatchReason::Alias(alias.to_string())),
        };
    }

    // 6. Description / help text match (20)
    if desc_match {
        return MatchScore {
            score: SCORE_DESCRIPTION,
            reason: key_match.map(|k| MatchReason::Key(k.to_string())),
        };
    }

    // 7. Config key match (10)
    if let Some(key) = key_match {
        return MatchScore {
            score: SCORE_CONFIG_KEY,
            reason: Some(MatchReason::Key(key.to_string())),
        };
    }

    match row {
        Line::Niri(path) if niri_page::mentions(path, &query) => {
            return MatchScore {
                score: SCORE_DESCRIPTION,
                reason: Some(MatchReason::Key(path.to_string())),
            };
        }
        Line::NiriRule(id) if niri_page::mentions_rule(id, &query) => {
            return MatchScore {
                score: SCORE_ALIAS,
                reason: Some(MatchReason::Alias("niri rule".to_string())),
            };
        }
        Line::Umbriel(path) if umbriel_page::mentions(path, &query) => {
            return MatchScore {
                score: SCORE_DESCRIPTION,
                reason: Some(MatchReason::Key(path.to_string())),
            };
        }
        Line::UmbrielRule(id) if umbriel_page::mentions_rule(id, &query) => {
            return MatchScore {
                score: SCORE_ALIAS,
                reason: Some(MatchReason::Alias("umbriel rule".to_string())),
            };
        }
        _ => {}
    }

    MatchScore {
        score: 0,
        reason: None,
    }
}

pub(crate) fn row_stop_id(row: &Line) -> Option<String> {
    match row {
        Line::Key(MODULES) => Some(String::from("filter")),
        Line::Key("appearance.font") => Some(String::from("font")),
        Line::Key(key) => {
            let s = setting(key)?;
            match s.kind {
                Kind::Switch(_) => Some(format!("switch {key}")),
                Kind::Choice(..) => Some(format!("choice {key}")),
                Kind::Millis(..)
                | Kind::Pixels(..)
                | Kind::Percent(..)
                | Kind::Seconds(..)
                | Kind::Minutes(..)
                | Kind::Kelvin(..)
                | Kind::Count(..)
                | Kind::DayTime(..)
                | Kind::Path(..)
                | Kind::Text(..)
                | Kind::Node(..)
                | Kind::PamService(..)
                | Kind::Location(..) => Some(format!("field settings {key}")),
                _ => None,
            }
        }
        Line::Niri(path) => {
            let item = niri::item(path)?;
            match item.control {
                niri::Control::Flag | niri::Control::Drawn { .. } => {
                    Some(format!("switch niri {path}"))
                }
                niri::Control::Choice { .. } => Some(format!("choice niri {path}")),
                niri::Control::Number(_) | niri::Control::Text { .. } => {
                    Some(format!("field settings niri {path}"))
                }
            }
        }
        Line::NiriRule(id) => Some(format!("switch niri rule {id}")),
        Line::Umbriel(path) => {
            let item = umbriel::item(path)?;
            match item.control {
                umbriel::Control::Flag => Some(format!("switch umbriel {path}")),
                umbriel::Control::Choice { .. } => Some(format!("choice umbriel {path}")),
                umbriel::Control::Number(_) | umbriel::Control::Text { .. } => {
                    Some(format!("field settings umbriel {path}"))
                }
            }
        }
        Line::UmbrielRule(id) => Some(format!("switch umbriel rule {id}")),
        Line::Health => Some(String::from("check again")),
        Line::Position { .. } | Line::Outputs | Line::Login | Line::Place => None,
    }
}

pub(crate) const SUGGESTED_PAGES: &[(&str, &str, Icon)] = &[
    ("Displays", "displays", Icon::Capture),
    ("Keyboard & shortcuts", "compositor", Icon::Columns),
    ("Appearance", "appearance", Icon::Palette),
    ("Network", "network", Icon::Wifi),
    ("Audio", "audio", Icon::Speaker(2)),
    ("Power", "power", Icon::Bolt),
];

pub(crate) fn quick_link(
    label: &'static str,
    target: &'static str,
    icon: Icon,
    settings: &Settings,
    roles: &ThemeRoles,
) -> Box<dyn Widget> {
    let id = format!("empty destination {target}");
    let hovered = settings.is_hovered(&id);
    let hover_id = id.clone();
    let press = move || turn_to(target);

    let icon_widget = icon.on(14.0, roles.primary);
    let text_widget = Text::new(label)
        .size(theme::text::LABEL)
        .color(roles.on_surface)
        .weight(theme::text::SEMIBOLD);

    let width = 14.0 + 8.0 + fixed(Widget::width(&text_widget)) + 2.0 * 12.0;

    let row = Row::new(children![icon_widget, text_widget])
        .gap(8.0)
        .align(Center);

    let btn = Rectangle::new()
        .width(width)
        .height(34.0)
        .radius(radius::ROW)
        .padding(sides(12.0))
        .align_child(Start, Center)
        .cursor(Cursor::Pointer)
        .on_hover(move |inside| hover(hover_id.clone(), inside))
        .semantics(move || Semantics::new(Role::Button, label))
        .on_click(left(press))
        .child(row);

    let btn = if hovered {
        btn.fill(hover_wash(roles)).border(1.0, roles.primary)
    } else {
        btn.fill(roles.surface_container_high)
            .border(1.0, roles.edge())
    };

    outlined(
        btn,
        ring::register(Stop::new(id).press(press)),
        Some(roles.edge()),
        roles,
    )
}

fn empty_state(query: &str, settings: &Settings, roles: &ThemeRoles) -> Box<dyn Widget> {
    let title = Text::new(format!("No results for \u{201c}{query}\u{201d}"))
        .size(theme::text::TITLE)
        .color(roles.on_surface)
        .weight(theme::text::SEMIBOLD);

    let hint = Text::new("Try checking for typos, or jump directly to common settings:")
        .size(theme::text::LABEL)
        .color(roles.on_surface_variant)
        .weight(theme::text::MEDIUM);

    let mut buttons: Vec<Box<dyn Widget>> = SUGGESTED_PAGES
        .iter()
        .map(|&(label, target, icon)| quick_link(label, target, icon, settings, roles))
        .collect();

    let row2_buttons = buttons.split_off(3);
    let row1_buttons = buttons;

    let row1 = Row::new(row1_buttons).gap(10.0).align(Center);
    let row2 = Row::new(row2_buttons).gap(10.0).align(Center);

    let content_col = Column::new(children![title, hint, space(8.0), row1, row2,])
        .width(inner())
        .gap(10.0);

    let height = fixed(Widget::height(&content_col)) + 2.0 * CARD_INSET * 1.5;

    Box::new(
        Rectangle::new()
            .width(content())
            .height(height)
            .radius(radius::CARD)
            .fill(roles.card())
            .padding(CARD_INSET * 1.5)
            .child(content_col),
    )
}

// every row matching the search, ranked by match relevance
pub(crate) fn results(settings: &Settings, roles: &ThemeRoles) -> Column {
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
    if visible_pages(crate::sources::compositor::active())
        .iter()
        .flat_map(|page| page.sections)
        .flat_map(|section| section.rows)
        .any(|row| matches!(row, Line::Niri(_) | Line::NiriRule(_)) && matches(row, &query))
    {
        items.extend(niri_page::notices(settings));
    }
    if visible_pages(crate::sources::compositor::active())
        .iter()
        .flat_map(|page| page.sections)
        .flat_map(|section| section.rows)
        .any(|row| matches!(row, Line::Umbriel(_) | Line::UmbrielRule(_)) && matches(row, &query))
    {
        items.extend(umbriel_page::notices(settings));
    }

    struct MatchedSection {
        page: &'static Page,
        title: &'static str,
        rows: Vec<(&'static Line, MatchScore)>,
        max_score: u32,
    }

    let mut matched_sections: Vec<MatchedSection> = Vec::new();
    let comp = crate::sources::compositor::active();

    for page in visible_pages(comp) {
        for section in page.sections {
            let mut matched_rows: Vec<(&'static Line, MatchScore)> = Vec::new();
            let mut max_score = 0;

            for row in section.rows {
                let match_score = score_row(row, section.title, page.title, &query, comp);
                if match_score.score > 0 {
                    max_score = max_score.max(match_score.score);
                    matched_rows.push((row, match_score));
                }
            }

            if !matched_rows.is_empty() {
                // sort rows within the section by relevance score descending
                matched_rows.sort_by_key(|a| std::cmp::Reverse(a.1.score));
                matched_sections.push(MatchedSection {
                    page,
                    title: section.title,
                    rows: matched_rows,
                    max_score,
                });
            }
        }
    }

    // sort sections by their highest score descending (stable sort preserves layout order on ties)
    matched_sections.sort_by_key(|a| std::cmp::Reverse(a.max_score));

    let found = !matched_sections.is_empty();

    for matched in matched_sections {
        let first_stop = matched.rows.iter().find_map(|(row, _)| row_stop_id(row));
        items.push(Box::new(section_view_matched(
            matched.page,
            matched.title,
            first_stop.as_deref(),
            matched.rows,
            settings,
            Some(&query),
            roles,
        )));
    }

    if !found {
        items.push(empty_state(settings.query.trim(), settings, roles));
    }

    items.push(Box::new(space(PAD)));

    Column::new(items).width(content()).gap(SECTIONS)
}

// whether a row's settings mention the query; the Modules row, whether a Module does, as it shows those
pub(crate) fn matches(row: &Line, query: &str) -> bool {
    matches_for(row, query, crate::sources::compositor::active())
}

pub(crate) fn matches_for(row: &Line, query: &str, comp: Compositor) -> bool {
    score_row(row, "", "", query, comp).score > 0
}

pub(crate) fn module_matches(module: &Module, query: &str) -> bool {
    module.name.to_lowercase().contains(query) || module.about.to_lowercase().contains(query)
}
