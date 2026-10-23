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
 * wgpu frees a finished submission's command buffers only when a poll tells it the submission is
 * done, and nothing here polled: every frame of a window's visit stayed live for the whole run, so
 * a settings window opened and closed a dozen times held gigabytes that never came back.
 *
 * every frame waits, and that is affordable only because the wait comes after the present: the
 * frame is already on screen, so the wait runs while the compositor shows it. Waiting between the
 * submit and the present instead cost the island's morph 5.40 ms a frame.
 *
 * every frame, not one frame in eight: frames left unpolled keep command buffers live, and a
 * visit's frames are too few for a poll bound to matter. Heap in MiB over twelve settings cycles:
 * every frame flat at 174 from cycle 4; one in two 251; one in four 347; one in eight 414.
 *
 * The wait is what the moving gate was there to avoid: it took 1.6 ms at the median and 2.1 at
 * p90, and the island's morph draws in 1.30 ms a frame at the median against 1.70 before, because
 * the wait no longer runs between the submit and the present.
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
