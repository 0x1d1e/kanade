# 49. The shell is solid black

Status: accepted; the Island, Dock and Banners' black fill is superseded by ADR 0050 (the tinted solid), the rest stands, but ADR 0052 brings the capture back for one opt-in finish (Liquid Glass). Supersedes ADR 0048, ADR 0037 and ADR 0024's glass (its pane identity, variants and capture ownership go with them), and ADR 0020. Amends ADR 0025 (the lock screen's pill and disc are solid), ADR 0031 (one pane per merged shell stays, drawn solid) and ADR 0034 (the runtime no longer captures).

## Context

Frosted Glass needed a GPU copy of what lies behind each pane (wlr-screencopy), a lookahead that asked for it before a body moved, a frost shader, a pause while the session is locked, an exclusion of Kanade's own cast from the privacy cluster, and a separate CPU path on the lock screen. It cost memory and gpu time, and it let the desktop behind the Island decide the legibility of its text. The look wanted is the Island as a black pill: it reads on any wallpaper and any window.

## Decision

- The Island, Dock and Banners are solid black (`SemanticColors::island_surface`) with light text, whatever `appearance.tone` says. A merged Island and Dock (ADR 0031) stay one outline, filled by a small shader of the app's own (`src/pane.wgsl`), because a fillet between two rectangles is not a rectangle.
- `appearance.tone` stays, for Settings and the desktop app adapters only (`theme::roles`; the shell's `theme::island` is always dark).
- Settings is an opaque window; it no longer asks niri for blur.
- The lock screen keeps `lock.backdrop` (blurred, dimmed, sharp or solid wallpaper); its password pill and avatar disc are solid black.
- `appearance.material` and `appearance.highlight` are removed. Config schema 5 drops them silently.
- The runtime loses backdrop capture, the box blur pass, `Rectangle::blur`, `Rectangle::backdrop`, `Window::blur` and `ext-background-effect`. The privacy cluster counts every niri cast, as Kanade captures nothing.
- Panes with a shadow (the Island's body, Banners) always cast it.
- `lock.backdrop = solid` is the shell's dark surface whatever the tone, and the lock pill has a hairline rim so it shows on it.

## Consequences

- No screencopy, no lookahead, no per-pane textures: less memory and gpu work, and nothing to pause under the lock. Panes look the same on every wallpaper.
- The shell does not follow `appearance.tone` or the wallpaper's palette. A translucent or tinted shell would be a new decision.
- Old config files keep working; the two keys vanish on the next write.
