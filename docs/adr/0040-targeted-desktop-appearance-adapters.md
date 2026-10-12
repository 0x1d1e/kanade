# 40. Targeted desktop appearance adapters

Status: accepted; wallpaper colors, the Qt style rule and Zen's light/dark preference superseded by [ADR 0041](0041-wallpaper-colors-in-apps.md).

## Context

The desktop color preference reaches GTK4 and some Qt versions, but does not reliably update GTK3 or Qt5 on niri. Zen's browser chrome also has its own profile preferences. Matugen generates configured files; it does not switch arbitrary applications.

## Decision

- Keep `appearance.sync` opt-in and separate from Matugen.
- Set the desktop color preference, pair an installed GTK theme with its `-dark` variant, and update GTK3's theme and legacy dark preference in `gtk-3.0/settings.ini`.
- Update palette paths in existing qt5ct and qt6ct configs. When a config selects a known dark-only style (`kvantum-dark` or `Darkly`), use Fusion in light mode so the light palette is visible; restore the prior style when sync is disabled, unless the user changed it meanwhile.
- Add a marked preference block to discovered Zen `user.js` profiles. Zen reads it at launch, so a running browser needs restart.
- Save prior managed values and restore them only while their current values still match Kanade's last applied values. Do not claim coverage for apps without these adapters.

## Consequences

- GTK3 apps can follow a paired installed theme and its legacy preference; GTK4 and some Qt apps may follow the desktop preference. GTK3 theme changes may be live, while file-based GTK, Qt and Zen settings can require app restart.
- Qt apps must use qt5ct or qt6ct for those config changes to apply. Fusion in light mode gives palette-aware widgets a reliable light palette but replaces the configured dark-only style for that mode.
- This does not set every app theme, generate wallpaper-derived colors for these adapters, or provide universal live switching. Matugen remains responsible only for configured template outputs.
