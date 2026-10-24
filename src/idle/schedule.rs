//! When each idle stage fires (ADR 0047): pure, so the rules that keep a session from being left
//! unlocked are tested without a compositor or a clock. Time is an input, never read here.

use std::time::{Duration, Instant};

// how long the dim takes to fall with no lock after it
pub const FADE: Duration = Duration::from_secs(5);

// how soon a lock that failed is asked for again, for as long as the user stays idle
pub const RETRY: Duration = Duration::from_secs(10);

/// After how long idle each stage fires, none for a stage that never does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stages {
    pub dim: Option<Duration>,
    pub lock: Option<Duration>,
    pub screen_off: Option<Duration>,
    pub suspend: Option<Duration>,
}

impl Stages {
    // the earliest stage, which the compositor's idle notification is asked for
    pub fn first(&self) -> Option<Duration> {
        self.each().map(|(_, after)| after).min()
    }

    // each stage that fires, in the order they are tried at the same moment
    fn each(&self) -> impl Iterator<Item = (Stage, Duration)> {
        [
            (Stage::Dim, self.dim),
            (Stage::Lock, self.lock),
            (Stage::ScreenOff, self.screen_off),
            (Stage::Suspend, self.suspend),
        ]
        .into_iter()
        .filter_map(|(stage, after)| Some((stage, after?)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Dim,
    Lock,
    ScreenOff,
    Suspend,
}

/// What the driver does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Fade the outputs toward dark until `until`; to full dark when the lock comes then.
    Dim {
        until: Instant,
        dark: bool,
    },

    /// Take the dim away at once.
    Undim,

    /// Lock as `kanade lock` does; `lock_failed` if it did not.
    Lock,

    ScreenOff,
    Suspend,
}

#[derive(Debug, Clone)]
pub struct Schedule {
    stages: Stages,

    // where the stages count from: when the user went idle, or an inhibitor that ended since
    idle: Option<Instant>,

    inhibited: bool,

    // the stages fired since the count began
    fired: Vec<Stage>,

    dimmed: bool,

    // when a lock that failed is asked for again
    retry: Option<Instant>,

    // the lock held since the count began
    locked: bool,

    // the stages never count from before this: when they last changed
    floor: Option<Instant>,
}

impl Schedule {
    pub fn new(stages: Stages) -> Self {
        Self {
            stages,
            idle: None,
            inhibited: false,
            fired: Vec::new(),
            dimmed: false,
            retry: None,
            locked: false,
            floor: None,
        }
    }

    /// New stages at `at`: a user already idle is counted from then, as after input.
    pub fn since(stages: Stages, at: Instant) -> Self {
        Self {
            floor: Some(at),
            ..Self::new(stages)
        }
    }

    pub fn stages(&self) -> Stages {
        self.stages
    }

    // since when the stages count, none while not idle or while inhibited
    pub fn counting(&self) -> Option<Instant> {
        self.idle.filter(|_| !self.inhibited)
    }

    /// The compositor said, at `at`, the user had been idle for the first stage's timeout.
    pub fn idled(&mut self, at: Instant) {
        if self.idle.is_some() {
            return;
        }

        let first = self.stages.first().unwrap_or_default();
        let since = at.checked_sub(first).unwrap_or(at);

        self.idle = Some(self.floor.map_or(since, |floor| since.max(floor)));
    }

    /// Input came: nothing fires until idle again, and the dim goes at once.
    pub fn resumed(&mut self) -> Vec<Action> {
        self.idle = None;
        self.reset()
    }

    /*
     * an inhibitor holds, or none does since `at`. While one holds nothing fires and the dim goes;
     * once none does, a user still idle is counted from then, as if input had just come
     */
    pub fn inhibit(&mut self, inhibited: bool, at: Instant) -> Vec<Action> {
        if inhibited == self.inhibited {
            return Vec::new();
        }

        self.inhibited = inhibited;

        if inhibited {
            return self.reset();
        }

        if self.idle.is_some() {
            self.idle = Some(at);
        }

        Vec::new()
    }

    /// The lock asked at `at` did not hold: ask again after `RETRY` while still idle.
    pub fn lock_failed(&mut self, at: Instant) -> Vec<Action> {
        if self.counting().is_some() {
            self.retry = Some(at + RETRY);
        }

        if std::mem::take(&mut self.dimmed) {
            vec![Action::Undim]
        } else {
            Vec::new()
        }
    }

    /// The lock asked for holds: the stages waiting on it may fire.
    pub fn lock_held(&mut self) {
        self.locked = true;
        self.retry = None;
    }

    /*
     * whether `stage` must wait for the lock, when there is one: the monitors going off or the
     * machine suspending would make a session that never locked look secured
     */
    fn waits(&self, stage: Stage) -> bool {
        matches!(stage, Stage::ScreenOff | Stage::Suspend)
            && !self.locked
            && self.stages.lock.is_some()
    }

    /// What fires by `now`, in the order of the stages' times.
    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        let Some(since) = self.counting() else {
            return Vec::new();
        };

        let mut due: Vec<(Instant, Stage)> = self
            .stages
            .each()
            .filter(|&(stage, _)| !self.waits(stage))
            .map(|(stage, after)| (since + after, stage))
            .filter(|(at, stage)| *at <= now && !self.fired.contains(stage))
            .collect();

        due.sort_by_key(|&(at, _)| at);

        let mut actions = Vec::new();

        for (at, stage) in due {
            self.fired.push(stage);

            match stage {
                Stage::Dim => {
                    let lock = self.stages.lock.map(|after| since + after);
                    let dark = lock.is_some_and(|lock| lock > at);

                    self.dimmed = true;
                    actions.push(Action::Dim {
                        until: lock.filter(|_| dark).unwrap_or(at + FADE),
                        dark,
                    });
                }
                Stage::Lock => {
                    self.retry = None;
                    actions.push(Action::Lock);
                }
                Stage::ScreenOff => actions.push(Action::ScreenOff),
                Stage::Suspend => actions.push(Action::Suspend),
            }
        }

        if self.retry.is_some_and(|retry| retry <= now) {
            self.retry = None;
            actions.push(Action::Lock);
        }

        actions
    }

    /// When `tick` next has something to fire, none while nothing will without an event.
    pub fn next(&self) -> Option<Instant> {
        let since = self.counting()?;

        self.stages
            .each()
            .filter(|&(stage, _)| !self.fired.contains(&stage) && !self.waits(stage))
            .map(|(_, after)| since + after)
            .chain(self.retry)
            .min()
    }

    // nothing fired or waiting, the dim taken away
    fn reset(&mut self) -> Vec<Action> {
        self.fired.clear();
        self.retry = None;
        self.locked = false;

        if std::mem::take(&mut self.dimmed) {
            vec![Action::Undim]
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    fn stages() -> Stages {
        Stages {
            dim: Some(secs(270)),
            lock: Some(secs(300)),
            screen_off: Some(secs(330)),
            suspend: None,
        }
    }

    // idle reported at the first stage, as the compositor says it
    fn idle_at(stages: Stages) -> (Schedule, Instant) {
        let start = Instant::now();
        let mut schedule = Schedule::new(stages);

        schedule.idled(start + stages.first().unwrap());

        (schedule, start)
    }

    #[test]
    fn stages_fire_in_order_once_each() {
        let (mut schedule, start) = idle_at(stages());

        assert_eq!(schedule.next(), Some(start + secs(270)));
        assert_eq!(schedule.tick(start + secs(269)), []);
        assert_eq!(
            schedule.tick(start + secs(270)),
            [Action::Dim {
                until: start + secs(300),
                dark: true
            }]
        );
        assert_eq!(schedule.tick(start + secs(280)), []);
        assert_eq!(schedule.next(), Some(start + secs(300)));
        assert_eq!(schedule.tick(start + secs(300)), [Action::Lock]);
        schedule.lock_held();
        assert_eq!(schedule.tick(start + secs(330)), [Action::ScreenOff]);
        assert_eq!(schedule.next(), None);
        assert_eq!(schedule.tick(start + secs(3600)), []);
    }

    // a late tick, as after the thread was held up, still fires all that is due, the lock included
    #[test]
    fn a_late_tick_fires_everything_due_in_order() {
        let (mut schedule, start) = idle_at(stages());

        assert_eq!(
            schedule.tick(start + secs(400)),
            [
                Action::Dim {
                    until: start + secs(300),
                    dark: true
                },
                Action::Lock
            ]
        );

        // the monitors go off once the lock holds, not before
        schedule.lock_held();
        assert_eq!(schedule.next(), Some(start + secs(330)));
        assert_eq!(schedule.tick(start + secs(400)), [Action::ScreenOff]);
    }

    // a lock that keeps failing never lets the monitors off nor the machine suspend
    #[test]
    fn nothing_after_the_lock_fires_while_it_fails() {
        let (mut schedule, start) = idle_at(Stages {
            suspend: Some(secs(600)),
            ..stages()
        });

        let mut at = start + secs(300);
        assert!(schedule.tick(at).contains(&Action::Lock));

        while at < start + secs(3600) {
            schedule.lock_failed(at);
            at = schedule.next().unwrap();
            assert_eq!(schedule.tick(at), [Action::Lock]);
        }

        schedule.lock_held();
        assert_eq!(schedule.tick(at), [Action::ScreenOff, Action::Suspend]);
    }

    // the monitors set to go off before the lock still wait for it
    #[test]
    fn a_screen_off_before_the_lock_waits_for_it() {
        let (mut schedule, start) = idle_at(Stages {
            dim: None,
            lock: Some(secs(300)),
            screen_off: Some(secs(60)),
            suspend: None,
        });

        assert_eq!(schedule.next(), Some(start + secs(300)));
        assert_eq!(schedule.tick(start + secs(300)), [Action::Lock]);
        schedule.lock_held();
        assert_eq!(schedule.tick(start + secs(300)), [Action::ScreenOff]);
    }

    // with no lock stage, nothing waits
    #[test]
    fn without_a_lock_the_monitors_go_off_on_time() {
        let (mut schedule, start) = idle_at(Stages {
            lock: None,
            ..stages()
        });

        assert!(
            schedule
                .tick(start + secs(330))
                .contains(&Action::ScreenOff)
        );
    }

    #[test]
    fn input_cancels_the_dim_and_the_lock() {
        let (mut schedule, start) = idle_at(stages());

        schedule.tick(start + secs(280));
        assert_eq!(schedule.resumed(), [Action::Undim]);
        assert_eq!(schedule.next(), None);
        assert_eq!(schedule.tick(start + secs(400)), []);

        // idle again counts from the start
        schedule.idled(start + secs(500) + secs(270));
        assert_eq!(schedule.next(), Some(start + secs(500) + secs(270)));
    }

    // an inhibitor stops every stage, the lock too, and the count starts over once it ends
    #[test]
    fn an_inhibitor_holds_every_stage() {
        let (mut schedule, start) = idle_at(stages());

        schedule.tick(start + secs(280));
        assert_eq!(schedule.inhibit(true, start + secs(290)), [Action::Undim]);
        assert_eq!(schedule.next(), None);
        assert_eq!(schedule.tick(start + secs(1000)), []);

        let ended = start + secs(2000);
        assert_eq!(schedule.inhibit(false, ended), []);
        assert_eq!(schedule.tick(ended + secs(269)), []);
        assert!(matches!(
            schedule.tick(ended + secs(270)).as_slice(),
            [Action::Dim { .. }]
        ));
        assert_eq!(schedule.tick(ended + secs(300)), [Action::Lock]);
    }

    #[test]
    fn idle_while_inhibited_fires_nothing() {
        let start = Instant::now();
        let mut schedule = Schedule::new(stages());

        schedule.inhibit(true, start);
        schedule.idled(start + secs(270));
        assert_eq!(schedule.next(), None);
        assert_eq!(schedule.tick(start + secs(1000)), []);
    }

    // a lock that failed is asked again while idle, so an error never leaves the session unlocked
    #[test]
    fn a_failed_lock_is_asked_again() {
        let (mut schedule, start) = idle_at(stages());

        assert!(schedule.tick(start + secs(300)).contains(&Action::Lock));
        assert_eq!(schedule.lock_failed(start + secs(301)), [Action::Undim]);
        assert_eq!(schedule.next(), Some(start + secs(311)));
        assert_eq!(schedule.tick(start + secs(310)), []);
        assert_eq!(schedule.tick(start + secs(311)), [Action::Lock]);

        // input ends the retries: the user is there
        assert_eq!(schedule.lock_failed(start + secs(312)), []);
        schedule.resumed();
        assert_eq!(schedule.tick(start + secs(400)), []);
    }

    // with no dim, the lock still comes on time
    #[test]
    fn the_lock_needs_no_dim() {
        let (mut schedule, start) = idle_at(Stages {
            dim: None,
            ..stages()
        });

        assert_eq!(schedule.next(), Some(start + secs(300)));
        assert_eq!(schedule.tick(start + secs(300)), [Action::Lock]);
    }

    // a dim with no lock after it fades only partly, over a fixed time
    #[test]
    fn a_dim_without_a_lock_after_it_is_not_dark() {
        let (mut schedule, start) = idle_at(Stages {
            dim: Some(secs(60)),
            lock: None,
            screen_off: None,
            suspend: Some(secs(600)),
        });

        assert_eq!(
            schedule.tick(start + secs(60)),
            [Action::Dim {
                until: start + secs(60) + FADE,
                dark: false
            }]
        );
        assert_eq!(schedule.tick(start + secs(600)), [Action::Suspend]);
    }

    // new stages while idle count from the change, even with the first stage's time the same
    #[test]
    fn a_reload_while_idle_counts_from_the_reload() {
        let start = Instant::now();
        let reload = start + secs(290);

        let later = Stages {
            lock: Some(secs(600)),
            ..stages()
        };
        let mut schedule = Schedule::since(later, reload);
        schedule.idled(start + secs(270));

        assert_eq!(schedule.next(), Some(reload + secs(270)));
        assert!(!schedule.tick(start + secs(600)).contains(&Action::Lock));
        assert_eq!(schedule.next(), Some(reload + secs(600)));
        assert_eq!(schedule.tick(reload + secs(600)), [Action::Lock]);

        // a lock moved sooner, already past, does not fire at once
        let sooner = Stages {
            lock: Some(secs(280)),
            ..stages()
        };
        let mut schedule = Schedule::since(sooner, reload);
        schedule.idled(start + secs(270));

        assert_eq!(schedule.tick(reload), []);
        assert!(schedule.tick(reload + secs(280)).contains(&Action::Lock));
    }

    #[test]
    fn no_stages_fire_nothing() {
        let mut schedule = Schedule::new(Stages::default());

        schedule.idled(Instant::now());
        assert_eq!(schedule.next(), None);
        assert_eq!(schedule.tick(Instant::now() + secs(86_400)), []);
    }
}
