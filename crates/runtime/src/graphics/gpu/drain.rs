use std::cell::{Cell, RefCell};
use std::time::Duration;

use vello::wgpu::{Device, PollError, PollType, SubmissionIndex};

use super::shared;

// how long the wait on a frame may take before the gpu is called stuck
const STUCK: Duration = Duration::from_secs(1);

// recycle commands within busy frames to reduce the device's retained pool high-water
const SUBMISSION_BUDGET: u32 = 32;

thread_local! {
    // the last frame sent, which the drain waits on
    static LAST: RefCell<Option<SubmissionIndex>> = const { RefCell::new(None) };
    static PENDING: Cell<u32> = const { Cell::new(0) };
}

// a frame was sent, and presenting it committed it; the drain waits on this one
pub(crate) fn note_submitted(submission: SubmissionIndex) {
    LAST.with_borrow_mut(|last| *last = Some(submission));
}

// wait between intermediate submissions, before another encoder is acquired
pub(super) fn intermediate_submission(device: &Device, submission: Option<SubmissionIndex>) {
    let pending = PENDING.get().saturating_add(1);
    PENDING.set(pending);
    if pending < SUBMISSION_BUDGET {
        return;
    }

    match device.poll(PollType::Wait {
        submission_index: submission,
        timeout: Some(STUCK),
    }) {
        Ok(_) => PENDING.set(0),
        Err(PollError::Timeout) => eprintln!("kanade: the gpu is still on a frame {STUCK:?} later"),
        Err(_) => {}
    }
}

/*
 * completion waits recycle command buffers, but the device retains its command-pool high-water.
 * limiting intermediate submissions reduces that high-water within busy frames; the final wait
 * stays after present, while the compositor displays the frame.
 */
pub(crate) fn after_frame() {
    drain();
}

// the frame last sent is on screen; a normal window's close calls this too, before its pool is
// built again, so its last frames are freed then rather than at the next one
pub(crate) fn drain() {
    let Some(shared) = shared::current() else {
        return;
    };

    let Some(submission) = LAST.with_borrow(Clone::clone) else {
        return;
    };

    let wait = PollType::Wait {
        submission_index: Some(submission),
        timeout: Some(STUCK),
    };

    /*
     * a device lost is already handled: wgpu calls the lost callback the shell exits from, and a
     * fatal error in the poll itself panics. what is left is a gpu still on a frame, whose
     * resources the next drain frees
     */
    match shared.device.poll(wait) {
        Ok(_) => PENDING.set(0),
        Err(PollError::Timeout) => eprintln!("kanade: the gpu is still on a frame {STUCK:?} later"),
        Err(_) => {}
    }
}
