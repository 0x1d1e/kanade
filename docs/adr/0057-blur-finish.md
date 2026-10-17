# 57. Blur, a fourth finish, by the compositor

Status: accepted; its "Kanade never edits niri's config" is superseded by [ADR 0119](0119-kanade-edits-nieris-config-on-request.md) for an explicit editor. Amends ADR 0049 (`ext-background-effect` comes back, for this finish only, as the compositor's own blur, with no capture), ADR 0051 (a fourth `Finish`) and ADR 0052 (its "no blur" stands for Liquid Glass; this finish is the blur). The tinted solid stays the default (ADR 0050).

## Context

ADR 0048's frosted glass copied the screen itself (wlr-screencopy) and blurred it in a shader, at a cost in memory and gpu time that ADR 0049 removed. niri 26.04 blurs what lies behind a surface that asks for it through `ext-background-effect-v1`, in the compositor, with nothing copied by Kanade. The protocol takes a `wl_region` of rects in the surface's coordinates, clipped to the surface, double-buffered with the commit; it has no corners.

niri's docs (Configuration: Layer Rules, Window Rules and Effects, Miscellaneous) say that blur a surface asks for follows exactly the shape it asks for, and is xray by default: it blurs a copy of the wallpaper alone, so windows behind the surface are not in it and are hidden by it. A layer rule with `background-effect { xray false }` makes it the normal blur of everything below. niri marks non-xray effects as experimental; they are recomputed whenever what lies below changes. `blur { off }` turns all blur off.

Observed on niri 26.04 with no such rule (E2E, see `TODO.md`): a Surface over a window's text shows the blurred wallpaper only, the text hidden.

## Decision

- `appearance.finish = "blur"` (`look::Finish::Blur`), live like the others. The ramp is the tinted solid's (dark, light text, ADR 0050) while `appearance.tone` is dark and paper's (light, dark ink, ADR 0051, with its edge, rim and signal colors) while it is light, the one finish that follows the tone; its fill translucent (`pane::BLUR_FILL`, 78%) over what the compositor blurs. Ink never flips with the page: no capture reads it, so the readability of the text over a bright or dark window rests on the fill alone.
- The blur follows each pane's outline. `pane::blurred` cuts a body's outline (one rounded rect, or two united, as `pane.wgsl` computes it, in whole pixels, each pixel wholly inside) into rects, 1 px rows with equal runs coalesced, and each window hands them to the runtime (`LayerWindow::blur`, window coordinates like the input region). A body alone has the circle's corners its widget clips it to, as a squircle's fuller ones would blur past the pane's visible edge; united bodies are drawn unclipped, so keep the shader's.
- The runtime owns the protocol (`crates/runtime/src/wayland/effect.rs`, from the SCTK re-export of wayland-protocols): it binds the manager, learns the blur capability, and sends a region only when it changes, as the input region is, so a body at rest costs nothing and idle frames stay 0. `kanade_runtime::blur_available()` says whether the compositor can blur; when it cannot, the finish draws the opaque tinted solid, and `kanade doctor` warns that the manager is missing.
- A window asks for blur only for panes shown whole: the Island's and Dock's while visible, a Banner once its fade is complete (the compositor's blur does not fade), none under a fullscreen window.
- Windows behind a pane are blurred only under niri's `layer-rule { match namespace="^kanade(-dock|-banners)?$"; background-effect { xray false } }`. Kanade does not edit this rule into niri's config on its own (ADR 0119 allows the opt-in editor, which does not add it), so the rule is in `dist/niri/config.kdl` and documented in `docs/wiki/Installation.md`, and `kanade doctor` warns while the finish is `blur` and no rule sets `xray false` on `kanade`, `kanade-dock` and `kanade-banners` (a pattern it cannot read as plain text counts as matching, as for the awww rule). Without the rule the finish is the tinted solid over the blurred wallpaper.
- No capture: `pane::glass()` stays Liquid Glass only, and `pane::place` captures for no other finish.
- Settings lists Blur with a sample of a translucent pill over a band, as ADR 0053's previews do.
- The shadow stays: it has no piece for the body's middle, but its corner pieces reach under the body's corners, which the translucent pane shows faintly.

## Consequences

- The blur is niri's: its radius, passes, noise and saturation are the user's `blur {}` and layer-rule `background-effect` settings; Kanade sets none.
- Non-xray blur costs niri gpu time as what lies below the Island, Dock or a Banner changes, and while a body morphs the region changes each frame (a region sent per frame, a few hundred rects at most). Not measured, see `TODO.md`.
- Rects approximate the outline within a pixel; a fillet of two united panes and the corners are stairs a pixel wide under the pane's anti-aliased edge.
- A Banner leaving stops blurring at once, and one entering starts once it is whole, so the blur never shows past the card as it fades.
- Windows-behind blur depends on a niri rule the user must have. With it absent, only `kanade doctor` says why a window behind a pane does not show blurred.
- Files: `src/pane/blur.rs`, `crates/runtime/src/wayland/effect.rs`, `src/doctor/`, `src/sources/binds.rs` (`xrayed`).
