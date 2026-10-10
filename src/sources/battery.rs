//! Low and critical battery (plan 5.1, 7): one Persistent Activity while the battery drains below
//! `LOW`, shown amber with its number. Below `CRITICAL` it turns Critical, red, and preempts an open
//! Surface. Charging withdraws it.
//!
//! The kernel's power supply files have no subscription, so this reads them every 5 s, and every
//! second for a while after the kernel announces a charger going in or out (`wake`), as the status
//! may settle after the announcement. A read that changes nothing the island shows posts nothing.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kanade_runtime::service::Service;

use super::wake::{Announcer, Pace, Wakes};
use crate::island::activity::{
    Activity, Charge, Detail, Id, Interrupt, Kind, Lifetime, Priority, Scope,
};
use crate::island::service::IslandService;
use crate::supervise;

// at or below, the battery shows (plan 7: amber)
const LOW: u8 = 20;

// at or below, it preempts (plan 5.1 rule 4: red)
const CRITICAL: u8 = 10;

const PACE: Pace = Pace {
    // a charger that goes in shows within this of the kernel's status
    poll: Duration::from_secs(1),

    // longer than a supply's status takes to settle after it is announced
    settle: Duration::from_secs(6),

    // a draining battery announces nothing on some laptops, so its percent is read this often
    idle: Some(Duration::from_secs(5)),
};

// the kernel announces each change to a power supply, like a charger going in
pub const POWER: Announcer = Announcer {
    program: "udevadm",
    args: &["monitor", "--kernel", "--subsystem-match=power_supply"],
    announces: |line| line.starts_with("KERNEL["),
};

const POWER_SUPPLIES: &str = "/sys/class/power_supply";

// the battery as Rest shows it, `rest.battery`; written only by `follow`, none without a battery
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BatteryLevel {
    pub percent: Option<u8>,
    pub charging: bool,
}

impl BatteryLevel {
    // whether the machine has a battery now: a desktop's Rest has none to show, a hot-plugged one shows
    pub fn present(self) -> bool {
        self.percent.is_some()
    }
}

impl From<Reading> for BatteryLevel {
    fn from(reading: Reading) -> Self {
        BatteryLevel {
            percent: reading.present.then_some(reading.percent),
            charging: reading.present && !reading.draining,
        }
    }
}

impl Service for BatteryLevel {
    // read as it is first asked, so Rest knows at its first frame whether there is a battery
    fn new() -> Self {
        Reading::read().into()
    }

    fn listen() {}
}

// the battery as last read
#[derive(Debug, Clone, Copy, PartialEq)]
struct Reading {
    // false without a battery, as on a desktop
    present: bool,
    percent: u8,

    // on battery power; false without a battery, or plugged in, charging or not
    draining: bool,
}

impl Reading {
    // the first supply whose type is Battery, so a laptop's AC adapter is skipped
    fn read() -> Reading {
        let Some(path) = battery() else {
            return Reading {
                present: false,
                percent: 0,
                draining: false,
            };
        };

        // `Not charging` is plugged in, usually because it is full
        let status = supply(&path, "status");

        Reading {
            present: true,
            percent: supply(&path, "capacity").parse().unwrap_or(0),
            draining: !matches!(status.as_str(), "Charging" | "Full" | "Not charging"),
        }
    }
}

/*
 * what the island shows for `now`, given what it shows. Once shown, the charge stays shown, and
 * once critical stays critical, until the charger goes in: a reading that wobbles one percent up
 * at the threshold neither withdraws nor de-escalates it
 */
fn charge(shown: Option<Charge>, now: Reading) -> Option<Charge> {
    if !now.draining {
        return None;
    }

    let critical = now.percent <= CRITICAL || shown.is_some_and(|charge| charge.critical);

    (critical || now.percent <= LOW || shown.is_some()).then_some(Charge {
        percent: now.percent,
        critical,
    })
}

fn battery() -> Option<PathBuf> {
    fs::read_dir(POWER_SUPPLIES)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| supply(path, "type") == "Battery")
}

fn supply(path: &Path, name: &str) -> String {
    let text = fs::read_to_string(path.join(name)).unwrap_or_default();

    text.trim().to_owned()
}

fn activity(charge: Charge) -> Activity {
    // Ongoing, so it becomes a Satellite beside a higher primary rather than nag over it
    let (priority, interrupt) = if charge.critical {
        (Priority::Critical, Interrupt::Preempt)
    } else {
        (Priority::Ongoing, Interrupt::None)
    };

    Activity::new(
        id(),
        priority,
        Lifetime::Persistent,
        Scope::Global,
        interrupt,
    )
    .expect("a Persistent that does not auto-expand is valid")
    .with_detail(Detail::Battery(charge))
}

// one, so escalating to critical replaces the low one
fn id() -> Id {
    Id::new(Kind::Battery, "battery")
}

// runs on its own thread for good
pub fn follow() {
    let mut wakes = Wakes::new(PACE, vec![POWER]);
    let mut shown = None;
    let mut last = None;

    supervise::run("battery", || {
        loop {
            let reading = Reading::read();
            let next = charge(shown, reading);

            if next != shown {
                let now = Instant::now();
                let mut island = IslandService::write();

                match next {
                    Some(charge) => island.post(activity(charge), now),
                    None => island.withdraw(&id(), now),
                }
            }

            let busy = last != Some(reading);
            if busy {
                *BatteryLevel::write() = reading.into();
            }

            shown = next;
            last = Some(reading);
            wakes.wait(busy);
        }
    });
}
