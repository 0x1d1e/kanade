//! power-profiles-daemon for `system.rs`: which power profile is on, and which the machine has,
//! for Controls to show and switch. Switching shows nothing on the island, Controls already does.

use std::thread;

use amane::{Argument, Bus, Service, Value};

pub const NAME: &str = "org.freedesktop.UPower.PowerProfiles";

pub const PATH: &str = "/org/freedesktop/UPower/PowerProfiles";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    PowerSaver,
    Balanced,
    Performance,
}

impl Profile {
    pub const ALL: [Profile; 3] = [Profile::PowerSaver, Profile::Balanced, Profile::Performance];

    // the daemon's name for it
    fn key(self) -> &'static str {
        match self {
            Profile::PowerSaver => "power-saver",
            Profile::Balanced => "balanced",
            Profile::Performance => "performance",
        }
    }

    fn of(key: &str) -> Option<Profile> {
        Profile::ALL
            .into_iter()
            .find(|profile| profile.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Profile::PowerSaver => "Power Saver",
            Profile::Balanced => "Balanced",
            Profile::Performance => "Performance",
        }
    }
}

// written only by `system::follow`; nothing while the daemon is not running
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profiles {
    pub active: Option<Profile>,

    // Performance needs hardware support, Balanced and Power Saver are always there
    pub available: Vec<Profile>,
}

impl Service for Profiles {
    fn new() -> Self {
        Profiles::default()
    }

    fn listen() {}
}

// the daemon's one object says when the profile, or the ones there are, change
pub fn concerns(path: &str, interface: &str) -> bool {
    path == PATH && interface == NAME
}

// asks the daemon from scratch
pub fn read() -> Profiles {
    let properties = Bus::system().call(
        NAME,
        PATH,
        "org.freedesktop.DBus.Properties",
        "GetAll",
        &[Argument::from(NAME)],
    );

    profiles(&properties)
}

fn profiles(properties: &Value) -> Profiles {
    let available = properties
        .get("Profiles")
        .list()
        .iter()
        .filter_map(|profile| Profile::of(profile.get("Profile").text()))
        .collect();

    Profiles {
        active: Profile::of(properties.get("ActiveProfile").text()),
        available,
    }
}

// the daemon answers with the change, which `system::follow` reads back
pub fn set(profile: Profile) {
    thread::spawn(move || {
        Bus::system().set_property(
            NAME,
            PATH,
            NAME,
            "ActiveProfile",
            Argument::from(profile.key()),
        );
    });
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn map(entries: &[(&str, Value)]) -> Value {
        Value::Map(
            entries
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    fn text(text: &str) -> Value {
        Value::Text(text.into())
    }

    fn offered(keys: &[&str]) -> Value {
        Value::List(
            keys.iter()
                .map(|key| map(&[("Profile", text(key)), ("Driver", text("multiple"))]))
                .collect(),
        )
    }

    #[test]
    fn reads_the_active_profile_and_the_ones_there_are() {
        let properties = map(&[
            ("ActiveProfile", text("balanced")),
            ("Profiles", offered(&["power-saver", "balanced"])),
        ]);

        assert_eq!(
            profiles(&properties),
            Profiles {
                active: Some(Profile::Balanced),
                available: vec![Profile::PowerSaver, Profile::Balanced],
            }
        );
    }

    #[test]
    fn no_daemon_has_no_profiles() {
        assert_eq!(profiles(&Value::Nothing), Profiles::default());
    }

    // a newer daemon may add one Controls has no place for
    #[test]
    fn unknown_profiles_are_left_out() {
        let properties = map(&[
            ("ActiveProfile", text("turbo")),
            ("Profiles", offered(&["turbo", "performance"])),
        ]);

        assert_eq!(
            profiles(&properties),
            Profiles {
                active: None,
                available: vec![Profile::Performance],
            }
        );
    }
}
