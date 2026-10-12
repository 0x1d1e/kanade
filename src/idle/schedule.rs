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

        self.idle = Some(at.checked_sub(first).unwrap_or(at));
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
    pub fn lock_failed(&mut self, at: Instant) {
        if self.counting().is_some() {
            self.retry = Some(at + RETRY);
        }
    }

    /// What fires by `now`, in the order of the stages' times.
    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        let Some(since) = self.counting() else {
            return Vec::new();
        };

        let mut due: Vec<(Instant, Stage)> = self
            .stages
            .each()
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
            .filter(|(stage, _)| !self.fired.contains(stage))
            .map(|(_, after)| since + after)
            .chain(self.retry)
            .min()
    }

    // nothing fired or waiting, the dim taken away
    fn reset(&mut self) -> Vec<Action> {
        self.fired.clear();
        self.retry = None;

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
                Action::Lock,
                Action::ScreenOff
            ]
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
        schedule.lock_failed(start + secs(301));
        assert_eq!(schedule.next(), Some(start + secs(311)));
        assert_eq!(schedule.tick(start + secs(310)), []);
        assert_eq!(schedule.tick(start + secs(311)), [Action::Lock]);

        // input ends the retries: the user is there
        schedule.lock_failed(start + secs(312));
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

    #[test]
    fn no_stages_fire_nothing() {
        let mut schedule = Schedule::new(Stages::default());

        schedule.idled(Instant::now());
        assert_eq!(schedule.next(), None);
        assert_eq!(schedule.tick(Instant::now() + secs(86_400)), []);
    }
}
