//! power-profiles-daemon for `system.rs`: which power profile is on, and which the machine has,
//! for Controls to show and switch. Switching shows nothing on the island, Controls already does.

use std::thread;

use crate::bus::{Argument, Bus, Value};
use crate::later::Later;
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
    pub fn key(self) -> &'static str {
        match self {
            Profile::PowerSaver => "power-saver",
            Profile::Balanced => "balanced",
            Profile::Performance => "performance",
        }
    }

    pub fn of(key: &str) -> Option<Profile> {
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

fn set_now(profile: Profile) {
    Bus::system().set_property(
        NAME,
        PATH,
        NAME,
        "ActiveProfile",
        Argument::from(profile.key()),
    );
}

// the daemon answers with the change, which `system::follow` reads back
pub fn set(profile: Profile) {
    thread::spawn(move || set_now(profile));
}

// what `kanade power-profile` asks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Set(Profile),
    Cycle,
    Status,
}

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        match arguments {
            ["set", profile] => Profile::of(profile).map(Request::Set),
            ["cycle"] => Some(Request::Cycle),
            ["status"] => Some(Request::Status),
            _ => None,
        }
    }
}

// `kanade power-profile`, answered once the daemon has the profile as asked
pub fn request(request: Request, later: Later) {
    let profiles = Profiles::read().clone();

    let Some(active) = profiles.active else {
        return later.refused("unavailable: power-profiles-daemon is not running");
    };

    let want = match request {
        Request::Status => {
            let available: Vec<&str> = profiles.available.iter().map(|each| each.key()).collect();

            return later.done(format!(
                "{} (available: {})",
                active.key(),
                available.join(", ")
            ));
        }
        Request::Set(profile) => profile,
        // the next one the machine has, after this one, then the first again
        Request::Cycle => profiles
            .available
            .iter()
            .position(|each| *each == active)
            .map_or(active, |at| {
                profiles.available[(at + 1) % profiles.available.len()]
            }),
    };

    if !profiles.available.contains(&want) {
        return later.refused(format!("{} is not available on this machine", want.key()));
    }

    later.unsettled("power-profiles-daemon got the call; the profile may still have changed");

    thread::spawn(move || {
        set_now(want);

        let read = Bus::system().property(NAME, PATH, NAME, "ActiveProfile");

        match Profile::of(read.text()) {
            Some(now) if now == want => later.done(now.key()),
            _ => later.refused(format!(
                "the daemon left it {}",
                if read.text().is_empty() {
                    "unset"
                } else {
                    read.text()
                }
            )),
        }
    });
}
