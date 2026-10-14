# 51. Paper, a second finish

Status: accepted. Amends ADR 0050: the tinted solid stays the default and the Pane's one look is now `appearance.finish`. ADR 0049 (no capture, no blur) stands.

## Context

ADR 0050 gave the shell one look, a dark tinted solid. A shell on a light desktop wants a light one, and Settings and the app adapters already follow `appearance.tone`, which the Island deliberately does not. Frosted glass and a translucent tint were compared against it on a prototype board (the same Controls Surface in five looks, over real wallpapers and a real window) and left out: glass needs the capture ADR 0049 removed, and a translucent pane lets the window behind decide its text.

## Decision

- `appearance.finish` (`look::Finish`) is `tinted-solid` (default) or `paper`, and applies live. It is not `appearance.tone`, which is for Settings and apps; a dark Settings beside a paper Island is allowed.
- Paper is the same opaque ramp turned over (`theme::tint::paper`, OKLCH from the same Matugen source color): surface L 0.955, container 0.915, container-high 0.87, near-black text (L 0.22), secondary text (L 0.42) and a dark ink accent (L 0.30). Contrast is held by the same test across hues and chromas as the tinted solid's (text 7:1 and 4.5:1, accent 4.5:1).
- A pane is still its fill, a hair lighter at the top, a top light edge and a rim, in `pane.wgsl`, which does not change. Paper's edge is white at 90%, its rim a faint ink line, its pinned ring ink; the shadows are the same two.
- `theme::island()` and `theme::signals()` follow the finish: paper takes the darker signal hues the light Settings window already uses. `theme::pane_edge()` and `pane_rim()` replace the two white constants.
- The lock screen keeps its black pill and disc, whatever the finish.
- Settings: Appearance, section Finish, a plain choice.

## Consequences

- A second ramp is built with the first from each Matugen run (`Palettes::paper`), at no extra process or frame; a finish change redraws every window that reads the config.
- Values computed once from the finish stay as they were until their source updates: a Media track's accent and a tray item's symbolic icon, which `sources::icons::lit` only lightens (a light symbolic icon can vanish on paper). They follow on the next track or tray refresh.
- Next finishes (a bezel, say) are new `Finish` values with a ramp or shader values, not a new setting.
