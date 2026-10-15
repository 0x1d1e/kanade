//! The desktop's accessibility preferences, from the settings portal (ADR 0080): reduced motion
//! and higher contrast, `org.freedesktop.appearance` `reduced-motion` and `contrast`. `config`
//! reads them when it layers the files, so a preference the desktop states reaches the shell as
//! the setting it stands for, without a file being written.
//!
//! Followed over the session bus: the namespace is read once, after the portal's `SettingChanged`
//! is subscribed to, so no change falls between; each change reads the config again. A desktop
//! with no portal says nothing, and nothing changes. The thread blocks on the signal, so it adds
//! no idle wakeups of its own.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::bus::{Argument, Bus, Value};
use crate::reload;

const DESTINATION: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.freedesktop.appearance";

static REDUCED_MOTION: AtomicBool = AtomicBool::new(false);
static CONTRAST: AtomicBool = AtomicBool::new(false);

/// What the desktop prefers; `false` is "no preference", also for a value this does not know.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Preferences {
    pub reduced_motion: bool,
    pub higher_contrast: bool,
}

/// What the portal last said, none until it has.
pub fn current() -> Preferences {
    Preferences {
        reduced_motion: REDUCED_MOTION.load(Ordering::Relaxed),
        higher_contrast: CONTRAST.load(Ordering::Relaxed),
    }
}

/// Runs on its own thread for good, or ends at once without a session bus or portal signals.
pub fn follow() {
    let signals = Bus::session().signals_from(PATH, SETTINGS, "SettingChanged");

    // read after subscribing, so a change in between is heard
    let asked = read();

    if store(asked) {
        reload::reload();
    }

    for signal in signals {
        let [namespace, key, value] = signal.arguments() else {
            continue;
        };

        let mut next = current();

        if namespace.text() != NAMESPACE || !next.set(key.text(), value) {
            continue;
        }

        if store(next) {
            reload::reload();
        }
    }
}

// the namespace as the portal has it now; what it does not answer stays no preference
fn read() -> Preferences {
    let all = Bus::session().call(
        DESTINATION,
        PATH,
        SETTINGS,
        "ReadAll",
        &[Argument::TextList(vec![NAMESPACE.to_owned()])],
    );

    Preferences::of(all.get(NAMESPACE))
}

// whether it differs from what was stored
fn store(preferences: Preferences) -> bool {
    let before = current();

    REDUCED_MOTION.store(preferences.reduced_motion, Ordering::Relaxed);
    CONTRAST.store(preferences.higher_contrast, Ordering::Relaxed);

    before != preferences
}

impl Preferences {
    /// From the namespace's keys, as `ReadAll` gives them.
    fn of(keys: &Value) -> Self {
        let mut preferences = Self::default();

        for key in ["reduced-motion", "contrast"] {
            preferences.set(key, keys.get(key));
        }

        preferences
    }

    // whether `key` is one of these; the portal says 1 for a preference, and anything else, a value
    // of another kind included, is none
    fn set(&mut self, key: &str, value: &Value) -> bool {
        let preferred = matches!(value, Value::Number(number) if *number == 1.0);

        match key {
            "reduced-motion" => self.reduced_motion = preferred,
            "contrast" => self.higher_contrast = preferred,
            _ => return false,
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn keys(entries: &[(&str, Value)]) -> Value {
        Value::Map(
            entries
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    #[test]
    fn only_one_is_a_preference() {
        let said = keys(&[
            ("reduced-motion", Value::Number(1.0)),
            ("contrast", Value::Number(0.0)),
            ("color-scheme", Value::Number(1.0)),
        ]);

        assert_eq!(
            Preferences::of(&said),
            Preferences {
                reduced_motion: true,
                higher_contrast: false,
            }
        );
    }

    #[test]
    fn what_is_unknown_or_of_another_kind_is_no_preference() {
        for value in [
            Value::Number(2.0),
            Value::Number(-1.0),
            Value::Bool(true),
            Value::Text(String::from("1")),
            Value::Nothing,
        ] {
            let said = keys(&[("reduced-motion", value.clone()), ("contrast", value)]);

            assert_eq!(Preferences::of(&said), Preferences::default());
        }

        assert_eq!(Preferences::of(&Value::Nothing), Preferences::default());
    }

    #[test]
    fn a_change_names_the_key_it_is_for() {
        let mut preferences = Preferences::default();

        assert!(preferences.set("contrast", &Value::Number(1.0)));
        assert!(preferences.higher_contrast);

        assert!(!preferences.set("accent-color", &Value::Number(1.0)));
        assert_eq!(
            preferences,
            Preferences {
                reduced_motion: false,
                higher_contrast: true,
            }
        );

        assert!(preferences.set("contrast", &Value::Number(0.0)));
        assert!(!preferences.higher_contrast);
    }
}
