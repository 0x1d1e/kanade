<p align="center">
  <img src="docs/assets/kanade-mascot.png" width="320" alt="Kanade">
</p>

<h1 align="center">Kanade</h1>

<p align="center">
  A top-center Dynamic Island for niri.
</p>

https://github.com/user-attachments/assets/086ab84e-4144-456b-980e-5431f3439300

AI-generated concept, not a capture of the current build.

## What it is

One pill at the top center of each monitor that changes shape for what is happening: the clock at rest, the playing track, volume, brightness, Caps Lock, Num Lock and airplane mode, workspace changes, notifications, battery, a timer. Hovering it while it shows something peeks, a click expands it into one of nine Surfaces: Media, Notifications, Controls, Launcher, Tray, Clipboard, Calendar, Weather and Session. Hovering it at rest shows the apps' tray items beside the clock: left click activates one, middle click is its secondary action, the wheel scrolls it, and right click opens its menu. It is also the session's notification daemon. While a microphone, camera or screen capture is in use, dots at its trailing end show it, and the Controls Surface names the apps; a fullscreen window covers them, and `privacy.indicators` turns them off. At rest it draws one frame a minute, for the clock, and nothing else.

On each monitor, at the edge and side set in `dock.edge` and `dock.align`, a Dock launches and focuses apps. `kanade lock` locks the session with a lock screen that shows the date, the time, who is signed in and a password field over the wallpaper, and no notification; the session also locks before the machine sleeps.

It is not a bar, wallpaper renderer or system settings app, and it shows no permanent Wi-Fi, CPU or RAM indicators. It follows niri's focused output and workspaces; Hyprland and Sway are not supported.

## Install

Kanade needs:

- niri, which it talks to over `$NIRI_SOCKET`.
- A Rust toolchain and these system libraries with their headers: a C compiler, pkg-config, wayland, libxkbcommon, fontconfig, freetype, expat, vulkan-loader, libpulseaudio and linux-pam.
- Optional, see Limitations: `pactl`, `udevadm`, `dbus-monitor`, `setpriv` (util-linux) and `pw-dump` (pipewire).
- For the clipboard history: `wl-paste` and `wl-copy` from wl-clipboard 2.3 or later, the first that tells a password manager's copy; with an older one Kanade keeps no history.
- For screen recording: `wf-recorder`, and a VA-API driver for the GPU.
- For the wallpaper: [awww](https://codeberg.org/LGFae/awww) (`awww` and `awww-daemon`), which draws it; Kanade picks the image and runs `awww-daemon` unless one runs already, only through `setpriv` (util-linux), so it never outlives Kanade.
- For caffeine and locking before sleep: `systemd-inhibit` (systemd) and `setpriv` (util-linux), which ties the inhibitor to Kanade so it never outlives it.
- For Google Calendar: a Secret Service, like GNOME Keyring, KWallet or KeePassXC, and `xdg-open`. See [Google Calendar](#google-calendar).

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

## CLI

`kanade` with no verb runs the shell. `kanade <verb> [args]` asks the running one:

| Verb | Does |
|---|---|
| `launcher\|controls\|media\|notifications\|tray\|clipboard\|calendar\|weather open` | opens that Surface on the focused output |
| `... close` | collapses it when it is open there |
| `... toggle` | opens it, or collapses it when it is already open there |
| `island collapse` | collapses the open island on the focused output |
| `notifications clear` | dismisses every notification |
| `notifications dnd on\|off\|toggle` | Do Not Disturb: notification Banners stop showing, Critical ones still do |
| `clipboard clear` | forgets the clipboard history; what is on the clipboard stays |
| `capture screenshot area\|window\|output` | asks niri for a screenshot of a picked area, the focused window or the focused output, saved under `~/Pictures/Screenshots`; prints its path |
| `capture record start\|stop\|status` | records the focused output with `wf-recorder`, encoded on the GPU, to `~/Videos/Screencasts`; `start` and `stop` print its path once it records or is saved, `status` says how it stands |
| `caffeine on\|off\|toggle [<duration>]\|status` | keeps the session from going idle, so idle daemons like hypridle neither lock nor suspend it, until turned off or for a duration like `90s`, `25m` or `1h30m`, up to 24h; the island shows it while on. `on` returns once the inhibitor is held, `status` says how it stands |
| `settings open [<page>]\|close` | opens the Settings window, on a page (`appearance`, `island`, `dock`, `motion`, `notifications`, `modules`, `data` or `lock`; a Module's name opens the page with its settings) or the one it last showed, or closes it |
| `wallpaper set <path>\|status` | sets the wallpaper to an image through awww, and prints its path once awww shows it; `status` names the one set since start |
| `google-calendar sign-in <client.json>\|sign-out\|sync\|status` | signs in to Google Calendar through the browser, with a Desktop app OAuth client's JSON file, see [Google Calendar](#google-calendar); `sign-out` revokes access and deletes the credentials and synced events, `sync` syncs now, `status` says how the account stands |
| `weather refresh\|status` | fetches the weather now, or says how it stands and when it was fetched |
| `osd volume\|brightness` | shows the OSD for the volume or brightness as it is, on the focused output, for a keybind that changes it outside Kanade; refused while `osd`, or `audio` or `brightness`, is off |
| `timer start <duration>` | starts the timer, like `90s`, `25m` or `1h30m`, up to 24h |
| `timer pause\|resume\|cancel` | pauses, resumes or cancels it |
| `config reload` | reads the config again now |
| `config validate` | says what a reload would find, without applying it |
| `status` | the config generation, the last reload error and the keys pending restart |
| `module list` | each Module: whether it runs, why not, and what a restart would change |
| `module enable\|disable <name>` | turns a Module on or off in the settings file, applied at the next restart; prints what the restart will change, the Modules that turn off with it included. `island` cannot be turned off |
| `lock` | locks the session: every monitor shows the date, the time, who is signed in and a password field over its wallpaper, checked by PAM through its `login` service. Exits 0 once niri holds the lock for that call, 1 when niri refuses it, as while another locker holds the session, 3 if not within 5 s or when a password typed on it is accepted or being checked, see [Keyboard](#keyboard). Refused without the `login` PAM service |
| `session menu\|suspend\|reboot\|poweroff\|logout` | `menu` opens the Session Surface: lock, sleep, restart, power off and log out; it has no `toggle`, and Escape closes it. `suspend` asks logind to suspend at once; `reboot`, `poweroff` and `logout` count down 60 s on every island first, with Cancel and a button to do it now, and a newer one replaces the countdown. Kanade does not let polkit ask for a password, so a request that needs one is refused; for a countdown, that happens once it runs out. The exit status says only that it was asked: a refusal shows later on the island with its reason, over a countdown too. A countdown's refusal stays until dismissed. A countdown starting collapses any other open Surface; under the overview it shows once that closes. Until logind answers a request, another sleep, restart, power off or log out is refused, and so is a countdown running out meanwhile. A request logind never answers may still happen: the island says so until dismissed, a countdown running is refused, and nothing more is asked until Kanade restarts |
| `doctor` | read-only diagnostics: Kanade and niri versions, Wayland protocols, buses and sockets, the config, and what each Module on runs worse without, like a missing `pw-dump` or another notification daemon; works without a shell, exits 1 on a failure |

`kanade help` lists every verb with a line on what it does; `kanade help <verb>` or `kanade <verb> --help` gives the forms it takes and what their arguments are, for `debug` the `post` and `withdraw` forms that post test Activities. `kanade --version` prints the version. Output is bold on a terminal unless `NO_COLOR` is set. A verb of a Module turned off in the config answers `module <name> is off`. With no shell running, a verb says so. Exit status: 0 done, 1 refused, no shell or a failed write to stdout (not a pipe its reader closed, as `head` does), 2 not a verb or not its arguments, 3 not known whether it was done: the shell, or niri for a screenshot, got the call but did not answer in time, so it may still be done; not worth repeating blindly.

## Configuration

Kanade reads its config at start, and again whenever one of its files changes, in layers, each over the ones before it:

1. the defaults below
2. every `*.toml` in `$XDG_CONFIG_HOME/kanade/` (else `~/.config/kanade/`), in alphabetical order, so `config.toml` can sit beside files like `10-theme.toml` that another tool manages. Hidden files are skipped.
3. `$XDG_STATE_HOME/kanade/settings.toml` (else `~/.local/state/kanade/settings.toml`), which only the Settings window (`kanade settings open`) and `kanade module` write. It holds just what it changed from the layers below: a value set back to theirs removes the key

Tables merge key by key, so a later file only changes the keys it sets; any other value, a list too, replaces the one below. Kanade never writes your files. Every key is optional; these are the defaults, which `kanade config defaults` prints:

```toml
# the island snaps to its new shape and only fades its content, over 80 ms; KANADE_REDUCED_MOTION=1 or 0 overrides it
reduced_motion = false

# the time an idle island shows: "24h" (14:05) or "12h" (2:05 PM), per output
clock = "24h"

[timings]
# pointer resting on a Compact island before it peeks, ms 1-60000
hover = 120
# morph to a larger form, ms 1-60000
expand = 180
# one Surface replacing another, and a new track dissolving in, ms 1-60000
surface_change = 220
# morph to a smaller form, ms 1-60000
collapse = 180
# pointer out before a Peek or Surface collapses, ms 1-60000
grace = 250
# the OSD, and a workspace switch on the island, ms 1-60000
osd = 1200

[theme]
# the image the Settings window takes its tone from
# palette = "~/Pictures/wallpaper.jpg"

[appearance]
# what the Island, Dock and Banners are made of, with the lock screen's field: "liquid-glass" bends what is under it into its rim, "monochrome" is one flat tone with nothing seen through it
material = "liquid-glass"
# dark glass under light text, or light glass under dark text
tone = "dark"
# how bright the light on the Island's rim and under the pointer is, whatever it is made of: "subtle", "standard", "bright" or "off"
highlight = "subtle"
# how the Island and Dock settle: "bouncy" like macOS, "smooth" with no overshoot, "snappy", or "playful"
motion = "bouncy"
# the font family every label is set in, else Inter, else the system's sans-serif, takes a restart
# font = "Inter"

[island]
# the edge of the screen the Island hangs from
edge = "top"
# where along its edge the Island sits
align = "center"
# how wide the Surfaces with a list grow, as their content asks, px 520-800
width = 520
# how tall the Surfaces with a list grow, as their content asks, px 330-600
height = 400
# an idle Island slides past its edge, back as the pointer touches the edge under it or anything shows
autohide = false
# windows keep off the Island's strip of the screen, unless it autohides; off, it floats over them
reserve = false

[rest]
# an idle Island shows the time; with it and the battery off, the Island hides as it does with autohide, per output
clock = true
# an idle Island shows the battery's percent after the time, per output
battery = false

[rest.peek]
# the time's Peek shows the battery's percent beside the time, where an autohidden Island is seen, per output
battery = true
# the time's Peek shows the weather beside the date, per output
weather = true
# the time's Peek shows the next event beside the date, per output
agenda = true

[privacy]
# dots at the Island's trailing end while a microphone, camera or screen cast is in use, and the apps named in Controls; not shown over a fullscreen window
indicators = true

[banners]
# how a Banner comes in: "morph" out from under the Island, "drop" from the edge, or "fade"
entrance = "morph"

[modules]
# every Module is on unless set to false here, takes a restart
# media = false

[windows.apps]
# app id = the .desktop file it belongs to, where Kanade's guess is wrong
# jetbrains-idea = "intellij-idea-ultimate-edition"

[dock]
# .desktop file ids, in the Dock's order
# pinned = ["firefox", "kitty"]
# the edge of the screen the Dock sits on; beside the Island, on its edge and side or either centered, it sits further in than the Island; wherever the Island grows over it, as an open Surface, it steps aside and fades back as the Island closes
edge = "bottom"
# where along its edge the Dock sits
align = "center"
# the Dock hides until the pointer touches the edge under it; beside the Island it always shows, as the pointer could not reach past the Island; merged with it, it folds into the Island as `merge = "fold"` does
autohide = false
# windows keep off the Dock's strip of the screen, unless it autohides; off, it floats over them
reserve = true
# how much an icon grows under the pointer, its neighbors less
magnification = "classic"
# how big the icons are at rest; the gaps between and how far the swell under the pointer reaches grow with them
size = "medium"
# how the Dock and the Island join where they share an edge and a side, along a top or bottom edge: crown, the Island rising from the Dock's middle; keystone, the Island the Dock's middle piece; fold, the Dock unfolding out of the Island under the pointer; off, the Dock beside the Island, further in
merge = "crown"

[media]
# what moves beside a playing track
visualizer = "bars"

[wallpaper]
# the images the Launcher offers after `@`, else ~/Pictures/Wallpapers
# directory = "~/Pictures/Wallpapers"

[calendar]
# iCalendar (.ics) files and directories of them, read recursively, else $XDG_DATA_HOME/calendars
# paths = ["~/.local/share/calendars", "~/Documents/holidays.ics"]

[weather]
# where the weather is for, as [latitude, longitude] in degrees, north and east positive; unset fetches nothing
# location = [52.52, 13.41]
# the location's name, which the Weather Surface shows
# place = "Berlin"
# °C and km/h, or °F and mph
units = "metric"

[lock]
# what the lock screen shows behind the clock: the wallpaper awww shows on that output "blurred", sharp as "wallpaper", "dimmed", or "solid"; solid also stands in for a wallpaper that is not png, jpeg, webp, gif or svg
backdrop = "blurred"

[output."eDP-1"]
# for the output of this name only, over the keys above from any file
# clock = "24h"
# rest.clock = true
# rest.battery = false
# rest.peek.battery = true
# rest.peek.weather = true
# rest.peek.agenda = true
```

- `theme.palette`: the Settings window takes its tone from this image and follows it when the file changes. The island, Dock and Banners are glass tinted by `appearance.tone` whatever the wallpaper, so amber (capture, low battery), red (critical) and green (mic/camera) keep their meaning.
- `windows.apps`: which `.desktop` file a window's app id belongs to, for an app Kanade matches wrongly or not at all (see `kanade status`). Kanade otherwise matches the file id, then `StartupWMClass`, then either ignoring case, then the file id's last part (ADR 0014).
- `wallpaper.directory`: the folder of images the Launcher lists after `@`, not its subfolders.
- `calendar.paths`: the local calendars, as `.ics` files or directories of them (a vdir, as vdirsyncer writes), read recursively and followed as they change. Read-only; the calendar fetches nothing (ADR 0015), and a Google account syncs through `google-calendar` instead.
- `weather.location`: where the weather is for. Kanade asks [Open-Meteo](https://open-meteo.com/) for it, sending only these coordinates, at start, every 30 minutes, on `kanade weather refresh`, when this key changes, and when the Weather Surface opens on a forecast 30 minutes old; a failed fetch tries again after a minute, doubling up to 30. Unset, nothing is fetched (ADR 0017). Weather data by Open-Meteo.com, under CC BY 4.0.
- `dock.pinned`: the apps the Dock keeps, as `.desktop` file ids (the `.desktop` optional), left to right. A later file's list replaces the one below. `kanade status` names a pinned id no `.desktop` file has.
- `output."<name>"`: a key marked per output, for the monitor niri names so (`niri msg outputs`) only. It wins over the same key outside it in any file, and a later file's value for that output replaces an earlier one's. A key not marked per output is skipped there, with a warning.
- `KANADE_REDUCED_MOTION`: overrides `reduced_motion`. `1` turns it on, `0` off.
- `modules`: turns a feature off. An off Module starts no thread or helper process, posts nothing and answers its verbs with `module <name> is off`. Turning one on or off takes a restart. The Modules are `island` (the island itself, cannot be turned off; a held Backspace or arrow repeats on it as the compositor repeats keys, read from the keyboards, masked in the kernel to those keys, which needs the `input` group, while letters type once, as on macOS; a key remapped to Backspace, like `caps:backspace`, does not repeat), `workspace`, `windows` (the running apps for the Dock, read from niri; `kanade status` names the app ids no `.desktop` file matched), `dock` (where `dock.edge` and `dock.align` put it on each monitor, the pinned apps, then the running ones not pinned, a dot between each running one and the edge; a click launches an app without windows as its `.desktop` file says, focuses its window, or the next one if it is focused already; an app with `Terminal=true` needs `xdg-terminal-exec`; requires `windows`), `privacy` (microphone, camera and screen cast; turning it off is warned about at every start), `battery`, `media`, `timer`, `audio` (volume and mute; with it off, nothing reads them), `brightness` (the backlight; likewise), `osd` (volume, brightness and microphone mute, the keyboard's backlight as UPower announces it, its firmware key included, Caps Lock, Num Lock and airplane mode on the focused output's island, over any Activity, over an open Surface as a capsule, and over fullscreen windows while it shows, and nothing while another session has the seat; shows what of `audio` and `brightness` is on. A level key shows the level even at its limit: Kanade reads the keyboards, masked in the kernel to the level keys and lock lights, which needs the `input` group; airplane mode needs the `rfkill` group; `kanade doctor` says which is missing), `notifications` (with it off, Kanade is not the notification daemon and Do Not Disturb is unavailable), `banners` (notification cards on the focused output, coming out of the island and hanging from it, past a Dock beside it; requires `notifications`), `network`, `bluetooth` and `power` (the Controls tiles), `tray` (Kanade hosts the apps' tray items, and is their StatusNotifierWatcher unless another program already is; hovering the island at rest shows them, and the Tray Surface lists them with their menus), `clipboard` (a history of copied text and images through `wl-paste` and `wl-copy`, kept only in memory and never logged; a copy its app marks sensitive, like a password manager's, is never read), `capture` (screenshots through niri; each one saved, by Kanade or by niri's own binds, shows on the island for 10 s, where Copy path needs `wl-copy` and Open needs `xdg-open`; a recording shows with Stop while it runs, and as a screenshot does once saved; recording needs `wf-recorder`), `lock` (the lock screen, over the wallpaper `awww query` says each output shows, solid without awww; at start it locks again a session logind still counts as locked, after a crash; before the machine sleeps it locks, holding sleep back through a `systemd-inhibit` delay inhibitor until niri holds the lock, at most logind's `InhibitDelayMaxSec`, 5 s by default, and until it wakes no password typed is checked, staying in the field to send again, while one being checked as sleep comes is dropped and typed again (one already accepted may still unlock for a moment before the lock is asked again); a system bus lost is connected to again, and a gate shut stays shut until logind says the machine woke; an inhibitor lost is taken again, and `kanade status` says how it stands; without `systemd-inhibit` or `setpriv`, sleep may come before the lock; the password goes only to PAM, and is never written to disk, logged or shown), `session` (the Session Surface and `kanade session`; Lock shows only while `lock` is on), `caffeine` (holds a logind idle inhibitor through `systemd-inhibit` while on, shown on the island with Turn off; without `setpriv` it refuses to turn on), `wallpaper` (sets the wallpaper through awww, from `kanade wallpaper set` or the Launcher after `@`; runs `awww-daemon` while Kanade runs, unless one runs already; without `setpriv` it runs none; an `awww img` that takes over 10 s is stopped), `calendar` (local iCalendar files read from `calendar.paths`, followed as they change; nothing is fetched), `google-calendar` (a Google account's calendars, synced every 15 minutes into files `calendar` reads; idle until signed in; requires `calendar`), `weather` (the weather for `weather.location` from Open-Meteo; with no location, or off, nothing is fetched), `settings` (the Settings window: each setting on a page per Module, applied as soon as it changes, or marked Restart to apply), and the island's Surfaces: `controls` (with it off, a click at Rest does nothing), `launcher` (an app starts as its `.desktop` file says, as from the Dock, so one with `Terminal=true` needs `xdg-terminal-exec`; copying a sum's value or an emoji needs `wl-copy`), `notification-surface` (the notification list; requires `notifications`, and with it off a click on a notification opens Controls), `clipboard-surface` (the clipboard history; requires `clipboard`), `calendar-surface` (a month and the agenda of a day; requires `calendar`) and `weather-surface` (the weather now and the next days; requires `weather`). An off Surface never opens. A Module whose requirement is off turns off too, and stderr names why.
- `schema_version`: the config layout a file is written in, per file, `1` without one. Version 2 drops `timings.toast`; version 3 drops `privacy.style`. When a Kanade release changes the layout, it migrates older files in memory as it reads them and leaves them as they are on disk. A file with a version newer than this Kanade reads, or one that is not a version, is skipped whole, so a downgrade never applies settings it cannot read.
- Problems are reported on stderr with file and line. At start, a key that is unknown or a value out of range is skipped, keeping what the layers below gave it, and a file that is not valid TOML, like one that sets a key twice, is skipped whole.
- A change while running applies without a restart, except `modules`, which stays as it started and is reported pending restart. A change with any problem applies nothing: the config in effect stays whole, and stderr and `status` report why.

## Google Calendar

The `google-calendar` Module shows a Google account's calendars on the Calendar Surface beside the local ones (ADR 0016). Kanade signs in with your own OAuth client, so you make one once:

1. In [Google Cloud](https://console.cloud.google.com/), create a project and enable the Google Calendar API in it.
2. Under Google Auth Platform, set up the consent screen as External and add yourself as a test user.
3. Create an OAuth client of type Desktop app and download its JSON file.
4. Run `kanade google-calendar sign-in ~/Downloads/client_secret_….json` and allow read access in the browser that opens.

The client and the refresh token go to the Secret Service, never to the config or a log, so the downloaded file can be deleted afterwards. Kanade syncs at start, every 15 minutes and on `kanade google-calendar sync`. It syncs each calendar shown in Google Calendar, from a year back to two years ahead, into `~/.cache/kanade/google-calendar/`, readable only by you. A problem, like access revoked or no network, shows under the agenda and in `kanade google-calendar status`. The events synced before stay, and the local calendars are not affected. While the consent screen is in Testing, Google ends the sign-in after 7 days and the Surface asks you to sign in again; publishing it to production avoids that. `kanade google-calendar sign-out` revokes access at Google and deletes the credentials and the synced events.

## Keyboard

Every Surface opens from a verb, so a niri keybind reaches it. Add these lines inside the `binds` block of `~/.config/niri/config.kdl`:

```kdl
Mod+Alt+Space hotkey-overlay-title="Island: Launcher" { spawn "kanade" "launcher" "toggle"; }
Mod+Alt+N hotkey-overlay-title="Island: Notifications" { spawn "kanade" "notifications" "toggle"; }
Mod+Alt+M hotkey-overlay-title="Island: Media" { spawn "kanade" "media" "toggle"; }
Mod+Alt+C hotkey-overlay-title="Island: Controls" { spawn "kanade" "controls" "toggle"; }
Mod+Alt+T hotkey-overlay-title="Island: Tray" { spawn "kanade" "tray" "toggle"; }
Mod+Alt+V hotkey-overlay-title="Island: Clipboard" { spawn "kanade" "clipboard" "toggle"; }
Mod+Alt+D hotkey-overlay-title="Island: Calendar" { spawn "kanade" "calendar" "toggle"; }
Mod+Alt+W hotkey-overlay-title="Island: Weather" { spawn "kanade" "weather" "toggle"; }
Mod+Alt+P hotkey-overlay-title="Island: Session" { spawn "kanade" "session" "menu"; }
Mod+Alt+Escape hotkey-overlay-title="Island: Collapse" { spawn "kanade" "island" "collapse"; }
```

Screenshots and caffeine work the same way:

```kdl
Print hotkey-overlay-title="Screenshot: Area" { spawn "kanade" "capture" "screenshot" "area"; }
Ctrl+Print hotkey-overlay-title="Screenshot: Output" { spawn "kanade" "capture" "screenshot" "output"; }
Alt+Print hotkey-overlay-title="Screenshot: Window" { spawn "kanade" "capture" "screenshot" "window"; }
Shift+Print hotkey-overlay-title="Recording: Start" { spawn "kanade" "capture" "record" "start"; }
Shift+Ctrl+Print hotkey-overlay-title="Recording: Stop" { spawn "kanade" "capture" "record" "stop"; }
Mod+Alt+K hotkey-overlay-title="Caffeine" { spawn "kanade" "caffeine" "toggle"; }
Mod+Alt+L hotkey-overlay-title="Lock" { spawn "kanade" "lock"; }
```

An idle daemon locks the same way. For hypridle, in `~/.config/hypr/hypridle.conf`:

```ini
general {
    lock_cmd = kanade lock
}

listener {
    timeout = 300
    on-timeout = loginctl lock-session
}
```

Kanade locks before sleep on its own, so no `before_sleep_cmd` is needed, and caffeine keeps hypridle from locking or suspending. An explicit suspend still locks while caffeine is on. `kanade lock` exits 0 only once niri holds the lock for that call and no password typed since was accepted, never for a lock ending right before it, so a suspend hook can wait on it. It exits 3 at once when a password typed on the lock screen unlocks it or is being checked as it is called, and with 1 when niri refuses the lock, as while another locker holds the session. On the lock screen, type the password and press Enter.

An island opened this way takes the keyboard. If nothing on it is used for 5 s and the pointer never comes onto it, it collapses and gives the keyboard back. An island opened with a click gets keys after the click.

| Where | Keys |
|---|---|
| Any Surface | Escape collapses. A pinned island gives the keyboard back, so the `collapse` bind closes it |
| Notifications | arrows and Tab move the ring, Enter or Space presses what it is on, Backspace dismisses the card |
| Launcher | typing searches apps, a sum like `2+2*3`, emoji after `:` (`:smile`), or a wallpaper after `@` (`@moon`), or where Kanade goes (`wifi`, `bluetooth`, `clipboard`, a setting like `magnification` or a page like `dock settings`); Up/Down/Home/End move the selection, Enter starts the app, copies the value or emoji, sets the wallpaper, or opens Controls, the Clipboard or Settings |
| Clipboard | typing searches, arrows, Tab, Home and End move the ring, Enter copies the entry, deletes it or clears the history, whichever the ring is on |
| Calendar | Left/Right move a day, Up/Down a week, `n`/`p` a month, Home or `t` go to today; Tab or Enter enters the day's agenda, where Up/Down move through its events; Tab, Enter or Escape leave it |
| Session | arrows move the ring, Enter or Space presses what it is on; a restart, power off or log out keeps it open on its countdown, the ring on Cancel |
| Tray | arrows move the ring, Enter or Space presses what it is on, Right enters a submenu, Left or Escape goes back a level |
| Media, Controls, Weather, Session | a typed character collapses an island opened from a keybind, so it goes to the window beneath |

## Limitations

- niri only. Hyprland and Sway are not supported: focused-output routing, workspace changes and screen capture come from niri IPC.
- No screen reader support. Kanade draws with the GPU and builds no accessibility tree, so the island never registers on the AT-SPI bus and Orca cannot see it. Checked on niri with `Atspi.get_desktop(0)`: every other app is listed, Kanade is not.
- A fullscreen window covers the island, Critical Activities like low battery included; only the OSD and an open Surface show over it. The island sits on the Top layer, which niri draws below fullscreen windows; maximized and windowed-fullscreen windows do not cover it. See [ADR 0006](docs/adr/0006-island-on-top-layer.md) and [ADR 0021](docs/adr/0021-the-osd-returns-to-the-island.md).
- The Launcher closes as soon as an app is pressed, so an app that fails to start says why only on stderr, as the Dock's does.
- The Launcher shows emoji only from a color emoji font that holds them as pictures (CBDT or sbix), like Noto Color Emoji or Twemoji. Kanade's text draws only outlined letters, so it draws the font's picture instead; a COLR or SVG emoji font shows none, though copying still works.
- Without `pactl`, `udevadm` or `dbus-monitor`, the OSD, notification Banners and battery reads poll instead of waiting for the system to announce a change, adding up to about 23 CPU wakeups a second at idle. A backlight changed by firmware without a kernel uevent shows no brightness OSD. See [ADR 0004](docs/adr/0004-wake-sources-on-announcements.md).
- Without `setpriv` from util-linux, helper processes like `pactl`, `udevadm`, `dbus-monitor` and `pw-dump` still work but may survive Kanade being killed abruptly. See [ADR 0004](docs/adr/0004-wake-sources-on-announcements.md).
- Screen recording records video only, the focused output whole, with `wf-recorder`'s `h264_vaapi` encoder; without a working VA-API driver it fails and the island says why. niri hands out a frame only when the screen changes, so a still screen records few frames. See [ADR 0013](docs/adr/0013-screen-recording-with-wf-recorder.md).
- Locking before sleep waits until niri says it holds the lock, which it says once every monitor on shows a lock screen; if it does not by logind's `InhibitDelayMaxSec`, sleep comes anyway and the lock shows on resume.
- An unlock sent just before sleep is asked over: the session unlocks for a moment, then locks again before sleep is let go, unless that outlasts logind's `InhibitDelayMaxSec`. Kanade sends the unlock under the same lock as sleep's gate, so only an unlock already sent races it. See [ADR 0025](docs/adr/0025-the-lock-screen-over-its-own-connection.md).
- Caffeine's duration does not count time the machine is suspended. An idle daemon sees it only if it honors logind idle inhibitors; hypridle does, but one started while caffeine is already on misses it until it changes.
- The clipboard history leaves out only a copy its app marks sensitive with `x-kde-passwordManagerHint`, as `wl-copy --sensitive` does. A password copied without it is kept like any other copy until removed or cleared. See [ADR 0019](docs/adr/0019-clipboard-sensitive-content.md).
- The privacy cluster's microphone and camera see only capture that goes through PipeWire, using `pw-dump` from the `pipewire` package. An app that opens `/dev/video*` or ALSA directly, like `ffmpeg -f v4l2`, does not show. See [ADR 0003](docs/adr/0003-privacy-from-pipewire-graph.md).

## Acknowledgements

- [Amane](https://github.com/MystiaFin/amane) by MystiaFin: Kanade's runtime is ported from it (MIT, see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) and [ADR 0028](docs/adr/0028-the-runtime-is-ported-from-amane.md)).
- [niri](https://github.com/niri-wm/niri): the compositor Kanade is made for, and its IPC.
- [Suzuha](https://github.com/MystiaFin/suzuha) by MystiaFin: panels drawn as one liquid shape on one surface.
- [iNiR](https://github.com/snowarch/iNiR): an island on niri, one body in many contexts, satellites.
- [Noctalia](https://github.com/noctalia-dev/noctalia): a native shell's config and architecture.
- [Wayle](https://github.com/wayle-rs/wayle): a Rust Wayland shell with its settings, OSD and device controls built in.
- [HyprGlass](https://github.com/hyprnux/hyprglass) and [Liquid Glass Studio](https://github.com/iyinchao/liquid-glass-studio): references for liquid glass: refraction at the rim, chromatic aberration, highlights.

Besides Amane, they are references: Kanade holds none of their code.
