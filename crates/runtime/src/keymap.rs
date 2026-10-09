//! Safe XKB translation of wl_keyboard events.
//! Native text uses the keymap sent by the compositor, including user remaps
//! and non-US layouts. No process reads /dev/input.
use xkbcommon::xkb;

use crate::input::{Key, Stroke};

pub struct Mapper {
    keymap: xkb::Keymap,
    state: xkb::State,
}

impl Mapper {
    pub fn compile(source: String) -> Option<Self> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_string(
            &context,
            source,
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::COMPILE_NO_FLAGS,
        )?;
        let state = xkb::State::new(&keymap);
        Some(Self { keymap, state })
    }

    pub fn modifiers(&mut self, depressed: u32, latched: u32, locked: u32, group: u32) {
        self.state
            .update_mask(depressed, latched, locked, 0, 0, group);
    }

    pub fn stroke(&self, physical: u32) -> Stroke {
        // wl_keyboard sends evdev keycodes; XKB codes are offset by 8.
        let keycode = xkb::Keycode::new(physical.saturating_add(8));
        let symbol = self.state.key_get_one_sym(keycode).raw();
        let logical = match symbol {
            0xff0d | 0xff8d => Key::Enter,
            0xff1b => Key::Escape,
            0xff09 | 0xfe20 => Key::Tab,
            0xff08 => Key::Backspace,
            0xffff => Key::Delete,
            0xff51 => Key::Left,
            0xff53 => Key::Right,
            0xff52 => Key::Up,
            0xff54 => Key::Down,
            0xff50 => Key::Home,
            0xff57 => Key::End,
            0xff55 => Key::PageUp,
            0xff56 => Key::PageDown,
            _ => {
                let text = self.state.key_get_utf8(keycode);
                if !text.is_empty() && !text.chars().any(char::is_control) {
                    Key::Text(text)
                } else {
                    Key::Other(symbol)
                }
            }
        };
        Stroke {
            code: physical,
            logical,
            repeatable: self.keymap.key_repeats(keycode),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compositor_xkb_keymap_drives_logical_keys() {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(
            &context, "", "", "us", "", None, xkb::COMPILE_NO_FLAGS,
        ).expect("system must provide a US XKB layout");
        let mut mapper = Mapper::compile(keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1))
            .expect("compiled XKB layout can be loaded from the compositor");
        assert_eq!(mapper.stroke(14).logical, Key::Backspace);
        assert_eq!(mapper.stroke(30).logical, Key::Text("a".into()));
        assert!(!mapper.stroke(42).repeatable); // Left Shift.
        mapper.modifiers(0, 0, 0, 0);
    }

    #[test]
    fn malformed_keymap_fails_closed() {
        assert!(Mapper::compile("not a keymap".into()).is_none());
    }
}
