use smithay_client_toolkit::seat::keyboard::{
    KeyEvent, KeyboardHandler, Keysym, Modifiers as SctkModifiers, RawModifiers,
};
use wayland_client::{
    Connection, QueueHandle,
    protocol::{wl_keyboard::WlKeyboard, wl_surface::WlSurface},
};

use crate::input::{focus, paste, released};
use crate::{Key, Modifiers};

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
        self.modifiers = Modifiers::default();

        // keys stop arriving, so no text input can stay focused, and no key is held
        focus::clear();
        released::send();

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
        self.deliver(to_key(&event, self.modifiers), self.modifiers);
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
        released::send();
    }

    // the state keys are read with, kept here since a key event does not carry it
    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlKeyboard,
        _: u32,
        modifiers: SctkModifiers,
        _: RawModifiers,
        _: u32,
    ) {
        self.modifiers = Modifiers {
            shift: modifiers.shift,
            ctrl: modifiers.ctrl,
            alt: modifiers.alt,
            logo: modifiers.logo,
        };
    }
}

impl WaylandState {
    // a held key, again: only the keys that edit and move repeat, as on macOS, where a held letter
    // offers accents instead
    pub(super) fn repeat(&mut self, event: &KeyEvent) {
        let key = to_key(event, self.modifiers);

        if repeats(key) {
            self.deliver(key, self.modifiers);
        }
    }

    // to the focused text input, else the focused window's on_key
    fn deliver(&mut self, key: Key, modifiers: Modifiers) {
        let Some(surface) = self.keyboard_focus.clone() else {
            return;
        };

        self.deliver_to(&surface, key, modifiers, true);
    }

    // a key for a window; `to_input` is whether the focused text input is to have it first
    pub(super) fn deliver_to(
        &mut self,
        surface: &WlSurface,
        key: Key,
        modifiers: Modifiers,
        to_input: bool,
    ) {
        // a focused text input takes the key before the window's on_key sees it
        if to_input && focus::send(key, modifiers) {
            if let Some(done) = paste::take() {
                self.paste(done, surface);
            }

            self.request_frame_on(surface);

            return;
        }

        // a window has no use for a shortcut's letter, which it would take for typing
        let key = match key {
            Key::Character(_) if modifiers.ctrl => Key::Other,
            key => key,
        };

        let Some(window) = self.window(surface) else {
            return;
        };

        let Some(on_key) = &window.on_key else {
            return;
        };

        on_key(key);

        // a service the handler changed wakes its own readers, this window redraws for the rest
        self.request_frame_on(surface);
    }
}

fn repeats(key: Key) -> bool {
    matches!(
        key,
        Key::Backspace | Key::Delete | Key::Up | Key::Down | Key::Left | Key::Right
    )
}

fn to_key(event: &KeyEvent, modifiers: Modifiers) -> Key {
    match event.keysym {
        Keysym::Return | Keysym::KP_Enter => Key::Enter,
        Keysym::Escape => Key::Escape,
        Keysym::Tab => Key::Tab,
        Keysym::ISO_Left_Tab => Key::BackTab,
        Keysym::BackSpace => Key::Backspace,
        Keysym::space => Key::Space,
        Keysym::Up => Key::Up,
        Keysym::Down => Key::Down,
        Keysym::Left => Key::Left,
        Keysym::Right => Key::Right,
        Keysym::Home => Key::Home,
        Keysym::End => Key::End,
        Keysym::Delete | Keysym::KP_Delete => Key::Delete,
        Keysym::Page_Up => Key::PageUp,
        Keysym::Page_Down => Key::PageDown,
        _ if modifiers.ctrl => shortcut(event),
        _ => character(event),
    }
}

// with ctrl held a key types a control character, so the key's own letter names it, lower case
fn shortcut(event: &KeyEvent) -> Key {
    let Some(letter) = event
        .keysym
        .key_char()
        .filter(|letter| !letter.is_control())
    else {
        return Key::Other;
    };

    let mut lower = letter.to_lowercase();

    match (lower.next(), lower.next()) {
        (Some(letter), None) => Key::Character(letter),
        _ => Key::Other,
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

    // a key can still type an invisible control character
    if letter.is_control() {
        return Key::Other;
    }

    Key::Character(letter)
}
