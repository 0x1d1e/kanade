# 156. A pane's light is directional and its joins are circular fillets

Status: accepted. Amends [ADR 0050](0050-the-shell-is-a-tinted-solid.md) (the pane's rim, top light
edge and pane path) and [ADR 0101](0101-satellites-as-one-liquid-shape.md) (the Satellites' union).
ADR 0121 stands: no pane samples the backdrop, so the light is derived, never captured.

## Context

`pane.wgsl` and `blob.wgsl` united two rounded rects with a polynomial smooth minimum,
`min(a, b) - pull² * blend * 0.25`: a fillet only 0.25 of the blend radius deep where the contours
cross, and not an arc. The Island and Dock read as a soft crease rather than a rounded join.

A pane that was not merged took a different path: `pane::layer` returned a plain `Rectangle` with a
gradient fill and a 1 px border, so a standalone Island, Dock or Banner had neither the shader's rim
nor any directional light. A merged pane had a rim and a top light edge, but both painted at one
weight all the way round and across: the rim glowed equally on the shaded bottom, and the top edge
did not fade toward the corners.

The reference for the join is Caelestia's circular smooth minimum
(`acknowledgements/shell/plugin/src/Caelestia/Blobs/shaders/blob.frag`), and for a light that is
part of the rim rather than a picture of the room, Vitreus' bar glass
(`acknowledgements/Vitreus/pages/bar_glass.plm`).

## Decision

- `united(a, b, k)` in `src/pane.wgsl`, `src/blob.wgsl` and the CPU copy in
  `src/pane/blur.rs::united` is the exact circular fillet:

  `max(k, min(a, b)) - length(max(vec2(k) - vec2(a, b), 0))`

  where both contours are within `k`, and the nearer one alone beyond that band. The fillet is an
  arc of radius `k` tangent to both contours, deepest at `(sqrt(2) - 1) * k` where they cross, about
  0.414 of the radius instead of the polynomial's 0.25.
- `merge::BLEND` is retuned from 14 to 9 px so the join keeps its old depth: `(sqrt(2) - 1) * 9` is
  about `0.25 * 14`. In `src/blob.wgsl` the neck passes `reach * 0.62` as the fillet radius, the
  same depth compensation; `reach` itself still sets the neck's extent, its opacity fade and the
  shape's rect, so a dot's travel and snap do not change.
- The CPU copy exists because the blur region is cut from the same outline: `pane::blur::behind`
  walks whole pixels against `Outline::distance`, so a blur rect can never sit outside the visible
  fill. It must stay the same formula as the shader.
- Every pane goes through `pane.wgsl`: `pane::layer` no longer has the `Rectangle` fast path. A pane
  that is not united sets `values[0].w = 0`, and `outline` falls back to one continuous rounded
  rect. Fill, lit fill, edge, rim and the Blur finish's alpha are unchanged in meaning, so a
  standalone pane and a merged one now share the same outline, light and bevel.
- The rim and the top light edge are directional, from the distance field alone:
  `g = vec2(dpdx(d), dpdy(d))`, `normal = g / max(length(g), 1e-5)`, and one fixed top-left light
  `normalize(vec2(-0.45, -0.89))`. The rim is weighted by `0.35 + 0.65 * max(dot(normal, L), 0)` and
  the top-edge band by `0.25 + 0.75 * lit`. Both stay 1 px, the shader still reads no clock, and the
  zero-gradient interior reads as no light rather than a NaN.
- A translucent pane gets a subtle bevel: an inside distance `-d`, width
  `min(10, min(size.x, size.y) * 0.15)`, `bevel = 1 - smoothstep(0, width, -d)`. When
  `fill.a < 0.95` the fill is mixed `bevel * 0.35` toward a slightly darker, more saturated version
  of itself, before the lit fill, edge and rim are applied. Opaque tints and paper are unchanged.
  No color is sampled, so the bevel is not a picture of what lies behind.
- No finish is added and no setting is introduced.

## Consequences

- Standalone Island, Dock and Banner panes and the merged Island + Dock draw through one shader and
  one outline function; a visual change to the rim reaches all of them.
- The join is a true circular arc; the old depth is kept by the smaller radii, and the blur region
  follows the arc because its CPU copy uses the same formula.
- A translucent pane darkens slightly just inside its edge, a bevel the tint only suggests; the
  opaque finishes are flat as before.
- Every pane now runs a fragment shader rather than a rectangular gradient, the cost of the shared
  path. Nothing new runs at idle: the shader reads no clock and samples nothing.
- Files: `src/pane.wgsl`, `src/blob.wgsl`, `src/pane/blur.rs`, `src/pane/mod.rs`, `src/merge.rs`,
  `docs/adr/0050-the-shell-is-a-tinted-solid.md`,
  `docs/adr/0101-satellites-as-one-liquid-shape.md`.
- The visual result needs a live session; this ADR does not claim an E2E measurement.
