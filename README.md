<p align="center">
  <img src="docs/assets/kanade-mascot.png" width="320" alt="Kanade">
</p>

<h1 align="center">Kanade</h1>

<p align="center">
  One island that changes shape for what is happening: a desktop shell for niri, in Rust.
</p>

<p align="center">
  <a href="docs/wiki/Installation.md">Install</a> | <a href="docs/wiki/CLI.md">CLI</a> | <a href="docs/wiki/Configuration.md">Configuration</a> | <a href="docs/wiki/Keyboard.md">Keyboard</a> | <a href="docs/wiki/Limitations.md">Limitations</a> | <a href="docs/design.md">Design</a>
</p>

> [!WARNING]
> Kanade is in a very immature phase. Expect bugs, missing pieces and breaking changes to config, CLI and behavior until 1.0. See [Limitations](docs/wiki/Limitations.md) for known gaps.

<!-- TODO screenshots: a strip of the Island's forms (Rest, a Compact track, a Split with a Satellite, an Expanded Surface) and the Dock; see the report for the shots wanted. -->

## The idea

There is no bar. Each monitor has one Island, by default at the top center. At Rest it shows the clock. When something happens, such as a playing track, a volume change, a notification, a low battery or a timer, it takes the Island over and the Island morphs to fit it, then settles back. Hover to peek; click to open a Surface, a full interface for that thing (thirteen of them, from Media to the Launcher), all reachable by keyboard. Things that last, like a timer or a script's job, stay beside it as Satellites.

Around the Island, Kanade is the session's notification daemon, a Dock and a lock screen. It is written in Rust on its own Wayland and GPU runtime, and at Rest it draws one frame a minute, for the clock.

## Features

**The Island**
- Activities take it over for as long as they last: media, volume and brightness, Caps and Num Lock, airplane mode, workspace changes, battery, timers, and what your own scripts post with `kanade activity` (a Claude Code hook recipe is in [Agent hooks](docs/wiki/Agent-hooks.md)).
- Thirteen Surfaces: Media, Notifications, Controls, Launcher, Tray, Clipboard, Calendar, Weather, Wallpapers, Keys, Session, Timer and Capture.
- Tray items appear beside the clock on hover. Left click activates, middle is the secondary action, the wheel scrolls, right opens the menu.
- Privacy dots show a microphone, camera or screen cast in use. Controls names the apps and ends a share with Stop sharing.

**The session**
- Notifications: Kanade is the daemon. Banners come out of the Island, with Do Not Disturb, grouping and live progress.
- Dock on each monitor: left click launches or focuses, middle click starts a new instance, right click opens the menu.
- Lock screen (`kanade lock`), locking before sleep, and idle stages with the `idle` Module (off by default). A matching SDDM login screen theme, written by `kanade sddm` ([SDDM](docs/wiki/SDDM.md)).
- Focus quiets the session in one press; Caffeine and Night light are switches.

**Capture and time**
- Screenshots, a screen color picker, and recording of an output, an area or a window, with sound if you ask and a pause.
- Calendar from iCalendar files, Google Calendar and read-only subscriptions, with a Join action for meeting links; weather from Open-Meteo; a Timer with pomodoro.

**Looks and control**
- Four finishes: tinted solid, paper, liquid glass, and blur (needs a niri layer rule, see [Installation](docs/wiki/Installation.md)).
- With Matugen, shell colors follow the wallpaper awww shows, and `appearance.sync` carries them to GTK, Qt, KDE apps, niri's borders, kitty and Firefox or Zen.
- A Settings window with search, Inspect and Undo; changes apply live unless marked Restart. Every feature but the Island is a Module you can turn off, and then it starts nothing.
- Scriptable: a `kanade` CLI, `kanade ui` to drive the shell by control name, and `kanade doctor` to check your setup.

## Scope

Kanade is not a bar, wallpaper renderer or system settings app, and it shows no permanent Wi-Fi, CPU or RAM indicators. It follows niri's focused output and workspaces; Hyprland and Sway are not supported.

## Quick start

```sh
curl -fsSL https://raw.githubusercontent.com/0x1d1e/kanade/main/scripts/install | sh
```

Then `niri-session` from the TTY. It installs everything a fresh TTY install lacks with pacman (niri, a Vulkan driver, audio, fonts), clones and builds Kanade, sets up the systemd unit and icon, and adds `dist/niri/config.kdl` as niri's config only when you have none. Options go after `sh -s --`: `--shell-only` for an existing niri desktop, `--optional`, `--uninstall`. Read the [script](scripts/install) first if you like; it uses sudo only for pacman and, with `--optional`, enabling services. Or by hand:

```sh
cargo install --locked --path .
mkdir -p ~/.config/systemd/user
cp dist/kanade.service ~/.config/systemd/user/
systemctl --user enable --now kanade.service
```

Kanade must be the only notification daemon. The systemd unit and recovery are in [Installation](docs/wiki/Installation.md).

## Dependencies

On Arch Linux and derivatives, all from the official repositories unless marked AUR.

Required, to build and run:

```sh
sudo pacman -S --needed niri rust base-devel pkgconf wayland libxkbcommon fontconfig freetype2 \
  expat vulkan-icd-loader libpulse pam
```

Also a Vulkan driver for the GPU (`vulkan-intel`, `vulkan-radeon` or `nvidia-utils`). niri must run as a systemd session (`niri-session` or a display manager).

Optional, each for the feature named; Kanade runs without them:

| Feature | Packages |
|---|---|
| Audio, privacy dots (`pactl`, `pw-dump`), device events (`udevadm`, `dbus-monitor`), `setpriv` | `libpulse pipewire systemd dbus util-linux` |
| Clipboard history (needs wl-clipboard 2.3 or later) | `wl-clipboard` |
| Screen recording, encoded on the GPU: VA-API (`intel-media-driver` for Intel, `mesa` for AMD) or NVENC (`nvidia-utils`), else on the CPU with libx264 (`ffmpeg`); `slurp` draws an area, `ffmpeg` allows a pause and records a window with `gst-launch-1.0` (`gst-plugin-pipewire`, `gst-plugins-bad`; `gst-plugins-ugly` for the CPU encoder, `gst-plugin-va` for VA-API, `gst-plugins-bad` and `nvidia-utils` for NVENC; VA-API (`gst-plugin-va`) or `gst-plugins-base` with EGL downloads the frames for both) | `wf-recorder` (`slurp`, `ffmpeg`, `gstreamer`) |
| Wallpaper | `awww` |
| Wallpaper-derived colors | `matugen` |
| `appearance.sync` | `qt5ct qt6ct glib2 dconf`; AUR: Darkly (Qt6 style), Pywalfox and its browser extension |
| Night light | `wlsunset` |
| Caffeine, lock before sleep, idle inhibitors | `systemd util-linux` |
| Google Calendar | `gnome-keyring` (or KWallet, KeePassXC) and `xdg-utils` |
| Terminal apps in the Launcher | `xdg-terminal-exec` |

Per-feature detail is in [Installation](docs/wiki/Installation.md).

## Documentation

- [Installation](docs/wiki/Installation.md): requirements, building, running as a systemd user unit.
- [CLI](docs/wiki/CLI.md): every `kanade <verb>`.
- [Configuration](docs/wiki/Configuration.md): config layers and every key.
- [Agent hooks](docs/wiki/Agent-hooks.md): a Claude Code session as a live Activity.
- [Dictation](docs/wiki/Dictation.md): speech to text with the Gemini API, and what it sends to Google.
- [Google Calendar](docs/wiki/Google-Calendar.md): signing in with your own OAuth client.
- [SDDM](docs/wiki/SDDM.md): the login screen theme and `kanade sddm`.
- [Keyboard](docs/wiki/Keyboard.md): niri keybinds and keys inside each Surface.
- [Limitations](docs/wiki/Limitations.md): known gaps.
- [Design](docs/design.md) and [architecture decisions](docs/adr/): the target design and why it is built this way.
- [Domain terms](CONTEXT.md): the names used here, such as Island, Surface and Activity.
- [Branding](docs/branding.md): the mascot and the mark.

## Acknowledgements

- [Amane](https://github.com/MystiaFin/amane) by MystiaFin: a Rust Wayland shell runtime.
- [iNiR](https://github.com/snowarch/iNiR): an island on niri, one body in many contexts, satellites.

## License

MIT, see [LICENSE](LICENSE).
