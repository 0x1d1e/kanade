# Kanade

A top-center Dynamic Island for niri, built with [Amane](https://github.com/MystiaFin/amane).

https://github.com/user-attachments/assets/27e6837e-8cf1-4faa-8815-e5b14cdd631b

AI-generated concept, not a capture of the current build.

## What it is

One pill at the top center of each monitor that changes shape for what is happening: the clock at rest, the playing track, volume, brightness and workspace changes, notifications, battery, a timer. Hovering it while it shows something peeks, a click expands it into one of four Surfaces: Media, Notifications, Controls and Launcher. It is also the session's notification daemon. Apart from it, at the top right of each monitor, a privacy cluster shows a microphone, camera or screen capture in use, over fullscreen windows too; the Controls Surface names the apps. At rest it draws one frame a minute, for the clock, and nothing else.

It is not a bar, dock, wallpaper, lock screen or settings app, and it shows no permanent Wi-Fi, CPU or RAM indicators. It follows niri's focused output and workspaces; Hyprland and Sway are not supported, even though Amane runs there.

## Install

Kanade needs:

- niri, which it talks to over `$NIRI_SOCKET`.
- A Rust toolchain and the system libraries Amane builds against: a C compiler, pkg-config, wayland, libxkbcommon, fontconfig, freetype, expat, vulkan-loader, libpulseaudio and linux-pam, with their headers. Amane's `install.sh` installs them with pacman, apt or dnf, see [Amane's README](https://github.com/MystiaFin/amane#installation).
- The `amane` CLI, only to send IPC verbs (`amane ipc call island ...`). Any version works.
- Optional, see Limitations: `pactl`, `udevadm`, `dbus-monitor`, `setpriv` (util-linux) and `pw-dump` (pipewire).

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

## IPC

```sh
amane ipc call island <verb>
```

| Verb | Does |
|---|---|
| `open <surface>` | opens `media`, `notifications`, `controls` or `launcher` on the focused output |
| `toggle <surface>` | opens it, or collapses it when it is already open there |
| `collapse` | collapses the open island on the focused output |
| `dnd toggle` | turns Do Not Disturb on or off: notification toasts stop showing, Critical ones still do |
| `timer start <duration>` | starts the timer, like `90s`, `25m` or `1h30m`, up to 24h |
| `timer stop` | stops it |

A verb of a Module turned off in the config answers `module <name> is off`. With no verb it prints every verb, including the `debug post` and `debug withdraw` verbs that post test Activities.

## Configuration

Kanade reads `$XDG_CONFIG_HOME/kanade/config.toml` (else `~/.config/kanade/config.toml`) once at start. Kanade accepts the TOML subset shown below: table headers, booleans, millisecond integers, double-quoted strings (only `\"` and `\\` escapes), and comments. Every key is optional; these are the defaults:

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
osd = 1200           # volume, brightness and workspace Transients
toast = 5000         # a notification shown as a Transient

[theme]
# palette = "~/Pictures/wallpaper.jpg"

[modules]            # every Module is on unless set to false here
# media = false
```

- `theme.palette`: the body and text take their tone from this image, and follow it when the file changes, so a wallpaper script that overwrites it re-themes the island. The body stays near-black and both stay near grey, so amber (capture, low battery), red (critical) and green (mic/camera) keep their meaning. Unset, the body is near-black.
- `KANADE_REDUCED_MOTION`: overrides `reduced_motion`. `1` turns it on, `0` off.
- `modules`: turns a feature off. An off Module starts no thread or helper process, posts nothing and answers its IPC verbs with `module <name> is off`. Turning one on or off takes a restart. The Modules are `island` (the island itself, cannot be turned off), `workspace`, `privacy` (microphone, camera and screen cast; turning it off is warned about at every start), `battery`, `media`, `timer`, `osd` (volume and brightness), `notifications` (with it off, Kanade is not the notification daemon and Do Not Disturb is unavailable), `network`, `bluetooth` and `power` (the Controls tiles and their Transients). A Module whose requirement is off turns off too, and stderr names why.
- A bad line keeps its default and is reported on stderr, naming the line.

## Keyboard

Every Surface opens from an IPC verb, so a niri keybind reaches it. Add these lines inside the `binds` block of `~/.config/niri/config.kdl`:

```kdl
Mod+Alt+Space hotkey-overlay-title="Island: Launcher" { spawn "amane" "ipc" "call" "island" "toggle" "launcher"; }
Mod+Alt+N hotkey-overlay-title="Island: Notifications" { spawn "amane" "ipc" "call" "island" "toggle" "notifications"; }
Mod+Alt+M hotkey-overlay-title="Island: Media" { spawn "amane" "ipc" "call" "island" "toggle" "media"; }
Mod+Alt+C hotkey-overlay-title="Island: Controls" { spawn "amane" "ipc" "call" "island" "toggle" "controls"; }
Mod+Alt+Escape hotkey-overlay-title="Island: Collapse" { spawn "amane" "ipc" "call" "island" "collapse"; }
```

An island opened this way takes the keyboard. If nothing on it is used for 5 s and the pointer never comes onto it, it collapses and gives the keyboard back. An island opened with a click gets keys after the click.

| Where | Keys |
|---|---|
| Any Surface | Escape collapses. A pinned island gives the keyboard back, so the `collapse` bind closes it |
| Notifications | arrows and Tab move the ring, Enter or Space presses what it is on, Backspace dismisses the card |
| Launcher | typing searches, Up/Down/Home/End move the selection, Enter starts it |
| Media, Controls | a typed character collapses an island opened from a keybind, so it goes to the window beneath |

## Limitations

- niri only. Hyprland and Sway are not supported: focused-output routing, workspace changes and screen capture come from niri IPC.
- No screen reader support. Amane draws with the GPU and builds no accessibility tree, so the island never registers on the AT-SPI bus and Orca cannot see it. Checked on niri with `Atspi.get_desktop(0)`: every other app is listed, Kanade is not.
- A fullscreen window covers the island, Critical Activities like low battery included. The island sits on the Top layer, which niri draws below fullscreen windows; maximized windows do not cover it. See [ADR 0006](docs/adr/0006-island-on-top-layer.md).
- The Launcher cannot tell when an app fails to start. Amane's `DesktopApp::launch` runs the entry through `sh -c` and reports nothing back, so a broken `Exec` line just closes the island.
- The Launcher shows "Finding apps" forever on a system with no launchable `.desktop` entries. Amane's `Apps` is empty both while scanning and after finding nothing, and exposes no scan-complete state.
- Without `pactl`, `udevadm` or `dbus-monitor`, the volume and brightness Transients, notification toasts and battery reads poll instead of waiting for the system to announce a change, adding up to about 23 CPU wakeups a second at idle. A backlight changed by firmware without a kernel uevent shows no brightness Transient. See [ADR 0004](docs/adr/0004-wake-sources-on-announcements.md).
- Without `setpriv` from util-linux, helper processes like `pactl`, `udevadm`, `dbus-monitor` and `pw-dump` still work but may survive Kanade being killed abruptly. See [ADR 0004](docs/adr/0004-wake-sources-on-announcements.md).
- The privacy cluster's microphone and camera see only capture that goes through PipeWire, using `pw-dump` from the `pipewire` package. An app that opens `/dev/video*` or ALSA directly, like `ffmpeg -f v4l2`, does not show. See [ADR 0003](docs/adr/0003-privacy-from-pipewire-graph.md).
