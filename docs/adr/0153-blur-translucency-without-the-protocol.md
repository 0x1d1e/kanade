# 153. Blur translucency without the background-effect protocol

Status: accepted. Amends [ADR 0057](0057-blur-finish.md).

## Context

ADR 0057 made the Blur finish translucent only while `kanade_runtime::blur_available()` reports
`ext_background_effect_manager_v1`. It was written for niri, which offers that protocol and takes a
region of rects Kanade cuts from each pane's outline. Umbriel (ADR 0147) has no such protocol: its
blur is a layer rule, `blur-kanade`, and it blurs the layer surface where the fill's alpha passes
`blur_ignore_alpha`, with no region supplied by the shell. Under the old gate an Umbriel pane was
opaque even while the documented rule blurred behind it, so the rule had nothing to show through.

The recommended rule also matched only `kanade`, `kanade-dock` and `kanade-banners`. It missed
`kanade-dock-menu` and `kanade-launcher`, which draw panes too, so a user following the
recommendation still saw those layers opaque or sharp.

## Decision

- `pane::blur::translucent()` is true when `appearance.finish` is `Blur` and either the compositor
  offers the protocol or, under the selected Umbriel compositor, its config has an active
  `blur-kanade` layer rule. A missing, unreadable or invalid Umbriel config is false. The rule is
  cached by config path, mtime and length, with a short freshness window, so a pane draw does not
  parse TOML and a rule edit is seen within half a second.
- `pane::blur::behind_available()` is true when the finish is `Blur` and `blur_available()` holds.
  `pane::blur::behind()` uses it and emits no rects otherwise. The shaped-region request stays
  protocol-only: on Umbriel Kanade requests no blur region, and Umbriel's own rule does the blur.
- The pane fill uses `translucent()`, so an Umbriel pane with the layer rule is translucent and one
  without it is the opaque tinted solid. `blurs(shown)` is unchanged: a pane asks for the
  compositor's blur for as long as any of it shows.
- The recommended namespace list is `kanade`, `kanade-dock`, `kanade-dock-menu`, `kanade-banners`
  and `kanade-launcher`, with the pattern `^kanade(-dock(-menu)?|-banners|-launcher)?$`. The older
  narrow patterns still read as an active `blur-kanade` rule, so existing configs are not reported
  missing.
- `kanade doctor` under Umbriel warns while the finish is `Blur` and no `blur-kanade` rule is
  active, saying panes draw the opaque tinted solid; with the rule it says the compositor blurs
  behind Kanade's layers. It does not claim the rule shapes blur to Kanade's outline. Under niri the
  existing xray and protocol checks remain, over the extended namespace list; the protocol check is
  not asked under Umbriel, which does not use it.

## Consequences

- On Umbriel the blur's shape and strength are the compositor's: the layer surface's alpha against
  `blur_ignore_alpha`, `blur_popups` and the compositor's own blur settings. Kanade's outline does
  not shape it.
- On niri the protocol path is unchanged: the region still follows the pane outline, and a missing
  manager still leaves the opaque tinted solid.
- A rule change is picked up without a restart, within the cache's freshness window.
- Files: `src/pane/blur.rs`, `src/pane/mod.rs`, `src/sources/binds.rs`, `src/umbriel_config/rules.rs`,
  `src/niri_config/rules.rs`, `src/doctor/`, `src/doctor/wayland.rs`,
  `docs/wiki/Installation.md`, `docs/wiki/Configuration.md`.
