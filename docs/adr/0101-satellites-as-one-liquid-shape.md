# 101. Satellites bud out of the body as one liquid shape

Status: accepted. Amends the Satellite rule of `CONTEXT.md` (it still comes out from under the body, none pops) and ADR 0050's single outline (the body's pane is unchanged; the dots under it are a shape of their own).

## Context

A Satellite was a round dot with its own fill that slid out from under the body's end. Next to a liquid body it read as a separate object. The references (suzuha's `liquid.wgsl`, k4's `Gota`, ukishima's `Ame`) unite shapes with a smooth minimum whose radius follows how far apart they are, so a neck stretches and snaps.

## Decision

- `src/blob.wgsl`, drawn by `pane::blob`, is the smooth union of the body's rounded rect and up to 6 dots (circles), nearest the body first: `d = united(d, dot, reach)` in turn, with `united` as `pane.wgsl` has it (a fillet within `reach`, exactly the nearer shape beyond).
- `reach` follows the separation and the dot's opacity: `REACH * (1 - gap / SNAP) * opacity`, the fraction clamped to 0..1, where `gap` is how far the dot's edge is from the body and the dots before it, and `SNAP` is the dots' resting gap (`SATELLITE_GAP`, 6 px). At rest the fraction is 0, so a dot is drawn exactly as before, and a dot fading in pulls a small neck, not a wide faint one. `REACH` is 32 px: the neck is wide while the dot is within the body and thins as it parts, to a thread and a snap in the last part of a dot's travel out, and the reverse tucking back. The neck is not tied to a clock: it is a function of the dot's place, which the Satellites' own springs move.
- The shape's rect (`pane::blob`) is the body's box plus each dot's reach, so a neck is never cut at the rect's edge.
- The union is drawn under the body and only outside it (the dots themselves aside), in the dots' own color, shading to the body's fill where it leaves the body. The body's pane is unchanged, so its single outline (ADR 0050), the merged shells (ADR 0031), the blur region (ADR 0057) and the capture (ADR 0052) are untouched: dots were never part of them.
- Each dot keeps its own opacity (the fade in and out), and none pops. Outside the dots the neck takes the largest of the dots' opacities, each fading out over a reach from its edge, so it never changes at once where one dot's nearness passes to another's. At a dot's edge and inside it only that dot's own opacity counts, rising to the shared one over a reach, so a faint dot is not drawn at its neighbour's.
- Degrades to the previous look, each dot with its own fill (`pane::blob` returns None): under Liquid Glass (the screen copy would read the neck, which is no pane, and it cannot be checked off a live session), under reduced motion (a dot is already in its place, so no neck would show) and past 6 dots.

## Not done

- Banners. They hang in a Wayland window of their own (ADR 0023), so no shape can join them with the Island's body; their pane is a separate pane. A true bud needs the Island's window to hold the Banners (ADR 0023's rejected alternative).
- Merged shells' Dock: the union is with the Island's body only, as dots hang from it.
- Liquid Glass's dots stay plain.

## Consequences

- One more shader file, `blob-<pid>.wgsl`, written, locked and swept as the others (`pane/shader.rs`). Nothing new at idle: the shader reads no clock.
- The neck is visible only while a dot moves. Seen live in blur, tinted and paper over white and dark; a faint tongue at low opacity during a fade-in is accepted.
