//! The answer to a `kanade` call that is not known when it arrives, like a lock niri has yet to
//! hold. The source that did the work answers when it is done; the shell answers for it, saying
//! what is unknown, once `LATER` passes. `kanade` just waits on the socket (`cli::PATIENCE`).

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::{IpcReply, MutexExt};

use crate::cli::Reply;

// how long a source may take before the shell answers unknown; under `cli::PATIENCE`, so its words arrive
pub const LATER: Duration = Duration::from_secs(5);

struct Waiting {
    reply: Option<IpcReply>,

    // what the shell answers when `LATER` passes first
    unsettled: String,
}

struct Shared {
    waiting: Mutex<Waiting>,
    answered: Condvar,
    deadline: Instant,
}

// one call's answer, sent by whichever of its holders gets there first
#[derive(Clone)]
pub struct Later(Arc<Shared>);

impl Later {
    pub fn new(reply: IpcReply) -> Self {
        let shared = Arc::new(Shared {
            waiting: Mutex::new(Waiting {
                reply: Some(reply),
                unsettled: format!("the shell did not finish within {LATER:?}; it may still have"),
            }),
            answered: Condvar::new(),
            deadline: Instant::now() + LATER,
        });

        let watching = Arc::clone(&shared);

        // not worth failing the call over: the client's own patience ends it
        let _ = thread::Builder::new()
            .name(String::from("later"))
            .spawn(move || watching.expire());

        Self(shared)
    }

    // a call nobody waits on, as a press on the island's own targets: every answer is dropped
    pub fn detached() -> Self {
        Self(Arc::new(Shared {
            waiting: Mutex::new(Waiting {
                reply: None,
                unsettled: String::new(),
            }),
            answered: Condvar::new(),
            deadline: Instant::now(),
        }))
    }

    // what the shell says if the source does not answer within `LATER`, as only it knows what may still happen
    pub fn unsettled(&self, unsettled: impl Into<String>) {
        self.0.waiting.locked().unsettled = unsettled.into();
    }

    // whether nothing answered yet and the deadline has not passed
    pub fn open(&self) -> bool {
        self.0.waiting.locked().reply.is_some() && Instant::now() < self.0.deadline
    }

    // what is left until the shell answers unknown
    pub fn left(&self) -> Duration {
        self.0.deadline.saturating_duration_since(Instant::now())
    }

    // the first answer is sent and later ones are dropped
    pub fn answer(&self, reply: &Reply) {
        let taken = self.0.waiting.locked().reply.take();

        if let Some(to) = taken {
            to.send(&reply.encode());
        }

        self.0.answered.notify_all();
    }

    pub fn done(&self, text: impl Into<String>) {
        self.answer(&Reply::Done(text.into()));
    }

    pub fn refused(&self, why: impl Into<String>) {
        self.answer(&Reply::Refused(why.into()));
    }

    // asked of another program that did not say whether it did it
    pub fn unknown(&self, why: impl Into<String>) {
        self.answer(&Reply::Unknown(why.into()));
    }

    // a source's result, answered as `Reply::Done` or `Reply::Refused`
    pub fn result(&self, result: Result<String, String>) {
        match result {
            Ok(text) => self.done(text),
            Err(why) => self.refused(why),
        }
    }
}

impl Shared {
    // answers unknown at the deadline, unless it was answered before
    fn expire(&self) {
        let left = self.deadline.saturating_duration_since(Instant::now());

        let (mut waiting, _) = self
            .answered
            .wait_timeout_while(self.waiting.locked(), left, |waiting| {
                waiting.reply.is_some()
            })
            .unwrap_or_else(PoisonError::into_inner);

        let Some(reply) = waiting.reply.take() else {
            return;
        };
        let why = waiting.unsettled.clone();

        drop(waiting);
        reply.send(&Reply::Unknown(why).encode());
    }
}
