# 100. Liquid Glass takes its own drawing out of the copy

Status: accepted, experimental and off by default (`appearance.refraction`). Amends ADR 0052 (its "no refraction of what lies under the body": it holds with the setting off, and where the page cannot be told) and ADR 0054 (the ink's ring stays as it is).

## Context

ADR 0052 said true refraction is impossible, as no interface leaves a client's own surface out of a copy of the screen, so the edge reflects what lies just outside the outline. That is true of the copy alone. The shell knows its own drawing: the compositor blends the window over the page, so for a pixel

`copy = ours + (1 - alpha) * page`

with `ours` the window's premultiplied pixel and `alpha` its alpha, and so `page = (copy - ours) / (1 - alpha)`. Pleamar (`src/lens.rs`, `backdrop.wgsl`) recovers its background this way. Where the shell is clear, the copy already is the page; where it is partly drawn (the scrim, the veils, the edge's glints), the page is recovered exactly; where it is opaque (text, icons, the edge band), nothing can be told.

## Decision

- `appearance.refraction` (default false, live, Liquid Glass only) lets the glass edge read inward, as liquid-glass-studio's does, from the same distance and channel by channel, from the page under the body, where it can be told. Where any channel cannot be told it reads outward as ADR 0052 says. With the setting off, or on another finish, nothing below runs and no frame is kept.
- The runtime keeps the window as last presented: `Rectangle::presented()` hands the shader the previous frame as `canvas` (binding 6, premultiplied; one transparent texel while there is none), and the window copies each frame to a texture only while a shader of that frame asked, dropping it the first frame none does and when the window resizes. The shader reads `canvas` where the copy lies in the window, mapped from the values `pane::capture` hands back with the copy.
- The unmix is `glass.wgsl`'s `page`, with its rule pinned by `pane::unmix` (reference and tests). A pixel is rejected, and the edge falls back, when: no frame is kept, the place is off the copy or the window, `alpha` is over 0.95 (`MAX_ALPHA`; past it a level of 8 bit is magnified over 20 times), or a channel of the result is outside 0 to 1 by more than `SLACK` (0.03) plus half a level over what remains (the copy and the frame are 8 bit). Results within the slack are clamped. The rejection is the only guard against the next three limits.
- The body's interior is read, so a change under it is news for the copy (`unread` leaves it out of the list): a page animating behind the pane draws at the refresh rate while this is on, as true refraction follows it.
- The ink keeps the ring of ADR 0054: the ring lies outside the body, where the copy is the page already, so the unmix would add nothing.

## Limits

- Frame skew. The copy is of the screen when the compositor took it, a frame or two before or after the frame the shell kept. Where the window moved or changed between them (a morphing body, a ticking clock) the result is no color and the pixel is rejected, or, worse, a plausible wrong one. The body's edge is the likeliest place.
- The glass's own edge band is nearly opaque, so what lies under the outermost pixels is told only by reading further in than the band. Where the profile reads least, it falls back to the outward reflection, so a body shows both looks.
- A copy of the output includes everything above the window: another Kanade pane (skipped as before), a window in a higher layer, the cursor if composited. Their pixels would be read as page. Not under Blur (no copy) or the solid finishes (none kept).
- Fractional scales and a UI scale not 1 (ADR 0091) put the copy and the frame off the pixel grid by a fraction; bilinear reads blur a sharp edge, which then fails the range check there.
- Cost: a copy of each frame of the window on the gpu while on, a texture of the window's size, one more sample per channel for the edge. A compositor that dims or blends layers other than at full opacity (a niri layer rule `opacity`) breaks the model; nothing detects it.

## Consequences

- One more binding (6) on every custom shader, bound to the transparent texel when unasked. No shader but `glass.wgsl` declares a use.
- Verified by build, naga validation of the shader and unit tests of the rule only. How it looks, whether the skew rejects too much or too little, and its cost are for a live session (`TODO.md`).
