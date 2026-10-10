# 1. Fixed canvas with an input region instead of resizing the layer window

Status: accepted (Phase 0, #2-#5). Layer superseded by ADR 0006. Amended by ADR 0030: the canvas follows the largest body, the output and the edge.

## Context

The island body morphs between a 150x32 pill and a 440x160 surface, and later between more Presentations. Each morph runs for 180 ms at up to 144 Hz. Kanade runs on niri through Amane, where a layer window has a size, an anchor, a keyboard mode, and an optional input region. Amane re-sends the keyboard mode and the input region on every frame in which they change.

The body has to animate without jitter. Outside the body the pointer must reach the windows below, and the space the island takes must not depend on what it shows.

## Decision

Each monitor gets one layer window with a fixed canvas: top-center, `Layer::Overlay`, `Zone::Ignore`, 560x380, transparent. The window never resizes. The body is drawn inside the canvas and morphs there. The input region is the body plus 8 px of hover padding, rounded out to whole pixels, and it is recomputed every frame from the animated shape.

## Alternatives

**Resize the layer window per frame.** Every size change sends `set_size` plus a commit and is applied after an asynchronous compositor configure. During animation this adds a compositor-mediated resize/configure cycle per size change. Whether that causes visible jitter was not measured. Until configure arrives, Amane may still draw at the previous configured size, so growing content can be clipped.

**A separate popup window per surface.** The pill and its expanded surface become different windows, so one shape cannot morph into the other. It also breaks the goal of one body showing many contexts.

## Consequences

- One large window per monitor stays mapped at all times, and most of it is invisible. It reserves no space (`Zone::Ignore`); `island.reserve` keeps windows off the Island's strip with a window of its own. Because of the input region, clicks outside the body reach the window below (verified in #2).
- The input region is a rectangle, so a click in the rounded corners or in the padding counts as on the body.
- The region updates on every frame of a morph. In #2 this cost nothing measurable: niri 27.5% vs 27.9% CPU, 126.6 vs 127.0 fps, against a fixed region during continuous morphing at 144 Hz.
- Leaving the island means leaving the input region. In #3, niri sent the pointer leave when the region shrank away from a still pointer, so collapse and hover grace (#5) need no fallback region.
- Keyboard mode belongs to the window, so the island and every surface in it share one mode at a time. It can switch from one frame to the next (#4). A surface that needs a different mode while the island shows something else needs its own window.
- The largest body plus its shadow is limited to the canvas. A bigger Presentation means a bigger canvas, which is a one-line change but enlarges the invisible window.

Sizes above are the values when this was decided. Current ones live in `src/island/geometry.rs`. Measurements are in #2 and PR #44.
