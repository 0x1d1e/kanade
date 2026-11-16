<p align="center">
  <img src="docs/assets/kanade-mascot.png" width="320" alt="Kanade">
</p>

<h1 align="center">Kanade</h1>

<p align="center">
  One island that changes shape for what is happening: a desktop shell for niri and Umbriel, in Rust.
</p>

<p align="center">
  <a href="docs/wiki/Installation.md">Install</a> |
  <a href="docs/wiki/CLI.md">CLI</a> |
  <a href="docs/wiki/Configuration.md">Configuration</a> |
  <a href="docs/wiki/Settings.md">Settings</a> |
  <a href="docs/wiki/Keyboard.md">Keyboard</a> |
  <a href="docs/wiki/Limitations.md">Limitations</a> |
  <a href="CONTEXT.md">Domain</a> |
  <a href="docs/adr/">Decisions</a> |
  <a href="docs/wiki/Home.md">Wiki</a>
</p>

> [!WARNING]
> Kanade is in active pre-1.0 development. Expect bugs, missing pieces, and breaking changes to config, CLI, and behavior until 1.0. See [Limitations](docs/wiki/Limitations.md) for known gaps.

Kanade is a personal project, made for the author's own niri session and shared as is. There is no support or stability promise: issues and pull requests may go unanswered, and APIs and config can change before 1.0.

## What Kanade is

Kanade is an Island-first desktop shell for niri and Umbriel, written in Rust. It replaces the status bar: each monitor hosts one Island, at the top center by default. The notification daemon with its Banners, the Dock, the PAM-based lock screen and the OSD for hardware feedback are part of the same program.

At Rest the Island shows only the local clock and battery level, drawing one frame a minute. An Activity takes it over: a track starts, the volume changes, a notification arrives, the battery runs low, a timer counts down, a background script reports progress. The Island morphs to show it, then returns to Rest when it ends. Hovering peeks at the details without expanding, a click or a shortcut opens a full interactive Surface, and other ongoing Activities stay beside the body as Satellites.

Kanade draws through its own Wayland layer-shell and Vulkan GPU runtime in `crates/runtime`, and draws only when content changes.

## Island presentations

The Island changes its geometry and contents across these Presentation states:

- **Rest**: the baseline form, showing the time and battery level, drawn once a minute. With both the clock and the battery indicators disabled, it autohides past the screen edge until touched or summoned.
- **Compact**: the primary Activity in a compact pill, showing glyphs, progress meters or a track title.
- **Split**: the primary Activity and a Satellite in two connected segments, the primary leading and the top Satellite trailing.
- **Peek**: the pointer reveals richer information and inline controls, such as playback controls, action buttons or countdown values, without expanding.
- **Expanded**: a click or a shortcut expands the Island into the full interactive Surface for that Activity or control domain.
- **Satellites**: small secondary indicators that emerge from under the body for persistent work, such as an active timer, a live build or a screen recording.
- **Privacy cluster**: dots nested inside the trailing end of the body, round for the microphone, square for the camera and a ring for a screen cast, shown while an application captures media through PipeWire. The Controls Surface names the capturing apps and ends a screen share with Stop sharing.

## The Surfaces

Expanding the Island opens one of its Surfaces, navigable by keyboard or pointer:

- **Media**: track metadata, album artwork and seekable timelines for supported MPRIS players, with synchronized lyrics from LRCLIB or local LRC files.
- **Notifications**: the notification center, with app grouping, expandable history, Do Not Disturb toggles and dismiss actions.
- **Controls**: quick toggles, plus sub-surfaces for Wi-Fi networks, Bluetooth pairing, per-app audio output routing, power profiles, night light, caffeine, battery charge limits and ending an active share.
- **Launcher**: fuzzy search over installed apps, open windows, desktop actions, unit and base conversions, a calculator and built-in shell commands; `:`, `>`, `@` and `?` open Emoji, Clipboard, Wallpapers or the command list.
- **Clipboard**: searchable text and image history, with pinned entries that survive restarts and password-manager selections left out automatically.
- **Calendar**: an agenda over local iCalendar files, Google Calendar sync and read-only webcal subscriptions, with one-click Join for video meeting links.
- **Weather**: current conditions with hourly and multi-day forecasts from Open-Meteo.
- **Timer**: countdown, stopwatch and configurable pomodoro intervals with automatic phase transitions.
- **Capture**: full-screen, output or region screenshots, a screen color picker, and output, area or window recording.
- **Wallpapers**: a wallpaper browser driven by awww, with Matugen palette generation. With `wallpaper.moving` on, an animated GIF or APNG is drawn by Kanade itself at its own speed, pausing while the session is locked, the niri or Umbriel overview is open, or a fullscreen window covers its output.
- **Keys**: a searchable cheatsheet of compositor keybindings and Kanade shell shortcuts.
- **Session**: lock, logout, suspend, reboot and power off, with a grace-period countdown and inhibitor warnings.
- **Tray**: a StatusNotifierItem host for application status icons, with item menus in sub-surfaces.
- **Files**: a read-only shelf of the desktop's recent files, newest first, searched by name and folder; Enter opens the file and Right its folder. Not a file manager: nothing is deleted, renamed, moved, copied or dragged out.

## Session shell

Kanade also covers the desktop around the Island:

- **Desk**: optional plain-text faces of the shell's Services drawn on the desktop under every window, one per monitor: the clock (`desk.clock`), the weather (`desk.weather`), the next event (`desk.agenda`), the player's previous, play-pause and next buttons (`desk.media`) and your own notes (`desk.notes`), typed into in place. Every face is off by default, as are event titles (`desk.agenda_titles`) and the track's title and artist (`desk.media_titles`). A face stands where `desk.place` puts it, otherwise in the corner `desk.clock_corner` names; drag its handle to move it, and the place is kept per output.
- **Notification daemon**: Kanade is the session's notification server. Banners come out of the Island edge on the focused monitor, with urgency rules, per-app sounds, Do Not Disturb and inline text replies on the Banner.
- **Dock**: a per-monitor strip of pinned and running applications, with multi-window preview menus, middle-click to launch a new instance and drag-and-drop file launching. It autohides, sits on any edge, and merges with the Island in three modes where they share an edge: `crown` (the Island atop the Dock), `keystone` (the Island centered in the Dock row) and `fold` (the Dock revealed on hover).
- **Lock screen and idle**: a PAM screen locker in `crates/lock` on its own Wayland connection, with lock-before-sleep hooks, optional idle stages (`idle.dim`, `idle.lock`, `idle.screen_off`, `idle.suspend`) and an SDDM theme generator (`kanade sddm`) that mirrors the login screen.
- **Panes and finishes**: shell Panes have three finishes, tinted solid, paper and compositor blur. Matugen extracts a palette from the wallpaper, and `appearance.sync` applies it across GTK, Qt, kitty, niri borders and Firefox or Zen.

## Integrations and scripting

- **`kanade <verb>` CLI**: an IPC client for every Surface, switch and session action, usable from shell scripts and niri keybindings.
- **`kanade activity`**: external processes post their own Live Activities to the Island, for long compiles, downloads or AI agent sessions such as Claude Code hooks, with action buttons on Peek.
- **`kanade ui`**: inspect the shell's semantic control tree by name, then press or type on it without screen coordinates.
- **`kanade doctor`**: verify the runtime environment, GPU render nodes, audio helpers, video encoders, notification bus state and required niri layer rules.

## Scope and non-goals

Kanade supports niri and Umbriel over their IPC sockets, choosing a reachable Umbriel socket first and niri after. Window and Strip Surfaces, the Dock, Launcher window selection, keybindings and compositor Settings follow the selected compositor. Hyprland and Sway are unsupported; [Limitations](docs/wiki/Limitations.md) lists the compositor-specific gaps, capture included.

There is no permanent status bar: no static tray strip, and no permanent CPU, RAM or network graphs. Information appears when an Activity needs attention or when you summon it.

Kanade is not a video wallpaper renderer. It selects wallpapers and extracts palettes through awww and Matugen, and draws desktop wallpapers through them; the only exception is an animated GIF or APNG picked with `wallpaper.moving` on.

## Install

On Arch Linux and derivatives the script installs dependencies, clones `main` and builds Kanade with `cargo install --locked --path .` into `~/.cargo/bin`; run it again to update. Packages come from `paru`, or an existing `yay`; if neither is available the script installs `paru` through pacman. See [Installation](docs/wiki/Installation.md).

```sh
curl -fsSL https://raw.githubusercontent.com/0x1d1e/kanade/main/scripts/install | sh
```

For an existing niri or Umbriel setup, or a selective install, pass options after `sh -s --`:

- `--shell-only`: install Kanade without the desktop packages; a missing niri config is installed only with `--niri` or `--sddm`.
- `--yes`: accepted for compatibility.
- `--optional`: install the optional feature packages and enable the Bluetooth, UPower and power-profiles-daemon services.
- `--ref REF`: build that branch or tag instead of `main`.
- `--uninstall`: remove the Kanade binary, completions, systemd unit and desktop icon, keeping the `kanade.service.bak.*` copies. It also removes the root-owned SDDM files that `--sddm` wrote, with `sudo`, and nothing else.

### Manual build

To build and run from source:

```sh
cargo install --locked --path .
mkdir -p ~/.config/systemd/user
cp dist/kanade.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now kanade.service
```

> [!IMPORTANT]
> Kanade must be the only active notification daemon in your session. Disable alternative daemons such as mako or dunst (`systemctl --user disable --now mako`) before starting Kanade.

## Dependencies

On Arch Linux and derivatives, all packages come from official repositories unless marked AUR.

### Required

To build and run the shell:

```sh
sudo pacman -S --needed niri rust base-devel pkgconf wayland libxkbcommon fontconfig freetype2 \
  expat vulkan-icd-loader libpulse pam
```

A Vulkan driver matching your GPU is also required: `vulkan-intel` for Intel, `vulkan-radeon` for AMD, or `nvidia-utils` for NVIDIA. The command above installs niri; for an Umbriel session, install Umbriel separately and omit `niri`. The supplied systemd unit starts with niri's graphical session (`niri-session` or display manager); Umbriel startup and screenshot helpers are covered in [Installation](docs/wiki/Installation.md).

### Optional

Kanade runs without these packages, but individual features require them:

| Feature | Packages | Notes |
|---|---|---|
| Audio and device tracking | `libpulse pipewire systemd dbus util-linux` | Audio control (`pactl`), privacy dots (`pw-dump`), device detection (`udevadm`, `dbus-monitor`) and process isolation (`setpriv`). |
| Clipboard history | `wl-clipboard` | Version 2.3 or later detects sensitive password-manager selections. |
| Screen recording (base) | `wf-recorder` | Records the focused output or a selected area. |
| Recording pause and join | `ffmpeg` | Pauses recordings and joins their segments into one file. |
| GPU recording (VA-API) | `intel-media-driver` or `mesa` | H.264 encoding on Intel (`intel-media-driver`) or AMD (`mesa`). Window recording also needs `gst-plugin-va`. |
| GPU recording (NVENC) | `nvidia-utils` | H.264 encoding on NVIDIA cards. Window recording also needs `gst-plugins-bad`. |
| CPU recording (fallback) | `ffmpeg` | Software H.264 with libx264. Window recording also needs `gst-plugins-ugly`. |
| Window recording pipeline | `gstreamer gst-plugin-pipewire gst-plugins-base` | Reads niri DMA-BUF window casts over PipeWire with EGL frame download. |
| Wallpaper management | `awww` | Wallpaper daemon and image display. |
| Wallpaper-derived colors | `matugen` | Material You palette extraction from the active wallpaper. |
| Theme synchronization (`appearance.sync`) | `qt5ct qt6ct glib2 dconf`<br>AUR: `darkly-qt6-git`, `python-pywalfox` | Applies the wallpaper palette across Qt, GTK, kitty, niri borders and Firefox or Zen. |
| Night light | `wlsunset` | Color temperature by location or time. |
| Power and idle inhibitors | `systemd util-linux` | Inhibitors (`systemd-inhibit`) and process lifetime scoping (`setpriv`). |
| Notification sounds | `pipewire` or `libcanberra` | Cues through `pw-play` or `canberra-gtk-play`. |
| Google Calendar | `gnome-keyring` (or KWallet, KeePassXC) and `xdg-utils` | Secret Service storage for OAuth tokens and browser redirect via `xdg-open`. |
| Terminal applications | `xdg-terminal-exec` | Opens terminal apps launched from the Launcher. |

## Documentation

| Guide | Description |
|---|---|
| [Installation](docs/wiki/Installation.md) | Package requirements, manual builds, systemd service management, and niri layer rules. |
| [CLI](docs/wiki/CLI.md) | Command-line IPC syntax and available verbs for controlling Kanade. |
| [Configuration](docs/wiki/Configuration.md) | Configuration file format, option reference, and defaults. |
| [Settings](docs/wiki/Settings.md) | Built-in graphical settings editor and niri/Umbriel configuration support. |
| [Keyboard](docs/wiki/Keyboard.md) | Keyboard shortcuts for every Surface and recommended niri keybindings. |
| [Limitations](docs/wiki/Limitations.md) | Documented design boundaries, trade-offs, and known gaps. |
| [Agent hooks](docs/wiki/Agent-hooks.md) | Recipe for connecting Claude Code sessions as Live Activities. |
| [Dictation](docs/wiki/Dictation.md) | Voice dictation setup with the Gemini API and privacy disclosures. |
| [Google Calendar](docs/wiki/Google-Calendar.md) | OAuth client creation and synchronization setup. |
| [SDDM](docs/wiki/SDDM.md) | Matching SDDM login theme generation via `kanade sddm`. |
| [Architecture decisions](docs/adr/) | Decisions and supersessions. |
| [Domain terms](CONTEXT.md) | Canonical domain language, invariants, and naming rules. |
| [Branding](docs/branding.md) | Mascot assets, mark specifications, and visual identity. |

## Acknowledgements

- [Amane](https://github.com/MystiaFin/amane) by MystiaFin: A foundational Rust Wayland shell runtime.
- [iNiR](https://github.com/snowarch/iNiR): Adaptive island concept for niri, demonstrating one body across multiple contexts.

## License

Apache-2.0, see [LICENSE](LICENSE).
