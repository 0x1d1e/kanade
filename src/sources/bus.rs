//! What the D-Bus daemon says about a bus name: who holds it, and whether one would start on demand.

use std::fs;

use amane::{Argument, Bus};

const DBUS: &str = "org.freedesktop.DBus";
const PATH: &str = "/org/freedesktop/DBus";

// whether the bus answers at all; Amane's Bus answers nothing for one it could not reach
pub fn reachable(bus: Bus) -> bool {
    !bus.call(DBUS, PATH, DBUS, "GetId", &[]).text().is_empty()
}

// the unique name, like :1.42, of whoever has `name`, which a signal names as its sender
pub fn unique(bus: Bus, name: &str) -> Option<String> {
    let owner = bus.call(DBUS, PATH, DBUS, "GetNameOwner", &[Argument::from(name)]);

    Some(owner.text().to_owned()).filter(|owner| !owner.is_empty())
}

// the process that has `name`, none while no one has it
pub fn owner(bus: Bus, name: &str) -> Option<u32> {
    let owner = unique(bus, name)?;
    let pid = bus.call(
        DBUS,
        PATH,
        DBUS,
        "GetConnectionUnixProcessID",
        &[Argument::from(owner)],
    );

    Some(pid.number() as u32).filter(|&pid| pid > 0)
}

// whether the bus starts a service for `name` when asked, like BlueZ
pub fn activatable(bus: Bus, name: &str) -> bool {
    bus.call(DBUS, PATH, DBUS, "ListActivatableNames", &[])
        .list()
        .iter()
        .any(|each| each.text() == name)
}

// as the process names itself, like "mako"
pub fn process(pid: u32) -> String {
    fs::read_to_string(format!("/proc/{pid}/comm"))
        .map_or_else(|_| format!("process {pid}"), |name| name.trim().to_owned())
}
