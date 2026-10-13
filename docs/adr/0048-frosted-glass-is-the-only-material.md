# 48. Frosted Glass is the only material

Status: superseded by ADR 0049.

## Context

`appearance.material` once selected either liquid glass, which left the captured backdrop sharp and bent it into a rim, or monochrome, which hid it. The shell now has one intended look: Frosted Glass. The GPU backdrop capture and the lock screen's separate rendering path already provide the source imagery, but the two material branches leave different behavior across the Island, Dock, Banners, Settings and lock screen.

## Decision

- Keep `appearance.material` and its `Material` choice for future additions, with `frosted-glass` as its sole value for now.
- Use the existing edge-sampled frost for every captured shell pane, under a tone-aware tint and a restrained edge highlight. Island and Dock panes keep the lighter `Clear` tint; Banners keep the stronger `Regular` tint for text.
- The lock screen continues to use its own wallpaper preparation and CPU painter, frosting the prepared backdrop beneath the field and avatar with the same tone-aware tint and highlight.
- Settings keeps compositor blur when the protocol is available; without it, the window remains opaque. A pane with no usable capture shows its tint rather than failing or blocking the rest of the shell.
- Silently migrate legacy `liquid-glass` and `monochrome` values to `frosted-glass`, including files at the previous schema version.

## Consequences

- The Island, Dock and Banners have one consistent translucent, softened treatment. Users no longer choose between a sharp refracted rim and a flat tone; further materials can be added as explicit `Material` values later.
- Captured panes now use the backdrop path regardless of their former material, so former monochrome configurations may incur capture work. The runtime's existing capture cost and privacy boundary remain as documented in ADR 0037.
- Legacy configurations retain a valid appearance without warnings or manual edits.
