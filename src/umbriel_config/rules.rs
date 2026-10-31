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
        description: "Enable blur with popups blurred and non-optimized blur for Kanade Island, Dock and Banners",
        toml_body: r#"[[layer_rule]]
match.namespace = "^kanade(-dock|-banners)?$"
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
            let matches_ns = ns.is_some_and(|n| {
                n == "^kanade(-dock|-banners)?$"
                    || n == "^kanade(-banners|-dock)?$"
                    || n.contains("kanade")
            });
            has_blur && matches_ns
        }
        _ => false,
    }
}

/// Checks whether a rule preset is active in the given TOML text.
pub fn is_rule_active(text: &str, id: &str) -> bool {
    let Ok(doc) = text.parse::<DocumentMut>() else {
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

    let mut doc = text
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

    Ok(doc.to_string())
}

/// Removes a rule preset from the TOML document.
pub fn remove_rule_from_text(text: &str, id: &str) -> Result<Option<String>, String> {
    if !is_rule_active(text, id) {
        return Ok(None);
    }

    if id == "overview-wallpaper" {
        return edit::remove(text, "overview.workspace_wallpaper");
    }

    let mut doc = text
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
        Ok(Some(doc.to_string()))
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
}
