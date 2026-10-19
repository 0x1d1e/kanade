# 122. The installer ships a binary

Status: accepted.

## Context

A piped `curl | sh` used to clone `release/0.1` and `cargo install` it. That compiles on the user's machine. The binary published for install is built on Arch and dynamically linked to that system's PulseAudio (`libpulsecommon-17.0.so` and the rest of that `ldd` closure). It is not static and not portable.

## Decision

- Piped curl reads the script from `main`. With no `--ref`, it downloads the Arch `x86_64` `kanade` published at `install-0.1` (built from `release/0.1`), verifies sha256, and installs it to `~/.cargo/bin/kanade`. It falls back to cloning `release/0.1` and `cargo install --locked --path .` only when that asset is absent (HTTP 404). Any other download or checksum failure stops. A short file or a mismatch is not an install, and does not fall back.
- `--ref REF` compiles that branch or tag. It does not download `install-0.1`. A checkout compiles itself.
- `uname -m` must be `x86_64` on the download path. Anything else exits 1. Arch is still required (`pacman` on PATH).
- Kanade's packages are installed by `paru` if it is on PATH, otherwise by `yay` if that is already on PATH. The helper runs as the user, never under `sudo`, with `--needed --noconfirm`. If neither helper is on PATH, the script runs `sudo pacman -S --needed paru` and then uses that paru. If pacman cannot install paru, it stops. It does not build paru with makepkg. It does not publish an AUR package for Kanade, and it does not add AUR dependencies beyond the packages the installer already took from the repos.
- The TUI is `kanade-install`, a separate binary. It only calls this script. Ratatui and crossterm stay in that crate. Its plan choices are shell-only, niri, sddm and optional packages. While sddm is on, one more row chooses the login screen's display server: keep, or niri.
- The standard desktop package set includes `awww` whenever that install selects Niri (including explicit `--niri` and `--sddm` with `--shell-only`), because Kanade's default Wallpapers Surface and random wallpaper seed depend on it. Pure shell-only installs without Niri or SDDM omit it. `matugen` remains in the optional package set.
- `--sddm` is optional. The script, not the shell, installs `sddm`, `qt6-declarative` and `qt6-5compat` with the helper, then does the root setup from ADR 0120: the overlay, `kanade sddm setup`, the user's `kanade sddm sync`, and `zz-kanade.conf` only after the theme is installed. A separate drop-in, `zy-kanade-greeter.conf`, can set the login screen's display server. The script enables `sddm.service` for the next boot when it is not already enabled, and does not start it. The shell still never runs sudo. ADR 0120 is unchanged: the executable theme stays root-owned, the picture stays in the user-owned data directory, and `zz-kanade.conf` sets only `[Theme]`.
- Not static. Not an AUR package. Not a distro installer.

## Consequences

- The downloaded binary runs on Arch x86_64 with the libraries it was linked against. Another distro, or an Arch where those sonames differ, is unsupported for that binary. Compiling is the path when the asset is missing, or when `--ref` or a checkout is used, not when the checksum fails.
- `kanade`, `kanade.sha256`, `kanade-install`, `kanade-install.sha256`, and `kanade.rev` sit on the `install-0.1` prerelease. They are replaced in place, not `v0.1.0`. They are built locally from `release/0.1` on Arch x86_64 and uploaded by hand. GitHub Actions does not build or upload them.

Files: `scripts/install`, `crates/install`.
