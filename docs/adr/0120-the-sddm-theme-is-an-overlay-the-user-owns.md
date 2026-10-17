# 120. The SDDM theme is an overlay the user owns

Status: accepted. Narrows the non-goal "greeter/login manager" in `docs/design.md`: Kanade still ships no greeter and manages no login; it ships a theme for the one SDDM already runs. Supersedes no ADR.

## Context

The lock screen (ADR 0025) has a login-time twin: SDDM's greeter should look the same. SDDM draws a QML theme as the `sddm` user, before any login, from `ThemeDir` (default `/usr/share/sddm/themes`), the theme named by `Current`. Kanade runs as the user, never as root, and never runs `sudo` (AGENTS.md). The picture behind the clock should be the user's wallpaper, which lives in the user's home, unreadable by `sddm`.

## Decision

- **An overlay Kanade owns, outside `/usr/share`.** `/usr/local/share/kanade-sddm/` holds `themes/<theme>/` and `bg/backdrop.png`. Root makes it once (`install -d -o UID -g GID -m 755 ... prefix prefix/themes prefix/bg`, printed by `kanade sddm status`); afterwards the user writes it without root. Kanade's own theme (`dist/sddm/kanade`, embedded) is copied there; any other installed theme is a directory of links to its files with Kanade's `theme.conf.user`, built beside the old overlay and swapped in whole.
- **One root conf, named to sort last.** `/etc/sddm.conf.d/zz-kanade.conf` sets `[Theme] ThemeDir` and `Current`. Kanade prints the command and checks, in the order sddm.conf(5) reads, that nothing read later overrides it, naming the file if so. It never edits another config.
- **The order is part of the safety.** Pointing SDDM at an overlay without the theme can leave the login screen without one, so what is printed is in order: the overlay, then `kanade sddm sync`, then the conf step last, which is also guarded by `test -f <overlay>/themes/<theme>/metadata.desktop` and does nothing without it. The same holds in the Settings page's Copy strings.
- **A farmed theme can go stale.** Its links lead to the installed theme; after that package is upgraded or removed they may lead nowhere. `kanade sddm sync` after a theme package changes is the remedy; `kanade doctor`, Settings and a note on stderr at start say when SDDM reads the overlay and its theme is not current. The recovery from a broken login screen is `sudo rm /etc/sddm.conf.d/zz-kanade.conf` from a text console, then restarting sddm.
- **Theme classes decide the picture.** A theme that reads a `background` setting (or `config.background` in QML) takes the picture (class A); a fixed image (B), no setting (C) and a video (D) are linked and get none. Reads of a theme are bounded and what is shown of it has its control characters removed.
- **The picture is a derived file.** The wallpaper, blurred and dimmed as the lock screen's backdrop is, re-encoded (no metadata), written atomically as 0644. `sddm.sync`, off by default, writes it again when the wallpaper changes, on a worker thread, coalesced, with no polling, under a flock in the state directory. It rechecks `sddm.sync` and the background under the lock before writing, and `disable` turns the sync off before it removes the picture, so a write waiting on the lock cannot put a picture back. The wallpaper is only read, so a link to it is followed; every write refuses links. It is off by default because the file is readable by `sddm` and by anyone who can read the overlay, and shows the user's desktop.
- **Writes are refused through links.** A symbolic link, a non-file, or a file with another hard link in place of a target is refused; modes are set to 0755/0644 whatever the umask, since `sddm` is not the owner. The overlay directory itself must be a directory the user owns that no one else can write.
- **Surfaces.** `kanade sddm status|sync|enable|disable|theme list|info|use` works without a shell; `kanade doctor` checks the overlay, which file wins, the `sddm` user's read access by mode, and a Qt greeter on `PATH`; Settings' Login screen page shows the same status read-only with the root commands to copy. Neither runs a root command.

## Trade-offs

The overlay is the user's, so any process running as the user can change QML the greeter then loads as `sddm`, before login. For a one-user machine this adds little to what such a process already does (it could edit the shell or the user's autostart); on a shared machine or with untrusted code under the account it is real. Anyone who can read the overlay can read the picture.

## Alternatives

**A root-owned copy.** No new trust, but Kanade could not refresh the picture or the theme without root, and the picture would go stale; the wallpaper's following would need a privileged helper.

**A privileged helper or polkit action.** Runs as root on Kanade's behalf; a standing root surface for a cosmetic feature. Refused.

**Writing under the user's home or `/usr/share`.** `sddm` cannot read a private home; `/usr/share` is the package manager's.

**Editing `/etc/sddm.conf`.** Kanade would edit a file the user or a package owns; a drop-in is removable by `rm`.
