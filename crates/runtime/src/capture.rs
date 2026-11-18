//! One-shot full-output frames over the compositor's `wlr-screencopy` (ADR 0162).
//!
//! The shell asks for a fresh frame of an output and waits for it off the draw thread, then reads
//! its RGBA pixels. The copying and the per-frame conversion into RGBA run on the runtime's own
//! loop, which the request wakes; only the wait is elsewhere. One frame is one copy: there is no
//! damage tracking, pacing or pause, as the frozen region picker only needs the screen as it was
//! when it opened.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use smithay_client_toolkit::reexports::calloop::ping::Ping;

use crate::unpoisoned::MutexExt;

/// One output's frame, RGBA8 with its top row first.
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Arc<Vec<u8>>,
}

type Outcome = Result<Frame, String>;

// what one request asked in, and the outcome it got until a caller takes it
struct Held {
    // the request this output is being captured for; 0 before the first
    generation: u64,

    // the outcome of that request, none while the copy is still in flight or once taken
    result: Option<Outcome>,
}

// what the runtime copied, waiting for the caller that asked: at most one per output
static HELD: Mutex<Option<HashMap<String, Held>>> = Mutex::new(None);

// hands each request the generation after the last, so a grab of an older one is told apart
static GENERATION: AtomicU64 = AtomicU64::new(1);

fn generation_after_last() -> u64 {
    GENERATION.fetch_add(1, Ordering::Relaxed)
}

// woken as an outcome lands, so `await_frame` rests without polling
static READY: Condvar = Condvar::new();

// the outputs asked for since the runtime last served them
static ASKED: Mutex<Option<Vec<String>>> = Mutex::new(None);

static PING: OnceLock<Ping> = OnceLock::new();

static CAPABLE: AtomicBool = AtomicBool::new(false);

fn held() -> MutexGuard<'static, Option<HashMap<String, Held>>> {
    HELD.locked()
}

fn asked() -> MutexGuard<'static, Option<Vec<String>>> {
    ASKED.locked()
}

/// Whether the compositor advertised `wlr-screencopy` when the runtime connected. Known only once
/// it has; a frame asked for may still fail to arrive.
pub fn capable() -> bool {
    CAPABLE.load(Ordering::Relaxed)
}

/// Asks for one fresh frame of `output`, replacing whatever was held for it. Safe from any thread:
/// the runtime's loop does the copying.
pub fn request(output: &str) {
    if !capable() {
        return;
    }

    // the request is stamped before the loop can be pinged, so a grab of the last one is stale
    let generation = generation_after_last();

    // the last request's outcome goes with it: its pixels are not this picker's to serve
    held().get_or_insert_default().insert(
        output.to_owned(),
        Held {
            generation,
            result: None,
        },
    );

    {
        let mut asked = asked();
        let asked = asked.get_or_insert_default();

        if !asked.iter().any(|name| name == output) {
            asked.push(output.to_owned());
        }
    }

    if let Some(ping) = PING.get() {
        ping.ping();
    }
}

/// Waits on `output`'s frame for at most `timeout`, off the draw thread, and takes it. Only the
/// current request's outcome is ever held, so this never returns an older pick's picture. A
/// failure is the compositor's short reason, never any pixel.
pub fn await_frame(output: &str, timeout: Duration) -> Result<Frame, String> {
    let deadline = Instant::now() + timeout;
    let mut held = held();

    loop {
        let Some(slot) = held.as_mut().and_then(|held| held.get_mut(output)) else {
            return Err(format!("no frame of {output} is awaited"));
        };

        if let Some(result) = slot.result.take() {
            return result;
        }

        let left = deadline.saturating_duration_since(Instant::now());

        if left.is_zero() {
            return Err(format!("no frame of {output} arrived in time"));
        }

        held = READY
            .wait_timeout(held, left)
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0;
    }
}

/// Drops whatever is held for `output`, its request's generation with it, so a grab still in
/// flight for it finds nothing to serve when it lands.
pub fn clear(output: &str) {
    if let Some(held) = held().as_mut() {
        held.remove(output);
    }
}

/// Drops every output's held frame and generation. Called when a screenshot ends, whatever way it
/// did: a grab that lands late has no slot to fill, so no pixel waits for the next picker.
pub fn clear_all() {
    let _ = asked().take();
    let _ = held().take();
    READY.notify_all();

    if let Some(ping) = PING.get() {
        ping.ping();
    }
}

/// The generation of `output`'s request as it stands now, which a grab is stamped with: an outcome
/// settled for another generation is dropped.
pub(crate) fn generation(output: &str) -> u64 {
    held()
        .as_ref()
        .and_then(|held| held.get(output))
        .map_or(0, |held| held.generation)
}

// the runtime connected, with whether the compositor has screencopy and how to wake its loop
pub(crate) fn connected(capable: bool, ping: Ping) {
    // the ping first, so a request that sees the capability also finds the handle
    let _ = PING.set(ping);
    CAPABLE.store(capable, Ordering::Relaxed);

    // a request that raced the loop's start is served now
    if capable
        && asked().as_ref().is_some_and(|asked| !asked.is_empty())
        && let Some(ping) = PING.get()
    {
        ping.ping();
    }
}

// the outputs asked for since this last ran
pub(crate) fn take_requests() -> Vec<String> {
    asked().as_mut().map(std::mem::take).unwrap_or_default()
}

// a capture that finished or could not be made; wakes the caller waiting on it. An outcome of a
// request since replaced is dropped, its pixels with it
pub(crate) fn settle(output: &str, generation: u64, result: Outcome) {
    let mut held = held();

    let Some(slot) = held.as_mut().and_then(|held| held.get_mut(output)) else {
        return;
    };

    if slot.generation != generation {
        return;
    }

    slot.result = Some(result);

    READY.notify_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_TURN: Mutex<()> = Mutex::new(());

    fn frame(width: u32) -> Frame {
        Frame {
            width,
            height: 1,
            pixels: Arc::new(vec![0; width as usize * 4]),
        }
    }

    // a grab that outlived its picker is dropped, and the current request's frame is taken
    #[test]
    fn a_stale_grab_is_dropped_and_a_current_one_is_taken() {
        let _turn = TEST_TURN.locked();
        let output = "generation-test";

        CAPABLE.store(true, Ordering::Relaxed);

        request(output);
        let first = generation(output);
        assert!(first > 0);

        // the picker ended and the next one asked before the first grab landed
        request(output);
        let second = generation(output);
        assert_ne!(first, second);

        settle(output, first, Ok(frame(1)));
        assert!(await_frame(output, Duration::from_millis(1)).is_err());

        // ending the picker drops the slot, so its grab has nowhere to land
        clear(output);
        assert_eq!(generation(output), 0);

        request(output);
        let third = generation(output);
        assert!(![first, second].contains(&third));

        settle(output, second, Ok(frame(2)));
        assert!(await_frame(output, Duration::from_millis(1)).is_err());

        settle(output, third, Ok(frame(3)));
        assert_eq!(
            await_frame(output, Duration::from_millis(1)).unwrap().width,
            3
        );
    }

    #[test]
    fn clear_all_wakes_waiter_drops_generation_and_clears_asked() {
        let _turn = TEST_TURN.locked();
        let output = "clear-all-test";

        CAPABLE.store(true, Ordering::Relaxed);
        request(output);
        assert!(generation(output) > 0);

        let started = Instant::now();
        let (waiting_tx, waiting_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let _ = waiting_tx.send(());
            await_frame(output, Duration::from_secs(5))
        });

        waiting_rx.recv().unwrap();
        std::thread::sleep(Duration::from_millis(10));
        clear_all();

        assert_eq!(generation(output), 0);
        assert!(take_requests().is_empty());
        assert!(handle.join().unwrap().is_err());
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "clear_all did not wake await_frame promptly"
        );
    }
}
