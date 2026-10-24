use std::cell::RefCell;
use std::time::Duration;

use vello::wgpu::{PollError, PollType, SubmissionIndex};

use super::shared;

// how long the wait on a frame may take before the gpu is called stuck
const STUCK: Duration = Duration::from_secs(1);

thread_local! {
    // the last frame sent, which the drain waits on
    static LAST: RefCell<Option<SubmissionIndex>> = const { RefCell::new(None) };
}

// a frame was sent, and presenting it committed it; the drain waits on this one
pub(crate) fn note_submitted(submission: SubmissionIndex) {
    LAST.with_borrow_mut(|last| *last = Some(submission));
}

/*
 * wgpu frees command buffers only on poll; waiting on submission completion runs after
 * present (while compositor displays the frame) so the morph does not stall, polling every
 * frame to keep heap bounded across window visits.
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
    if let Err(PollError::Timeout) = shared.device.poll(wait) {
        eprintln!("kanade: the gpu is still on a frame {STUCK:?} later");
    }
}
