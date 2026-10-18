//! The words `kanade ui key` and `kanade ui type` are given, as the keys a keyboard would send.

use crate::{Key, Modifiers};

// `enter`, `ctrl+a`, `shift+tab`: modifiers joined by `+`, then a key's name or a single character
pub(crate) fn parse_key(words: &str) -> Result<(Key, Modifiers), String> {
    let mut modifiers = Modifiers::default();
    let mut parts: Vec<&str> = words.split('+').collect();

    // `ctrl++` is ctrl and the plus sign
    if words.ends_with("++") {
        parts.pop();
        parts.pop();
        parts.push("+");
    }

    let Some(name) = parts.pop() else {
        return Err(String::from("no key"));
    };

    for modifier in parts {
        match modifier.to_ascii_lowercase().as_str() {
            "shift" => modifiers.shift = true,
            "ctrl" | "control" => modifiers.ctrl = true,
            "alt" => modifiers.alt = true,
            "super" | "logo" | "meta" => modifiers.logo = true,
            other => return Err(format!("not a modifier: {other}")),
        }
    }

    let key = match name.to_ascii_lowercase().as_str() {
        "enter" | "return" => Key::Enter,
        "escape" | "esc" => Key::Escape,
        "tab" if modifiers.shift => {
            modifiers.shift = false;

            Key::BackTab
        }
        "tab" => Key::Tab,
        "backtab" => Key::BackTab,
        "backspace" => Key::Backspace,
        "space" => Key::Space,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "delete" | "del" => Key::Delete,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "selectall" | "select-all" => Key::SelectAll,
        "a" if modifiers.ctrl => Key::SelectAll,
        _ => character(name, modifiers)?,
    };

    Ok((key, modifiers))
}

// a single typed character; with ctrl held the key's own letter names it, lower case, as a keyboard does
fn character(name: &str, modifiers: Modifiers) -> Result<Key, String> {
    let mut letters = name.chars();

    let (Some(letter), None) = (letters.next(), letters.next()) else {
        return Err(format!("not a key: {name}"));
    };

    if letter.is_control() {
        return Err(String::from("not a key: a control character"));
    }

    if letter == ' ' {
        return Ok(Key::Space);
    }

    if modifiers.ctrl {
        return Ok(Key::Character(
            letter.to_lowercase().next().unwrap_or(letter),
        ));
    }

    Ok(Key::Character(letter))
}

// the keys that type a text, one per character
pub(crate) fn typed(text: &str) -> Result<Vec<Key>, String> {
    text.chars()
        .map(|letter| match letter {
            ' ' => Ok(Key::Space),
            letter if letter.is_control() => {
                Err(String::from("a control character cannot be typed"))
            }
            letter => Ok(Key::Character(letter)),
        })
        .collect()
}
