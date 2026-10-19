# 121. Remove the Liquid Glass finish

Status: accepted. Supersedes ADRs 0052, 0054 and 0100 for current product behavior. Those ADRs remain historical records. The shell no longer captures its own screen for a pane finish.

## Decision

- `appearance.finish` supports tinted solid, paper and compositor blur. Migration 6 maps `"liquid-glass"` to `"tinted-solid"` and removes the obsolete `appearance.refraction` key.
- Remove pane screencopy APIs and their runtime GPU/Wayland support. No Island, Dock or Banner behavior depends on captured pixels.
- Keep compositor blur, which asks the compositor to blur behind the shell's panes without copying the screen. Wallpaper and lock screen backdrop handling are independent and unchanged.
- Keep the Satellites' connected shape available for supported finishes. Reduced motion still draws them separately.
