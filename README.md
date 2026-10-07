<p align="center">
  <img src="docs/assets/kanade-mascot.png" width="320" alt="Kanade">
</p>

<h1 align="center">Kanade</h1>

<p align="center">
  A top-center Dynamic Island for niri, built with <a href="https://github.com/MystiaFin/amane">Amane</a>.
</p>

https://github.com/user-attachments/assets/27e6837e-8cf1-4faa-8815-e5b14cdd631b

AI-generated concept, not a capture of the current build.

## What it is

One pill at the top center of each monitor that changes shape for what is happening: the clock at rest, the playing track, volume, brightness and workspace changes, notifications, battery, a timer. Hovering it while it shows something peeks, a click expands it into one of six Surfaces: Media, Notifications, Controls, Launcher, Tray and Clipboard. Hovering it at rest shows the apps' tray items beside the clock: left click activates one, middle click is its secondary action, the wheel scrolls it, and right click opens its menu. It is also the session's notification daemon. Apart from it, at the top right of each monitor, a privacy cluster shows a microphone, camera or screen capture in use, over fullscreen windows too; the Controls Surface names the apps. At rest it draws one frame a minute, for the clock, and nothing else.

It is not a bar, dock, wallpaper, lock screen or settings app, and it shows no permanent Wi-Fi, CPU or RAM indicators. It follows niri's focused output and workspaces; Hyprland and Sway are not supported, even though Amane runs there.

## Install

Kanade needs:

- niri, which it talks to over `$NIRI_SOCKET`.
- A Rust toolchain and the system libraries Amane builds against: a C compiler, pkg-config, wayland, libxkbcommon, fontconfig, freetype, expat, vulkan-loader, libpulseaudio and linux-pam, with their headers. Amane's `install.sh` installs them with pacman, apt or dnf, see [Amane's README](https://github.com/MystiaFin/amane#installation).
- Optional, see Limitations: `pactl`, `udevadm`, `dbus-monitor`, `setpriv` (util-linux) and `pw-dump` (pipewire).
- For screen recording: `wf-recorder`, and a VA-API driver for the GPU.

```sh
cargo install --locked --path .
```

This puts `kanade` in `~/.cargo/bin`.

Kanade must be the only notification daemon. Stop mako, dunst or any other one and keep it from starting again, for example `systemctl --user disable --now mako` and removing it from niri's `spawn-at-startup`. Otherwise the Notifications Surface says which daemon has the bus name, and no notification reaches the island until it is stopped and Kanade restarted.

Only one Amane shell runs per session. A second one, Kanade or any other, stops at start with `failed to listen: amane is already running`.

## Run

Start it with niri, in `~/.config/niri/config.kdl`, using the full path if `~/.cargo/bin` is not on niri's `PATH`:

```kdl
spawn-at-startup "kanade"
```

Starting early also keeps D-Bus from activating another notification daemon for the first notification of the session.

From a checkout, `cargo run` runs it and `scripts/dev` rebuilds and restarts it on every save.

## CLI

`kanade` with no verb runs the shell. `kanade <verb> [args]` asks the running one:

| Verb | Does |
|---|---|
| `launcher\|controls\|media\|notifications\|tray\|clipboard open` | opens that Surface on the focused output |
| `... close` | collapses it when it is open there |
| `... toggle` | opens it, or collapses it when it is already open there |
| `island collapse` | collapses the open island on the focused output |
| `notifications clear` | dismisses every notification |
| `notifications dnd on\|off\|toggle` | Do Not Disturb: notification Banners stop showing, Critical ones still do |
| `clipboard clear` | forgets the clipboard history; what is on the clipboard stays |
| `capture screenshot area\|window\|output` | asks niri for a screenshot of a picked area, the focused window or the focused output, saved under `~/Pictures/Screenshots`; prints its path |
| `capture record start\|stop\|status` | records the focused output with `wf-recorder`, encoded on the GPU, to `~/Videos/Screencasts`; `start` and `stop` print its path once it records or is saved, `status` says how it stands |
| `timer start <duration>` | starts the timer, like `90s`, `25m` or `1h30m`, up to 24h |
| `timer pause\|resume\|cancel` | pauses, resumes or cancels it |
| `config reload` | reads the config again now |
| `config validate` | says what a reload would find, without applying it |
| `status` | the config generation, the last reload error and the keys pending restart |
| `doctor` | read-only diagnostics: Amane and niri versions, Wayland protocols, buses and sockets, the config, and what each Module on runs worse without, like a missing `pw-dump` or another notification daemon; works without a shell, exits 1 on a failure |

`kanade help` prints every verb, including the `debug post` and `debug withdraw` verbs that post test Activities. A verb of a Module turned off in the config answers `module <name> is off`. With no shell running, a verb says so. Exit status: 0 done, 1 refused or no shell, 2 not a verb, 3 not known whether it was done: the shell, or niri for a screenshot, got the call but did not answer in time, so it may still be done; not worth repeating blindly.

## Configuration

Kanade reads its config at start, and again whenever one of its files changes, in layers, each over the ones before it:

1. the defaults below
2. every `*.toml` in `$XDG_CONFIG_HOME/kanade/` (else `~/.config/kanade/`), in alphabetical order, so `config.toml` can sit beside files like `10-theme.toml` that another tool manages. Hidden files are skipped.
3. `$XDG_STATE_HOME/kanade/settings.toml` (else `~/.local/state/kanade/settings.toml`), for the Settings app to come

Tables merge key by key, so a later file only changes the keys it sets; any other value, a list too, replaces the one below. Kanade never writes your files. Every key is optional; these are the defaults:

```toml
# the island snaps to its new shape and only fades its content, over 80 ms
reduced_motion = false

# the time an idle island shows: "24h" (14:05) or "12h" (2:05 PM)
clock = "24h"

[timings]            # milliseconds, 1-60000
hover = 120          # pointer resting on a Compact island before it peeks
expand = 180         # morph to a larger form
surface_change = 220 # one Surface replacing another, and a new track dissolving in
collapse = 180       # morph to a smaller form
grace = 250          # pointer out before a Peek or Surface collapses
osd = 1200           # the OSD, and a workspace switch on the island

[theme]
# palette = "~/Pictures/wallpaper.jpg"

[modules]            # every Module is on unless set to false here
# media = false
```

- `theme.palette`: the theme roles for what draws beside the island, like the Banners and the OSD, take their tone from this image and follow it when the file changes. The island itself stays black and white whatever the wallpaper, so amber (capture, low battery), red (critical) and green (mic/camera) keep their meaning.
- `KANADE_REDUCED_MOTION`: overrides `reduced_motion`. `1` turns it on, `0` off.
- `modules`: turns a feature off. An off Module starts no thread or helper process, posts nothing and answers its verbs with `module <name> is off`. Turning one on or off takes a restart. The Modules are `island` (the island itself, cannot be turned off), `workspace`, `privacy` (microphone, camera and screen cast; turning it off is warned about at every start), `battery`, `media`, `timer`, `audio` (volume and mute; with it off, nothing reads them), `brightness` (the backlight; likewise), `osd` (volume, brightness and microphone mute bottom centre on the focused output, over fullscreen windows too; shows what of `audio` and `brightness` is on), `notifications` (with it off, Kanade is not the notification daemon and Do Not Disturb is unavailable), `banners` (notification cards top-right on the focused output; requires `notifications`), `network`, `bluetooth` and `power` (the Controls tiles), `tray` (Kanade hosts the apps' tray items, and is their StatusNotifierWatcher unless another program already is; hovering the island at rest shows them, and the Tray Surface lists them with their menus), `clipboard` (a history of copied text and images through `wl-paste` and `wl-copy`, kept only in memory and never logged), `capture` (screenshots through niri; each one saved, by Kanade or by niri's own binds, shows on the island for 10 s, where Copy path needs `wl-copy` and Open needs `xdg-open`; a recording shows with Stop while it runs, and as a screenshot does once saved; recording needs `wf-recorder`), and the island's Surfaces: `controls` (with it off, a click at Rest does nothing), `launcher` (copying a sum's value or an emoji needs `wl-copy`), `notification-surface` (the notification list; requires `notifications`, and with it off a click on a notification opens Controls) and `clipboard-surface` (the clipboard history; requires `clipboard`). An off Surface never opens. A Module whose requirement is off turns off too, and stderr names why.
- `schema_version`: the config layout a file is written in, per file, `1` without one. Version 2 drops `timings.toast`. When a Kanade release changes the layout, it migrates older files in memory as it reads them and leaves them as they are on disk. A file with a version newer than this Kanade reads, or one that is not a version, is skipped whole, so a downgrade never applies settings it cannot read.
- Problems are reported on stderr with file and line. At start, a key that is unknown or a value out of range is skipped, keeping what the layers below gave it, and a file that is not valid TOML, like one that sets a key twice, is skipped whole.
- A change while running applies without a restart, except `modules`, which stays as it started and is reported pending restart. A change with any problem applies nothing: the config in effect stays whole, and stderr and `status` report why.

## Keyboard

Every Surface opens from a verb, so a niri keybind reaches it. Add these lines inside the `binds` block of `~/.config/niri/config.kdl`:

```kdl
Mod+Alt+Space hotkey-overlay-title="Island: Launcher" { spawn "kanade" "launcher" "toggle"; }
Mod+Alt+N hotkey-overlay-title="Island: Notifications" { spawn "kanade" "notifications" "toggle"; }
Mod+Alt+M hotkey-overlay-title="Island: Media" { spawn "kanade" "media" "toggle"; }
Mod+Alt+C hotkey-overlay-title="Island: Controls" { spawn "kanade" "controls" "toggle"; }
Mod+Alt+T hotkey-overlay-title="Island: Tray" { spawn "kanade" "tray" "toggle"; }
Mod+Alt+V hotkey-overlay-title="Island: Clipboard" { spawn "kanade" "clipboard" "toggle"; }
Mod+Alt+Escape hotkey-overlay-title="Island: Collapse" { spawn "kanade" "island" "collapse"; }
```

Screenshots work the same way:

```kdl
Print hotkey-overlay-title="Screenshot: Area" { spawn "kanade" "capture" "screenshot" "area"; }
Ctrl+Print hotkey-overlay-title="Screenshot: Output" { spawn "kanade" "capture" "screenshot" "output"; }
Alt+Print hotkey-overlay-title="Screenshot: Window" { spawn "kanade" "capture" "screenshot" "window"; }
Shift+Print hotkey-overlay-title="Recording: Start" { spawn "kanade" "capture" "record" "start"; }
Shift+Ctrl+Print hotkey-overlay-title="Recording: Stop" { spawn "kanade" "capture" "record" "stop"; }
```

An island opened this way takes the keyboard. If nothing on it is used for 5 s and the pointer never comes onto it, it collapses and gives the keyboard back. An island opened with a click gets keys after the click.

| Where | Keys |
|---|---|
| Any Surface | Escape collapses. A pinned island gives the keyboard back, so the `collapse` bind closes it |
| Notifications | arrows and Tab move the ring, Enter or Space presses what it is on, Backspace dismisses the card |
| Launcher | typing searches apps, a sum like `2+2*3`, or emoji after `:` (`:smile`); Up/Down/Home/End move the selection, Enter starts the app or copies the value or emoji |
| Clipboard | typing searches, arrows, Tab, Home and End move the ring, Enter copies the entry, deletes it or clears the history, whichever the ring is on |
| Tray | arrows move the ring, Enter or Space presses what it is on, Right enters a submenu, Left or Escape goes back a level |
| Media, Controls | a typed character collapses an island opened from a keybind, so it goes to the window beneath |

## Limitations

- niri only. Hyprland and Sway are not supported: focused-output routing, workspace changes and screen capture come from niri IPC.
- No screen reader support. Amane draws with the GPU and builds no accessibility tree, so the island never registers on the AT-SPI bus and Orca cannot see it. Checked on niri with `Atspi.get_desktop(0)`: every other app is listed, Kanade is not.
- A fullscreen window covers the island, Critical Activities like low battery included. The island sits on the Top layer, which niri draws below fullscreen windows; maximized windows do not cover it. See [ADR 0006](docs/adr/0006-island-on-top-layer.md).
- The Launcher cannot tell when an app fails to start. Amane's `DesktopApp::launch` runs the entry through `sh -c` and reports nothing back, so a broken `Exec` line just closes the island.
- The Launcher shows "Finding apps" forever on a system with no launchable `.desktop` entries. Amane's `Apps` is empty both while scanning and after finding nothing, and exposes no scan-complete state.
- The Launcher shows emoji only from a color emoji font that holds them as pictures (CBDT or sbix), like Noto Color Emoji or Twemoji. Amane draws only outlined letters, so Kanade draws the font's picture instead; a COLR or SVG emoji font shows none, though copying still works.
- Without `pactl`, `udevadm` or `dbus-monitor`, the OSD, notification Banners and battery reads poll instead of waiting for the system to announce a change, adding up to about 23 CPU wakeups a second at idle. A backlight changed by firmware without a kernel uevent shows no brightness OSD. See [ADR 0004](docs/adr/0004-wake-sources-on-announcements.md).
- Without `setpriv` from util-linux, helper processes like `pactl`, `udevadm`, `dbus-monitor` and `pw-dump` still work but may survive Kanade being killed abruptly. See [ADR 0004](docs/adr/0004-wake-sources-on-announcements.md).
- Screen recording records video only, the focused output whole, with `wf-recorder`'s `h264_vaapi` encoder; without a working VA-API driver it fails and the island says why. niri hands out a frame only when the screen changes, so a still screen records few frames. See [ADR 0013](docs/adr/0013-screen-recording-with-wf-recorder.md).
- The privacy cluster's microphone and camera see only capture that goes through PipeWire, using `pw-dump` from the `pipewire` package. An app that opens `/dev/video*` or ALSA directly, like `ffmpeg -f v4l2`, does not show. See [ADR 0003](docs/adr/0003-privacy-from-pipewire-graph.md).
