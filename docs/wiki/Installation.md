# Installation

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/0x1d1e/kanade/main/scripts/install | sh
```

The script does all of this on Arch Linux and derivatives, from a fresh TTY install: niri with its portal and polkit agent, the GPU's Vulkan driver, audio, fonts, kitty and qt6ct, then it clones the `release/0.1` branch to `~/.local/share/kanade/src`, builds it, and sets up the unit, the icon and a default niri config (`dist/niri/config.kdl`) only when `~/.config/niri/config.kdl` is missing. The curl URL only fetches the script from `main`. Afterwards run `niri-session` from the TTY. Run it again to update. From a checkout, `scripts/install` builds that checkout instead. Options go after `sh -s --`, as in `curl ... | sh -s -- --shell-only` or `curl ... | sh -s -- --ref v0.1.0` (an override, not the default):

- `--shell-only` installs only the build dependencies and Kanade, for a niri desktop you already have.
- `--ref REF` overrides the branch or tag, including `--ref main`. Refused from a checkout, which builds that tree.
- `--optional` adds the optional packages and enables bluetooth, upower and power-profiles-daemon; NetworkManager is installed but left for you to enable.
- `--uninstall` removes Kanade's binary, shell completions, unit, icon and the build checkout, after asking if niri's config still runs `kanade` (its binds, locking included, would stop working). Your configs and the packages stay.

The binary goes to `~/.cargo/bin`, where the unit expects it, and the shell completions to `~/.local/share/bash-completion/completions/kanade`, `~/.config/fish/completions/kanade.fish` and `~/.local/share/zsh/site-functions/_kanade` (under `XDG_DATA_HOME` and `XDG_CONFIG_HOME` when set); zsh needs that directory in its `fpath` before `compinit`. A run replaces the unit and keeps the old one as `kanade.service.bak.DATE`; customize it with `systemctl --user edit kanade.service`, whose drop-in an update leaves alone. A failed build leaves the installed binary as it was. The steps below are what it runs.

Kanade needs:

- niri, which it talks to over `$NIRI_SOCKET`.
- A Rust toolchain and these system libraries with their headers: a C compiler, pkg-config, wayland, libxkbcommon, fontconfig, freetype, expat, vulkan-loader, libpulseaudio and linux-pam.
- Optional, see [Limitations](Limitations.md): `pactl`, `udevadm`, `dbus-monitor`, `setpriv` (util-linux) and `pw-dump` (pipewire).
- For the clipboard history: `wl-paste` and `wl-copy` from wl-clipboard 2.3 or later, the first that tells a password manager's copy; with an older one Kanade keeps no history.
- For screen recording: `wf-recorder`, and an encoder that works: a VA-API driver for an Intel or AMD GPU (`intel-media-driver`, `mesa`), NVENC on an NVIDIA GPU (`nvidia-utils`), or neither, when it encodes on the CPU with libx264 (`ffmpeg` built with it, which Arch's is). `recording.encoder` is `auto`, which takes the first that works in that order and never wakes an NVIDIA GPU while VA-API works; `kanade doctor` says which it chose, on which render node (`recording.device`), why, and what is missing. `recording.quality` sets the size against the picture. Optional: `slurp` to draw the area to record, and `ffmpeg` to pause a recording and to test the encoders. For recording a window: `gst-launch-1.0` with the PipeWire plugin and the plugin of the encoder (`gst-plugin-pipewire`, `gst-plugins-bad` and `gst-plugin-va` for VA-API, `gst-plugins-ugly` for libx264) and `ffmpeg`. NVENC and libx264 take niri's DMA-BUF cast through the VA-API post-processor (a working VA-API driver, `gst-plugin-va`; it tries the Intel and AMD nodes, the boot GPU first, and a node that fails on the live cast gives way to the OpenGL path), else through GStreamer's OpenGL plugin (`gst-plugins-base`, with EGL), which cannot import Intel's compressed tiled buffers and is not verified on NVIDIA-only machines.
- For the wallpaper: [awww](https://codeberg.org/LGFae/awww) (`awww` and `awww-daemon`), which draws it; Kanade picks the image and runs `awww-daemon` unless one runs already, only through `setpriv` (util-linux), so it never outlives Kanade.
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

Kanade runs as a systemd user unit tied to niri's graphical session, so it starts with niri, stops with it, and restarts after a crash. A crash while the session is locked leaves niri locked on its red screen until the restart locks it again. This needs niri started as a systemd session, as `niri-session` or a display manager does, see [niri's systemd integration](https://github.com/YaLTeR/niri/wiki/Example-systemd-Setup).

```sh
mkdir -p ~/.config/systemd/user
cp dist/kanade.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now kanade.service
```

The unit runs `~/.cargo/bin/kanade`; edit its `ExecStart` for another install. Remove any `spawn-at-startup "kanade"` from niri's config: a second shell stops at start. `kanade doctor` warns about a unit that is not enabled or does not run the shell. The unit restarts the shell however it ends; `systemctl --user stop kanade.service` stops it.

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

From a checkout, `cargo run` runs it and `scripts/dev` rebuilds and restarts it on every save.

