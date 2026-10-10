use smithay_client_toolkit::seat::keyboard::{
    KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers,
};
use wayland_client::{
    Connection, QueueHandle,
    protocol::{wl_keyboard::WlKeyboard, wl_surface::WlSurface},
};

use crate::Key;
use crate::input::focus;

use super::WaylandState;

impl KeyboardHandler for WaylandState {
    // keys only say which keyboard they came from, so the focused window is kept here
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        surface: &WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        self.keyboard_focus = Some(surface.clone());
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        surface: &WlSurface,
        _: u32,
    ) {
        self.keyboard_focus = None;

        // keys stop arriving, so no text input can stay focused
        focus::clear();

        self.request_frame_on(surface);
    }

    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.deliver(to_key(&event));
    }

    // the repeat timer in the event loop does this; a compositor that sends the state itself is not heard twice
    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        _: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
    }
}

impl WaylandState {
    // a held key, again: only the keys that edit and move repeat, as on macOS, where a held letter
    // offers accents instead
    pub(super) fn repeat(&mut self, event: &KeyEvent) {
        let key = to_key(event);

        if repeats(key) {
            self.deliver(key);
        }
    }

    // to the focused text input, else the focused window's on_key
    fn deliver(&mut self, key: Key) {
        let Some(surface) = self.keyboard_focus.clone() else {
            return;
        };

        // a focused text input takes the key before the window's on_key sees it
        if focus::send(key) {
            self.request_frame_on(&surface);

            return;
        }

        let Some(window) = self.window(&surface) else {
            return;
        };

        let Some(on_key) = &window.on_key else {
            return;
        };

        on_key(key);

        // a service the handler changed wakes its own readers, this window redraws for the rest
        self.request_frame_on(&surface);
    }
}

fn repeats(key: Key) -> bool {
    matches!(
        key,
        Key::Backspace | Key::Up | Key::Down | Key::Left | Key::Right
    )
}

fn to_key(event: &KeyEvent) -> Key {
    match event.keysym {
        Keysym::Return | Keysym::KP_Enter => Key::Enter,
        Keysym::Escape => Key::Escape,
        Keysym::Tab => Key::Tab,
        Keysym::BackSpace => Key::Backspace,
        Keysym::space => Key::Space,
        Keysym::Up => Key::Up,
        Keysym::Down => Key::Down,
        Keysym::Left => Key::Left,
        Keysym::Right => Key::Right,
        Keysym::Home => Key::Home,
        Keysym::End => Key::End,
        _ => character(event),
    }
}

// the text a key types already has shift and the keyboard layout applied
fn character(event: &KeyEvent) -> Key {
    let Some(text) = &event.utf8 else {
        return Key::Other;
    };

    let mut letters = text.chars();

    // a key that types more than one character has no single letter to give
    let (Some(letter), None) = (letters.next(), letters.next()) else {
        return Key::Other;
    };

    // with ctrl held, keys type invisible control characters
    if letter.is_control() {
        return Key::Other;
    }

    Key::Character(letter)
}
