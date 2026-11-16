# 159. Press feedback on the Island

Status: accepted. Builds on [ADR 0150](0150-continuous-island-shape-and-content-motion.md) (the
Island's continuous shape and content motion), [ADR 0030](0030-the-canvas-follows-the-largest-body.md) (the canvas
never resizes during a morph) and [ADR 0031](0031-the-island-and-the-dock-merge.md) (a merged Dock
shares the Island's window).

## Context

A left press on the Island currently does one thing: it expands. Until the click lands, the Island
gives no sign that it felt the press, unlike macOS's island, which dips and springs back. The
runtime's `Rectangle::on_hold` already reports a left press and its end, and it deliberately sets no
click, so a target beneath it still gets the short press. The Island's body target already carries
the click, hover, move and wheel handlers, so the press can be reported from the same target with no
new `Input` variant and no change to how a click opens a Surface.

Two constraints shape the feedback. The window's size and its compositor input region belong to the
Presentation geometry: a press may not resize the surface or move the pointer's region, or a still
pointer would be left out or a click would be swallowed. And reduced motion must not scale anything,
while a press still has to be acknowledged by the click path as before.

## Decision

- **The body target reports the press, not a new input.** The existing Island body target in
  `src/view/mod.rs` gains `.on_hold(press)`. The handler writes `IslandService::press(down, now)`
  through `src/view/input.rs`: true on a left press, false on release, on the pointer leaving the
  target or the window, or on the target disappearing. It sets no click, so `on_click` still opens
  the Island as before, and `Input` gains no variant.
- **One critical `Spring<1>` owns the swell.** `IslandService` holds `press: Spring<1>`, at rest
  `1.0`. A press targets `1.04` while held and `1.0` on release, over `PRESS`, a 90 ms response,
  with critical damping (`motion::CRITICAL`) so it never overshoots. A write whose target is already
  what it asks is skipped, since a write wakes every window. A reload that crosses motion modes
  puts the swell at rest (`retime`), so it never jumps between them.
- **Reduced motion never scales.** `press_scale(now)` returns exactly `1.0` under `Mode::Reduced`,
  whatever the spring holds, and `press_settled(now)` reports settled so the view asks for no
  frames. The spring itself is sampled with `clock::now()` in the view, as every other Island spring
  is; the handler stamps the press with `Instant::now()`, as the other input handlers do.
- **The view scales drawing and hit targets only.** `view::island` scales the window's content
  rectangle, transparent and fill-less, by the sampled swell. Because the body is not at the
  window's centre (it hugs an edge), the rectangle also carries a compensating `translate`
  computed by `press_shift`, so the scale is about the body's own centre and the body never slides
  toward or past its edge. The content stays the same single rectangle in the tree, so a pressed
  target keeps its key and `on_hold` lasts the whole press; at rest the rectangle carries no
  transform at all.
- **Nothing else moves.** The window size, canvas, pane geometry, presentation layout and the
  compositor input region are unchanged; only the content's transform is. The view requests frames
  while `press_settled` is false, and none once it rests. Banners, the standalone Dock, Desk and
  Settings windows are untouched. A Dock merged into the Island's window (ADR 0031) shares that
  window, so its plate and icons take the same brief swell.

## Consequences

- A left press swells the Island by 4% over about 90 ms and the release settles it back; a click
  still expands exactly as before, since the hold sets no click and the press target sits above the
  content targets without taking their clicks.
- Reduced motion shows no swell, and the swell spring asks for no frames under it.
- Scaled hit targets briefly extend past the body with the drawing. The compositor's input region
  does not scale, so a pointer can still leave the window and end the hold there, and a release
  outside the body's area ends it as `on_hold(false)`.
- The swell is one per `IslandService`: every Island drawn shares it, but only the Island with the
  pointer can be held, so only that one is being pressed.
- **Not run live**: the swell on a real output, its absence under reduced motion, and a click
  opening mid-press (E2E NOT RUN). The spring's targets, release, repeated-press and reduced-motion
  behavior and the swell's centre-preserving transform are unit-tested.
