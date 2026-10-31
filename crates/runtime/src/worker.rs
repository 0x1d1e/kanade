//! One thread for the control calls input handlers ask for, run in the order
//! they were asked, so a slow bus or sound server never stalls drawing.

use std::collections::VecDeque;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::thread;

use crate::unpoisoned::MutexExt;

type Job = Box<dyn FnOnce() + Send>;

/// Target key for jobs where only the latest pending value matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key(pub &'static str);

impl From<&'static str> for Key {
    fn from(name: &'static str) -> Self {
        Self(name)
    }
}

struct Item {
    key: Option<Key>,
    job: Job,
}

// Bounded number of distinct pending keys to avoid unbounded memory growth.
const MAX_PENDING_KEYS: usize = 8;

struct State {
    items: VecDeque<Item>,
}

struct Worker {
    state: Mutex<State>,
    ready: Condvar,
}

static WORKER: LazyLock<Arc<Worker>> = LazyLock::new(|| {
    let worker = Arc::new(Worker {
        state: Mutex::new(State {
            items: VecDeque::new(),
        }),
        ready: Condvar::new(),
    });

    let background = Arc::clone(&worker);
    thread::Builder::new()
        .name(String::from("kanade-worker"))
        .spawn(move || {
            loop {
                let job = {
                    let mut state = background.state.locked();
                    loop {
                        if let Some(item) = state.items.pop_front() {
                            break item.job;
                        }
                        state = background
                            .ready
                            .wait(state)
                            .unwrap_or_else(|error| error.into_inner());
                    }
                };

                // a failed call only loses that call, the thread keeps taking the next ones
                let _ = panic::catch_unwind(AssertUnwindSafe(job));
            }
        })
        .expect("failed to start worker thread");

    worker
});

/*
 * Invariant:
 * Ordinary jobs (`run`) maintain strict FIFO order and act as barriers:
 * any pending replaceable key is disarmed so later `run_latest` calls cannot
 * jump ahead of intervening FIFO work (e.g. mute toggles).
 *
 * For `run_latest`, if an un-started job with the same key is still waiting
 * in the queue, only its closure is replaced in-place. In-flight jobs are
 * never cancelled; distinct targets coalesce independently.
 */

pub fn run(job: impl FnOnce() + Send + 'static) {
    let mut state = WORKER.state.locked();

    // FIFO barrier: disarm pending keys so later jobs cannot execute before this
    for item in &mut state.items {
        item.key = None;
    }

    state.items.push_back(Item {
        key: None,
        job: Box::new(job),
    });
    WORKER.ready.notify_one();
}

pub fn run_latest(key: impl Into<Key>, job: impl FnOnce() + Send + 'static) {
    let key = key.into();
    let mut state = WORKER.state.locked();

    // replace an already-queued job with the same key in-place
    if let Some(item) = state
        .items
        .iter_mut()
        .rev()
        .find(|item| item.key == Some(key))
    {
        item.job = Box::new(job);
        return;
    }

    // bound pending keys to prevent unbounded memory growth
    let bounded = state.items.iter().filter(|item| item.key.is_some()).count() < MAX_PENDING_KEYS;
    state.items.push_back(Item {
        key: if bounded { Some(key) } else { None },
        job: Box::new(job),
    });
    WORKER.ready.notify_one();
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn keeps_order_after_a_failed_call() {
        let (sender, receiver) = mpsc::channel();
        let first = sender.clone();

        run(move || first.send(1).expect("failed to report job"));
        run(|| panic!("a failed call"));
        run(move || sender.send(2).expect("failed to report job"));

        let order: Vec<i32> = receiver.iter().take(2).collect();

        assert_eq!(order, [1, 2]);
    }
}
