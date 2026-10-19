# 120. SDDM loads a root-owned theme and separate user data

Status: accepted. Narrows the non-goal "greeter/login manager" in `docs/design.md`: Kanade still ships no greeter and manages no login; it ships a theme for the one SDDM already runs. Supersedes the earlier user-owned executable overlay decision below.

## Context

The lock screen (ADR 0025) has a login-time twin: SDDM's greeter should look the same. SDDM draws a QML theme as the `sddm` user, before any login, from `ThemeDir` (default `/usr/share/sddm/themes`), the theme named by `Current`. A desktop process must not be able to replace QML, JavaScript, assets, or configuration that SDDM executes before login. The picture behind the clock should still follow the user's wallpaper, which is unreadable by `sddm` in the user's home.

## Decision

- **Root-owned executable theme.** `/usr/local/share/kanade-sddm/themes/kanade/` contains the built-in theme. The installer ensures `/usr/local/share/kanade-sddm/` and its `themes/` child are plain directories, then resets ownership to root and mode to 0755 on those two managed directories before running `kanade sddm setup` as root. This repairs stale user-owned directory metadata without recursively changing theme contents or touching unrelated themes. The separately user-owned `/usr/local/share/kanade-sddm-data/` is not chowned. Setup writes a complete staged directory and exchanges it with the active theme using `renameat2` with `RENAME_EXCHANGE`, falling back to two renames through `.old`; `recover()` restores the old directory after an interrupted fallback. Files and directories are readable by SDDM and not writable by the desktop user. Setup retains `QtVersion` and existing `theme.conf.user` settings.
- **Mutable data outside executable trees.** `/usr/local/share/kanade-sddm-data/backdrop.png` is a fixed data file outside every SDDM-loaded theme directory. Root creates this directory owned by the desktop user. `kanade sddm sync` and wallpaper following write only this image; they never write QML, JavaScript, assets, `theme.conf`, or `theme.conf.user`. The built-in theme validates and loads only this fixed absolute image path as external data.
- **One root conf, named to sort last.** `/etc/sddm.conf.d/zz-kanade.conf` sets `[Theme] ThemeDir` and `Current`. Kanade prints the command and checks, in the order sddm.conf(5) reads, that nothing read later overrides it, naming the file if so. It never edits another config.
- **The order is part of the safety.** Setup installs the theme before the root command points SDDM at it. The config command is guarded by `test -f <themes>/kanade/metadata.desktop`. `kanade sddm status` and Settings show the required actions in order.
- **Third-party themes.** Existing classification and inspection remain available. User-owned links for third-party themes are not enabled through the secure built-in setup path because SDDM would execute code writable by the desktop user.
- **The picture is a derived file.** The wallpaper, blurred and dimmed as the lock screen's backdrop is, is re-encoded without metadata and written atomically as 0644. `sddm.sync`, off by default, updates it on wallpaper changes, on a worker thread, coalesced, without polling, under a flock in the state directory. It rechecks `sddm.sync` and the background under the lock before writing; `disable` turns sync off before removing the picture. The wallpaper is only read, so a link to it is followed; writes refuse links and hard-linked targets. The picture is readable by `sddm` and anyone who can read the data directory, and shows the user's desktop.
- **Surfaces.** `kanade sddm status|setup|sync|enable|disable|theme list|info|use` works without a shell; `setup` is explicitly run as root. `kanade doctor` checks executable ownership and modes, data access, which config wins, and a Qt greeter on `PATH`. Settings' Login screen page shows status read-only and the root commands to copy. Kanade never invokes a root command.

## Trade-offs

The built-in executable theme cannot be refreshed by the desktop user; updating its code requires running `kanade sddm setup` as root. The picture remains user-writable so wallpaper following needs no privileged helper. Anyone who can read the data directory can read that picture. Third-party themes do not receive the built-in theme's root-owned protection.

## Alternatives

**A wholly root-owned tree.** This would prevent Kanade from refreshing the wallpaper without a privileged helper, so mutable picture data is kept separately.

**A privileged helper or polkit action.** Runs as root on Kanade's behalf; a standing root surface for a cosmetic feature. Refused.

**Writing under the user's home or `/usr/share`.** `sddm` cannot read a private home; `/usr/share` is the package manager's.

**Editing `/etc/sddm.conf`.** Kanade would edit a file the user or a package owns; a drop-in is removable by `rm`.
