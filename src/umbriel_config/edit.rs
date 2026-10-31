//! Pure text-in/text-out engine for Umbriel TOML configuration.
//!
//! Preserves comments, formatting, table ordering, and blank lines using `toml_edit`.

use toml_edit::{DocumentMut, Item, Table, Value};

/// A segment in a dotted configuration path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Key(String),
    Index(usize),
}

/// Parses a dotted path into segments, supporting:
/// - Standard dotted keys (`layout.gap`, `appearance.blur.radius`)
/// - Quoted keys (`keybinds."Mod+comma"`, `keybinds.'Mod+Space'`)
/// - Array indexing (`layer_rule[0].blur`, `extent_presets[1]`)
pub fn parse_path(path: &str) -> Result<Vec<Segment>, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err(String::from("empty path"));
    }

    let mut segments = Vec::new();
    let chars: Vec<char> = path.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '.' {
            i += 1;
            continue;
        }

        if chars[i] == '[' {
            i += 1;
            let mut num_str = String::new();
            while i < chars.len() && chars[i] != ']' {
                num_str.push(chars[i]);
                i += 1;
            }
            if i >= chars.len() || chars[i] != ']' {
                return Err(format!("{path}: unclosed bracket in path"));
            }
            i += 1; // skip ']'
            let idx = num_str
                .parse::<usize>()
                .map_err(|_| format!("{path}: invalid array index '{num_str}'"))?;
            segments.push(Segment::Index(idx));
            continue;
        }

        if chars[i] == '"' || chars[i] == '\'' {
            let quote = chars[i];
            i += 1;
            let mut key = String::new();
            let mut closed = false;
            while i < chars.len() {
                if chars[i] == '\\' && quote == '"' && i + 1 < chars.len() {
                    i += 1;
                    key.push(chars[i]);
                    i += 1;
                } else if chars[i] == quote {
                    closed = true;
                    i += 1;
                    break;
                } else {
                    key.push(chars[i]);
                    i += 1;
                }
            }
            if !closed {
                return Err(format!("{path}: unclosed quote in path"));
            }
            segments.push(Segment::Key(key));
            continue;
        }

        // Bare key until '.' or '['
        let mut key = String::new();
        while i < chars.len() && chars[i] != '.' && chars[i] != '[' {
            key.push(chars[i]);
            i += 1;
        }
        if key.is_empty() {
            return Err(format!("{path}: empty segment in path"));
        }
        segments.push(Segment::Key(key));
    }

    if segments.is_empty() {
        return Err(format!("{path}: empty path"));
    }

    Ok(segments)
}

/// Parses a value string into a `toml_edit::Value`.
///
/// Tries parsing as a valid TOML value (integers, floats, booleans, arrays, inline tables,
/// quoted strings). If that fails, treats unquoted text as a string literal.
pub fn parse_value(value_str: &str) -> Result<Value, String> {
    let trimmed = value_str.trim();
    if trimmed.is_empty() {
        return Err(String::from("cannot set an empty value"));
    }

    if let Ok(val) = trimmed.parse::<Value>() {
        return Ok(val);
    }

    // Bare text -> TOML string
    Ok(Value::from(trimmed))
}

enum Node<'a> {
    Item(&'a Item),
    Table(&'a Table),
    Value(&'a Value),
}

impl Node<'_> {
    fn to_rendered_string(&self) -> String {
        match self {
            Node::Item(item) => item.to_string().trim().to_string(),
            Node::Table(table) => table.to_string().trim().to_string(),
            Node::Value(val) => val.to_string().trim().to_string(),
        }
    }
}

/// Retrieves the formatted string value at `path` from `text`.
pub fn get(text: &str, path: &str) -> Result<String, String> {
    let doc = text
        .parse::<DocumentMut>()
        .map_err(|e| format!("cannot parse TOML: {e}"))?;
    let segments = parse_path(path)?;

    let mut current = Node::Item(doc.as_item());

    for seg in &segments {
        current = match (current, seg) {
            (Node::Item(Item::Table(t)), Segment::Key(k)) | (Node::Table(t), Segment::Key(k)) => {
                let item = t.get(k).ok_or_else(|| format!("{path}: not set"))?;
                Node::Item(item)
            }
            (Node::Item(Item::Value(Value::InlineTable(it))), Segment::Key(k))
            | (Node::Value(Value::InlineTable(it)), Segment::Key(k)) => {
                let val = it.get(k).ok_or_else(|| format!("{path}: not set"))?;
                Node::Value(val)
            }
            (Node::Item(Item::ArrayOfTables(aot)), Segment::Index(i)) => {
                let t = aot.get(*i).ok_or_else(|| {
                    format!("{path}: index {i} out of bounds (len {})", aot.len())
                })?;
                Node::Table(t)
            }
            (Node::Item(Item::Value(Value::Array(arr))), Segment::Index(i))
            | (Node::Value(Value::Array(arr)), Segment::Index(i)) => {
                let val = arr.get(*i).ok_or_else(|| {
                    format!("{path}: index {i} out of bounds (len {})", arr.len())
                })?;
                Node::Value(val)
            }
            _ => return Err(format!("{path}: not set")),
        };
    }

    Ok(current.to_rendered_string())
}

/// Sets a dotted path to `value_str` in `text`.
///
/// Returns `Ok(Some(new_text))` if changed, or `Ok(None)` if the value was already identical.
pub fn set(text: &str, path: &str, value_str: &str) -> Result<Option<String>, String> {
    let mut doc = text
        .parse::<DocumentMut>()
        .map_err(|e| format!("cannot parse TOML: {e}"))?;
    let segments = parse_path(path)?;
    let new_val = parse_value(value_str)?;

    // Check if already set to same value
    if let Ok(existing) = get(text, path) {
        let new_rendered = new_val.to_string().trim().to_string();
        if existing == new_rendered {
            return Ok(None);
        }
    }

    set_in_doc(&mut doc, &segments, new_val)?;
    Ok(Some(doc.to_string()))
}

enum MutNode<'a> {
    Item(&'a mut Item),
    Table(&'a mut Table),
}

fn set_in_doc(doc: &mut DocumentMut, segments: &[Segment], val: Value) -> Result<(), String> {
    if segments.is_empty() {
        return Err(String::from("cannot set at empty path"));
    }

    let (last_seg, parents) = segments.split_last().unwrap();
    let mut current = MutNode::Item(doc.as_item_mut());

    for seg in parents {
        current = match (current, seg) {
            (MutNode::Item(Item::Table(t)), Segment::Key(k))
            | (MutNode::Table(t), Segment::Key(k)) => {
                if !t.contains_key(k) {
                    t.insert(k, Item::Table(Table::new()));
                }
                MutNode::Item(t.get_mut(k).unwrap())
            }
            (MutNode::Item(Item::ArrayOfTables(aot)), Segment::Index(i)) => {
                let len = aot.len();
                if *i >= len {
                    return Err(format!("array index {i} out of bounds (len {len})"));
                }
                MutNode::Table(aot.get_mut(*i).unwrap())
            }
            _ => return Err(format!("cannot traverse through {seg:?}")),
        };
    }

    match (current, last_seg) {
        (MutNode::Item(Item::Table(t)), Segment::Key(k)) | (MutNode::Table(t), Segment::Key(k)) => {
            if let Some(existing) = t.get_mut(k) {
                if let Item::Value(existing_val) = existing {
                    *existing_val = val;
                } else {
                    *existing = Item::Value(val);
                }
            } else {
                t.insert(k, Item::Value(val));
            }
            Ok(())
        }
        (MutNode::Item(Item::Value(Value::InlineTable(it))), Segment::Key(k)) => {
            it.insert(k, val);
            Ok(())
        }
        (MutNode::Item(Item::Value(Value::Array(arr))), Segment::Index(i)) => {
            let len = arr.len();
            if *i < len {
                arr.replace(*i, val);
                Ok(())
            } else if *i == len {
                arr.push(val);
                Ok(())
            } else {
                Err(format!("array index {i} out of bounds (len {len})"))
            }
        }
        _ => Err(format!("cannot set {last_seg:?}")),
    }
}

/// Removes the setting at `path` from `text`.
///
/// Returns `Ok(Some(new_text))` if removed, or `Ok(None)` if the key was not set.
pub fn remove(text: &str, path: &str) -> Result<Option<String>, String> {
    let mut doc = text
        .parse::<DocumentMut>()
        .map_err(|e| format!("cannot parse TOML: {e}"))?;
    let segments = parse_path(path)?;

    if segments.is_empty() {
        return Ok(None);
    }

    let (last_seg, parents) = segments.split_last().unwrap();
    let mut current = MutNode::Item(doc.as_item_mut());

    for seg in parents {
        current = match (current, seg) {
            (MutNode::Item(Item::Table(t)), Segment::Key(k))
            | (MutNode::Table(t), Segment::Key(k)) => {
                let Some(next) = t.get_mut(k) else {
                    return Ok(None);
                };
                MutNode::Item(next)
            }
            (MutNode::Item(Item::ArrayOfTables(aot)), Segment::Index(i)) => {
                let Some(next) = aot.get_mut(*i) else {
                    return Ok(None);
                };
                MutNode::Table(next)
            }
            _ => return Ok(None),
        };
    }

    let removed = match (current, last_seg) {
        (MutNode::Item(Item::Table(t)), Segment::Key(k)) | (MutNode::Table(t), Segment::Key(k)) => {
            t.remove(k).is_some()
        }
        (MutNode::Item(Item::Value(Value::InlineTable(it))), Segment::Key(k)) => {
            it.remove(k).is_some()
        }
        (MutNode::Item(Item::Value(Value::Array(arr))), Segment::Index(i)) if *i < arr.len() => {
            arr.remove(*i);
            true
        }
        (MutNode::Item(Item::ArrayOfTables(aot)), Segment::Index(i)) if *i < aot.len() => {
            aot.remove(*i);
            true
        }
        _ => false,
    };

    if removed {
        Ok(Some(doc.to_string()))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# Main config
[general]
autostart = ["kanade"]
attach_modal_dialogs = true
xwayland = true

# Visual appearance
[appearance]
corner_radius = 10
border_width = 2

[appearance.blur]
radius = 5
passes = 2
optimized = true

[layout]
mode = "scrolling"
gap = 8
extent_presets = [0.333, 0.5, 0.667]

[keybinds]
"Mod+comma" = "kanade settings open"
"Mod+Space" = "kanade launcher toggle"
"Mod+L" = "kanade lock"
"Mod+Return" = "kitty"

[include.optional]
files = ["kanade.toml"]
"#;

    #[test]
    fn parse_path_handles_various_formats() {
        assert_eq!(
            parse_path("layout.gap").unwrap(),
            vec![Segment::Key("layout".into()), Segment::Key("gap".into())]
        );
        assert_eq!(
            parse_path("appearance.blur.radius").unwrap(),
            vec![
                Segment::Key("appearance".into()),
                Segment::Key("blur".into()),
                Segment::Key("radius".into())
            ]
        );
        assert_eq!(
            parse_path(r#"keybinds."Mod+comma""#).unwrap(),
            vec![
                Segment::Key("keybinds".into()),
                Segment::Key("Mod+comma".into())
            ]
        );
        assert_eq!(
            parse_path("layout.extent_presets[0]").unwrap(),
            vec![
                Segment::Key("layout".into()),
                Segment::Key("extent_presets".into()),
                Segment::Index(0)
            ]
        );
    }

    #[test]
    fn get_values() {
        assert_eq!(get(SAMPLE, "layout.gap").unwrap(), "8");
        assert_eq!(get(SAMPLE, "layout.mode").unwrap(), "\"scrolling\"");
        assert_eq!(get(SAMPLE, "appearance.blur.radius").unwrap(), "5");
        assert_eq!(get(SAMPLE, "general.attach_modal_dialogs").unwrap(), "true");
        assert_eq!(
            get(SAMPLE, r#"keybinds."Mod+comma""#).unwrap(),
            "\"kanade settings open\""
        );
        assert_eq!(get(SAMPLE, "layout.extent_presets[0]").unwrap(), "0.333");
        assert!(get(SAMPLE, "layout.nonexistent").is_err());
    }

    #[test]
    fn set_existing_key_preserves_comments_and_formatting() {
        let updated = set(SAMPLE, "layout.gap", "12").unwrap().unwrap();
        assert!(updated.contains("# Visual appearance"));
        assert!(updated.contains("gap = 12"));
        assert_eq!(get(&updated, "layout.gap").unwrap(), "12");

        // Same value returns None (no change)
        assert_eq!(set(&updated, "layout.gap", "12").unwrap(), None);
    }

    #[test]
    fn set_new_key_in_existing_table() {
        let updated = set(SAMPLE, "appearance.shadow", "true").unwrap().unwrap();
        assert_eq!(get(&updated, "appearance.shadow").unwrap(), "true");
        assert!(updated.contains("# Visual appearance"));
    }

    #[test]
    fn set_new_nested_table() {
        let updated = set(SAMPLE, "window_rules.opacity", "0.95")
            .unwrap()
            .unwrap();
        assert_eq!(get(&updated, "window_rules.opacity").unwrap(), "0.95");
    }

    #[test]
    fn remove_key() {
        let updated = remove(SAMPLE, "appearance.border_width").unwrap().unwrap();
        assert!(!updated.contains("border_width"));
        assert!(get(&updated, "appearance.border_width").is_err());
        assert!(updated.contains("# Visual appearance"));
        assert_eq!(get(&updated, "appearance.corner_radius").unwrap(), "10");

        // Removing non-existent key returns None
        assert_eq!(remove(&updated, "appearance.border_width").unwrap(), None);
    }

    #[test]
    fn array_of_tables_access() {
        let sample = r#"
[[layer_rule]]
match.namespace = "^kanade(-dock|-banners)?$"
blur = true
blur_optimized = false

[[layer_rule]]
match.namespace = "^awww-daemon$"
overview_wallpaper = true
"#;
        assert_eq!(get(sample, "layer_rule[0].blur").unwrap(), "true");
        assert_eq!(
            get(sample, "layer_rule[1].match.namespace").unwrap(),
            "\"^awww-daemon$\""
        );

        let updated = set(sample, "layer_rule[0].blur", "false").unwrap().unwrap();
        assert_eq!(get(&updated, "layer_rule[0].blur").unwrap(), "false");

        let removed = remove(&updated, "layer_rule[1]").unwrap().unwrap();
        assert!(get(&removed, "layer_rule[1]").is_err());
        assert_eq!(get(&removed, "layer_rule[0].blur").unwrap(), "false");
    }

    #[test]
    fn array_index_mutation() {
        let updated = set(SAMPLE, "layout.extent_presets[0]", "0.25")
            .unwrap()
            .unwrap();
        assert_eq!(get(&updated, "layout.extent_presets[0]").unwrap(), "0.25");
        assert_eq!(get(&updated, "layout.extent_presets[1]").unwrap(), "0.5");
    }

    #[test]
    fn unquoted_string_value_and_quoted_keys() {
        let updated = set(SAMPLE, "layout.mode", "tiling").unwrap().unwrap();
        assert_eq!(get(&updated, "layout.mode").unwrap(), "\"tiling\"");

        let updated_bind = set(SAMPLE, r#"keybinds."Mod+Shift+Return""#, r#""alacritty""#)
            .unwrap()
            .unwrap();
        assert_eq!(
            get(&updated_bind, r#"keybinds."Mod+Shift+Return""#).unwrap(),
            "\"alacritty\""
        );
    }
}
