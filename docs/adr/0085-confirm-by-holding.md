# 85. A destructive press is confirmed by holding

Status: accepted. Changes the "press twice" confirms of ADR 0045 (Calendar remove) and the Clipboard's Clear all.

## Context

Two Surfaces asked a destructive act twice: `d` `d` removes a Calendar event, and Clear all in the Clipboard armed on the first press and acted on the second. A second press is easy to land by habit, a double click or a key repeat, and says nothing about intent. Ukishima's `HeatHold` (press, fill, act on full, drain on early release) is a better confirm: it needs sustained intent and shows its progress.

## Decision

- **One small primitive**, `surfaces/confirm.rs`, a Service `Confirm` holding one press at a time (a pointer button or a key is held once): what is held (a hash), when it began, and the fill it ended with. `press(what, then)` starts it and a timer thread runs `then` once `HOLD` (700 ms) has passed with no release, in the same visit of the Surface. `release()` drains the fill in 180 ms and acts on nothing. A view reads `fill(what)` and asks frames only while it moves.
- **Pointer.** The runtime's `Rectangle::on_hold(Fn(bool))` is told true when the left button goes down on it, false when it comes up, the pointer leaves the rectangle or the window, or the rectangle is gone. A tap is a press that ends before `HOLD`: nothing happens (it claims the click, a right click pins).
- **Keyboard.** The runtime tells `App::on_key_release` when any key goes up or the keyboard leaves its window. It does not say which key: a hold that any key release ends fails safe, and so does the next key down (`view::key`). Hold `d` on a Calendar event Kanade added, or Enter with the ring on Clear all. Space types in the Clipboard, so only Enter holds there. Surfaces stay keyboard-complete.
- **Reduced motion.** `island.motion = reduced` has no fill and no drain: the target is full while held, and the act still waits `HOLD`.
- **Where.** Calendar remove and Clipboard Clear all, the two confirms that existed. A Calendar row fills behind its text, the Clear pill behind its label; a line says what to do ("Hold Enter to clear all 12, no undo").
- **Not the Session "Now"**: it has no confirm today (the 60 s countdown with Cancel is its confirm), so it is unchanged.

## Alternatives

**Counting key repeats for the keyboard hold.** Repeat delay and rate are the compositor's, and the runtime repeats only arrows and Backspace.

**A key-up for a named key.** A release event carries no text, and a modifier changing between press and release changes the keysym; "any release ends it" cannot get stuck.

**A shared confirm framework.** Two callers; one fixed duration.

## Consequences

- Holding takes 0.7 s every time; there is no way to confirm faster.
- Not tested live here: the fill and drain, the pointer hold and the key hold need a run on the session.
- Releasing any key during a key hold, including a modifier, cancels it.
