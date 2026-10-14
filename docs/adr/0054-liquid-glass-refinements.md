# 54. Liquid Glass refinements

Status: accepted. Amends ADR 0052 (its look and its readability stance) and ADR 0037 (what makes a copy news).

## Context

Liquid Glass drew at the refresh rate over a page animating behind it, its corners were circular arcs while the system's rounded shapes are not, its edge was as thick on a Banner as on the Dock, and over a bright page its light text was unreadable by decision (ADR 0052).

## Decision

- A copy is news only where the shader reads it. `backdrop::Request::unread` lists what the shader never reads (inside the body's outline, under the other panes it skips); a copy within `SAME` of the held one everywhere else is the same, so a page animating behind the body ends in the quiet path. The glass's own edge still feeds back, and settles within `SAME` as before.
- Corners are squircles in both shaders (`glass.wgsl`, `pane.wgsl`): the corner arc's Euclidean norm becomes a 4-norm (`SQUIRCLE`), blended back to 2 as the radius reaches half the short side, so a pill keeps round caps. The result is a distance, not an exact one, and a body is a hair fuller at a corner than the circle. The content's clip (`quads.wgsl`) stays circular and so stays inside.
- The edge is `clamp(0.35 * short side, 8, 20)` px thick, not always the studio's 20, so a small body (the Island at rest) keeps a clear middle. The read's reach stays 2% of the output's height, so the copy's margin is unchanged.
- Two touches from hyprglass: the Fresnel lift takes the hue of what the edge reflects (half toward it, white where nothing is reflected), and a shade of at most 6% black lies just inside the bottom edge.
- Readability is now a goal. The runtime already reads `Seen.light`, the mean luma of a ring 12 px outside the body, and the pane follows it (`src/pane/ink.rs`):
  - A black scrim under the glass grows with it from 0.35 to the flip, 25% at most (`glass.wgsl` `values[13].x`).
  - The ink flips to paper's (`theme::tint::paper`, containers as dark veils) when it reaches 0.65 and back at 0.55, a spring of the Island's motion between them (250 ms at the default speed), so frames are drawn only while it turns; the scrim fades out as the ink turns. Each pane has its own ink: the Island and the Dock (one pane when merged), each Banner.
  - The ink reaches the 285 call sites of `theme::island()` through a thread-local set for the length of a window's view (`pane::inked`, `theme::Inked`), in eighths, since each ink in between is an icon file and a raster. `theme::signals()` takes paper's darker hues from half way.
  - The mean is not a percentile: over a page half bright and half dark either ink may be wrong on one side.

## Consequences

- Idle frames stay 0: the ink asks for frames only while it turns, and a copy that is the same is not stored.
- A window's colors now depend on its pane's copy (thread-local state), so anything built outside a window's view, or on another thread, has the finish's own ink.
