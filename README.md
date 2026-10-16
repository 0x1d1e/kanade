<p align="center">
  <img src="docs/assets/kanade-mascot.png" width="320" alt="Kanade">
</p>

<h1 align="center">Kanade</h1>

<p align="center">
  An adaptive activity island and desktop shell for niri.
</p>

<p align="center">
  <a href="docs/wiki/Installation.md">Install</a> | <a href="docs/wiki/CLI.md">CLI</a> | <a href="docs/wiki/Configuration.md">Configuration</a> | <a href="docs/wiki/Keyboard.md">Keyboard</a> | <a href="docs/wiki/Limitations.md">Limitations</a> | <a href="docs/design.md">Design</a>
</p>

> [!WARNING]
> Kanade is in a very immature phase. Expect bugs, missing pieces and breaking changes to config, CLI and behavior until 1.0. See [Limitations](docs/wiki/Limitations.md) for known gaps.

## About

Kanade is a desktop shell for [niri](https://github.com/niri-wm/niri), written in Rust on its own Wayland and GPU runtime. Its center is the Island: one pill at the top center of each monitor that changes shape for what is happening. At rest it shows the clock. Activities such as the playing track, volume, brightness, Caps Lock, Num Lock, airplane mode, workspace changes, notifications, battery and a timer take it over for as long as they last.

It is also the session's notification daemon, a Dock, and a lock screen.

## Features

- **Island**: hovering it while it shows something peeks; a click expands it into one of thirteen Surfaces: Media, Notifications, Controls, Launcher, Tray, Clipboard, Calendar, Weather, Wallpapers, Keys, Session, Timer and Capture.
- **Tray**: hovering the Island at rest shows the apps' tray items beside the clock. Left click activates one, middle click is its secondary action, the wheel scrolls it, and right click opens its menu.
- **Notifications**: Kanade is the notification daemon. Banners come out of the Island, with Do Not Disturb; one app's notifications group, and a progress hint shows as a live Activity.
- **Privacy indicators**: dots at the Island's trailing end, or its top corner in a Surface, show a microphone, camera or screen capture in use, and the Controls Surface names the apps and ends a screen share with Stop sharing. A fullscreen window covers them, and `privacy.indicators` turns them off.
- **Dock**: on each monitor, at the edge and side set in `dock.edge` and `dock.align`, it launches and focuses apps.
- **Finishes**: tinted solid, paper, liquid glass, and blur, which blurs the wallpaper and the windows behind the Island, Dock and Banners (needs a niri layer rule, see [Installation](docs/wiki/Installation.md)).
- **Appearance**: with Matugen installed, Kanade derives light and dark shell colors from the wallpaper awww shows, however it was set, then renders configured app targets for the selected tone; `appearance.sync` carries them to GTK, Qt, KDE apps, niri's window borders, kitty, and Firefox or Zen live through Pywalfox.
- **Lock screen**: `kanade lock` shows the date, the time, who is signed in and a password field over the wallpaper, and no notification. The session also locks before the machine sleeps, and, with the `idle` Module on, after a dim grace when idle.
- **Capture**: screenshots and a screen color picker through niri and screen recording of the output, an area or a window, with sound if you ask and a pause, through `wf-recorder`, a saved recording shown on the Island, a saved screenshot by niri's notification.
- **Calendar and weather**: local iCalendar files and events you add, Google Calendar, read-only `https` calendar subscriptions, a Join action for a meeting link, and the forecast from Open-Meteo.
- **Live Activities**: scripts and agents (a Claude Code hook recipe in [Agent hooks](docs/wiki/Agent-hooks.md)) show what they do on the Island with `kanade activity`; Focus quiets the session in one press.
- **Settings window**: task-based pages with search, Inspect and Undo; changes apply live unless marked Restart.
- **Idle cost**: at rest the Island draws one frame a minute, for the clock, and nothing else.

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
| Screen recording (plus a VA-API driver: `intel-media-driver` for Intel, `mesa` for AMD); `slurp` draws an area, `ffmpeg` allows a pause and records a window with `gst-launch-1.0` (`gst-plugin-pipewire`, `gst-plugins-bad`) | `wf-recorder` (`slurp`, `ffmpeg`, `gstreamer`) |
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
