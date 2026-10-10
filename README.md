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

- **Island**: hovering it while it shows something peeks; a click expands it into one of nine Surfaces: Media, Notifications, Controls, Launcher, Tray, Clipboard, Calendar, Weather and Session.
- **Tray**: hovering the Island at rest shows the apps' tray items beside the clock. Left click activates one, middle click is its secondary action, the wheel scrolls it, and right click opens its menu.
- **Notifications**: Kanade is the notification daemon. Banners come out of the Island, with Do Not Disturb.
- **Privacy indicators**: dots at the Island's trailing end show a microphone, camera or screen capture in use, and the Controls Surface names the apps. A fullscreen window covers them, and `privacy.indicators` turns them off.
- **Dock**: on each monitor, at the edge and side set in `dock.edge` and `dock.align`, it launches and focuses apps.
- **Lock screen**: `kanade lock` shows the date, the time, who is signed in and a password field over the wallpaper, and no notification. The session also locks before the machine sleeps.
- **Capture**: screenshots through niri and screen recording through `wf-recorder`, each saved one shown on the Island.
- **Calendar and weather**: local iCalendar files and Google Calendar, and the forecast from Open-Meteo.
- **Settings window**: every setting on a page per Module, applied as soon as it changes.
- **Idle cost**: at rest the Island draws one frame a minute, for the clock, and nothing else.

## Scope

Kanade is not a bar, wallpaper renderer or system settings app, and it shows no permanent Wi-Fi, CPU or RAM indicators. It follows niri's focused output and workspaces; Hyprland and Sway are not supported.

## Quick start

```sh
cargo install --locked --path .
mkdir -p ~/.config/systemd/user
cp dist/kanade.service ~/.config/systemd/user/
systemctl --user enable --now kanade.service
```

Kanade needs niri, a Rust toolchain and a few system libraries, and must be the only notification daemon. Requirements, the systemd unit and recovery are in [Installation](docs/wiki/Installation.md).

## Documentation

- [Installation](docs/wiki/Installation.md): requirements, building, running as a systemd user unit.
- [CLI](docs/wiki/CLI.md): every `kanade <verb>`.
- [Configuration](docs/wiki/Configuration.md): config layers and every key.
- [Google Calendar](docs/wiki/Google-Calendar.md): signing in with your own OAuth client.
- [Keyboard](docs/wiki/Keyboard.md): niri keybinds and keys inside each Surface.
- [Limitations](docs/wiki/Limitations.md): known gaps.
- [Design](docs/design.md) and [architecture decisions](docs/adr/): the target design and why it is built this way.
- [Domain terms](CONTEXT.md): the names used here, such as Island, Surface and Activity.
- [Branding](docs/branding.md): the mascot and the mark.

## Acknowledgements

- [Amane](https://github.com/MystiaFin/amane) by MystiaFin: Kanade's runtime is ported from it (MIT, see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) and [ADR 0028](docs/adr/0028-the-runtime-is-ported-from-amane.md)).
- [niri](https://github.com/niri-wm/niri): the compositor Kanade is made for, and its IPC.
- [Suzuha](https://github.com/MystiaFin/suzuha) by MystiaFin: panels drawn as one liquid shape on one surface.
- [iNiR](https://github.com/snowarch/iNiR): an island on niri, one body in many contexts, satellites.
- [Noctalia](https://github.com/noctalia-dev/noctalia): a native shell's config and architecture.
- [Wayle](https://github.com/wayle-rs/wayle): a Rust Wayland shell with its settings, OSD and device controls built in.
- [HyprGlass](https://github.com/hyprnux/hyprglass) and [Liquid Glass Studio](https://github.com/iyinchao/liquid-glass-studio): references for liquid glass: refraction at the rim, chromatic aberration, highlights.

Besides Amane, they are references: Kanade holds none of their code.

## License

MIT, see [LICENSE](LICENSE).
