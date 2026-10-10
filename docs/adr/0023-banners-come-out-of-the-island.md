# 23. Banners come out of the Island

Status: accepted. Supersedes ADR 0007's top-right placement of Banners; the rest of ADR 0007 stands. Amended by ADR 0024: Banners are glass of their own.

## Context

ADR 0007 put Banners in the top-right corner of the focused output, in a window of their own. On macOS a notification comes from the menu bar's side, and on an iPhone with a Dynamic Island an alert grows out of the Island. A fixed corner also ignores `island.edge`/`island.align` and `dock.edge`/`dock.align`: a Dock placed top-right lay under the Banners.

## Decision

- Banners hang from the Island, on its edge and side: under a top Island, over a bottom one, lined up with its side or centered under it. Past a Dock beside the Island, they hang past the Dock, so neither covers the other (ADR 0030 keeps them clear of a Dock on any edge). The newest is nearest the Island.
- The window is one per output, laid out as the Island's (`CANVAS_WIDTH`, same anchor), taller by room for `MOST` Banners. Each frame it reads the Island's body as it is now, slid by autohide, so Banners follow the body as it grows or slides.
- `banners/motion.rs` keeps per output how far each Banner is out (0 to 1) and where it hangs, on springs of the Island's motion and damping, so they bounce as the body does and snap under reduced motion. Coming in, a Banner swings past its card; going, it is critically damped, never swinging back out. The others slide to make room or close the gap. Time is an input.
- `banners.entrance` picks how a Banner comes in: `morph` (default), the Island body's twin, of its material, stretching into the card, the card's fill showing as it parts and what it says once it is nearly out; `drop`, from just past the edge; `fade`, in place, from a little smaller. It goes back the same way.
- A Banner takes the pointer once it is nearly in place, and none while it goes.

## Alternatives

**Grow the Banners in the Island's own window.** One body, a true morph, but the Island's window is sized for its Surfaces and its input region, and a Banner would compete with the Activity and Surface for the body, which ADR 0007 moved them out of.

**Keep the corner, add a check against the Dock.** Fixes the overlap but not the link to the Island that macOS and iOS have.

## Consequences

- While a Banner moves, the Banners window draws every frame, and while the Island moves it follows it; at rest it draws nothing.
- The twin is the card's own glass, of the Island's material in its Regular variant, so under liquid glass it bends what is behind it from its first frame (ADR 0024); what the card says fades in once it is nearly out.
- The window is as tall as `MOST` Banners of the most a card shows (its body at the most lines, an action row), measured once, as the font is set only at start; clamped to the output; Banners that would not fit are not shown until room is made. A bouncy damping swings them past their card; a critically damped one does not.
- Under autohide, the Banners follow the Island's slide out and back, as `autohide::Wakes` is written when a slide starts. With the morph they come out of the body wherever it is.
- The window is taller than the Banners shown, but its input region is only the Banners, so the rest of it takes no pointer.
