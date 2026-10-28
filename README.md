<p align="center">
  <img src="docs/assets/kanade-mascot.png" width="320" alt="Kanade">
</p>

<h1 align="center">Kanade</h1>

<p align="center">
  One island that changes shape for what is happening: a desktop shell for niri, in Rust.
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

## The Concept

There is no status bar. Each monitor hosts one Island, positioned at the top center by default.

At Rest, the Island displays only the local clock and battery level, rendering one frame a minute. When an Activity occurs (a track starts playing, volume changes, a notification arrives, battery runs low, a timer counts down, or a background script reports progress), it takes over the Island. The Island morphs to display the Activity, then returns to Rest when it finishes. Hovering peeks at details without expanding; clicking or pressing a shortcut expands a full interactive Surface. Secondary ongoing Activities remain visible beside the Island body as Satellites.

Around the Island, Kanade provides the session shell: a notification daemon with Banners, an application Dock, a PAM-based lock screen, and an OSD for hardware feedback. Kanade is written in Rust on its own Wayland layer-shell and Vulkan GPU runtime (`crates/runtime`), drawing only when content changes.

## Island Presentations

The Island adapts its geometry and contents across distinct Presentation states:

- **Rest**: The baseline state showing the time and battery level. Draws once a minute. If both clock and battery indicators are disabled, the Island autohides past the screen edge until touched or summoned.
- **Compact**: The primary Activity occupies the Island in a compact pill shape, showing concise glyphs, progress meters, or track titles.
- **Split**: When a secondary Satellite accompanies the primary Activity, the Island divides into two connected segments. The primary leads and the top Satellite trails.
- **Peek**: Hovering the pointer reveals richer information and inline controls (such as playback controls, action buttons, or countdown values) without expanding into a full window.
- **Expanded**: A click or shortcut expands the Island into a full interactive Surface dedicated to that Activity or control domain.
- **Satellites**: Small secondary indicators that emerge from under the Island body for persistent, ongoing tasks like active timers, live builds, or screen recordings.
- **Privacy cluster**: Distinct dots nested inside the trailing end of the Island body (round for microphone, square for camera, ring for screen cast). They appear whenever an application captures media through PipeWire. The Controls Surface identifies active capture applications and lets you end screen shares with Stop sharing.

## The Thirteen Surfaces

Expanding the Island opens one of thirteen dedicated Surfaces, navigable by keyboard or pointer:

- **Media**: Track metadata, album artwork, timeline seeking for supported MPRIS players, and synchronized lyrics via LRCLIB or local LRC files.
- **Notifications**: Notification center with application grouping, expandable history, Do Not Disturb toggles, and dismiss actions.
- **Controls**: Quick toggles and sub-surfaces for Wi-Fi networks, Bluetooth pairing, per-app audio output routing, power profiles, night light, caffeine, battery charge limits, and active share termination.
- **Launcher**: Fuzzy search across installed applications, open window switcher, desktop actions, unit and base conversions, mathematical calculator, and built-in shell commands; `:`, `>`, `@` and `?` open Emoji, Clipboard, Wallpapers or the command list.
- **Clipboard**: Searchable clipboard history for text and images, persistent pinned entries that survive restarts, and automatic exclusion of sensitive password-manager selections.
- **Calendar**: Agenda view supporting local iCalendar files, Google Calendar synchronization, read-only webcal subscriptions, and one-click Join actions for video meeting links.
- **Weather**: Current conditions alongside hourly and multi-day forecasts retrieved from Open-Meteo.
- **Timer**: Quick countdown timer, stopwatch, and configurable pomodoro intervals with automatic phase transitions.
- **Capture**: Full-screen, output, or region screenshots, screen color picker, and output, area, or window screen recording.
- **Wallpapers**: Visual wallpaper browser and selector driven by awww, integrating with Matugen for dynamic palette generation.
- **Keys**: Searchable cheatsheet of active niri keybindings and Kanade shell shortcuts.
- **Session**: Lock, logout, suspend, reboot, and power off controls with a grace-period countdown and inhibitor warnings.
- **Tray**: StatusNotifierItem system tray host exposing application status icons and sub-surface menus.

## Session Shell

Beyond the Island, Kanade coordinates the surrounding desktop environment:

- **Desk**: optional plain-text faces of the shell's Services on the desktop, under every window, one per monitor; the clock (`desk.clock`), the weather (`desk.weather`), the next event (`desk.agenda`) and the player's previous, play-pause and next buttons (`desk.media`), each off by default. Event titles (`desk.agenda_titles`) and the track's title and artist (`desk.media_titles`) are off by default.
- **Notification daemon**: Kanade acts as the session notification server. Notification Banners emerge directly from the Island edge on the focused monitor. Features include urgency rules, per-app sound effects, Do Not Disturb, and inline text replies directly on the Banner.
- **Dock**: Per-monitor dock strip supporting pinned and running applications, multi-window preview menus, middle-click to launch new instances, and drag-and-drop file launching. The Dock supports autohide, edge positioning, and three Island merge modes when sharing an edge: `crown` (Island sitting atop the Dock), `keystone` (Island centered in the Dock row), and `fold` (Dock revealed on hover).
- **Lock screen and idle**: Built-in PAM screen locker (`crates/lock`) running over an independent Wayland connection. Features include lock-before-sleep hooks, optional idle stages (`idle.dim`, `idle.lock`, `idle.screen_off`, `idle.suspend`), and a companion SDDM theme generator (`kanade sddm`) to mirror the login experience.
- **Appearance and finishes**: Shell surfaces (Panes) support three visual finishes: tinted solid, paper, and compositor blur. Matugen extracts wallpaper color palettes, which `appearance.sync` applies across GTK, Qt, kitty, niri borders, and Firefox or Zen.

## Integrations & Scripting

- **CLI socket**: A comprehensive IPC client (`kanade <verb>`) allows controlling every Surface, switch, and session action from shell scripts or niri keybindings.
- **Live Activities**: External processes post custom Live Activities to the Island via `kanade activity`. Used for monitoring long-running compiles, downloads, or AI agent sessions (such as Claude Code hooks). Live Activities support interactive action buttons on Peek.
- **Semantic UI tree**: `kanade ui` inspects the shell's semantic control hierarchy by name and executes pointer or keyboard actions without hardcoded screen coordinates.
- **Diagnostic doctor**: `kanade doctor` verifies the runtime environment, GPU render nodes, audio helpers, video encoders, notification bus state, and required niri layer rules.

## Scope & Non-Goals

- **niri specific**: Kanade is designed specifically for niri, integrating with its IPC socket, window layout, and layer-shell protocols. Hyprland and Sway are unsupported.
- **No permanent status bar**: Kanade does not offer a static tray strip or permanent resource meters (such as continuous CPU, RAM, or network graphs). Information surfaces only when an Activity demands attention or when explicitly summoned.
- **Not a wallpaper renderer**: Kanade manages wallpaper selection and palette extraction through awww and Matugen, but does not draw desktop wallpapers directly.

## Quick Start & Installation

On Arch Linux x86_64, the script is fetched from `main`. A piped run with a terminal opens a short TUI (`kanade-install`) unless you pass `--yes`. With no `--ref`, it downloads the Arch binary from the `install-0.1` release, checks sha256, and installs it to `~/.cargo/bin`. That binary is dynamically linked to Arch's PulseAudio (`libpulsecommon-17.0.so` and the rest of that closure). It is not static and not portable. A missing asset compiles `release/0.1`; a bad checksum does not. Packages come from `paru`, or from `yay` if paru is not on PATH and yay already is. If neither is on PATH, `sudo pacman -S --needed paru` installs paru and that is used. If that fails, the install stops. See [Installation](docs/wiki/Installation.md).

```sh
curl -fsSL https://raw.githubusercontent.com/0x1d1e/kanade/main/scripts/install | sh
```

For existing niri setups or selective installs, pass options after `sh -s --`:

- `--shell-only`: Installs Kanade without the desktop packages, and does not install a niri config.
- `--yes`: Skips the TUI.
- `--optional`: Installs optional feature packages and enables Bluetooth, UPower, and power-profiles-daemon services.
- `--ref`: Compiles that branch or tag, including `--ref main`. Does not download the `install-0.1` binary.
- `--uninstall`: Removes the Kanade binary, completions, systemd unit, and desktop icon, and keeps the `kanade.service.bak.*` copies; removes the root-owned SDDM files `--sddm` wrote, with `sudo`, and nothing else.

### Manual Build

To build and run manually from source:

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

A Vulkan driver matching your GPU is also required: `vulkan-intel` for Intel, `vulkan-radeon` for AMD, or `nvidia-utils` for NVIDIA. niri must run as a systemd graphical session (`niri-session` or display manager).

### Optional

Kanade runs without these packages, but individual features require them:

| Feature | Packages | Notes |
|---|---|---|
| Audio and device tracking | `libpulse pipewire systemd dbus util-linux` | Audio control (`pactl`), privacy dots (`pw-dump`), device detection (`udevadm`, `dbus-monitor`), and process isolation (`setpriv`). |
| Clipboard history | `wl-clipboard` | Requires version 2.3 or later to detect sensitive password-manager selections. |
| Screen recording (base) | `wf-recorder` | Records focused output or selected area. |
| Recording pause and join | `ffmpeg` | Allows pausing recordings and concatenates multi-part segments into a single file. |
| GPU recording (VA-API) | `intel-media-driver` or `mesa` | Hardware-accelerated H.264 encoding for Intel (`intel-media-driver`) or AMD (`mesa`). Window recording also requires `gst-plugin-va`. |
| GPU recording (NVENC) | `nvidia-utils` | Hardware-accelerated H.264 encoding on NVIDIA cards. Window recording also requires `gst-plugins-bad`. |
| CPU recording (fallback) | `ffmpeg` | Software H.264 encoding with libx264. Window recording also requires `gst-plugins-ugly`. |
| Window recording pipeline | `gstreamer gst-plugin-pipewire gst-plugins-base` | Captures niri DMA-BUF window casts over PipeWire with EGL frame download. |
| Wallpaper management | `awww` | Wallpaper daemon and image display. |
| Wallpaper-derived colors | `matugen` | Material You palette extraction from the active wallpaper. |
| Theme synchronization (`appearance.sync`) | `qt5ct qt6ct glib2 dconf`<br>AUR: `darkly-qt6-git`, `python-pywalfox` | Synchronizes wallpaper palette across Qt, GTK, kitty, niri borders, and Firefox or Zen. |
| Night light | `wlsunset` | Color temperature adjustment based on location or time. |
| Power and idle inhibitors | `systemd util-linux` | Inhibitor management (`systemd-inhibit`) and process lifetime scoping (`setpriv`). |
| Notification sounds | `pipewire` or `libcanberra` | Audio cues via `pw-play` or `canberra-gtk-play`. |
| Google Calendar | `gnome-keyring` (or KWallet, KeePassXC) and `xdg-utils` | Secret Service storage for OAuth tokens and browser redirection via `xdg-open`. |
| Terminal applications | `xdg-terminal-exec` | Opens terminal-based applications launched from the Launcher Surface. |

## Documentation

| Guide | Description |
|---|---|
| [Installation](docs/wiki/Installation.md) | Package requirements, manual builds, systemd service management, and niri layer rules. |
| [CLI](docs/wiki/CLI.md) | Command-line IPC syntax and available verbs for controlling Kanade. |
| [Configuration](docs/wiki/Configuration.md) | Configuration file format, option reference, and defaults. |
| [Settings](docs/wiki/Settings.md) | Built-in graphical settings editor and niri configuration support. |
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

MIT, see [LICENSE](LICENSE).
