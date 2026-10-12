//! Low and critical battery (plan 5.1, 7): one Persistent Activity while the battery drains to
//! `battery.low`, shown amber with its number. At `battery.critical` it turns Critical, red, and
//! preempts an open Surface, and `battery.critical_action`, unless none, counts down through the
//! session Module as a restart does, so Cancel stops it. Charging withdraws it and cancels the
//! countdown.
//!
//! The kernel's power supply files have no subscription, so this reads them every 5 s, and every
//! second for a while after the kernel announces a charger going in or out (`wake`), as the status
//! may settle after the announcement. A read that changes nothing the island shows posts nothing.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kanade_runtime::service::Service;

use super::session;
use super::wake::{Announcer, Pace, Wakes};
use crate::config::{self, CriticalAction};
use crate::island::activity::{
    Activity, Charge, Detail, Ending, Id, Interrupt, Kind, Lifetime, Priority, Scope,
};
use crate::island::service::IslandService;
use crate::{modules, supervise};

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

// at or below, the battery shows amber (plan 7), and at or below `critical` red, preempting
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Thresholds {
    low: u8,
    critical: u8,
}

impl Thresholds {
    fn configured() -> Thresholds {
        let battery = config::get().battery;
        let percent = |value: u32| u8::try_from(value).unwrap_or(u8::MAX);

        Thresholds {
            low: percent(battery.low),
            critical: percent(battery.critical()),
        }
    }
}

/*
 * what the island shows for `now`, given what it shows. Once shown, the charge stays shown, and
 * once critical stays critical, until the charger goes in: a reading that wobbles one percent up
 * at the threshold neither withdraws nor de-escalates it
 */
fn charge(shown: Option<Charge>, now: Reading, at: Thresholds) -> Option<Charge> {
    if !now.draining {
        return None;
    }

    let critical = now.percent <= at.critical || shown.is_some_and(|charge| charge.critical);

    (critical || now.percent <= at.low || shown.is_some()).then_some(Charge {
        percent: now.percent,
        critical,
    })
}

// what the critical battery asks of the session Module
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ask {
    CountDown(Ending),
    Cancel(u64),
}

/*
 * counts down once each time the battery turns critical while draining: Cancel, or a countdown that
 * ran out and woke on battery, does not start another until a charger has gone in, which also
 * cancels one counting
 */
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Guard {
    armed: bool,

    // the countdown it started, by serial, while it may still be counting
    counting: Option<u64>,
}

impl Guard {
    fn new() -> Guard {
        Guard {
            armed: true,
            counting: None,
        }
    }

    // what to ask after a reading; `critical` as the island shows it, so it is sticky too
    fn next(&mut self, draining: bool, critical: bool, end: Option<Ending>) -> Option<Ask> {
        if !draining {
            self.armed = true;
            return self.counting.take().map(Ask::Cancel);
        }

        let end = end.filter(|_| critical && self.armed)?;
        self.armed = false;

        Some(Ask::CountDown(end))
    }

    fn started(&mut self, serial: u64) {
        self.counting = Some(serial);
    }

    // a countdown not asked after all, asked again at the next reading
    fn retry(&mut self) {
        self.armed = true;
    }
}

fn ending(action: CriticalAction) -> Option<Ending> {
    match action {
        CriticalAction::None => None,
        CriticalAction::Suspend => Some(Ending::Suspend),
        CriticalAction::Hibernate => Some(Ending::Hibernate),
        CriticalAction::PowerOff => Some(Ending::PowerOff),
    }
}

// asks the session Module, which owns countdowns; a refusal shows on the island from there
fn ask(guard: &mut Guard, ask: Ask) {
    match ask {
        Ask::Cancel(serial) => session::cancel(serial),
        Ask::CountDown(_) if !modules::on("session") => {
            eprintln!(
                "kanade: battery: battery.critical_action needs the session Module, which is off"
            );
        }
        // a battery critical as Kanade starts, read before the session thread is
        Ask::CountDown(_) if !session::started() => guard.retry(),
        Ask::CountDown(end) => match session::count_down(end) {
            Ok(serial) => guard.started(serial),
            Err(why) => eprintln!("kanade: battery: cannot count down: {why}"),
        },
    }
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
    let mut guard = Guard::new();

    supervise::run("battery", || {
        loop {
            let reading = Reading::read();
            let next = charge(shown, reading, Thresholds::configured());

            if next != shown {
                let now = Instant::now();
                let mut island = IslandService::write();

                match next {
                    Some(charge) => island.post(activity(charge), now),
                    None => island.withdraw(&id(), now),
                }
            }

            let critical = next.is_some_and(|charge| charge.critical);
            let end = ending(config::get().battery.critical_action);

            if let Some(next) = guard.next(reading.draining, critical, end) {
                ask(&mut guard, next);
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

#[cfg(test)]
mod tests {
    use super::*;

    const AT: Thresholds = Thresholds {
        low: 20,
        critical: 10,
    };

    fn draining(percent: u8) -> Reading {
        Reading {
            present: true,
            percent,
            draining: true,
        }
    }

    fn shown(percent: u8, critical: bool) -> Option<Charge> {
        Some(Charge { percent, critical })
    }

    #[test]
    fn the_battery_shows_at_its_thresholds_and_holds_until_charging() {
        assert_eq!(charge(None, draining(21), AT), None);
        assert_eq!(charge(None, draining(20), AT), shown(20, false));
        assert_eq!(charge(None, draining(10), AT), shown(10, true));

        // a wobble up past a threshold neither withdraws nor de-escalates
        assert_eq!(charge(shown(20, false), draining(21), AT), shown(21, false));
        assert_eq!(charge(shown(10, true), draining(11), AT), shown(11, true));

        // a charger withdraws it, at any percent
        let plugged = Reading {
            draining: false,
            ..draining(5)
        };
        assert_eq!(charge(shown(5, true), plugged, AT), None);
    }

    #[test]
    fn a_critical_battery_counts_down_once_until_a_charger_goes_in() {
        let end = Some(Ending::Suspend);
        let mut guard = Guard::new();

        assert_eq!(guard.next(true, false, end), None);
        assert_eq!(
            guard.next(true, true, end),
            Some(Ask::CountDown(Ending::Suspend))
        );
        guard.started(7);

        // Cancel, or waking on battery after it ran out, starts no other
        assert_eq!(guard.next(true, true, end), None);

        // a charger cancels it and re-arms
        assert_eq!(guard.next(false, false, end), Some(Ask::Cancel(7)));
        assert_eq!(guard.next(false, false, end), None);
        assert_eq!(
            guard.next(true, true, end),
            Some(Ask::CountDown(Ending::Suspend))
        );
    }

    #[test]
    fn no_action_counts_nothing_down_and_stays_armed() {
        let mut guard = Guard::new();

        assert_eq!(guard.next(true, true, None), None);
        assert_eq!(
            guard.next(true, true, Some(Ending::Hibernate)),
            Some(Ask::CountDown(Ending::Hibernate))
        );
    }
}
