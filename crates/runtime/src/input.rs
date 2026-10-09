//! Keyboard focus and repeat policy.
//!
//! The native Wayland backend provides focused press/release events and
//! wl_keyboard.repeat_info. Only a currently focused surface receives keys;
//! no evdev reader or privileged input-group membership is needed. This state
//! machine has no timers or threads: the event loop sleeps until next_repeat()
//! and calls repeat_due() only when that deadline is reached.

use std::time::{Duration, Instant};

use crate::window::WindowId;

/// Physical code received from wl_keyboard, not a hardcoded US-layout keysym.
/// The compositor's keymap is responsible for producing logical keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stroke {
    pub code: u32,
    pub logical: Key,
    /// Determined by the keyboard/keymap backend, not by physical code.
    pub repeatable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Key {
    Text(String),
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Other(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Press,
    Release,
    Repeat,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub window: WindowId,
    pub stroke: Stroke,
    pub phase: Phase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RepeatInfo {
    pub delay: Duration,
    pub interval: Option<Duration>,
}

impl RepeatInfo {
    /// wl_keyboard.repeat_info: rate (events/s), delay (ms).
    /// A nonpositive rate means disabled, and a negative delay is clamped.
    /// One emitted repeat per wake prevents a late frame creating a burst.
    pub fn from_wayland(rate: i32, delay_ms: i32) -> Self {
        let interval = (rate > 0).then(|| {
            // Limit the shortest repeat to 1 ms, including an extreme rate.
            Duration::from_micros((1_000_000 / rate as u64).max(1_000))
        });
        Self {
            delay: Duration::from_millis(delay_ms.max(0) as u64),
            interval,
        }
    }
}

#[derive(Clone, Debug)]
struct Repeating {
    stroke: Stroke,
    next: Instant,
}

/// One wl_keyboard seat. The backend maintains a separate instance per seat.
pub struct Keyboard {
    focused: Option<WindowId>,
    repeat: RepeatInfo,
    active: Option<Repeating>,
    // Only keys pressed while we had focus are eligible for release events.
    held: Vec<Stroke>,
}

impl Keyboard {
    pub fn new(repeat: RepeatInfo) -> Self {
        Self {
            focused: None,
            repeat,
            active: None,
            held: Vec::new(),
        }
    }

    /// A leave, seat removal or surface destruction cancels repeats immediately.
    pub fn focus(&mut self, window: Option<WindowId>) {
        if self.focused != window {
            self.focused = window;
            self.held.clear();
            self.active = None;
        }
    }

    pub fn focused(&self) -> Option<WindowId> {
        self.focused
    }

    /// Apply compositor settings without guessing niri's repeat defaults.
    pub fn configure_repeat(&mut self, repeat: RepeatInfo, now: Instant) {
        self.repeat = repeat;
        if let Some(active) = &mut self.active {
            if repeat.interval.is_some() {
                active.next = now + repeat.delay;
            } else {
                self.active = None;
            }
        }
    }

    /// Send the original key immediately. Key repeat is independent of frames.
    pub fn press(&mut self, stroke: Stroke, now: Instant) -> Option<KeyEvent> {
        let window = self.focused?;
        if self.held.iter().any(|held| held.code == stroke.code) {
            return None; // Duplicate press, not a second key.
        }

        if stroke.repeatable && self.repeat.interval.is_some() {
            self.active = Some(Repeating {
                stroke: stroke.clone(),
                next: now + self.repeat.delay,
            });
        }
        self.held.push(stroke.clone());

        Some(KeyEvent {
            window,
            stroke,
            phase: Phase::Press,
        })
    }

    /// Release matches the actual compositor keycode. Remapped Backspace,
    /// Delete and navigation keys work without inspecting global device input.
    pub fn release(&mut self, code: u32) -> Option<KeyEvent> {
        let window = self.focused?;
        let position = self.held.iter().position(|held| held.code == code)?;
        let stroke = self.held.remove(position);
        if self.active.as_ref().is_some_and(|active| active.stroke.code == code) {
            self.active = None;
        }

        Some(KeyEvent {
            window,
            stroke,
            phase: Phase::Release,
        })
    }

    pub fn next_repeat(&self) -> Option<Instant> {
        self.focused
            .and(self.repeat.interval)
            .and_then(|_| self.active.as_ref().map(|active| active.next))
    }

    /// Emit at most one repeat, then schedule from the current monotonic time.
    /// A stalled main loop never replays a backlog of missed repeats.
    pub fn repeat_due(&mut self, now: Instant) -> Option<KeyEvent> {
        let window = self.focused?;
        let interval = self.repeat.interval?;
        let active = self.active.as_mut()?;
        if now < active.next {
            return None;
        }
        active.next = now + interval;

        Some(KeyEvent {
            window,
            stroke: active.stroke.clone(),
            phase: Phase::Repeat,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: WindowId = WindowId(7);
    const OTHER: WindowId = WindowId(8);

    fn stroke(code: u32, logical: Key) -> Stroke {
        Stroke {
            code,
            logical,
            repeatable: true,
        }
    }

    fn keyboard() -> Keyboard {
        let mut keyboard = Keyboard::new(RepeatInfo::from_wayland(50, 200));
        keyboard.focus(Some(WINDOW));
        keyboard
    }

    #[test]
    fn short_tap_never_repeats() {
        let mut keyboard = keyboard();
        let now = Instant::now();
        assert_eq!(
            keyboard.press(stroke(22, Key::Backspace), now).unwrap().phase,
            Phase::Press
        );
        assert_eq!(keyboard.release(22).unwrap().phase, Phase::Release);
        assert_eq!(keyboard.repeat_due(now + Duration::from_secs(2)), None);
        assert_eq!(keyboard.next_repeat(), None);
    }

    #[test]
    fn held_key_repeats_at_compositor_cadence_without_catch_up() {
        let mut keyboard = keyboard();
        let now = Instant::now();
        keyboard.press(stroke(22, Key::Backspace), now);
        assert_eq!(keyboard.repeat_due(now + Duration::from_millis(199)), None);
        assert_eq!(
            keyboard.repeat_due(now + Duration::from_millis(200)).unwrap().phase,
            Phase::Repeat
        );
        assert_eq!(keyboard.repeat_due(now + Duration::from_millis(219)), None);
        assert_eq!(
            keyboard.repeat_due(now + Duration::from_millis(220)).unwrap().phase,
            Phase::Repeat
        );
        assert_eq!(
            keyboard.repeat_due(now + Duration::from_secs(5)).unwrap().phase,
            Phase::Repeat
        );
        assert_eq!(keyboard.repeat_due(now + Duration::from_secs(5)), None);
    }

    #[test]
    fn focus_loss_cancels_repeat_and_all_held_keys() {
        let mut keyboard = keyboard();
        let now = Instant::now();
        keyboard.press(stroke(22, Key::Backspace), now);
        keyboard.focus(Some(OTHER));
        assert_eq!(keyboard.repeat_due(now + Duration::from_secs(5)), None);
        assert_eq!(keyboard.release(22), None);
        keyboard.focus(None);
        assert_eq!(keyboard.press(stroke(22, Key::Backspace), now), None);
    }

    #[test]
    fn unknown_and_remapped_keys_keep_their_actual_code() {
        let mut keyboard = keyboard();
        let now = Instant::now();
        let mapped = stroke(58, Key::Backspace);
        keyboard.press(mapped.clone(), now);
        let event = keyboard.repeat_due(now + Duration::from_millis(200)).unwrap();
        assert_eq!(event.stroke, mapped);
        assert_eq!(keyboard.release(58).unwrap().stroke, mapped);
    }

    #[test]
    fn repeat_info_can_disable_and_reenable_held_key() {
        let mut keyboard = keyboard();
        let now = Instant::now();
        keyboard.press(stroke(22, Key::Backspace), now);
        keyboard.configure_repeat(RepeatInfo::from_wayland(0, 200), now);
        assert_eq!(keyboard.next_repeat(), None);
        assert_eq!(keyboard.repeat_due(now + Duration::from_secs(5)), None);
        keyboard.release(22);
        keyboard.configure_repeat(RepeatInfo::from_wayland(25, 600), now);
        keyboard.press(stroke(22, Key::Backspace), now);
        assert_eq!(keyboard.repeat_due(now + Duration::from_millis(599)), None);
        assert_eq!(
            keyboard.repeat_due(now + Duration::from_millis(600)).unwrap().phase,
            Phase::Repeat
        );
    }

    #[test]
    fn nonrepeatable_strokes_do_not_arm_a_timer() {
        let mut keyboard = keyboard();
        let now = Instant::now();
        let mut press = stroke(42, Key::Other(42));
        press.repeatable = false;
        keyboard.press(press, now);
        assert_eq!(keyboard.next_repeat(), None);
    }

    #[test]
    fn duplicate_press_is_not_forwarded() {
        let mut keyboard = keyboard();
        let now = Instant::now();
        let press = stroke(22, Key::Backspace);
        assert!(keyboard.press(press.clone(), now).is_some());
        assert!(keyboard.press(press, now).is_none());
    }

    #[test]
    fn rate_is_bounded_even_for_absurd_compositor_values() {
        assert_eq!(
            RepeatInfo::from_wayland(10_000, -10).interval,
            Some(Duration::from_millis(1))
        );
        assert_eq!(RepeatInfo::from_wayland(-1, 100).interval, None);
        assert_eq!(
            RepeatInfo::from_wayland(50, 200).delay,
            Duration::from_millis(200)
        );
    }
}
