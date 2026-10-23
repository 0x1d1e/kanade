# Installation

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/0x1d1e/kanade/main/scripts/install | sh
```

The curl URL only fetches the script from `main`. On Arch Linux and derivatives, a piped run with a terminal and without `--yes` downloads `kanade-install` from the `install-0.1` release, checks its sha256, and execs that TUI. The TUI scans the machine, then four choices (shell-only, niri, sddm, optional packages), then runs this script with `--yes`. niri starts on, and off when the TUI is started with `--shell-only`. sddm starts off, and on with `--sddm`. Turning shell-only on turns niri and sddm off. Turning sddm on turns niri on. Turning niri off turns sddm off. Turning niri on leaves shell-only. When sddm is on, one more row toggles the login screen between keep and niri. keep writes nothing and deletes nothing. niri is the default on a hybrid NVIDIA machine (a `0x10de` device under `/sys/class/drm/card*/device` and another device); otherwise keep. The TUI does not write a niri config itself. `--yes`, `--uninstall`, or no terminal skips the TUI and the script runs directly. If the `kanade-install` asset is missing (HTTP 404), the script says so and continues without the TUI. A checksum mismatch stops.

With no `--ref`, the script downloads `kanade` and `kanade.sha256` from `https://github.com/0x1d1e/kanade/releases/download/install-0.1/`, checks them with `sha256sum -c`, and installs the binary to `~/.cargo/bin/kanade`. That binary is built on Arch from `release/0.1`. It is dynamically linked to Arch's PulseAudio (`libpulsecommon-17.0.so` and the rest of that `ldd` closure). It is not static and not portable. `uname -m` must be `x86_64`; anything else exits 1. A missing asset (HTTP 404) is the only fallback: the script says so, clones `release/0.1` to `~/.local/share/kanade/src`, and runs `cargo install --locked --path .`. Any other download failure, a short file, or a checksum mismatch stops and does not compile. `kanade.rev`, when the release has it, is printed and is not required. `--ref REF` compiles that branch or tag and does not download `install-0.1`. From a checkout, `scripts/install` builds that checkout.

Packages are not installed with `pacman -S`. The script uses `paru` if it is on PATH, otherwise `yay` if that is already on PATH, as the user, with `--needed --noconfirm`. If neither is on PATH, it runs `sudo pacman -S --needed paru` and then uses that paru. If pacman cannot install paru, it stops. It does not build paru with makepkg, and it does not publish an AUR package. The scan and the script both print `helper: paru`, `helper: yay`, or `helper: none` for what they found.

A fresh TTY install still adds niri with its portal and polkit agent, the GPU's Vulkan driver, audio, fonts, kitty, qt6ct and awww for Kanade's default random wallpaper, then the unit, the icon, and a default niri config (`dist/niri/config.kdl`) only when `~/.config/niri/config.kdl` is missing. A niri config of yours is never replaced. Afterwards run `niri-session` from the TTY. Run it again to update. Options go after `sh -s --`, as in `curl ... | sh -s -- --shell-only` or `curl ... | sh -s -- --yes` or `curl ... | sh -s -- --ref v0.1.0` (an override, not the default):

- `--shell-only` installs Kanade without the desktop packages, for a niri desktop you already have. It does not install a niri config unless you also choose Niri (`--niri`) or SDDM (`--sddm`); those install the default config only when yours is missing. The TUI can add the niri package on that run. Explicit Niri or SDDM installs also include awww; pure shell-only without either omits it.
- `--no-niri` skips the niri package and does not write a niri config, even on a desktop install. `--sddm` still installs the niri package.
- `--niri` installs the niri package without the rest of the desktop packages. With `--shell-only` it also installs the default niri config when yours is missing.
- `--sddm` installs `sddm`, `qt6-declarative` and `qt6-5compat` with the helper, and `niri` if that package is not already being installed. After `~/.cargo/bin/kanade` is in place, the script creates the root-owned overlay and the user-owned data directory, runs `kanade sddm setup` as root and `kanade sddm sync` as you, and writes `/etc/sddm.conf.d/zz-kanade.conf` only when `metadata.desktop` is there and the file differs. That file sets `[Theme]` only. It does not turn `sddm.sync` on. A login screen of niri writes `/etc/sddm.conf.d/zy-kanade-greeter.conf` and a root-owned `/usr/local/share/kanade-sddm/niri-greeter.kdl`, separate from the theme file. keep writes neither and deletes nothing. If `sddm.service` is not already enabled, the script removes a `display-manager.service` symlink that points elsewhere, disables an enabled `gdm`, `lightdm`, `lxdm`, `greetd` or `plasmalogin`, and runs `systemctl enable sddm` with no `--now`. It does not start SDDM. If `sddm` is already enabled, other display managers are left alone and the theme is still refreshed. A failed step stops, and SDDM is not pointed at a theme that was not installed.
- `--sddm-greeter keep|niri` chooses that login screen with `--sddm`. Without it, `--sddm` uses niri on a hybrid NVIDIA machine and keep otherwise.
- `--yes` skips the TUI.
- `--ref REF` compiles that branch or tag, including `--ref main`. Refused from a checkout, which builds that tree. It does not download `install-0.1`.
- `--optional` adds the optional packages and enables bluetooth, upower and power-profiles-daemon; NetworkManager is installed but left for you to enable.
- `--uninstall` removes Kanade's binary, shell completions, unit, icon and the build checkout, after asking if niri's config still runs `kanade` (its binds, locking included, would stop working). It keeps the `kanade.service.bak.DATE` copies an update left, so a hand-edited unit is not lost. It also removes the root-owned files `--sddm` wrote, with `sudo`, and only those: `/etc/sddm.conf.d/zz-kanade.conf`, `/etc/sddm.conf.d/zy-kanade-greeter.conf`, `/usr/local/share/kanade-sddm/niri-greeter.kdl`, `/usr/local/share/kanade-sddm/themes/kanade` and `/usr/local/share/kanade-sddm-data/backdrop.png` (a manual `kanade sddm setup` writes the same ones); the three directories are removed only when empty, so another theme's overlay stays. `/etc/sddm.conf` and every other drop-in are left alone, and so is SDDM itself (enabled or not as it was). Anything `sudo` could not remove is printed with the `sudo rm` that removes it. Your configs and the packages stay.

The binary goes to `~/.cargo/bin`, where the unit expects it, and the shell completions to `~/.local/share/bash-completion/completions/kanade`, `~/.config/fish/completions/kanade.fish` and `~/.local/share/zsh/site-functions/_kanade` (under `XDG_DATA_HOME` and `XDG_CONFIG_HOME` when set); zsh needs that directory in its `fpath` before `compinit`. A run replaces the unit and keeps the old one as `kanade.service.bak.DATE`; customize it with `systemctl --user edit kanade.service`, whose drop-in an update leaves alone, and `--uninstall` keeps those copies too. A failed checksum or a failed build leaves the installed binary as it was. The steps below are what a manual build runs.

Kanade needs:

- niri, which it talks to over `$NIRI_SOCKET`.
- A Rust toolchain and these system libraries with their headers: a C compiler, pkg-config, wayland, libxkbcommon, fontconfig, freetype, expat, vulkan-loader, libpulseaudio and linux-pam.
- Optional, see [Limitations](Limitations.md): `pactl`, `udevadm`, `dbus-monitor`, `setpriv` (util-linux) and `pw-dump` (pipewire).
- For the clipboard history: `wl-paste` and `wl-copy` from wl-clipboard 2.3 or later, the first that tells a password manager's copy; with an older one Kanade keeps no history.
- For screen recording: `wf-recorder`, and an encoder that works: a VA-API driver for an Intel or AMD GPU (`intel-media-driver`, `mesa`), NVENC on an NVIDIA GPU (`nvidia-utils`), or neither, when it encodes on the CPU with libx264 (`ffmpeg` built with it, which Arch's is). `recording.encoder` is `auto`, which takes the first that works in that order and never wakes an NVIDIA GPU while VA-API works; `kanade doctor` says which it chose, on which render node (`recording.device`), why, and what is missing. `recording.quality` sets the size against the picture. Optional: `slurp` to draw the area to record, and `ffmpeg` to pause a recording and to test the encoders. For recording a window: `gst-launch-1.0` with the PipeWire plugin and the plugin of the encoder (`gst-plugin-pipewire`, `gst-plugins-bad` and `gst-plugin-va` for VA-API, `gst-plugins-ugly` for libx264) and `ffmpeg`. NVENC and libx264 take niri's DMA-BUF cast through the VA-API post-processor (a working VA-API driver, `gst-plugin-va`; it tries the Intel and AMD nodes, the boot GPU first, and a node that fails on the live cast gives way to the OpenGL path), else through GStreamer's OpenGL plugin (`gst-plugins-base`, with EGL), which cannot import Intel's compressed tiled buffers and is not verified on NVIDIA-only machines.
- For the wallpaper: [awww](https://codeberg.org/LGFae/awww) (`awww` and `awww-daemon`), which draws it; standard desktop and explicit Niri/SDDM installs include it. Kanade picks the image and runs `awww-daemon` unless one runs already, only through `setpriv` (util-linux), so it never outlives Kanade. Without awww the Wallpapers Surface and random wallpaper seed are unavailable; Kanade starts normally.
- For wallpaper-derived shell colors and configured app themes: Matugen 4.x (optional; without it, Kanade uses built-in colors). With `appearance.sync`: qt5ct/qt6ct for Qt apps, Darkly for a Qt6 style that draws the palette, and the Pywalfox browser extension for Firefox and Zen (all optional).
- For caffeine, locking before sleep and the `idle` Module's inhibitors: `systemd-inhibit` (systemd) and `setpriv` (util-linux), which ties the inhibitor to Kanade so it never outlives it.
- For notification sounds (`notifications.sound`, off by default; its sound files and volume are keys beside it): `pw-play` (PipeWire) or `canberra-gtk-play`, and `setpriv` so a sound never outlives Kanade. `kanade doctor` says whether a player is found.
- For the night light: [wlsunset](https://git.sr.ht/~kennylevinsen/wlsunset) and `setpriv` (util-linux), so it never outlives Kanade.
- For Google Calendar: a Secret Service, like GNOME Keyring, KWallet or KeePassXC, and `xdg-open`. See [Google Calendar](Google-Calendar.md).

```sh
cargo install --locked --path .
```

This puts `kanade` in `~/.cargo/bin`. `kanade completions bash|fish|zsh` prints the completions for a shell, from the commands `kanade help` lists, to put where that shell reads them, like `kanade completions fish > ~/.config/fish/completions/kanade.fish`.

Kanade must be the only notification daemon. Stop mako, dunst or any other one and keep it from starting again, for example `systemctl --user disable --now mako` and removing it from niri's `spawn-at-startup`. Otherwise the Notifications Surface says which daemon has the bus name, and no notification reaches the island until it is stopped and Kanade restarted.

To keep the wallpaper in niri's overview, add a layer rule to your niri config, which Kanade changes only when you ask it to (`kanade niri-config`, [CLI](CLI.md)), and not for this rule. awww draws on a Background layer named `awww-daemon`, and niri shows such a layer in the overview and between workspaces only with `place-within-backdrop` (niri 25.05 or later); without it the overview shows niri's background color. The default config from the installer has a rule for every layer. `kanade doctor` warns while the `wallpaper` Module is on and no rule places `awww-daemon` there; it reads `$NIRI_CONFIG`, else `config.kdl` in niri's config directory, and the files it includes. A pattern it cannot read as plain text counts as matching, so it never warns wrongly.

```kdl
layer-rule {
    match namespace="^awww-daemon"
    place-within-backdrop true
}
```

The `blur` finish (`appearance.finish`) asks niri to blur what lies behind the Island, Dock and Banners, which needs niri 26.04 or later. niri blurs a layer's request as xray, the wallpaper only, so windows behind a pane would show through sharp; a layer rule in your niri config (Kanade changes that file only when you ask, with `kanade niri-config`) turns xray off for Kanade's layers. The default config from the installer has it. niri marks non-xray effects experimental and recomputes them whenever what lies behind changes, so they cost more GPU than xray. `kanade doctor` warns while the finish is `blur` and no rule sets `xray false` on `kanade`, `kanade-dock` and `kanade-banners`, and when the compositor has no `ext_background_effect_manager_v1`; it reads the config as it does for the wallpaper rule. niri's own `blur { off }` turns all blur off, this one included.

```kdl
layer-rule {
    match namespace="^kanade(-dock|-banners)?$"

    background-effect {
        xray false
    }
}
```

A Banner shows a notification's text, and a screencast (a browser share, OBS) shows the Banner too. To hide Banners from screencasts, add a layer rule for `kanade-banners`; niri draws them as a black rectangle there. It is off by default, because a viewer then never sees a Banner you wanted to show, and the installer's config has it commented out. Use `"screencast"`, not `"screen-capture"`, which would also blank Kanade's privacy indicators. Leave the `kanade` namespace alone: the Island draws the privacy dots, which must stay visible. `kanade doctor` says whether the rule is set while the `banners` Module runs, and never warns about it. See [ADR 0070](../adr/0070-banners-out-of-screencasts.md).

```kdl
layer-rule {
    match namespace="^kanade-banners$"

    block-out-from "screencast"
}
```

Only one Kanade runs per session. A second one stops at start with `failed to listen: kanade is already running`.

## Run

Kanade runs as a systemd user unit wanted by `niri.service`, so it starts with niri, stops with its graphical session, and restarts after a crash. niri's service binds to `graphical-session.target` and starts it, including when launched by `niri-session` or a display manager. A crash while the session is locked leaves niri locked on its red screen until the restart locks it again; see [niri's systemd integration](https://github.com/YaLTeR/niri/wiki/Example-systemd-Setup).

```sh
mkdir -p ~/.config/systemd/user
cp dist/kanade.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now kanade.service
```

The unit runs `~/.cargo/bin/kanade`; edit its `ExecStart` for another install. Remove any `spawn-at-startup "kanade"` from niri's config: a second shell stops at start. `kanade doctor` warns about a unit that is not enabled or does not run the shell. The unit restarts the shell however it ends; `systemctl --user stop kanade.service` stops it.

On a machine with two GPUs, Kanade draws on the one its window can present on, which is usually the integrated one, but the Vulkan loader still opens every driver in `/usr/share/vulkan/icd.d/`. On an Intel or AMD laptop with an NVIDIA dGPU that means about 50 MiB of the NVIDIA libraries mapped and resident for a GPU the shell never draws on. Naming the drawing GPU's driver in the unit keeps the others out:

```sh
systemctl --user edit kanade.service
```

```ini
[Service]
Environment=VK_DRIVER_FILES=/usr/share/vulkan/icd.d/intel_icd.json
```

Use `radeon_icd.json` for AMD, or leave it out on a machine whose only GPU is NVIDIA. The log line `kanade: drawing with <name>` names the GPU the shell picked, before and after. See `docs/memory.md` for what it saves.

Kanade's own notifications, like the timer's, show its icon from the icon theme:

```sh
install -Dm644 dist/kanade.png ~/.local/share/icons/hicolor/256x256/apps/kanade.png
```

Starting early also keeps D-Bus from activating another notification daemon for the first notification of the session.

A shell that crashes at every start stops restarting after 5 starts in 60 s, and a locked session stays on niri's red screen. From another TTY (`Ctrl+Alt+F3`), fix the cause, then restart the unit, which locks again and keeps the session:

```sh
systemctl --user reset-failed kanade.service
systemctl --user restart kanade.service
```

Only if that fails, end the graphical session with `loginctl terminate-session <id>` (`loginctl list-sessions` lists them). This closes every app in it, and unsaved work is lost.

`kanade update` replaces `~/.cargo/bin/kanade` with the published Arch x86_64 binary from the `install-0.1` release, the same asset the installer uses, after checking `kanade.sha256`. It does not need a shell, does not compile, and does not install packages or write a niri config. A missing file, a short file, a checksum mismatch or any other download failure stops and leaves the installed binary as it was. It then refreshes the user unit from that release, keeping the old one when it differs, and leaves systemd drop-ins alone. If `kanade.service` is running it restarts that unit only. It does not stop other `kanade` processes. Another architecture is refused.

From a checkout, `cargo run` runs it and `scripts/dev` rebuilds and restarts it on every save.

