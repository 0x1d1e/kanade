# 125. The installer wires the chosen Niri session; the first wallpaper is picked

Status: accepted.

## Context

The installer can select Niri with `--niri` or SDDM while skipping the rest of the desktop packages. The installed default Niri config is still needed for a fresh SDDM/Niri session, and an enabled user unit must start with the graphical session. Wallpaper seeding fills the user's folder, while startup otherwise preserves a wallpaper already present in awww's cache.

## Decision

- When the installer is asked for Niri or SDDM, it installs `dist/niri/config.kdl` only if the user's config is absent, even with `--shell-only`. `--no-niri` suppresses the direct Niri path; SDDM implies Niri, so SDDM still installs the config. Existing configs are kept.
- The Kanade user unit is enabled for `niri.service`, the systemd session entry point used by `niri-session` and display managers. niri's unit binds to and starts `graphical-session.target`. If `niri.service` is already active, the installer restarts Kanade immediately so the new unit takes effect in the current session. It does not start a display manager.
- SDDM setup runs `kanade sddm status` after writing Kanade's drop-in, reporting any effective theme override. The installer does not edit the later file that wins.
- After the wallpaper folder seeder finishes, Kanade asks awww for its current image. If there is none, it picks one image from the folder at random and sets it on all outputs. An existing awww image is left alone. If no image is available, the next start may retry.

## Consequences

The first login to a fresh Niri session has the default keybinds and layer rules, Kanade starts with `niri.service`, and a fresh wallpaper folder gets an initial desktop image. User-owned Niri and wallpaper choices remain authoritative.
