use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::changes;
use crate::service::Service;
use crate::unpoisoned::MutexExt;

/// Whether the user has been idle for each timeout `watch` asked for, as the
/// compositor says through `ext-idle-notify-v1`. Its idle notifications honor
/// the Wayland idle inhibitors apps hold, as a playing video's, so a timeout
/// inhibited never goes idle.
#[derive(Debug, Default)]
pub struct Idle {
    // each timeout idle now, with when the compositor said so
    idled: Vec<(Duration, Instant)>,

    // why idle is not followed, as when the compositor has no `ext-idle-notify-v1`
    unavailable: Option<String>,
}

impl Idle {
    /// When the compositor said the user had been idle for `timeout`, none
    /// while not idle that long, or since input.
    pub fn idled(&self, timeout: Duration) -> Option<Instant> {
        self.idled
            .iter()
            .find(|(idle, _)| *idle == timeout)
            .map(|&(_, at)| at)
    }

    pub fn unavailable(&self) -> Option<&str> {
        self.unavailable.as_deref()
    }
}

impl Service for Idle {
    fn new() -> Self {
        Self::default()
    }

    // written only by the runtime's idle events
    fn listen() {}
}

// the timeouts asked for, which the event loop gives a notification each when it wakes
static WANTED: Mutex<Vec<Duration>> = Mutex::new(Vec::new());

/// Follows idle for each of `timeouts`, in place of what was asked before;
/// none follows nothing. A zero timeout is left out.
pub fn watch(timeouts: &[Duration]) {
    let mut wanted: Vec<Duration> = timeouts
        .iter()
        .copied()
        .filter(|timeout| !timeout.is_zero())
        .collect();

    wanted.sort();
    wanted.dedup();

    *WANTED.locked() = wanted;

    changes::wake();
}

pub(crate) fn wanted() -> Vec<Duration> {
    WANTED.locked().clone()
}

pub(crate) fn idled(timeout: Duration, at: Instant) {
    let mut idle = Idle::write();

    idle.idled.retain(|(idled, _)| *idled != timeout);
    idle.idled.push((timeout, at));
}

// input came, or the timeout is no longer followed; one not idle changes nothing
pub(crate) fn resumed(timeout: Duration) {
    let mut idle = Idle::write();

    if idle.idled(timeout).is_none() {
        idle.quiet();
    } else {
        idle.idled.retain(|(idled, _)| *idled != timeout);
    }
}

pub(crate) fn unavailable(why: &str) {
    Idle::write().unavailable = Some(why.to_owned());
}
