//! The keyboard layout in use, as the lock screen shows it beside Caps Lock (ADR 0089): niri says
//! which it is, the shell hands the label in with the scene. A label is only a hint: nothing here
//! can keep the lock from locking or unlocking, and a layout niri never told leaves it empty.

use kanade_runtime::service::Service;

use crate::sources::xkb;

// the short label of the layout in use, like "US"; empty when it is unknown or there is only one
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Keyboard {
    pub(super) label: String,
}

impl Service for Keyboard {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {}
}

/*
 * niri says the layout in use (`niri::on_keyboard`), by the name XKB gives it, on the niri
 * thread. Read first, as a write redraws the scenes
 */
pub fn heard(name: Option<String>) {
    let label = name.map(|name| xkb::short(&name)).unwrap_or_default();

    if Keyboard::read().label != label {
        Keyboard::write().label = label;
    }
}
