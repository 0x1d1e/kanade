# 50. The shell is a tinted solid

Status: accepted; the one look is now a default finish (ADR 0051), one of three (ADR 0052). Supersedes ADR 0049's black fill for the Island, Dock and Banners; the rest of ADR 0049 (no capture, no blur, one outline for a merged shell, the lock screen's solid pill and disc) stands.

## Context

ADR 0049 made the Island a black pill, which reads on any wallpaper and any window but looks the same on all of them, and apart from the desktop it sits on. It left open that "a translucent or tinted shell would be a new decision". A prototype board (opaque, wallpaper-tinted panes, depth from a top light edge, a rim and the existing soft elevation) was compared against black frame for frame, over five real wallpapers and the real shell's captures, and chosen.

## Decision

- The Island, Dock and Banners are opaque, tinted toward the wallpaper's hue. The ramp is built in OKLCH from the source color Matugen reports (`src/theme/tint.rs`): surface (L 0.16), container (+0.05), container-high (+0.10), near-white text (L 0.96), secondary text (L 0.82) and one accent, `primary` (L 0.84). Each step is opaque, since a pane is solid.
- Chroma scales with the source's own, from a quarter of the tint for a grey wallpaper to all of it, so a grey wallpaper still tints a little and a vivid one does not run away into a color. With no wallpaper known the ramp is a cool slate.
- The ramp is the same whatever `appearance.tone` says. `theme::island()` returns it; Settings and the app adapters keep Matugen's M3 roles (`theme::roles`).
- A pane is its fill, a hair lighter at the top, a top light edge (white at 10%, a 1 px line along the top of the outline, curving with the corners) and a rim (white at 6%, 1 px inside the whole outline), then the two shadows ADR 0049 already casts. All drawn by `src/pane.wgsl`, which still never reads the clock; a merged Island and Dock (ADR 0031) keep one outline, so edge and rim follow it.
- Signal colors (privacy, capture, warning, critical) keep their fixed hues; the tint never reaches them.
- The lock screen keeps its black pill and disc (`SemanticColors::island_surface`, now only theirs). `lock.backdrop = solid` takes the tinted surface.

## Consequences

- Panes follow the wallpaper when it changes, with the same Matugen run that already feeds Settings: no new process, no capture, no extra frame.
- Text on every step keeps its contrast whatever the hue; `tint.rs` tests it across hues and chromas (text 7:1 and 4.5:1, accent 4.5:1).
- States that the prototype defined past container-high (a pressed or selected step) are not roles yet; views keep their own.
- The tint is a hue and a chroma, not Matugen's tonal palette, so the Island's ramp and Settings' differ in lightness by design.
