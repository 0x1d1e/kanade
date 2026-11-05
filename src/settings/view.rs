//! The Settings window's view: the pages down the side under a search field, the page's sections
//! beside them, a footer with what the files say and Undo. Each row names a setting and says what
//! it does, with its control beside it or, for a list, under it; one the settings file sets has a
//! Reset, which hands the key back to the layers below. Show technical details shows each row's key.

use std::cell::RefCell;
use std::collections::HashMap;

pub(crate) use kanade_runtime::service::Service;
pub(crate) use kanade_runtime::ui::{Role, Semantics};
pub(crate) use kanade_runtime::{
    Center, Color, Column, Cursor, Key, Padding, Parent, Rectangle, Row, ScrollArea, SpaceBetween,
    Stack, Start, Text, TextInput, Widget, Window, children,
};

pub(crate) use crate::theme::space::TARGET;
pub(crate) use crate::theme::{self, ThemeRoles, radius};

pub(crate) use super::layout::{PAGES, Page};
pub(crate) use super::ring::{self, Stop};
pub(crate) use super::{Settings, hover};
pub(crate) use super::{niri, outputs, place, umbriel};

mod controls;
mod footer;
mod modules_page;
mod niri_page;
mod outputs_page;
mod place_page;
mod search;
mod sidebar;
mod umbriel_page;

pub(crate) use controls::*;
pub(crate) use footer::*;
pub(crate) use modules_page::*;
pub(crate) use search::*;
pub(crate) use sidebar::*;

pub(crate) const WIDTH: f32 = 920.0;
pub(crate) const HEIGHT: f32 = 640.0;
pub(crate) const MIN_WIDTH: f32 = 680.0;
pub(crate) const MIN_HEIGHT: f32 = 420.0;

// the page's column, a gutter short of the window's edge so a scrolled row clears it, no wider than reads well
pub(crate) const PAD: f32 = 28.0;
pub(crate) const GUTTER: f32 = 8.0;
pub(crate) const MOST_CONTENT: f32 = 780.0;
pub(crate) const PAGE_TITLE: f32 = 22.0;

// the area right of the sidebar, as wide as the window now is
pub(crate) fn main_width() -> f32 {
    let (width, _) = kanade_runtime::window_size();
    let width = if width > 0.0 { width } else { WIDTH };

    width.max(MIN_WIDTH) - sidebar_width()
}

// the page's column
pub(crate) fn content() -> f32 {
    (main_width() - 2.0 * PAD - GUTTER).min(MOST_CONTENT)
}

// a card's rows
pub(crate) fn inner() -> f32 {
    content() - 2.0 * CARD_INSET
}

// a control wider than this goes under its label
pub(crate) fn most_beside() -> f32 {
    inner() * 0.55
}

pub fn window() -> Window {
    ring::begin();

    let settings = Settings::read();
    let roles = theme::roles();

    // the niri page's inputs hold what the worker last read
    niri::sync_inputs(&settings.niri);
    umbriel::sync_inputs(&settings.umbriel);
    outputs::sync_inputs(&settings.outputs);
    place::sync_inputs(&settings);

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

// the header, the page or the search's results, the footer
fn main(settings: &Settings, roles: &ThemeRoles) -> Column {
    let page = PAGES
        .iter()
        .find(|page| page.name == settings.page)
        .unwrap_or(&PAGES[0]);

    let searching = !settings.query.trim().is_empty();
    let title = if searching { "Search" } else { page.title };

    // built in the order the keyboard goes through them
    let header = header(title, roles);
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
                left: column_left(),
            })
            .child(
                ScrollArea::new(
                    scroll_id(if searching { "search" } else { page.name }),
                    body,
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

// the page's title; "Show technical details" lives in the footer instead
fn header(title: &str, roles: &ThemeRoles) -> Rectangle {
    let title = Text::new(title)
        .size(PAGE_TITLE)
        .color(roles.on_surface)
        .weight(theme::text::SEMIBOLD);
    // measured before eliding: elided text takes the width it is given
    let natural = fixed(Widget::width(&title));
    let room = title_room().map_or(natural, |room| room.min(natural));
    let title = if room < natural { title.elide() } else { title };
    let title = Rectangle::new()
        .width(room)
        .height(fixed(Widget::height(&title)))
        .child(title);

    // the title spans the page's column, so it starts where the page does
    Rectangle::new()
        .width(Parent)
        .height(BAR)
        .padding(Padding {
            top: PAD,
            right: 0.0,
            bottom: PAD,
            left: column_left(),
        })
        .align_child(Start, Center)
        .child(
            Row::new(children![title])
                .width(content())
                .justify(SpaceBetween)
                .align(Center),
        )
}

// where the page's column starts in the main area; a window wider than the column centers it
fn column_left() -> f32 {
    PAD + ((main_width() - 2.0 * PAD - GUTTER - content()) / 2.0).max(0.0)
}

// a window within this of a monitor's width is maximized on it (niri's gaps leave a little)
pub(crate) const MAXIMIZED_WITHIN: f32 = 80.0;

// the Island's widest form that rests over the header (Compact) and a gap beside it
pub(crate) const UNDER_ISLAND: f32 = crate::island::geometry::COMPACT.width + 24.0;

/*
 * how wide the header's title may be: in a window maximized on a monitor the Island sits over the
 * middle of the window, so a long title ends before it instead of running under it; none otherwise
 * (niri does not say where a tiled window is, so a narrower window is not placed under it)
 */
fn title_room() -> Option<f32> {
    let (width, _) = kanade_runtime::window_size();
    let maximized = kanade_runtime::Monitors::read()
        .all()
        .iter()
        .any(|monitor| {
            let monitor = monitor.width as f32;

            width <= monitor && width >= monitor - MAXIMIZED_WITHIN
        });

    if !maximized {
        return None;
    }

    let island_left = width / 2.0 - UNDER_ISLAND / 2.0 - sidebar_width();

    Some((island_left - column_left()).max(0.0))
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
    } else if page.name == umbriel::PAGE {
        items.extend(umbriel_page::notices(settings));
    } else {
        items.extend(notices(settings));
    }

    for section in page.sections {
        items.push(Box::new(section_view(
            section.title,
            section.rows.iter().collect(),
            settings,
            None,
            roles,
        )));
    }

    items.push(Box::new(space(PAD)));

    Column::new(items).width(content()).gap(SECTIONS)
}

thread_local! {
    // each page's scroll id, so turning to a page keeps where it was scrolled to
    static SCROLLS: RefCell<HashMap<&'static str, &'static str>> = RefCell::new(HashMap::new());
}

pub(crate) fn scroll_id(page: &'static str) -> &'static str {
    SCROLLS.with_borrow_mut(|scrolls| {
        *scrolls
            .entry(page)
            .or_insert_with(|| Box::leak(format!("settings {page}").into_boxed_str()))
    })
}

#[cfg(test)]
mod compositor_tests {
    use super::*;
    use crate::settings::layout::{MODULES, Row as Line};
    use crate::sources::compositor::Compositor;

    #[test]
    fn search_excludes_inactive_compositor_rows_and_rules() {
        for (row, owner) in [
            (Line::Niri("layout.gaps"), Compositor::Niri),
            (Line::NiriRule("blur-xray"), Compositor::Niri),
            (Line::Umbriel("layout.gap"), Compositor::Umbriel),
            (Line::UmbrielRule("blur"), Compositor::Umbriel),
        ] {
            for comp in [Compositor::Niri, Compositor::Umbriel, Compositor::None] {
                if comp != owner {
                    assert!(!matches_for(&row, "", comp));
                }
            }
        }
        assert!(matches_for(
            &Line::Niri("layout.gaps"),
            "gap",
            Compositor::Niri
        ));
        assert!(matches_for(
            &Line::Umbriel("layout.gap"),
            "gap",
            Compositor::Umbriel
        ));
    }

    #[test]
    fn search_matches_non_schema_sections() {
        for query in [
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
        ] {
            assert!(
                matches_for(&Line::Outputs, query, Compositor::Niri),
                "{query}"
            );
        }

        for query in ["health", "doctor", "diagnostics", "status"] {
            assert!(
                matches_for(&Line::Health, query, Compositor::Niri),
                "{query}"
            );
        }

        for query in ["login", "sddm", "greeter", "theme"] {
            assert!(
                matches_for(&Line::Login, query, Compositor::Niri),
                "{query}"
            );
        }
    }

    #[test]
    fn copy_key_row_extracts_bare_key_from_all_origins() {
        for (raw, expected) in [
            ("island.autohide  (Settings override)", "island.autohide"),
            ("island.autohide  (config file)", "island.autohide"),
            ("island.autohide  (default)", "island.autohide"),
            ("island.autohide", "island.autohide"),
            ("layer_rule  (niri config.kdl)", "layer_rule"),
            ("layer_rule  (umbriel config.toml)", "layer_rule"),
        ] {
            let copy = raw.split_once("  (").map(|(key, _)| key).unwrap_or(raw);
            assert_eq!(copy, expected);
        }
    }

    #[test]
    fn footer_auto_save_and_restart_pending_states() {
        let roles = theme::roles();
        let mut settings = Settings::default();

        // Initial clean state shows settings file target
        let (status, ink) = footer_status(&settings, &roles);
        assert!(status.contains("Changes are saved to") || status.contains("Changes are written"));
        assert_eq!(ink, roles.on_surface_variant);

        // When a known Kanade setting is saved
        settings.history.push(vec![crate::settings::file::Replaced {
            path: vec!["appearance".to_string(), "tone".to_string()],
            was: None,
            now: None,
        }]);
        let (status, ink) = footer_status(&settings, &roles);
        assert_eq!(status, "Saved automatically: Color tone");
        assert_eq!(ink, roles.on_surface_variant);

        // When an unknown setting is saved
        settings.history.push(vec![crate::settings::file::Replaced {
            path: vec!["custom_unknown_key".to_string()],
            was: None,
            now: None,
        }]);
        let (status, ink) = footer_status(&settings, &roles);
        assert_eq!(status, "Saved automatically");
        assert_eq!(ink, roles.on_surface_variant);

        // Persistent restart notice takes precedence over auto-save confirmation
        settings.pending = vec!["island.position".to_string()];
        let (status, ink) = footer_status(&settings, &roles);
        assert_eq!(status, "1 change pending restart");
        assert_eq!(ink, roles.primary);

        settings.pending.push("appearance.font".to_string());
        let (status, ink) = footer_status(&settings, &roles);
        assert_eq!(status, "2 changes pending restart");
        assert_eq!(ink, roles.primary);
    }

    #[test]
    fn footer_staged_compositor_changes_and_search_mode() {
        let roles = theme::roles();
        let mut settings = Settings {
            page: niri::PAGE,
            ..Default::default()
        };

        // Clean niri page
        let (status, _) = footer_status(&settings, &roles);
        assert_eq!(status, niri::STANDING);

        // Staged niri change shows prominent warning status
        settings
            .niri
            .stage("layout.gaps", vec![("layout.gaps", Some("24".into()))]);
        let (status, ink) = footer_status(&settings, &roles);
        assert_eq!(status, "1 unsaved change");
        assert_eq!(ink, theme::window_signals().warning);

        // Multiple staged changes
        settings.niri.stage(
            "layout.border.width",
            vec![("layout.border.width", Some("8".into()))],
        );
        let (status, ink) = footer_status(&settings, &roles);
        assert_eq!(status, "2 unsaved changes");
        assert_eq!(ink, theme::window_signals().warning);

        // In search mode, staged change is displayed deterministically without ring dependence
        settings.query = "gaps".to_string(); // searching() == true
        let (status, ink) = footer_status(&settings, &roles);
        assert_eq!(status, "2 unsaved changes");
        assert_eq!(ink, theme::window_signals().warning);

        // Discard staged changes in search mode returns footer to clean state
        settings.niri.discard();
        let (status, ink) = footer_status(&settings, &roles);
        assert!(status.contains("Changes are saved to") || status.contains("Changes are written"));
        assert_eq!(ink, roles.on_surface_variant);
    }

    #[test]
    fn footer_display_trial_notice() {
        let roles = theme::roles();
        let mut settings = Settings {
            page: outputs_page::PAGE,
            ..Default::default()
        };

        assert!(outputs_page::actions(&settings, &roles).is_empty());

        // Under display trial
        settings.outputs.trial = Some(crate::settings::outputs::Trial {
            id: 1,
            output: "DP-1".to_string(),
            edits: std::collections::BTreeMap::new(),
            seconds: 15,
            stuck: None,
        });

        let (status, ink) = footer_status(&settings, &roles);
        assert_eq!(status, "Keep display changes? Reverting in 15s");
        assert_eq!(ink, roles.primary);

        let actions = outputs_page::actions(&settings, &roles);
        assert_eq!(actions.len(), 2);
    }

    #[test]
    fn search_scoring_relevance_ranking_tiers() {
        let row = Line::Key("appearance.tone");

        // 1. Exact label match (100)
        let exact = score_row(&row, "", "", "color tone", Compositor::None);
        assert_eq!(exact.score, SCORE_EXACT_LABEL);
        assert_eq!(exact.reason, None);

        // 2. Prefix / word start match in label (80)
        let prefix = score_row(&row, "", "", "tone", Compositor::None);
        assert_eq!(prefix.score, SCORE_PREFIX_LABEL);
        assert_eq!(prefix.reason, None);

        // 3. Substring in label (60)
        let substr = score_row(&row, "", "", "or ton", Compositor::None);
        assert_eq!(substr.score, SCORE_SUBSTRING_LABEL);
        assert_eq!(substr.reason, None);

        // 4. Section / page title match (50)
        let section = score_row(
            &row,
            "Appearance & Theme",
            "",
            "appearance",
            Compositor::None,
        );
        assert_eq!(section.score, SCORE_SECTION_OR_PAGE);

        // 5. Keyword / alias match (40)
        let alias = score_row(&row, "", "", "dark mode", Compositor::None);
        assert_eq!(alias.score, SCORE_ALIAS);
        assert_eq!(
            alias.reason,
            Some(MatchReason::Alias("dark mode".to_string()))
        );

        // 6. Description / help text match (20)
        let desc = score_row(&row, "", "", "light or dark", Compositor::None);
        assert_eq!(desc.score, SCORE_DESCRIPTION);

        // 7. Config key match (10)
        let key_only = score_row(
            &Line::Key("appearance.sync"),
            "",
            "",
            "appearance.sync",
            Compositor::None,
        );
        assert_eq!(key_only.score, SCORE_CONFIG_KEY);
        assert_eq!(
            key_only.reason,
            Some(MatchReason::Key("appearance.sync".to_string()))
        );
    }

    #[test]
    fn search_word_start_matching() {
        assert!(is_word_start("Color tone", "col"));
        assert!(is_word_start("Color tone", "tone"));
        assert!(!is_word_start("Color tone", "olor"));
        assert!(!is_word_start("Color tone", "one"));
    }

    #[test]
    fn direct_nav_stop_id_resolution() {
        assert_eq!(
            row_stop_id(&Line::Key("appearance.tone")),
            Some("choice appearance.tone".to_string())
        );
        assert_eq!(
            row_stop_id(&Line::Key("island.autohide")),
            Some("switch island.autohide".to_string())
        );
        assert_eq!(
            row_stop_id(&Line::Key("timings.expand")),
            Some("field settings timings.expand".to_string())
        );
        assert_eq!(
            row_stop_id(&Line::Key("weather.place")),
            Some("field settings weather.place".to_string())
        );
        assert_eq!(
            row_stop_id(&Line::Key("appearance.font")),
            Some("font".to_string())
        );
        assert_eq!(row_stop_id(&Line::Key(MODULES)), Some("filter".to_string()));
        assert_eq!(
            row_stop_id(&Line::Niri("layout.always-center-single-column")),
            Some("switch niri layout.always-center-single-column".to_string())
        );
        assert_eq!(
            row_stop_id(&Line::Niri("layout.default-column-display")),
            Some("choice niri layout.default-column-display".to_string())
        );
        assert_eq!(
            row_stop_id(&Line::Niri("layout.gaps")),
            Some("field settings niri layout.gaps".to_string())
        );
        assert_eq!(
            row_stop_id(&Line::NiriRule("blur-xray")),
            Some("switch niri rule blur-xray".to_string())
        );
        assert_eq!(
            row_stop_id(&Line::Umbriel("appearance.blur.enabled")),
            Some("switch umbriel appearance.blur.enabled".to_string())
        );
        assert_eq!(
            row_stop_id(&Line::Umbriel("layout.mode")),
            Some("choice umbriel layout.mode".to_string())
        );
        assert_eq!(
            row_stop_id(&Line::Umbriel("appearance.blur.radius")),
            Some("field settings umbriel appearance.blur.radius".to_string())
        );
        assert_eq!(
            row_stop_id(&Line::UmbrielRule("blur")),
            Some("switch umbriel rule blur".to_string())
        );
        assert_eq!(row_stop_id(&Line::Health), Some("check again".to_string()));
        assert_eq!(
            row_stop_id(&Line::Position {
                edge: "island.edge",
                align: "island.align"
            }),
            None
        );
        assert_eq!(row_stop_id(&Line::Outputs), None);
        assert_eq!(row_stop_id(&Line::Login), None);
        assert_eq!(row_stop_id(&Line::Place), None);
    }
}
