# 39. Wallpaper palettes through Matugen

Status: accepted; `appearance.sync` scope superseded in part by [ADR 0040](0040-targeted-desktop-appearance-adapters.md); external wallpaper changes and sync's colors by [ADR 0041](0041-wallpaper-colors-in-apps.md).

## Context

`appearance.tone` selects light or dark shell roles, but the roles were fixed constants. At startup, awww may already show a wallpaper that Kanade did not set. Matugen can derive both modes from an image and render application templates. Its configured targets are explicit; it does not theme applications with no target.

## Decision

- At startup, an off-thread lookup of awww's shown wallpaper seeds Matugen's palettes and configured template outputs. When Kanade successfully sets a wallpaper, it replaces that palette and rerenders the templates. Tone changes rerun Matugen for the known wallpaper; `appearance.tone` selects which parsed `ThemeRoles` Kanade uses.
- Matugen supplies one global shell palette. If outputs show different wallpapers, Kanade uses the focused output's image when known, otherwise the image on the first output in sorted name order.
- Kanade reads `~/.config/matugen/config.toml` (or its XDG equivalent) without changing it. It passes Matugen a private temporary config that preserves configured templates and imports, resolves their paths, removes template hooks, and disables wallpaper setting and its command. Only configured output paths are written.
- Matugen is optional. If it is missing or generation fails, Kanade uses its built-in roles. A template must be configured and mode-aware to produce matching output for both tones; no support for unconfigured applications is implied.
- `appearance.sync` is separate, opt-in, and uses targeted desktop, GTK3, qt5ct/qt6ct and Zen adapters described in [ADR 0040](0040-targeted-desktop-appearance-adapters.md). It does not render Matugen templates. Kanade reads awww at startup but does not watch wallpaper changes made outside Kanade afterward.

## Consequences

- The wallpaper already shown at startup can initialize shell roles and configured template outputs; Kanade's wallpaper selection and light/dark changes update them without blocking drawing or config reload.
- App coverage and fidelity depend on the user's Matugen templates. The `acknowledgements/` references show explicit target integrations, not a universal propagation mechanism.
- Untrusted hooks and wallpaper commands from the user's Matugen config are not run by Kanade's invocation. A missing or unreadable config, TOML parse failure, or Kanade safety rejection disables user templates and uses Matugen defaults for palette generation. Other Matugen validation or generation failures use Kanade's built-in shell colors.
