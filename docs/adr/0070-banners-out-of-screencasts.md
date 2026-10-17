# 70. Banners out of screencasts, by an opt-in niri layer rule

Status: accepted. Builds on [ADR 0057](0057-blur-finish.md) (the same kind of rule, in niri's config). Its "Kanade does not edit niri's config" is superseded by [ADR 0119](0119-kanade-edits-nieris-config-on-request.md) for an explicit, opt-in editor; this rule is still the user's to add.

## Context

A Banner shows a notification's summary and body, which can hold private text, and it sits on the Top layer, so a screencast (a portal share, OBS) shows it to whoever watches. niri can draw a layer as a black rectangle in screencasts with the layer rule `block-out-from "screencast"`; `"screen-capture"` blocks it from every capture, which would also blank what Kanade's own `wlr-screencopy` sees (the Liquid Glass edge).

The `kanade` namespace is the Island, which also draws the privacy dots (microphone, camera, capture); a block there would hide from a viewer the very sign that a cast runs. `kanade-dock` shows no private text.

## Decision

- `dist/niri/config.kdl` carries the rule for `^kanade-banners$` only, with `block-out-from "screencast"`, commented out: it is the user's choice, since a viewer of a share then never sees a Banner, including one the user wants to show.
- Never `"screen-capture"`, never the `kanade` namespace.
- `kanade doctor` reads the config as it does for the wallpaper and blur rules and says, while the `banners` Module runs, whether a layer rule blocks `kanade-banners`. Not blocking is an ok line, not a warning.
- Kanade does not add the rule to niri's config, and does nothing at run time (ADR 0119: the opt-in editor changes the file only on request).

## Consequences

- Off by default: Banners show on screencasts until the user adds the rule.
- A pattern the doctor cannot read as plain text counts as matching, as for the other rules.

Files: `dist/niri/config.kdl`, `src/sources/binds.rs` (`blocked_from_screencasts`), `src/doctor/mod.rs`, `docs/wiki/Installation.md`, `docs/wiki/Limitations.md`.
