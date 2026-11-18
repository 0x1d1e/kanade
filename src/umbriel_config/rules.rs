//! Recommended rules and presets for Umbriel TOML configuration.

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table};

use crate::umbriel_config::Editor;
use crate::umbriel_config::edit;

/// Status of a recommended rule in Umbriel's config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleStatus {
    Active,
    Missing,
}

/// Information about a recommended rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub status: RuleStatus,
}

/// A preset recommended rule definition.
#[derive(Debug, Clone, Copy)]
pub struct RulePreset {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub toml_body: &'static str,
}

pub const PRESETS: &[RulePreset] = &[
    RulePreset {
        id: "blur-kanade",
        label: "Blur behind Kanade surfaces",
        description: "Enable blur with popups blurred and non-optimized blur for Kanade Island, Dock, Dock menus, Banners and Launcher",
        toml_body: r#"[[layer_rule]]
match.namespace = "^kanade(-dock(-menu)?|-banners|-launcher)?$"
blur = true
blur_optimized = false
blur_ignore_alpha = 0.5
blur_popups = true
"#,
    },
    RulePreset {
        id: "overview-wallpaper",
        label: "Wallpaper in overview",
        description: "Keep wallpaper visible in the overview backdrop",
        toml_body: r#"[overview]
workspace_wallpaper = true
"#,
    },
];

pub fn preset(id: &str) -> Option<&'static RulePreset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// Checks if a layer_rule table matches a given preset id.
pub fn matches_preset(table: &Table, id: &str) -> bool {
    let ns = table
        .get("match")
        .and_then(|m| match m {
            Item::Table(t) => t.get("namespace").and_then(|item| item.as_str()),
            Item::Value(toml_edit::Value::InlineTable(it)) => {
                it.get("namespace").and_then(|val| val.as_str())
            }
            _ => None,
        })
        .or_else(|| {
            // Also check match.namespace dotted
            table.get("match.namespace").and_then(|item| item.as_str())
        });

    match id {
        "blur-kanade" => {
            let has_blur = table.get("blur").and_then(|b| b.as_bool()).unwrap_or(false);
            // the recommended pattern, and the narrower ones earlier releases wrote (their names
            // all contain "kanade"), so an existing rule still reads active
            let matches_ns = ns
                .is_some_and(|n| n == crate::sources::binds::PANE_PATTERN || n.contains("kanade"));
            has_blur && matches_ns
        }
        _ => false,
    }
}

/// Checks whether a rule preset is active in the given TOML text.
pub fn is_rule_active(text: &str, id: &str) -> bool {
    let (_, body) = edit::unbom(text);
    let Ok(doc) = body.parse::<DocumentMut>() else {
        return false;
    };

    match id {
        "blur-kanade" => {
            let Some(layer_rules) = doc.get("layer_rule") else {
                return false;
            };

            match layer_rules {
                Item::ArrayOfTables(aot) => aot.iter().any(|t| matches_preset(t, id)),
                Item::Table(t) => matches_preset(t, id),
                _ => false,
            }
        }
        "overview-wallpaper" => doc
            .get("overview")
            .and_then(|t| match t {
                Item::Table(tbl) => tbl.get("workspace_wallpaper").and_then(|v| v.as_bool()),
                Item::Value(toml_edit::Value::InlineTable(it)) => {
                    it.get("workspace_wallpaper").and_then(|v| v.as_bool())
                }
                _ => None,
            })
            .unwrap_or(false),
        _ => false,
    }
}

/// Appends a rule preset to the TOML document.
pub fn apply_rule_to_text(text: &str, id: &str) -> Result<String, String> {
    let preset = preset(id).ok_or_else(|| format!("unknown rule: {id}"))?;
    if is_rule_active(text, id) {
        return Ok(text.to_owned());
    }

    if id == "overview-wallpaper" {
        return edit::set(text, "overview.workspace_wallpaper", "true")
            .map(|opt| opt.unwrap_or_else(|| text.to_owned()));
    }

    let (bom, body) = edit::unbom(text);
    let mut doc = body
        .parse::<DocumentMut>()
        .map_err(|e| format!("cannot parse TOML: {e}"))?;

    let snippet_doc = preset
        .toml_body
        .parse::<DocumentMut>()
        .map_err(|e| format!("cannot parse rule snippet: {e}"))?;

    let snippet_table = match snippet_doc.get("layer_rule") {
        Some(Item::ArrayOfTables(aot)) => aot.get(0).cloned(),
        Some(Item::Table(t)) => Some(t.clone()),
        _ => None,
    }
    .ok_or_else(|| String::from("rule preset has invalid structure"))?;

    if !doc.contains_key("layer_rule") {
        let mut aot = ArrayOfTables::new();
        aot.push(snippet_table);
        doc.insert("layer_rule", Item::ArrayOfTables(aot));
    } else {
        match doc.get_mut("layer_rule") {
            Some(Item::ArrayOfTables(aot)) => {
                aot.push(snippet_table);
            }
            Some(Item::Table(existing_table)) => {
                let mut aot = ArrayOfTables::new();
                aot.push(existing_table.clone());
                aot.push(snippet_table);
                doc.insert("layer_rule", Item::ArrayOfTables(aot));
            }
            _ => return Err(String::from("cannot append layer_rule to non-table")),
        }
    }

    Ok(format!("{bom}{doc}"))
}

/// Removes a rule preset from the TOML document.
pub fn remove_rule_from_text(text: &str, id: &str) -> Result<Option<String>, String> {
    if !is_rule_active(text, id) {
        return Ok(None);
    }

    if id == "overview-wallpaper" {
        return edit::remove(text, "overview.workspace_wallpaper");
    }

    let (bom, body) = edit::unbom(text);
    let mut doc = body
        .parse::<DocumentMut>()
        .map_err(|e| format!("cannot parse TOML: {e}"))?;

    let Some(Item::ArrayOfTables(aot)) = doc.get_mut("layer_rule") else {
        return Ok(None);
    };

    let mut remove_idx = None;
    for (i, table) in aot.iter().enumerate() {
        if matches_preset(table, id) {
            remove_idx = Some(i);
            break;
        }
    }

    if let Some(i) = remove_idx {
        aot.remove(i);
        Ok(Some(format!("{bom}{doc}")))
    } else {
        Ok(None)
    }
}

impl Editor<'_> {
    /// Reports the status of the recommended rules.
    pub fn rules(&self) -> Result<Vec<RuleInfo>, String> {
        let text = match std::fs::read_to_string(&self.file) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("{}: {e}", self.file.display())),
        };

        let mut results = Vec::with_capacity(PRESETS.len());
        for p in PRESETS {
            let active = is_rule_active(&text, p.id);
            results.push(RuleInfo {
                id: p.id,
                label: p.label,
                description: p.description,
                status: if active {
                    RuleStatus::Active
                } else {
                    RuleStatus::Missing
                },
            });
        }

        Ok(results)
    }

    /// Applies a recommended rule preset.
    pub fn apply_rule(&self, id: &str) -> Result<String, String> {
        let p = preset(id).ok_or_else(|| format!("unknown rule: {id}"))?;
        self.change(true, |bytes| {
            let text =
                std::str::from_utf8(bytes).map_err(|_| String::from("the config is not UTF-8"))?;

            if is_rule_active(text, id) {
                return Ok(crate::umbriel_config::Plan::Same(format!(
                    "{id} is active already"
                )));
            }

            let new_text = apply_rule_to_text(text, id)?;
            Ok(crate::umbriel_config::Plan::Write(
                new_text.into_bytes(),
                format!("applied {}", p.id),
            ))
        })
    }

    /// Removes a recommended rule preset.
    pub fn remove_rule(&self, id: &str) -> Result<String, String> {
        let _ = preset(id).ok_or_else(|| format!("unknown rule: {id}"))?;
        self.change(true, |bytes| {
            let text =
                std::str::from_utf8(bytes).map_err(|_| String::from("the config is not UTF-8"))?;

            if !is_rule_active(text, id) {
                return Ok(crate::umbriel_config::Plan::Same(format!(
                    "{id} is not set"
                )));
            }

            match remove_rule_from_text(text, id)? {
                Some(new_text) => Ok(crate::umbriel_config::Plan::Write(
                    new_text.into_bytes(),
                    format!("removed {id}"),
                )),
                None => Ok(crate::umbriel_config::Plan::Same(format!(
                    "{id} is not set"
                ))),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[general]
autostart = ["kanade"]

[[layer_rule]]
match.namespace = "^kanade(-dock|-banners)?$"
blur = true
blur_optimized = false
blur_ignore_alpha = 0.5
blur_popups = true
"#;

    #[test]
    fn detects_active_rule() {
        assert!(is_rule_active(SAMPLE, "blur-kanade"));
        assert!(!is_rule_active(SAMPLE, "overview-wallpaper"));
    }

    // the recommended rule covers every namespace Kanade draws a pane on; an old narrow rule
    // still reads active, so an existing config is not reported missing
    #[test]
    fn the_recommended_pattern_covers_every_kanade_namespace() {
        use crate::sources::binds::{PANE_NAMESPACES, PANE_PATTERN, covers};

        let body = preset("blur-kanade").unwrap().toml_body;
        assert!(body.contains(PANE_PATTERN));

        for namespace in PANE_NAMESPACES {
            assert_eq!(covers(PANE_PATTERN, namespace), Some(true), "{namespace}");
        }
        assert_eq!(covers(PANE_PATTERN, "kanade-settings"), Some(false));

        assert!(is_rule_active(SAMPLE, "blur-kanade"));
    }

    #[test]
    fn applies_and_removes_rule() {
        let applied = apply_rule_to_text(SAMPLE, "overview-wallpaper").unwrap();
        assert!(is_rule_active(&applied, "overview-wallpaper"));
        assert!(is_rule_active(&applied, "blur-kanade"));

        let removed = remove_rule_from_text(&applied, "overview-wallpaper")
            .unwrap()
            .unwrap();
        assert!(!is_rule_active(&removed, "overview-wallpaper"));
        assert!(is_rule_active(&removed, "blur-kanade"));
    }

    #[test]
    fn bom_prefixed_configs_preserve_bom_across_rule_operations() {
        let bare = "\u{feff}[general]\nautostart = [\"kanade\"]\n";
        assert!(!is_rule_active(bare, "blur-kanade"));
        assert!(!is_rule_active(bare, "overview-wallpaper"));

        let with_blur = apply_rule_to_text(bare, "blur-kanade").unwrap();
        assert!(with_blur.starts_with('\u{feff}'));
        assert!(is_rule_active(&with_blur, "blur-kanade"));
        assert_eq!(
            apply_rule_to_text(&with_blur, "blur-kanade").unwrap(),
            with_blur
        );

        let with_both = apply_rule_to_text(&with_blur, "overview-wallpaper").unwrap();
        assert!(with_both.starts_with('\u{feff}'));
        assert!(is_rule_active(&with_both, "blur-kanade"));
        assert!(is_rule_active(&with_both, "overview-wallpaper"));

        let without_blur = remove_rule_from_text(&with_both, "blur-kanade")
            .unwrap()
            .unwrap();
        assert!(without_blur.starts_with('\u{feff}'));
        assert!(!is_rule_active(&without_blur, "blur-kanade"));
        assert!(is_rule_active(&without_blur, "overview-wallpaper"));

        let without_either = remove_rule_from_text(&without_blur, "overview-wallpaper")
            .unwrap()
            .unwrap();
        assert!(without_either.starts_with('\u{feff}'));
        assert!(!is_rule_active(&without_either, "overview-wallpaper"));

        let sample_bom = format!("\u{feff}{SAMPLE}");
        assert!(is_rule_active(&sample_bom, "blur-kanade"));
        assert!(!is_rule_active(&sample_bom, "overview-wallpaper"));

        let sample_applied = apply_rule_to_text(&sample_bom, "overview-wallpaper").unwrap();
        assert!(sample_applied.starts_with('\u{feff}'));
        assert!(is_rule_active(&sample_applied, "overview-wallpaper"));
        assert!(is_rule_active(&sample_applied, "blur-kanade"));

        let sample_removed_overview = remove_rule_from_text(&sample_applied, "overview-wallpaper")
            .unwrap()
            .unwrap();
        assert!(sample_removed_overview.starts_with('\u{feff}'));
        assert!(!is_rule_active(
            &sample_removed_overview,
            "overview-wallpaper"
        ));
        assert!(is_rule_active(&sample_removed_overview, "blur-kanade"));

        let sample_removed_blur = remove_rule_from_text(&sample_removed_overview, "blur-kanade")
            .unwrap()
            .unwrap();
        assert!(sample_removed_blur.starts_with('\u{feff}'));
        assert!(!is_rule_active(&sample_removed_blur, "blur-kanade"));
    }
}
