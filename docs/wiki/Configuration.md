# Configuration

Kanade reads its config at start, and again whenever one of its files changes, in layers, each over the ones before it:

1. the defaults below
2. every `*.toml` in `$XDG_CONFIG_HOME/kanade/` (else `~/.config/kanade/`), in alphabetical order, so `config.toml` can sit beside files like `10-theme.toml` that another tool manages. Hidden files are skipped.
3. `$XDG_STATE_HOME/kanade/settings.toml` (else `~/.local/state/kanade/settings.toml`), which only the Settings window (`kanade settings open`) and `kanade module` write. It holds just what it changed from the layers below: a value set back to theirs removes the key

Tables merge key by key, so a later file only changes the keys it sets; any other value, a list too, replaces the one below. Kanade never writes your files. Every key is optional; these are the defaults, which `kanade config defaults` prints:

```toml
# the island snaps to its new shape and fades its content; the fade is 80 ms at 100% animation speed; KANADE_REDUCED_MOTION=1 or 0 overrides it
reduced_motion = false

# the time an idle island shows: "24h" (14:05) or "12h" (2:05 PM), per output
clock = "24h"

[timings]
# pointer resting on a Compact island before it peeks, ms 1-60000
hover = 120
# morph to a larger form, ms 1-60000
expand = 144
# one Surface replacing another, and a new track dissolving in, ms 1-60000
surface_change = 176
# morph to a smaller form, ms 1-60000
collapse = 144
# pointer out before a Peek or Surface collapses, ms 1-60000
grace = 250
# the OSD, and a workspace switch on the island, ms 1-60000
osd = 1200

[theme]
# accepted from older config files; no longer changes the shell theme
# palette = "~/Pictures/wallpaper.jpg"

[shell]
# separate keeps the Island and Dock in their own positions; combined puts both in one shell, with apps on either side
mode = "separate"
# which top or bottom edge the combined shell hangs from; stored while Separate is active
edge = "top"
# where along that edge the combined shell sits; stored while Separate is active
align = "center"

[appearance]
# selects light or dark for Settings and apps; the Island, Dock and Banners stay black. Matugen derives both palettes from the wallpaper awww shows at startup or Kanade sets
tone = "dark"
# sets the desktop color preference and paired GTK theme; writes the wallpaper colors for GTK, qt5ct/qt6ct, KDE apps, niri, kitty and Pywalfox. Kvantum gives way to Darkly or Fusion; GTK and Qt apps need restart. Other apps are unchanged
sync = false
# how the Island and Dock settle: "bouncy" like macOS, "smooth" with no overshoot, "snappy", or "playful"
motion = "bouncy"
# installed family for shell labels; unset uses Inter, then sans-serif. A family without a static SemiBold face renders regular weights, takes a restart
# font = "Inter"

[animation]
# 100% is unchanged; higher values are faster. Interaction delays and countdowns do not change, % 25-400
speed = 100

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
# every Module is on unless set to false here; idle is off unless set to true, takes a restart
idle = false

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

[battery]
# at or below, a draining battery shows on the island, % 5-50
low = 20
# at or below, the battery interrupts and battery.critical_action counts down; one above battery.low counts as battery.low, % 2-30
critical = 10
# what a draining critical battery does after a 60 s countdown that Cancel or a charger stops: "none", "suspend", "hibernate" or "poweroff"
critical_action = "none"

[media]
# what moves beside a playing track
visualizer = "bars"

[timer.pomodoro]
# how long a pomodoro's focus lasts, unless `kanade timer pomodoro` says, min 1-180
work = 25
# how long the break after a focus lasts, min 1-180
break = 5
# how long the break after the last round's focus lasts, min 1-180
long_break = 15
# how many focus rounds come before a long break, rounds 1-12
rounds = 4

[live]
# how long a live Activity from `kanade activity` shows without a word from its program, min 1-1440
stale = 30

[osd]
# a keyboard layout switch shows as an OSD, with two or more layouts
layout = true

[notifications]
# per app: the first rule whose app, desktop_entry, category and summary piece all match, ignoring case, sets banner, history, bypass_dnd, timeout (ms) and urgency; file only
# rules = [{ app = "Slack", banner = false }, { bypass_dnd = true, summary = "battery" }]

[wallpaper]
# the images the Launcher offers after `@` and the Wallpapers Surface shows, else ~/Pictures/Wallpapers
# directory = "~/Pictures/Wallpapers"
# how awww moves to a new wallpaper: "simple" fades at its own pace, "fade" fades, "wipe" sweeps across, "grow" opens a circle and "outer" closes one, from the tile chosen on the Wallpapers Surface, else the middle
transition = "simple"
# how long a transition other than "simple" takes, ms 1-60000
transition_duration = 1000

[calendar]
# iCalendar (.ics) files and directories of them, read recursively, else $XDG_DATA_HOME/calendars
# paths = ["~/.local/share/calendars", "~/Documents/holidays.ics"]
# minutes before a timed event with no reminders of its own to remind of it, 0 for none, min 0-10080
reminder = 0

[weather]
# where the weather is for, as [latitude, longitude] in degrees, north and east positive; unset fetches nothing
# location = [52.52, 13.41]
# the location's name, which the Weather Surface shows
# place = "Berlin"
# °C and km/h, or °F and mph
units = "metric"

[night-light]
# warms the screen through wlsunset: "off", "on" all day, or on a "schedule" from sunset to sunrise
mode = "off"
# the screen's color temperature while warm; lower is warmer, 6500 is neutral, K 1000-6500
temperature = 4000
# when the schedule warms the screen, as "HH:MM"; with night-light.sunrise it is the schedule, else the sun at weather.location is
# sunset = "20:00"
# when the schedule turns the screen neutral again, as "HH:MM"
# sunrise = "07:00"

[lock]
# what the lock screen shows behind the clock: the wallpaper awww shows on that output "blurred", sharp as "wallpaper", "dimmed", or "solid"; solid also stands in for a wallpaper that is not png, jpeg, webp, gif or svg
backdrop = "blurred"

[idle]
# seconds idle before the screens fade toward dark, until the lock; any input brings them back; 0 is never, s 0-86400
dim = 270
# seconds idle before the session locks, as `kanade lock` does; 0 is never, s 0-86400
lock = 300
# seconds idle before niri powers the monitors off, back on at any input; never before the lock holds; 0 is never, s 0-86400
screen_off = 330
# seconds idle before the machine suspends, through logind; never before the lock holds; 0 is never, s 0-86400
suspend = 0

[output."eDP-1"]
# for the output of this name only, over the keys above from any file
# clock = "24h"
# rest.clock = true
# rest.battery = false
# rest.peek.battery = true
# rest.peek.weather = true
# rest.peek.agenda = true
```

- `theme.palette`: retained so older config files still load, but ignored. Media may still use cover art for its track accent.
- At startup, Kanade reads the wallpaper awww shows. Matugen derives Kanade's light and dark semantic palettes from it and renders the targets configured in `~/.config/matugen/config.toml` (or its XDG equivalent). A wallpaper set through Kanade, or with `awww img` outside it, replaces the palette; changing `appearance.tone` selects the matching palette and rerenders those targets. Matugen is optional; without it or on failure, Kanade uses built-in colors. Apps without configured Matugen targets are not themed by this integration. Kanade removes configured hooks and disables Matugen wallpaper setting for this run.
- `appearance.sync`: when enabled, Kanade sets the desktop color preference, pairs GTK3 themes and legacy preferences, and, with Matugen, writes the wallpaper colors as its own targets: `kanade.css` imported from `gtk-3.0/gtk.css` and `gtk-4.0/gtk.css`, `qt5ct`/`qt6ct` `colors/kanade.conf` selected in existing qt5ct/qt6ct configs, and a `Kanade` KDE color scheme selected in `kdeglobals` (`[UiSettings] ColorScheme`), without which KDE apps such as Dolphin draw Breeze over the qt6ct palette. niri's focus ring, border, tab indicator and insert hint take the scheme through `niri/kanade.kdl`, included by the last line of `config.kdl`, and change live; kitty's colors, with ANSI colors toned to the wallpaper, through `kitty/kanade.conf`, included by the last line of `kitty.conf`, and running kittys reload it. Either is skipped when its config file does not exist. Running GTK, Qt and KDE apps keep their colors until restarted. Qt apps need `QT_QPA_PLATFORMTHEME=qt6ct` (or `qt5ct`); a kvantum style, which ignores the palette, gives way to Darkly on Qt6 when installed, else Fusion, and comes back when sync is off. For Firefox and Zen, Kanade becomes the [Pywalfox](https://addons.mozilla.org/firefox/addon/pywalfox/) extension's native host, in place of the `pywalfox` Python one while sync is on: install the extension and press Fetch Pywal colors once, and the browser follows each change live. Zen follows light and dark through the desktop preference; a Zen profile that ran an older Kanade needs one restart. Kanade restores what it changed when sync is turned off; unsupported apps are unchanged. This is separate from Matugen's configured output templates.
- `shell.mode`: `separate` keeps the Island and Dock at their saved independent positions. `combined` joins them at `shell.edge` and `shell.align`, without changing those saved positions or `dock.merge`.
- `animation.speed`: scales transition durations from 25% (slower) to 400% (faster); 100% leaves them unchanged. Hover, leave, OSD and countdown durations are not scaled.
- `notifications.rules`: what a notification does, by app, as `[[notifications.rules]]` tables, set in config files only (Settings does not show them). Each matches by any of `app` (the sender's app name, whole), `desktop_entry` (the `.desktop` it names, the `.desktop` optional), `category` (like `"im.received"`) and `summary` (a piece of the title), ignoring case; every one given must match, at least one is needed, and the first rule that matches wins. Its effects, each optional: `banner = false` shows no Banner; `history = false` keeps it out of the Notifications Surface and closes it once its Banner is gone, or at once with no Banner; `bypass_dnd = true` shows its Banner under Do Not Disturb; `timeout` is how long its Banner shows, in ms, a Critical one too; `urgency` is `"low"`, `"normal"` or `"critical"` over the sender's. A rule applies as its notification arrives; a change applies to the next ones. A later file's list replaces the one below, and a bad rule keeps that list, naming the rule by number.
- `osd.layout`: a switch between two or more keyboard layouts in niri shows as an OSD: a lamp with the layout's short label, like `US`, read from XKB's `evdev.lst` (else the name's first letters), and its full name, like `English (US)`. Starting Kanade or changing niri's layouts in its config shows nothing.
- `windows.apps`: which `.desktop` file a window's app id belongs to, for an app Kanade matches wrongly or not at all (see `kanade status`). Kanade otherwise matches the file id, then `StartupWMClass`, then either ignoring case, then the file id's last part (ADR 0014).
- `live.stale`: how many minutes a live Activity from `kanade activity` stays on the Island after its program last said anything of it; then it is withdrawn, as for a script that died before `end`. `kanade activity run` keeps its own alive while its command runs. A changed value applies from the next `kanade activity` call or withdrawal.
- `wallpaper.directory`: the folder of images the Launcher lists after `@`, the Wallpapers Surface shows and `kanade wallpaper shuffle` picks from, not its subfolders.
- `wallpaper.transition` and `wallpaper.transition_duration`: how awww moves to a wallpaper Kanade sets. "grow" and "outer" start from the picture pressed on the Wallpapers Surface, else the middle of the output; "simple" ignores the length.
- `calendar.paths`: the local calendars, as `.ics` files or directories of them (a vdir, as vdirsyncer writes), read recursively and followed as they change. These are read only; the calendar fetches nothing (ADR 0015), and a Google account syncs through `google-calendar` instead. Events added on the Calendar Surface are kept apart, in `$XDG_DATA_HOME/kanade/events`, one `.ics` file each, always read, and the only ones the Surface changes or removes (ADR 0045).
- `calendar.reminder`: a reminder for each timed event that has none of its own, this many minutes before it starts; `0`, the default, gives none. An event's own reminders (its `VALARM`s that display or play a sound) always go off, and a change to this key never sets off again a reminder that already went off (ADR 0043).
- `weather.location`: where the weather is for. Kanade asks [Open-Meteo](https://open-meteo.com/) for it, sending only these coordinates, at start, every 30 minutes, on `kanade weather refresh`, when this key changes, and when the Weather Surface opens on a forecast 30 minutes old; a failed fetch tries again after a minute, doubling up to 30. Unset, nothing is fetched (ADR 0017). Weather data by Open-Meteo.com, under CC BY 4.0.
- `night-light.mode`: `on` warms the screen all day, `schedule` from `night-light.sunset` to `night-light.sunrise` when both are set, else from sunset to sunrise at `weather.location`, which the `weather` Module need not be on for; a schedule with neither runs nothing and says so. The Controls Night Light switch and `kanade night-light on|off` write this key to `settings.toml`; turning it on restores the mode your config file gives, else `schedule` when both times are set, else `on`. Kanade runs `wlsunset` while it is not off and restarts it when these keys change; one that ends on its own runs again after 1 s, doubling to 60 s, and its failure shows once on the island (ADR 0046).
- `battery.low`, `battery.critical`, `battery.critical_action`: a draining battery shows on the island at or below `low` %, and at or below `critical` % turns red and interrupts; both stay until a charger goes in. At critical, an action other than `none` counts down 60 s on the island, with Cancel and Now, then asks logind to suspend, hibernate or power off; plugging in cancels it, and after Cancel it does not count down again until a charger has gone in. It needs the `session` Module. Hibernation needs swap and a resume setup logind accepts; a refusal shows on the island.
- `dock.pinned`: the apps the Dock keeps, as `.desktop` file ids (the `.desktop` optional), left to right. A later file's list replaces the one below. A pinned id no `.desktop` file has, as of an app uninstalled or renamed, shows as a `?` the Dock's right-click menu unpins, and `kanade status` names it.
- `output."<name>"`: a key marked per output, for the monitor niri names so (`niri msg outputs`) only. It wins over the same key outside it in any file, and a later file's value for that output replaces an earlier one's. A key not marked per output is skipped there, with a warning.
- `KANADE_REDUCED_MOTION`: overrides `reduced_motion`. `1` turns it on, `0` off.
- `modules`: turns a feature off. An off Module starts no thread or helper process, posts nothing and answers its verbs with `module <name> is off`. Turning one on or off takes a restart. The Modules are `island` (the island itself, cannot be turned off; a held Backspace or arrow repeats on it as the compositor repeats keys, read from the keyboards, masked in the kernel to those keys, which needs the `input` group, while letters type once, as on macOS; a key remapped to Backspace, like `caps:backspace`, does not repeat), `workspace`, `windows` (the running apps for the Dock, read from niri; `kanade status` names the app ids no `.desktop` file matched), `dock` (where `dock.edge` and `dock.align` put it on each monitor, the pinned apps, then the running ones not pinned, a dot between each running one and the edge; a click launches an app without windows as its `.desktop` file says, focuses its window, or the next one if it is focused already; an app with `Terminal=true` needs `xdg-terminal-exec`; requires `windows`), `privacy` (microphone, camera and screen cast; turning it off is warned about at every start), `battery` (the low and critical battery, `battery.critical_action` through `session`, and the charge limit through UPower, its Controls switch and `kanade battery limit`), `media`, `timer` (a countdown or a pomodoro on the island, see `kanade timer`), `live` (live Activities other programs show with `kanade activity`; with it off, those verbs are refused and `activity run` runs its command without one), `audio` (volume and mute; with it off, nothing reads them), `brightness` (the backlight; likewise), `osd` (volume, brightness and microphone mute, the keyboard's backlight as UPower announces it, its firmware key included, Caps Lock, Num Lock, airplane mode and a keyboard layout switch (while `osd.layout` is on) on the focused output's island, over any Activity, over an open Surface as a capsule, and over fullscreen windows while it shows, and nothing while another session has the seat; shows what of `audio` and `brightness` is on. A level key shows the level even at its limit: Kanade reads the keyboards, masked in the kernel to the level keys and lock lights, which needs the `input` group; airplane mode needs the `rfkill` group; `kanade doctor` says which is missing), `notifications` (with it off, Kanade is not the notification daemon and Do Not Disturb is unavailable), `banners` (notification cards on the focused output, coming out of the island and hanging from it, past a Dock beside it; requires `notifications`), `network`, `bluetooth` and `power` (the Controls tiles), `tray` (Kanade hosts the apps' tray items, and is their StatusNotifierWatcher unless another program already is; hovering the island at rest shows them, and the Tray Surface lists them with their menus), `clipboard` (a history of copied text and images through `wl-paste` and `wl-copy`, kept only in memory and never logged; a copy its app marks sensitive, like a password manager's, is never read), `capture` (screenshots through niri; each one saved, by Kanade or by niri's own binds, shows on the island for 10 s, where Copy path needs `wl-copy` and Open needs `xdg-open`; a recording shows with Stop while it runs, and as a screenshot does once saved; recording needs `wf-recorder`), `lock` (the lock screen, over the wallpaper `awww query` says each output shows, solid without awww; at start it locks again a session logind still counts as locked, after a crash; before the machine sleeps it locks, holding sleep back through a `systemd-inhibit` delay inhibitor until niri holds the lock, at most logind's `InhibitDelayMaxSec`, 5 s by default, and until it wakes no password typed is checked, staying in the field to send again, while one being checked as sleep comes is dropped and typed again (one already accepted may still unlock for a moment before the lock is asked again); a system bus lost is connected to again, and a gate shut stays shut until logind says the machine woke; an inhibitor lost is taken again, and `kanade status` says how it stands; without `systemd-inhibit` or `setpriv`, sleep may come before the lock; the password goes only to PAM, and is never written to disk, logged or shown), `session` (the Session Surface and `kanade session`; Lock shows only while `lock` is on), `idle` (off unless set to true, so an idle daemon like hypridle keeps working: after `idle.dim` seconds idle a click-through layer fades every output toward dark until `idle.lock`, which locks as `kanade lock` does, then `idle.screen_off` has niri power the monitors off and `idle.suspend` asks logind to suspend; any input takes the dim away at once. The stages follow niri's idle, which an app's Wayland idle inhibitor holds, and a logind idle inhibitor or caffeine holds them too. A lock that fails is asked again every 10 s while idle, and with a lock stage the monitors stay on and the machine awake until it holds; the lock never waits on the dim. Reduced motion locks with no fade. Locking needs `lock` and suspending `session`; `kanade doctor` warns when hypridle or swayidle runs beside it), `caffeine` (holds a logind idle inhibitor through `systemd-inhibit` while on, shown on the island with Turn off; without `setpriv` it refuses to turn on), `night-light` (warms the screen through `wlsunset` as `night-light.mode` says; without `wlsunset` or `setpriv` its Controls switch is unavailable), `wallpaper` (sets the wallpaper through awww, from `kanade wallpaper set` or the Launcher after `@`; runs `awww-daemon` while Kanade runs, unless one runs already; without `setpriv` it runs none; an `awww img` that takes over 10 s is stopped), `calendar` (local iCalendar files read from `calendar.paths`, followed as they change; nothing is fetched), `google-calendar` (a Google account's calendars, synced every 15 minutes into files `calendar` reads; idle until signed in; requires `calendar`), `calendar-reminders` (an event's reminders on the island, from its `VALARM`s or `calendar.reminder`, each counting down to the event with Open and Dismiss; which went off is kept in `$XDG_STATE_HOME/kanade/reminders`, so a restart never sets one off again; requires `calendar`), `weather` (the weather for `weather.location` from Open-Meteo; with no location, or off, nothing is fetched), `settings` (the Settings window: task-based pages with search, Inspect and Undo; changes apply live unless marked Restart), and the island's Surfaces: `controls` (with it off, a click at Rest does nothing), `launcher` (an app starts as its `.desktop` file says, as from the Dock, so one with `Terminal=true` needs `xdg-terminal-exec`; copying a sum's value or an emoji needs `wl-copy`; `kanade dmenu` is refused with it off), `notification-surface` (the notification list; requires `notifications`, and with it off a click on a notification opens Controls), `clipboard-surface` (the clipboard history; requires `clipboard`), `calendar-surface` (a month and the agenda of a day; requires `calendar`), `weather-surface` (the weather now and the next days; requires `weather`), `wallpaper-surface` (the wallpaper folder as a grid of pictures, one pressed to set it; requires `wallpaper`) and `keys-surface` (niri's keybinds, read from `$NIRI_CONFIG`, else `niri/config.kdl` in the config directory, and the files it includes, then the keys Kanade's Surfaces take). An off Surface never opens. A Module whose requirement is off turns off too, and stderr names why.
- `schema_version`: the config layout a file is written in, per file, `1` without one. Version 2 drops `timings.toast`; version 3 drops `privacy.style`. When a Kanade release changes the layout, it migrates older files in memory as it reads them and leaves them as they are on disk. A file with a version newer than this Kanade reads, or one that is not a version, is skipped whole, so a downgrade never applies settings it cannot read.
- Problems are reported on stderr with file and line. At start, a key that is unknown or a value out of range is skipped, keeping what the layers below gave it, and a file that is not valid TOML, like one that sets a key twice, is skipped whole.
- A change while running applies without a restart, except `modules`, which stays as it started and is reported pending restart. A change with any problem applies nothing: the config in effect stays whole, and stderr and `status` report why.

