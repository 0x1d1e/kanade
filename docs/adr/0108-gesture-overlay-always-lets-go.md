# 108. The gesture overlay always lets go

Status: accepted. Part of the gesture launcher (ADR 0105).

## Context

The overlay holds the pointer on a whole output and the keyboard while drawing. A stuck grab in a shell that is the user's desktop is a lock-out.

## Decision

- **A window per monitor, one visible.** Overlay layer, full size, exclusive zone ignored. Keyboard `Exclusive` only while drawing; hidden and `click_through()` otherwise. Idle, it draws nothing and holds no frame.
- **Every way out closes it:** Escape, a right click, the stroke ending, an 8 s timeout, the session locking (`lock::Said`, and `kanade gesture` while locked is refused), `gesture` run again (a toggle), and the Module off (no windows at all). A record client is answered when the overlay closes for any reason. A recording's timeout renews per stroke, since three strokes take longer than one.
- **The timeout is a ticket.** Each opening or recorded stroke takes a number; the timeout thread cancels only its own. If the thread cannot spawn the overlay closes at once.
- **Closing hands back the grab first**, then the trail fades on a click-through window.
- **A short stroke** (under the recognizer's minimum) is "No gesture" when running, and redrawn when recording, so a twitch cannot waste a take.
- **No match or ambiguous** shows a Transient "No gesture" (or "too close to another") and runs nothing.

## Alternatives

**Grab only with the pointer, no keyboard.** Escape would not work with a pointer left away; the keyboard grab is held only while the user draws.

**A longer timeout.** 8 s is long enough to think and short enough that a forgotten overlay frees the screen.

## Consequences

- One output is covered; the others stay usable.
- Not verified on a session: the grab releasing in each case, the lock interaction. See the manual steps in the change's report.
