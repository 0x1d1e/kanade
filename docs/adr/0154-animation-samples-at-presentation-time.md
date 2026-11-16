# 154. Animation samples at presentation time

Status: accepted. Amends [ADR 0036](0036-the-islands-scene-is-resolved-once.md) and
[ADR 0150](0150-continuous-island-shape-and-content-motion.md) where they describe motion sampling.

## Context

Every view sampled its springs with `Instant::now()` while the frame callback ran. A frame callback
arrives one present latency after the compositor began the frame, and its arrival jitters, so the
sampled motion carried both: two views of one frame could read different instants, and the body
lagged the pointer and the clock by a present.

niri freezes one timestamp per event-loop pass and advances it to the predicted presentation time
before evaluating animations. Kanade already records `started`, `viewed`, `drawn` and `presented`
per frame in `crates/runtime/src/wayland/redraw.rs` and `crates/runtime/src/timing.rs`.

## Decision

- `kanade_runtime::clock` holds a thread-local frozen instant. `now()` returns it while a frame is
  frozen and real time otherwise; `set` and `clear` bracket the frame. A view that is not drawing
  therefore still sees real time.
- `OpenWindow::redraw` freezes one instant immediately before `frame::run_view`, keeps it through
  `frame::build`, and clears it after presenting and before returning, including the hidden and
  unconfigured early returns. The hidden-check `frame::run_view` in `request_frame` freezes real
  time for its own run and clears it after.
- Each window's `FrameClock` keeps its last frame start and a smoothed interval, 16.667 ms until
  frames say otherwise and clamped to 4–50 ms. `next` returns `now + interval` when the previous
  frame started within 4–200 ms, and `now` otherwise; only a gap in that range moves the interval
  toward it with weight 0.2. A hidden or unconfigured frame resets the chain, so no gap across that
  period teaches the interval.
- Views sample `clock::now()` instead of `Instant::now()`: `view::island`, `dock::lay`,
  `dock::between`, the confirm fill, and the auth spinner. Every deadline, service loop, retry, IPC
  wait, banner timer, source thread and input handler keeps real time: the freeze is for animation
  sampling, not for state transitions.
- Phase 2 is future work: binding `wp_presentation_feedback` will replace the prediction with the
  compositor's reported presentation time and measure the actual refresh from it. Until then the
  smoothed frame interval is the predictor, and the same fallback remains when feedback does not
  arrive.

## Consequences

- Within one redraw every animation sample is the same instant, and no earlier than the real time
  the frame began. Motion advances by the predicted frame interval instead of the callback's
  jitter, so a view does not show a pose from before the present.
- Real-time phases are untouched: deadlines, retries and input still transition at the time they
  happen, and a resumed window starts a fresh frame chain.
- `KANADE_FRAMES` keeps measuring the real `started`/`viewed`/`drawn`/`presented` instants, so its
  gap/view/draw/gpu lines still describe the frame that actually ran.
- Files: `crates/runtime/src/clock.rs`, `crates/runtime/src/lib.rs`,
  `crates/runtime/src/wayland/redraw.rs`, `crates/runtime/src/wayland/surface.rs`,
  `crates/runtime/src/wayland/windows.rs`, `src/view/mod.rs`, `src/dock.rs`,
  `src/surfaces/confirm.rs`, `src/surfaces/auth.rs`, `CONTEXT.md`.
- A live frame-gap improvement is not claimed here; it needs a session measurement.
