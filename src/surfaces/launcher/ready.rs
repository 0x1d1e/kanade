//! An IPC opening answers only when this visit's host can receive keys. No keys are kept here;
//! ownership comes from the runtime's existing keyboard enter, not from requesting exclusivity.

use std::sync::Mutex;

use kanade_runtime::MutexExt;
use kanade_runtime::service::Service;

use crate::island::service::IslandService;
use crate::later::Later;

struct Waiting {
    monitor: String,
    visit: u64,
    reply: Later,
}

// Only the newest opening can become ready. A replaced opening receives an explicit refusal.
static WAITING: Mutex<Option<Waiting>> = Mutex::new(None);

pub fn wait(monitor: String, visit: u64, reply: Later) {
    reply.unsettled("the Launcher has not confirmed keyboard focus; it may still open");
    let previous = WAITING.locked().replace(Waiting {
        monitor,
        visit,
        reply,
    });
    if let Some(previous) = previous {
        previous
            .reply
            .refused("another Launcher opening replaced this request");
    }
    follow();
}

// Called only by the drawn host that actually has the compositor's keyboard focus.
pub fn confirm(monitor: &str, visit: u64) {
    if crate::lock::holds() {
        return;
    }
    let island = IslandService::read();
    let mut waiting = WAITING.locked();
    let ready = waiting.as_ref().is_some_and(|waiting| {
        waiting.monitor == monitor && waiting.visit == visit && current(&island, waiting)
    });
    let answered = ready.then(|| waiting.take()).flatten();
    drop(waiting);
    drop(island);
    if let Some(waiting) = answered {
        waiting.reply.done(String::new());
    }
}

fn current(island: &IslandService, waiting: &Waiting) -> bool {
    island.visit() == waiting.visit
        && island.open_launcher_on().as_deref() == Some(waiting.monitor.as_str())
}

// A collapse, another Surface, a style change or a later visit ends the wait without success.
pub(super) fn follow() {
    let island = IslandService::read();
    let mut waiting = WAITING.locked();
    let over = waiting
        .as_ref()
        .is_some_and(|waiting| !waiting.reply.open() || !current(&island, waiting));
    let ended = over.then(|| waiting.take()).flatten();
    drop(waiting);
    drop(island);
    if let Some(waiting) = ended {
        waiting
            .reply
            .refused("the Launcher closed before confirming keyboard focus");
    }
}
