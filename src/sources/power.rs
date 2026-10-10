//! power-profiles-daemon for `system.rs`: which power profile is on, and which the machine has,
//! for Controls to show and switch. Switching shows nothing on the island, Controls already does.

use std::thread;

use crate::bus::{Argument, Bus, Value};
use kanade_runtime::service::Service;

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
