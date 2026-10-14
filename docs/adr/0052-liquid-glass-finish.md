# 52. Liquid Glass, a third finish

Status: accepted; its readability stance, edge thickness, corners and what makes a copy news are amended by ADR 0054. Amends ADR 0049 (the capture and the glass it removed come back, for this finish only) and ADR 0051 (a third `Finish`). The tinted solid stays the default (ADR 0050); a pane of any other finish captures nothing.

## Context

ADR 0049 removed frosted glass for its cost and for what a translucent pane lets the window behind decide. ADR 0051 made the look a setting, and left room for more finishes. Glass was a look worth having back as a choice, not as the only material, and this time from the thick-glass models of liquid-glass-studio (Snell edge profile, per-channel dispersion, Fresnel, glare); the look kept is liquid-glass-studio's clear glass with its reflective edge, at the studio's default values, not the frosted pane ADR 0048 had.

## Decision

- `appearance.finish = "liquid-glass"` (`look::Finish::LiquidGlass`), live like the others. The ramp is the tinted solid's (dark, light text); the pane is clear glass: no tint, no blur, no frost. The screen shows through the body (the compositor, through the pane's transparent pixels), and the edge reflects what lies around the body.
- The shader (`src/glass.wgsl`) is the final pass of liquid-glass-studio (`fragment-main.wgsl`) at its default values: the Snell edge profile over 20 px (factor 1.4), per-channel dispersion (N 0.98, 1, 1.02, times 1, the studio's 7 fringed text under the bottom edge), Fresnel (range 30, hardness 0.2, factor 0.2) and glare in LCH (range 30, hardness 0.2, factor 0.9, convergence 0.5, opposite 0.8, angle -45 deg), cut to the outline of one rounded rect or two united. A refraction distance of 2% of the output's height stands in for the studio's (5% smeared a window's edge into a band along the bottom).
- The copy holds the pane itself, so what lies under the body cannot be read. Where the studio reads inward, the edge reads the screen just outside the outline, along the same profile and channel by channel; a read of another pane of Kanade (a Banner under the Island) in any channel reads none, and a read past the screen's edge is skipped. That is a reflection of the screen, not true refraction.
- Capture is the runtime's wlr-screencopy path of ADR 0037 (`crates/runtime/src/backdrop.rs`, `wayland/backdrop.rs`), covering the body and the reach of the edge's read, with a lookahead (`pane::LEAD`) so a morphing body is read where it is heading; the Dock adds the icons' rise and the slide, the Banners each follow their own pane. It is driven only through `pane::place`, which releases the copy unless the finish is Liquid Glass, so switching away stops the capture and brings back the pane's shadow. A captured pane casts no shadow. `pane::locked` pauses every capture over the lock screen.
- The glass's containers (tiles, tracks, segments) are white veils at 11% and 19%, not the solid fills of the tinted ramp (`theme::tint::clear`), so a tile is more glass, not an opaque patch.
- The screen shows through, text behind the pane included. Over a bright page the light text was hard to read, which ADR 0054 answers with a scrim and dark ink.
- Kanade's own capture is not a screen cast: `sources::niri::stream` ignores casts of its own pid, so the privacy dot does not light.
- Files: `src/pane/{capture,glass}.rs` and `glass.wgsl`; `pane/shader.rs` writes `glass-<pid>.wgsl` beside `pane-<pid>.wgsl` when first asked for, under the same lock and sweep.

## Consequences

- Cost returns only for this finish: a screencopy per changed frame of each pane's region, read by the shader. Other finishes pay nothing. The idle-frame, wakeup, frame-gap and memory baselines are not taken yet, see `TODO.md`.
- No refraction of what lies under the body, and no blur: no compositor interface we may use leaves a client's own surface out of a copy. The studio's shadow is not drawn.
- A Media track's accent and the tray icons are fitted to the island's ramp when drawn, so a finish switch recolors them at once; the lock screen stays solid.
