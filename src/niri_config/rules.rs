//! Recommended layer rules for niri's config.kdl (ADR 0119 Slice 3, docs/wiki/Installation.md).

use std::fs;
use std::path::PathBuf;

use kdl::{KdlDocument, KdlNode, KdlValue};

use crate::niri_config::{Editor, edit};
use crate::sources::binds;

/// Status of a recommended rule in niri's config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleStatus {
    /// Active in niri's config.kdl directly.
    Active,
    /// Not set in niri's config or any included file.
    Missing,
    /// Active via an included file (read-only for Kanade).
    Included(PathBuf),
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
    pub kdl_body: &'static str,
}

pub const PRESETS: &[RulePreset] = &[
    RulePreset {
        id: "blur-xray",
        label: "Blur behind panes",
        description: "Turn off xray so windows behind Island, Dock and Banners blur instead of showing sharp",
        kdl_body: "{\n    match namespace=\"^kanade(-dock|-banners)?$\"\n    background-effect {\n        xray false\n    }\n}",
    },
    RulePreset {
        id: "screencast-banners",
        label: "Hide Banners from screencasts",
        description: "Draw Banners as a black rectangle on screencasts to protect notification contents",
        kdl_body: "{\n    match namespace=\"^kanade-banners$\"\n    block-out-from \"screencast\"\n}",
    },
    RulePreset {
        id: "overview-backdrop",
        label: "Wallpaper in overview",
        description: "Place wallpaper in the overview backdrop so windows show over wallpaper rather than background color",
        kdl_body: "{\n    match namespace=\"^awww-daemon\"\n    place-within-backdrop true\n}",
    },
];

pub fn preset(id: &str) -> Option<&'static RulePreset> {
    PRESETS.iter().find(|p| p.id == id)
}

pub fn matches_preset(node: &KdlNode, id: &str) -> bool {
    if node.name().value() != "layer-rule" {
        return false;
    }
    let children = node.children().map(KdlDocument::nodes).unwrap_or_default();
    let named = |name: &str| -> Vec<&KdlNode> {
        children
            .iter()
            .filter(|c| c.name().value() == name)
            .collect()
    };
    let patterns: Vec<Option<&str>> = named("match")
        .iter()
        .map(|c| c.get("namespace").and_then(KdlValue::as_string))
        .collect();
    let has_match = !patterns.is_empty();

    match id {
        "blur-xray" => {
            let has_xray_false = named("background-effect").iter().any(|c| {
                c.children()
                    .map(KdlDocument::nodes)
                    .unwrap_or_default()
                    .iter()
                    .any(|sub| {
                        sub.name().value() == "xray"
                            && sub.get(0).and_then(KdlValue::as_bool) == Some(false)
                    })
            });
            let matches_ns = !has_match
                || patterns.iter().flatten().any(|p| {
                    *p == "^kanade(-dock|-banners)?$"
                        || *p == "^kanade(-banners|-dock)?$"
                        || p.contains("kanade")
                });
            has_xray_false && matches_ns
        }
        "screencast-banners" => {
            let has_block = named("block-out-from").iter().any(|c| {
                matches!(
                    c.get(0).and_then(KdlValue::as_string),
                    Some("screencast" | "screen-capture")
                )
            });
            let matches_ns = !has_match
                || patterns
                    .iter()
                    .flatten()
                    .any(|p| *p == "^kanade-banners$" || p.contains("kanade-banners"));
            has_block && matches_ns
        }
        "overview-backdrop" => {
            let has_backdrop = named("place-within-backdrop")
                .iter()
                .any(|c| c.get(0).and_then(KdlValue::as_bool) == Some(true));
            let matches_ns = !has_match
                || patterns
                    .iter()
                    .flatten()
                    .any(|p| p.starts_with("^awww-daemon") || p.contains("awww-daemon"));
            has_backdrop && matches_ns
        }
        _ => false,
    }
}

impl Editor<'_> {
    /// Reports the status of the recommended installation rules.
    pub fn rules(&self) -> Result<Vec<RuleInfo>, String> {
        let text = match fs::read_to_string(&self.file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(format!("{}: {error}", self.file.display())),
        };
        let doc = edit::parse(&text).unwrap_or_default();
        let includes = self.includes().unwrap_or_default();

        let mut results = Vec::with_capacity(PRESETS.len());

        for preset in PRESETS {
            let found_in_self = doc
                .nodes()
                .iter()
                .any(|node| matches_preset(node, preset.id));

            let active = match preset.id {
                "blur-xray" => binds::xrayed(
                    &self.file,
                    &["kanade", "kanade-dock", "kanade-banners"],
                    &mut |p| fs::read_to_string(p),
                )
                .map(|x| x.is_empty())
                .unwrap_or(found_in_self),
                "screencast-banners" => {
                    binds::blocked_from_screencasts(&self.file, "kanade-banners", &mut |p| {
                        fs::read_to_string(p)
                    })
                    .unwrap_or(found_in_self)
                }
                "overview-backdrop" => {
                    binds::places_within_backdrop(&self.file, "awww-daemon", &mut |p| {
                        fs::read_to_string(p)
                    })
                    .unwrap_or(found_in_self)
                }
                _ => found_in_self,
            };

            let status = if !active {
                RuleStatus::Missing
            } else if found_in_self {
                RuleStatus::Active
            } else {
                let owner = includes.iter().find(|inc| {
                    fs::read_to_string(inc)
                        .ok()
                        .and_then(|t| edit::parse(&t).ok())
                        .is_some_and(|doc| {
                            doc.nodes()
                                .iter()
                                .any(|node| matches_preset(node, preset.id))
                        })
                });
                RuleStatus::Included(owner.cloned().unwrap_or_else(|| {
                    includes
                        .first()
                        .cloned()
                        .unwrap_or_else(|| self.file.clone())
                }))
            };

            results.push(RuleInfo {
                id: preset.id,
                label: preset.label,
                description: preset.description,
                status,
            });
        }

        Ok(results)
    }

    /// Applies a recommended rule by appending it to niri's config.kdl.
    /// If already active, returns Ok without modifying.
    pub fn apply_rule(&self, id: &str) -> Result<String, String> {
        let preset = preset(id).ok_or_else(|| format!("unknown rule: {id}"))?;
        let rules = self.rules()?;
        let info = rules
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| format!("unknown rule: {id}"))?;

        match &info.status {
            RuleStatus::Active => Ok(format!("{id} is active already")),
            RuleStatus::Included(path) => Ok(format!(
                "{id} is active already (included from {})",
                path.display()
            )),
            RuleStatus::Missing => self
                .set("layer-rule[new]", preset.kdl_body)
                .map(|_| format!("rule {id} applied")),
        }
    }

    /// Removes a recommended rule from niri's config.kdl.
    pub fn remove_rule(&self, id: &str) -> Result<String, String> {
        let _ = preset(id).ok_or_else(|| format!("unknown rule: {id}"))?;

        let text = fs::read_to_string(&self.file)
            .map_err(|error| format!("{}: {error}", self.file.display()))?;
        let doc = edit::parse(&text)?;

        let mut found_ordinal = None;
        let mut layer_rule_count = 0;

        for node in doc.nodes() {
            if node.name().value() == "layer-rule" {
                if matches_preset(node, id) {
                    found_ordinal = Some(layer_rule_count);
                    break;
                }
                layer_rule_count += 1;
            }
        }

        let Some(ordinal) = found_ordinal else {
            let rules = self.rules()?;
            if let Some(RuleStatus::Included(path)) =
                rules.iter().find(|r| r.id == id).map(|r| &r.status)
            {
                return Err(format!(
                    "rule {id} is set by {}, an include; Kanade edits only {}",
                    path.display(),
                    self.file.display()
                ));
            }
            return Err(format!("rule {id} is not set in {}", self.file.display()));
        };

        self.remove(&format!("layer-rule[{ordinal}]"))
            .map(|_| format!("rule {id} removed"))
    }
}
