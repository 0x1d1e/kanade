# 109. The gesture trail is a glowing line; reduced motion draws a plain one

Status: accepted. Part of the gesture launcher (ADR 0105).

## Context

The user needs to see the stroke, and the overlay must sit on any desktop. Kanade has a Canvas with Path shapes in the runtime; no particle system is justified.

## Decision

- **A polyline** of the stroke's kept points (2 px apart, at most 4096) drawn as a `Canvas` `Path`: a wide soft stroke (16 px, `GESTURE_GLOW`) under a narrow bright one (4 px, `GESTURE_CORE`), round caps, over a light scrim (`GESTURE_SCRIM`) so the output reads as taken. Colors are in `src/theme.rs`.
- **Fade on release.** 350 ms by opacity, asking frames only while it fades (`request_frame`); the window is click-through and the keyboard already released.
- **Reduced motion** (`island.motion = reduced`): no glow, just the plain line, fading in 80 ms.
- A hint ("Draw a gesture. Esc or right click cancels", or "Draw NAME, stroke 2 of 3. Esc cancels") shows centered while the overlay is open.

## Alternatives

**Particles, as Hexecute.** More draw cost and state for no gain in use; the brief asked none for reduced motion, and the plain glow reads well.

## Consequences

- Look and motion are unverified on a session; the numbers are a first guess.
- Zero idle frames: nothing requests one while the overlay is idle.
