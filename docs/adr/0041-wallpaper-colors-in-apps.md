# 41. Wallpaper colors in apps

Status: accepted; amends [ADR 0039](0039-wallpaper-palettes-through-matugen.md) and [ADR 0040](0040-targeted-desktop-appearance-adapters.md).

## Context

`appearance.sync` switched GTK, Qt and Zen between light and dark, but only Matugen's user-configured templates carried the wallpaper's colors, and a wallpaper set with `awww img` outside Kanade was never noticed. Zen read a light/dark preference from `user.js` only at launch. qt6ct's dark-only styles hid the palette, and kvantum ignores it.

## Decision

- The `wallpaper` Module watches awww's cache (`$XDG_CACHE_HOME/awww/<version>/<output>`, written on each `awww img`), and after 150 ms of quiet reruns Matugen for the shown wallpaper, as a wallpaper Kanade sets does. A newer Kanade set wins over an older look.
- Each Matugen run also yields the full role scheme for both modes. With `appearance.sync` on, the appearance worker writes Kanade's own targets from it, no Matugen config needed:
  - GTK3 and GTK4: `gtk-{3,4}.0/kanade.css`, imported by one marked line in `gtk.css`. A `gtk.css` symlink (as to a theme's file) becomes a file importing its old target; it is linked again on disable if unchanged.
  - Qt: `qt{5,6}ct/colors/kanade.conf` as the palette. While it is used, a kvantum style becomes Darkly on Qt6 when installed, else Fusion, since both draw the palette; the old style returns on disable.
  - KDE apps (Dolphin, Kate): outside Plasma, KDE Frameworks' `KColorSchemeManager` puts Breeze Light or Dark over the qt*ct palette unless `[UiSettings] ColorScheme` names a scheme. Kanade writes `color-schemes/Kanade.colors` and sets that key in `kdeglobals`, which every KDE app's config falls back to; the old value returns on disable if still Kanade's.
  - niri: `niri/kanade.kdl` with only the focus ring, border, tab indicator, insert hint and recent-windows colors, included by a marked last line of `config.kdl` (`include optional=true`, niri 26.04), so it wins over the user's colors and a missing file is no error. Borders stay on or off as the user set. niri reloads it live. With no `config.kdl` niri runs its defaults, so nothing is written.
  - kitty: `kitty/kanade.conf`, included by the last line of `kitty.conf` after any theme, then SIGUSR1 to each kitty, which reloads its config. A kitty younger than 2 s is skipped: before it handles the signal, SIGUSR1 ends it, and it reads the new file as it starts. Its 6 ANSI hues are Matugen custom colors (`kanade_red`, ...) Kanade adds to each run, harmonized toward the wallpaper and toned for each mode; black and white come from the surface roles.
  - Firefox and Zen: Kanade is Pywalfox's native host. It takes over the host manifest (saving a foreign one to restore) and serves the 16 colors and the tone from a state file, pushed to the extension each time it changes, so the browser follows live with no restart.
- Zen's light/dark comes from the desktop preference through the portal; `ui.systemUsesDarkTheme` is no longer managed, and a value Kanade set is restored. `browser.theme.toolbar-theme` stays managed.
- Generated files carry a Kanade header; a same-path file without it is left alone. The last scheme is kept in sync state, so a restart rewrites the same colors.

## Consequences

- An external wallpaper change reaches the shell, the user's Matugen templates and these targets within about a second.
- The browser changes live once the Pywalfox extension is installed and has fetched colors once. A Zen profile that ran with the old preference keeps it in memory until one restart.
- niri and kitty change live. GTK, Qt and KDE apps read the new colors at start. qt6ct reloads its config live but Qt 6 drops a palette equal to its current one, so a running Qt app keeps its colors; GTK does not reload user CSS.
- The custom colors also reach the user's own Matugen templates, under names prefixed `kanade_`.
- A config.kdl or kitty.conf that is a symlink, as a dotfile manager makes, is left alone, so it keeps its colors.
- A scheme chosen in a KDE app's own Window Color Scheme menu is kept in its config and wins over Kanade's.
- Qt apps still need `QT_QPA_PLATFORMTHEME=qt5ct`/`qt6ct`. Kanade does not change the session environment.
- Another Pywalfox host (Python `pywalfox`) is displaced while sync is on and put back only when sync is turned off; removing Kanade with sync on leaves the browser a manifest naming a missing host, until `pywalfox install` runs again. The manifest names the running binary, so a moved binary is named on its next apply.
- While a Zen profile still holds the retired preference and Zen runs, restoring it is retried every 2 s, ending once Zen exits.
