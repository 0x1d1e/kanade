# Configuration

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

