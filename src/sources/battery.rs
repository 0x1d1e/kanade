//! Low and critical battery (plan 5.1, 7): one Persistent Activity while the battery drains below
//! `LOW`, shown amber with its number. Below `CRITICAL` it turns Critical, red, and preempts an open
//! Surface. Charging withdraws it.
//!
//! Amane's Battery reads the kernel's files every 5 s, and has no subscription, so this reads it
//! again as often, and every second for a while after the kernel announces a charger going in or
//! out (`wake`), so that shows within a second of Amane's own read. A read that changes nothing the
//! island shows posts nothing.

use std::time::{Duration, Instant};

use amane::{Battery, Service};

use super::wake::{Announcer, Pace, Wakes};
use crate::island::activity::{Activity, Charge, Detail, Id, Kind, Priority};
use crate::island::service::IslandService;
use crate::supervise;

// at or below, the battery shows (plan 7: amber)
const LOW: u8 = 20;

// at or below, it preempts (plan 5.1 rule 4: red)
const CRITICAL: u8 = 10;

const PACE: Pace = Pace {
    // a charger that goes in shows within this of Amane's own read
    poll: Duration::from_secs(1),

    // longer than Amane takes to read the battery again, every 5 s
    settle: Duration::from_secs(6),

    // a draining battery announces nothing on some laptops, so its percent is read this often
    idle: Some(Duration::from_secs(5)),
};

// the kernel announces each change to a power supply, like a charger going in
const POWER: Announcer = Announcer {
    program: "udevadm",
    args: &["monitor", "--kernel", "--subsystem-match=power_supply"],
    announces: |line| line.starts_with("KERNEL["),
};

// the battery as Amane last read it
#[derive(Debug, Clone, Copy, PartialEq)]
struct Reading {
    percent: u8,

    // on battery power; false without a battery, or plugged in, charging or not
    draining: bool,
}

impl Reading {
    fn read() -> Reading {
        let battery = Battery::read();

        Reading {
            percent: battery.percent(),
            draining: battery.present() && !battery.charging() && !battery.full(),
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

fn activity(charge: Charge) -> Activity {
    // Ongoing, so it becomes a Satellite beside a higher primary rather than nag over it
    let priority = if charge.critical {
        Priority::Critical
    } else {
        Priority::Ongoing
    };

    Activity::persistent(id(), priority).with_detail(Detail::Battery(charge))
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
            shown = next;
            last = Some(reading);
            wakes.wait(busy);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::activity::{Interrupt, Lifetime};

    fn draining(percent: u8) -> Reading {
        Reading {
            percent,
            draining: true,
        }
    }

    fn low(percent: u8) -> Option<Charge> {
        Some(Charge {
            percent,
            critical: false,
        })
    }

    fn critical(percent: u8) -> Option<Charge> {
        Some(Charge {
            percent,
            critical: true,
        })
    }

    #[test]
    fn thresholds() {
        assert_eq!(charge(None, draining(21)), None);
        assert_eq!(charge(None, draining(LOW)), low(20));
        assert_eq!(charge(None, draining(11)), low(11));
        assert_eq!(charge(None, draining(CRITICAL)), critical(10));
        assert_eq!(charge(None, draining(0)), critical(0));
    }

    #[test]
    fn a_drain_escalates_low_to_critical() {
        let shown = charge(None, draining(15));
        assert_eq!(shown, low(15));

        let shown = charge(shown, draining(14));
        assert_eq!(shown, low(14));

        assert_eq!(charge(shown, draining(10)), critical(10));
    }

    #[test]
    fn only_a_charge_withdraws() {
        let wobble = |shown, percent| charge(shown, draining(percent));

        assert_eq!(wobble(low(20), 21), low(21));
        assert_eq!(wobble(critical(10), 11), critical(11));

        let plugged = Reading {
            percent: 5,
            draining: false,
        };

        assert_eq!(charge(critical(5), plugged), None);
        assert_eq!(charge(low(15), plugged), None);
        assert_eq!(charge(None, plugged), None);
    }

    // a low battery is a Satellite beside a higher primary, a critical one preempts
    #[test]
    fn critical_preempts_and_low_does_not() {
        let low = activity(low(15).unwrap());
        let critical = activity(critical(8).unwrap());

        assert_eq!(low.id(), critical.id());
        assert_eq!(low.lifetime(), Lifetime::Persistent);
        assert_eq!(low.priority(), Priority::Ongoing);
        assert_eq!(low.interrupt(), Interrupt::Never);
        assert_eq!(critical.priority(), Priority::Critical);
        assert_eq!(critical.interrupt(), Interrupt::Preempt);
    }
}
