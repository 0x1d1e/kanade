//! Network and Bluetooth (plan 2, 5.1, 7): one thread follows the system bus for both. Joining or
//! leaving a network, or a Bluetooth device connecting or going away, shows as a short Transient on
//! the focused island; nothing about them stays on it, so there is never a permanent Wi-Fi
//! indicator. The latest state is kept in `Connectivity` and `Adapter` for the Controls Surface.
//!
//! Amane's Network and Bluetooth poll every second or two for good once read, so this never reads
//! them. It waits on PropertiesChanged signals instead and asks NetworkManager or BlueZ again only
//! when one of their own objects says something changed: at idle its threads sleep, and a signal
//! from anyone else, like an access point's strength, is dropped by its path without a call.

use std::sync::mpsc;
use std::time::{Duration, Instant};
use std::{iter, thread};

use amane::{Bus, Service, Value};

use super::{bluetooth, network};
use crate::island::activity::Activity;
use crate::island::service::IslandService;

// longer than OSD, so a network's name can be read; shorter than a toast, nothing to act on
pub const SHOWN: Duration = Duration::from_millis(2000);

// a radio the Controls Surface can switch, or show disabled (plan 7)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Radio {
    // no Wi-Fi device or Bluetooth adapter, or its daemon is not running
    #[default]
    Missing,

    Off,
    On,
}

impl Radio {
    pub fn of(present: bool, on: bool) -> Radio {
        match (present, on) {
            (false, _) => Radio::Missing,
            (true, false) => Radio::Off,
            (true, true) => Radio::On,
        }
    }
}

// which daemon said something changed
#[derive(Clone, Copy, PartialEq)]
enum Daemon {
    NetworkManager,
    BlueZ,
}

// runs on its own thread for good, or until the system bus goes away
pub fn follow() {
    // subscribed before the first read, so no change falls between them
    let signals = Bus::system().signals("org.freedesktop.DBus.Properties", "PropertiesChanged");

    /*
     * signals queue up to 64 while unread, and a full queue stops the bus reading anything, replies
     * included: a read made while a burst arrives, as switching networks sends, would wait for
     * good. So another thread only sorts signals, and never calls
     */
    let (sender, changed) = mpsc::channel();

    thread::spawn(move || {
        for signal in signals {
            let interface = signal.arguments().first().map_or("", Value::text);

            let daemon = if network::concerns(signal.path(), interface) {
                Daemon::NetworkManager
            } else if bluetooth::concerns(signal.path(), interface) {
                Daemon::BlueZ
            } else {
                continue;
            };

            if sender.send(daemon).is_err() {
                return;
            }
        }
    });

    // the first reads only set where things start, so starting the shell shows nothing
    *network::Connectivity::write() = network::read();
    *bluetooth::Adapter::write() = bluetooth::read();

    while let Ok(first) = changed.recv() {
        // a burst, or what came during the last read, asks once
        let daemons: Vec<Daemon> = iter::once(first).chain(changed.try_iter()).collect();

        if daemons.contains(&Daemon::NetworkManager) {
            refresh(network::read(), network::changes);
        }

        if daemons.contains(&Daemon::BlueZ) {
            refresh(bluetooth::read(), bluetooth::changes);
        }
    }
}

/*
 * keeps `now` for Controls and posts what changed since the last read. A read that changes nothing
 * writes nothing, so a signal about something Kanade does not show redraws nothing
 */
fn refresh<S: Service + PartialEq>(now: S, changes: fn(&S, &S) -> Vec<Activity>) {
    let posts = {
        let before = S::read();

        if *before == now {
            return;
        }

        changes(&before, &now)
    };

    *S::write() = now;

    if posts.is_empty() {
        return;
    }

    let at = Instant::now();
    let mut island = IslandService::write();

    for activity in posts {
        island.post(activity, at);
    }
}
