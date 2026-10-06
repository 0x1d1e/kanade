# Kanade

A top-center Dynamic Island for niri, built with [Amane](https://github.com/MystiaFin/amane).

https://github.com/user-attachments/assets/27e6837e-8cf1-4faa-8815-e5b14cdd631b

AI-generated concept, not a capture of the current build.

## Configuration

Kanade reads `$XDG_CONFIG_HOME/kanade/config.toml` (else `~/.config/kanade/config.toml`) once at start. Kanade accepts the TOML subset shown below: table headers, booleans, millisecond integers, double-quoted strings (only `\"` and `\\` escapes), and comments. Every key is optional; these are the defaults:

```toml
# the island snaps to its new shape and only fades its content, over 80 ms
reduced_motion = false

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
```

- `theme.palette`: the body and text take their tone from this image, and follow it when the file changes, so a wallpaper script that overwrites it re-themes the island. The body stays near-black and both stay near grey, so amber (capture, low battery), red (critical) and green (mic/camera) keep their meaning. Unset, the body is near-black.
- `KANADE_REDUCED_MOTION`: overrides `reduced_motion`. `1` turns it on, `0` off.
- A bad line keeps its default and is reported on stderr, naming the line.

## Keyboard

Every Surface opens from `amane ipc call island <verb>`, so a niri keybind reaches it. Add these lines inside the `binds` block of `~/.config/niri/config.kdl`:

```kdl
Mod+Alt+Space hotkey-overlay-title="Island: Launcher" { spawn "amane" "ipc" "call" "island" "toggle" "launcher"; }
Mod+Alt+N hotkey-overlay-title="Island: Notifications" { spawn "amane" "ipc" "call" "island" "toggle" "notifications"; }
Mod+Alt+M hotkey-overlay-title="Island: Media" { spawn "amane" "ipc" "call" "island" "toggle" "media"; }
Mod+Alt+C hotkey-overlay-title="Island: Controls" { spawn "amane" "ipc" "call" "island" "toggle" "controls"; }
Mod+Alt+Escape hotkey-overlay-title="Island: Collapse" { spawn "amane" "ipc" "call" "island" "collapse"; }
```

`amane ipc call island` with no verb prints every verb.

An island opened this way takes the keyboard. If nothing on it is used for 5 s and the pointer never comes onto it, it collapses and gives the keyboard back. An island opened with a click gets keys after the click.

| Where | Keys |
|---|---|
| Any Surface | Escape collapses. A pinned island gives the keyboard back, so the `collapse` bind closes it |
| Notifications | arrows and Tab move the ring, Enter or Space presses what it is on, Backspace dismisses the card |
| Launcher | typing searches, Up/Down/Home/End move the selection, Enter starts it |
| Media, Controls | a typed character collapses an island opened from a keybind, so it goes to the window beneath |

## Limitations

- No screen reader support. Amane draws with the GPU and builds no accessibility tree, so the island never registers on the AT-SPI bus and Orca cannot see it. Checked on niri with `Atspi.get_desktop(0)`: every other app is listed, `amane-shell` is not.
- The island stays visible over fullscreen windows. niri 26.04 does not report fullscreen state, and guessing it from window size also catches maximized windows, so suppression waits for [niri#2836](https://github.com/niri-wm/niri/pull/2836). See [ADR 0002](docs/adr/0002-defer-fullscreen-suppression.md).
- The Launcher cannot tell when an app fails to start. Amane's `DesktopApp::launch` runs the entry through `sh -c` and reports nothing back, so a broken `Exec` line just closes the island.
- The Launcher shows "Finding apps" forever on a system with no launchable `.desktop` entries. Amane's `Apps` is empty both while scanning and after finding nothing, and exposes no scan-complete state.
- Without `pactl`, `udevadm` or `dbus-monitor`, the volume and brightness Transients, notification toasts and battery reads poll instead of waiting for the system to announce a change, adding up to about 23 CPU wakeups a second at idle. A backlight changed by firmware without a kernel uevent shows no brightness Transient. See [ADR 0004](docs/adr/0004-wake-sources-on-announcements.md).
- Without `setpriv` from util-linux, helper processes like `pactl`, `udevadm`, `dbus-monitor` and `pw-dump` still work but may survive Kanade being killed abruptly. See [ADR 0004](docs/adr/0004-wake-sources-on-announcements.md).
- The microphone and camera indicator sees only capture that goes through PipeWire, using `pw-dump` from the `pipewire` package. An app that opens `/dev/video*` or ALSA directly, like `ffmpeg -f v4l2`, does not show. See [ADR 0003](docs/adr/0003-privacy-from-pipewire-graph.md).
