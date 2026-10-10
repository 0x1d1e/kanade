# Installation

## Install

Kanade needs:

- niri, which it talks to over `$NIRI_SOCKET`.
- A Rust toolchain and these system libraries with their headers: a C compiler, pkg-config, wayland, libxkbcommon, fontconfig, freetype, expat, vulkan-loader, libpulseaudio and linux-pam.
- Optional, see [Limitations](Limitations.md): `pactl`, `udevadm`, `dbus-monitor`, `setpriv` (util-linux) and `pw-dump` (pipewire).
- For the clipboard history: `wl-paste` and `wl-copy` from wl-clipboard 2.3 or later, the first that tells a password manager's copy; with an older one Kanade keeps no history.
- For screen recording: `wf-recorder`, and a VA-API driver for the GPU.
- For the wallpaper: [awww](https://codeberg.org/LGFae/awww) (`awww` and `awww-daemon`), which draws it; Kanade picks the image and runs `awww-daemon` unless one runs already, only through `setpriv` (util-linux), so it never outlives Kanade.
- For caffeine and locking before sleep: `systemd-inhibit` (systemd) and `setpriv` (util-linux), which ties the inhibitor to Kanade so it never outlives it.
- For Google Calendar: a Secret Service, like GNOME Keyring, KWallet or KeePassXC, and `xdg-open`. See [Google Calendar](Google-Calendar.md).

```sh
cargo install --locked --path .
```

This puts `kanade` in `~/.cargo/bin`.

Kanade must be the only notification daemon. Stop mako, dunst or any other one and keep it from starting again, for example `systemctl --user disable --now mako` and removing it from niri's `spawn-at-startup`. Otherwise the Notifications Surface says which daemon has the bus name, and no notification reaches the island until it is stopped and Kanade restarted.

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

Starting early also keeps D-Bus from activating another notification daemon for the first notification of the session.

A shell that crashes at every start stops restarting after 5 starts in 60 s, and a locked session stays on niri's red screen. From another TTY (`Ctrl+Alt+F3`), fix the cause, then restart the unit, which locks again and keeps the session:

```sh
systemctl --user reset-failed kanade.service
systemctl --user restart kanade.service
```

Only if that fails, end the graphical session with `loginctl terminate-session <id>` (`loginctl list-sessions` lists them). This closes every app in it, and unsaved work is lost.

From a checkout, `cargo run` runs it and `scripts/dev` rebuilds and restarts it on every save.

