# 54. Liquid Glass refinements

Status: accepted. Amends ADR 0052 (its look and its readability stance) and ADR 0037 (what makes a copy news).

## Context

Liquid Glass drew at the refresh rate over a page animating behind it, its corners were circular arcs while the system's rounded shapes are not, its edge was as thick on a Banner as on the Dock, and over a bright page its light text was unreadable by decision.

## Decision

- A copy is news only where the shader reads it. `backdrop::Request::unread` lists what the shader never reads (inside the body's outline, under the other panes it skips); a copy within `SAME` of the held one everywhere else is the same, so a page animating behind the body ends in the quiet path. The glass's own edge still feeds back, and settles within `SAME` as before.
- Corners are squircles in both shaders (`glass.wgsl`, `pane.wgsl`): the corner arc's Euclidean norm becomes a 4-norm (`SQUIRCLE`), blended back to 2 as the radius reaches half the short side, so a pill keeps round caps. The result is a distance, not an exact one, and a body is a hair fuller at a corner than the circle. The content's clip (`quads.wgsl`) stays circular and so stays inside.
- The edge is `clamp(0.35 * short side, 8, 20)` px thick, not always the studio's 20, so a small body (the Island at rest) keeps a clear middle. The read's reach stays 2% of the output's height, so the copy's margin is unchanged.
- Two touches from hyprglass: the Fresnel lift takes the hue of what the edge reflects (half toward it, white where nothing is reflected), and a shade of at most 6% black lies just inside the bottom edge.
