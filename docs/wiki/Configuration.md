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
# selects light or dark for Settings and apps; the Island, Dock and Banners follow the finish. Matugen derives both palettes from the wallpaper awww shows at startup or Kanade sets
tone = "dark"
# what the Island, Dock and Banners are made of, tinted toward the wallpaper's hue in each: "tinted-solid", dark with light text, "paper", light with dark ink, "liquid-glass", clear glass over the screen, its edge reflecting what lies around, or "blur", the tinted solid translucent over what the compositor blurs behind it, paper while `appearance.tone` is "light" (needs niri 26.04)
finish = "tinted-solid"
# Liquid Glass only: bend what lies under its edge, taking the shell's own drawing out of the copy of the screen, where that can be told apart, else the edge reflects what lies around as before; costs a copy of each frame of the window
refraction = false
# sets the desktop color preference and paired GTK theme; writes the wallpaper colors for GTK, qt5ct/qt6ct, KDE apps, niri, kitty and Pywalfox. Kvantum gives way to Darkly or Fusion; GTK and Qt apps need restart. Other apps are unchanged
sync = false
# follow the desktop's reduced motion and higher contrast preferences (the settings portal): the first turns reduced_motion on, the second makes the Island, Dock and Banners a solid finish, with nothing written to your files; reduced_motion = true or KANADE_REDUCED_MOTION=1 win over a desktop that does not ask, KANADE_REDUCED_MOTION=0 over one that does; turning it on takes a restart
accessibility = true
# how the Island and Dock settle: "bouncy" like macOS, "smooth" with no overshoot, "snappy", or "playful"
motion = "bouncy"
# installed family for shell labels; unset uses Inter, then sans-serif. A family without a static SemiBold face renders regular weights, takes a restart
# font = "Inter"
# the size of everything Kanade draws, on top of each output's own scale: 100% is unchanged, 125% a quarter larger. Windows already open keep their size; the lock screen is not scaled, % 80-200
scale = 100

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
# the wheel over the Island sets the volume while it shows Media or the volume; over anything else it does nothing
wheel = true
# a Bluetooth device connecting or going, the charger plugged in or out, a new sound output and a Wi-Fi network joined show on the Island for a moment, each while its Module is on
device_events = true

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
# dots at the Island's trailing end while a microphone, camera or screen cast is in use, and the apps named in Controls, with whether a cast captures a window or a screen; not shown over a fullscreen window
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
# the current line of a playing track's synced lyrics on the Media Surface, from a .lrc file beside a local track, or also from LRCLIB, which is told the track's title, artist, album and length
lyrics = "off"

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

[brightness]
# what sets the brightness: "backlight", an external monitor over "ddcci" (DDC/CI, needs the i2c-dev module and access to /dev/i2c-*), or "auto" for the backlight, else a monitor, takes a restart
backend = "auto"

[osd]
# a keyboard layout switch shows as an OSD, with two or more layouts
layout = true

[notifications]
# a notification arriving plays the sound it names, else a default, through pw-play or canberra-gtk-play; never under Do Not Disturb
sound = false
# per app: the first rule whose app, desktop_entry, category and summary piece all match, ignoring case, sets banner, history, sound, bypass_dnd, timeout (ms) and urgency; file only
# rules = [{ app = "Slack", banner = false }, { bypass_dnd = true, summary = "battery" }]

[controls]
# the switches the Controls Surface shows, in order, each "name" or "name:size" (wifi, bluetooth, caffeine, dnd, night-light, focus, charge-limit, power-profile; half, full or icon, where wifi and bluetooth have no icon); at most three rows, a tile this machine cannot use, like the charge limit without a battery that has one, is left out; the pencil in Controls edits them
tiles = ["wifi", "bluetooth", "caffeine", "dnd", "night-light", "focus"]

[wallpaper]
# the images the Wallpapers Surface shows, else ~/Pictures/Wallpapers
# directory = "~/Pictures/Wallpapers"
# on, a wallpaper picked in the shell (the Wallpapers Surface, `kanade wallpaper shuffle`) is set on that monitor alone; off, on every monitor. `kanade wallpaper set --output` names one either way
per_monitor = true
# how awww moves to a new wallpaper: "simple" fades at its own pace, "fade" fades, "wipe" sweeps across, "grow" opens a circle and "outer" closes one, from the tile chosen on the Wallpapers Surface, else the middle, and "random" lets awww pick one each time
transition = "simple"
# how long a transition other than "simple" takes, ms 1-60000
transition_duration = 1000

[launcher]
# an app's own actions, like a browser's New Private Window, are rows of the Launcher once two letters are typed
actions = true

[calendar]
# iCalendar (.ics) files and directories of them, read recursively, else $XDG_DATA_HOME/calendars
# paths = ["~/.local/share/calendars", "~/Documents/holidays.ics"]
# minutes before a timed event with no reminders of its own to remind of it, 0 for none, min 0-10080
reminder = 0
# https:// (or webcal://) addresses of public iCalendar feeds, fetched into read-only calendars; an address may hold a secret, so none is logged or shown
# subscriptions = ["https://example.org/holidays.ics", "webcal://example.org/team.ics"]
# minutes between fetches of each subscription, at least 30, min 30-1440
subscription_interval = 60

[weather]
# where the weather is for, as [latitude, longitude] in degrees, north and east positive; unset fetches nothing
# location = [52.52, 13.41]
# the location's name, which the Weather Surface shows
# place = "Berlin"
# °C and km/h, or °F and mph
units = "metric"

[gestures]
# gesture name = the Kanade verb it runs, like "island collapse"; record a gesture with `kanade gesture record <name>`; never a program, which a hook or a verb of yours runs; file only
# check = "island collapse"
# circle = "launcher open"

[night-light]
# warms the screen through wlsunset: "off", "on" all day, or on a "schedule" from sunset to sunrise
mode = "off"
# the screen's color temperature while warm; lower is warmer, 6500 is neutral, K 1000-6500
temperature = 4000
# when the schedule warms the screen, as "HH:MM"; with night-light.sunrise it is the schedule, else the sun at weather.location is
# sunset = "20:00"
# when the schedule turns the screen neutral again, as "HH:MM"
# sunrise = "07:00"

[focus]
# Focus turns Do Not Disturb on, and off again if it was off before
dnd = true
# Focus keeps the session awake, and releases it again if it was not held before
caffeine = true
# Focus warms the screen, and turns the night light off again if it was off before
night_light = false
# what turns Focus on by itself, and off again when it ends, only if nobody turned Focus on or off meanwhile: "cast" is a screen cast running (needs the privacy Module); empty is never
# auto = ["cast"]

[lock]
# what the lock screen shows behind the clock: the wallpaper awww shows on that output "blurred", sharp as "wallpaper", "dimmed", or "solid"; solid also stands in for a wallpaper that is not png, jpeg, webp, gif or svg
backdrop = "blurred"
# the PAM service that checks the password, a file of that name in /etc/pam.d; unset uses login. Letters, digits, dots, dashes and underscores. Only the first hidden prompt gets the password; an echoed or later one fails the check, and PAM's own messages show under the field. A name with no file refuses every lock
# pam_service = "login"
# seconds after the idle Module locks the session in which any key, click, scroll or pointer movement unlocks it without a password; never for `kanade lock`, a lock before sleep or a lock asked again after a failure; 0 is none, s 0-30
grace = 0

[idle]
# seconds idle before the screens fade toward dark, until the lock; any input brings them back; 0 is never, s 0-86400
dim = 270
# seconds idle before the session locks, as `kanade lock` does; 0 is never, s 0-86400
lock = 300
# seconds idle before niri powers the monitors off, back on at any input; never before the lock holds; 0 is never, s 0-86400
screen_off = 330
# seconds idle before the machine suspends, through logind; never before the lock holds; 0 is never, s 0-86400
suspend = 0

[settings]
# tell once, at the first start with checks that need attention, to open Settings' Health page
health_notice = true

[hooks]
# a command run, as a list of strings with no shell, when niri holds the lock; the program is looked up on PATH, a leading ~/ in it is your home, and KANADE_HOOK names the event
# lock = ["notify-send", "Kanade", "hook"]
# a command run when the lock ends
# unlock = ["notify-send", "Kanade", "hook"]
# a command run when a wallpaper is set or shown, its path in KANADE_WALLPAPER
# wallpaper = ["notify-send", "Kanade", "hook"]
# a command run when appearance.tone changes, "dark" or "light" in KANADE_TONE
# tone = ["notify-send", "Kanade", "hook"]
# a command run when the charger goes in or out, "plugged" or "unplugged" in KANADE_CHARGER
# charger = ["notify-send", "Kanade", "hook"]
# a command run as the machine goes to sleep, before the lock is asked; sleep waits for it only until logind's delay ends
# suspend = ["notify-send", "Kanade", "hook"]
# a command run when the machine wakes, or a sleep failed
# resume = ["notify-send", "Kanade", "hook"]
# a command run when the shell has started, once every Module that is on has
# started = ["notify-send", "Kanade", "hook"]
# a command run when Do Not Disturb goes on or off, "on" or "off" in KANADE_DND
# dnd = ["notify-send", "Kanade", "hook"]
# a command run when the power profile changes, its name (power-saver, balanced or performance) in KANADE_POWER_PROFILE; not the one at start
# power_profile = ["notify-send", "Kanade", "hook"]
# a command run before logind is asked to end the session, after the countdown or at once; the log out waits for it up to hooks.timeout
# logout = ["notify-send", "Kanade", "hook"]
# a command run before logind is asked to restart, after the countdown or at once; the restart waits for it up to hooks.timeout
# reboot = ["notify-send", "Kanade", "hook"]
# a command run before logind is asked to power off, after the countdown or at once; the power off waits for it up to hooks.timeout
# poweroff = ["notify-send", "Kanade", "hook"]
# a command still running after this is killed; one run of an event at a time, a fire meanwhile is dropped, s 1-300
timeout = 10

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
- `appearance.accessibility`: follows what the desktop's settings portal (`org.freedesktop.portal.Settings`, namespace `org.freedesktop.appearance`) says. `reduced-motion` = 1 acts as `reduced_motion = true`; `contrast` = 1 turns `liquid-glass` and `blur` into the solid finish of `appearance.tone` (`paper` for light, `tinted-solid` for dark), since a pane that shows what lies behind it cannot promise its ink's contrast, and leaves the two solid finishes, whose ink holds 7:1, as they are. Precedence for motion, highest first: `KANADE_REDUCED_MOTION`, `reduced_motion = true`, the portal, off. A file's `reduced_motion = false` does not override a desktop that asks; set `appearance.accessibility = false` for that. Nothing is written to your files, Settings shows the values your files give, and a change in the desktop applies live. Kanade follows the portal only if this is on at start; without a portal, nothing changes. See [ADR 0080](../adr/0080-desktop-accessibility-from-the-settings-portal.md).
- `appearance.scale`: scales everything Kanade draws (text, icons, bodies, the Dock, Banners, Settings) by 80% to 200%, on top of each output's own scale, so 125% on a 1.5 output draws at 1.875. It is a whole percent, like `animation.speed`. It applies live on a config reload: the Island, Dock and Banners resize at once, every pointer and input region with them. A window already open (Settings) keeps its size and its content scales inside it. The lock screen is not scaled. The Island's canvas still never exceeds the output. See [ADR 0091](../adr/0091-ui-scale-at-the-runtime-seam.md).
- `animation.speed`: scales transition durations from 25% (slower) to 400% (faster); 100% leaves them unchanged. Hover, leave, OSD and countdown durations are not scaled.
- `notifications.sound`: a notification new to the list plays a sound, off by default: the file its sender gives in the `sound-file` hint (an absolute path to an existing file), else its `sound-name` hint (a name of the freedesktop sound theme, like `message-new-instant`), else `message-new-instant`; `suppress-sound` plays none. It plays through `pw-play`, else `canberra-gtk-play` (also for a name `pw-play` finds no file for in the freedesktop theme), which `kanade doctor` says is found or not; Kanade links no audio library. One sound plays at a time (a notification arriving meanwhile is silent) for at most 5 s, started through `setpriv` so it ends with Kanade; a replacement of a notification, like a progress, plays none. Never under Do Not Disturb, a Critical notification included; `sound = false` in a rule silences its notifications. With it off, nothing is started.
- `notifications.rules`: what a notification does, by app, as `[[notifications.rules]]` tables, set in config files only (Settings does not show them). Each matches by any of `app` (the sender's app name, whole), `desktop_entry` (the `.desktop` it names, the `.desktop` optional), `category` (like `"im.received"`) and `summary` (a piece of the title), ignoring case; every one given must match, at least one is needed, and the first rule that matches wins. Its effects, each optional: `banner = false` shows no Banner (nor a progress Activity, below); `history = false` keeps it out of the Notifications Surface and closes it once its Banner is gone, or at once with no Banner; `sound = false` plays no sound for it; `bypass_dnd = true` shows its Banner under Do Not Disturb; `timeout` is how long its Banner shows, in ms, a Critical one too; `urgency` is `"low"`, `"normal"` or `"critical"` over the sender's. A rule applies as its notification arrives; a change applies to the next ones. A later file's list replaces the one below, and a bad rule keeps that list, naming the rule by number.
- Notification progress: a notification whose sender gives the `value` hint (a percent from 0 to 100, as in `notify-send -h int:value:40 ...`) also shows as a live Activity on the Island while the value is under 100, with the notification's summary as its title and its body, else the app, as its line. It ends as done when the value reaches 100, and goes when the notification closes or is replaced without the hint. A sender's `expire_timeout`, when positive, ends it after that long without a new value, but never later than `live.stale`; otherwise `live.stale` does. `value` is a de facto hint, not in the freedesktop specification, which only a few senders use; others never show it. It is one of the 8 live Activities, but at most 3 are a notification's: a fourth takes the place of the oldest, so scripts and agents keep at least 5. It needs the `live` Module on, and shows when a Banner would: a rule with `banner = false` shows none, and Do Not Disturb hides it unless the notification is Critical or its rule says `bypass_dnd`.
- Notification stacks: the Notifications Surface shows an app's notifications as one stack, the newest with a count, once there is more than one. Pressing a stack, or Enter or Space on it, opens it to each notification under a head that closes it; the cross on a stack or head, or Backspace, dismisses them all. Each opening of the Surface starts with every stack closed. The history keeps the 20 newest of each app and 100 in all; an older one closes as expired. Banners are not stacked.
- `hooks.<event>`: a command to run on an event, as a list of strings (a program and its arguments, no shell; `["sh", "-c", "..."]` for one), set in config files only: `kanade config set` and Settings do not touch them. The events are `lock` and `unlock` (the `lock` Module), `suspend` (before sleep, which waits for it up to logind's delay) and `resume` (also after a sleep that did not suspend), `wallpaper` (a wallpaper set through Kanade or shown by awww; not the one at start), `tone` (`appearance.tone` changed), `charger` (plugged or unplugged; not the state at start), `started` (the shell started, once every Module that is on has), `dnd` (Do Not Disturb went on or off), `power_profile` (the power profile changed; not the one at start) and `logout`, `reboot` and `poweroff` (fired just before logind is asked to end the session, restart or power off, after the countdown or at once; the action waits for the command up to `hooks.timeout`, and nothing is prevented or cancelled). The command gets `KANADE_HOOK` (the event) and `KANADE_WALLPAPER` (the path), `KANADE_TONE` (`dark` or `light`), `KANADE_CHARGER` (`plugged` or `unplugged`), `KANADE_DND` (`on` or `off`) or `KANADE_POWER_PROFILE` (`power-saver`, `balanced` or `performance`) as its event says, no stdin, and runs as your user. It is stopped after `hooks.timeout` seconds, though a `sh -c` command's own children may live on (`exec` the last one); one run of an event at a time, a fire meanwhile is dropped; and it dies with Kanade where `setpriv` is found. A failure shows in `kanade status` and once on stderr, never on the island, with the first line of the command's own error, which may name what it was given. A command that leaves a background process behind is not held by it: its run ends with the command. A Module that is off, `hooks` included, fires nothing. See ADR 0055.
- `media.visualizer`: `bars`, `wave` and `dots` are drawn, not heard. `audio` draws four bars that follow what the default sink plays, read through `cava` (install it; `kanade doctor` says when it is missing), which Kanade starts through `setpriv` only while a playing track's visualizer is on screen and stops about 1.5 s after. Cava reads the sink's monitor, never a microphone, so the privacy indicator does not light. Until cava answers, or without it, the synthetic bars draw. Kanade gives cava its own config, so `~/.config/cava/config` never applies; it needs cava built with PulseAudio support, which PipeWire serves through `pipewire-pulse`. See ADR 0056.
- `media.lyrics`: `off` (the default) shows none. `local` reads the `.lrc` file named like a local track and beside it (`song.flac` and `song.lrc`) and sends nothing. `lrclib` does that, then asks [LRCLIB](https://lrclib.net) over https, **sending the playing track's title, artist, album and length** with a `Kanade/<version>` User-Agent and nothing else, only while a track plays and the Media Surface is open. The current line shows under the artist on the Media Surface, in the artwork's accent. Answers are cached under hashed names in `$XDG_CACHE_HOME/kanade/lyrics` (at most 200; a miss is believed for 2 hours); `kanade status` shows how the last look-up ended. See ADR 0063.
- `osd.layout`: a switch between two or more keyboard layouts in niri shows as an OSD: a lamp with the layout's short label, like `US`, read from XKB's `evdev.lst` (else the name's first letters), and its full name, like `English (US)`. Starting Kanade or changing niri's layouts in its config shows nothing.
- `windows.apps`: which `.desktop` file a window's app id belongs to, for an app Kanade matches wrongly or not at all (see `kanade status`). Kanade otherwise matches the file id, then `StartupWMClass`, then either ignoring case, then the file id's last part (ADR 0014).
- `brightness.backend`: `auto` sets the backlight, else the external monitor on the focused output over DDC/CI; `backlight` only the backlight; `ddcci` only monitors. DDC/CI talks to the monitor's I2C bus (`/dev/i2c-N`), so it needs the `i2c-dev` kernel module and write access to the node (the `i2c` group, or a udev rule). A monitor's level is read once and kept, so a change made on its own buttons is not seen, and a laptop's own panel is never written. `kanade brightness set|up|down|status` and the slider act on the monitor under the focused output. Needs a restart. See [ADR 0082](../adr/0082-ddc-ci-brightness-for-external-monitors.md).
- `live.stale`: how many minutes a live Activity from `kanade activity` stays on the Island after its program last said anything of it; then it is withdrawn, as for a script that died before `end`. `kanade activity run` keeps its own alive while its command runs. A changed value applies from the next `kanade activity` call or withdrawal.
- `wallpaper.directory`: the folder of images the Wallpapers Surface shows and `kanade wallpaper shuffle` picks from, not its subfolders. With several monitors, the Wallpapers Surface sets the wallpaper of the monitor it is on and marks what that monitor shows; `kanade wallpaper set` and `shuffle` take `--output`. Which wallpapers show persists in awww's own cache, not in Kanade. The shell colors follow the monitor that was focused at Kanade's last pick, else the focused one.
- `wallpaper.per_monitor`: on (the default), a wallpaper picked on the Wallpapers Surface or shuffled is set on that monitor alone and the palette follows the monitor focused at the pick; off, every pick sets all monitors. `kanade wallpaper set <path>` sets all unless `--output NAME` says one, whatever this is.
- `wallpaper.transition` and `wallpaper.transition_duration`: how awww moves to a wallpaper Kanade sets. "grow" and "outer" start from the picture pressed on the Wallpapers Surface, else the middle of the output, and "random" has awww pick another transition for each wallpaper, a circle among them; "simple" ignores the length.
- `calendar.paths`: the local calendars, as `.ics` files or directories of them (a vdir, as vdirsyncer writes), read recursively and followed as they change. These are read only; the calendar fetches nothing (ADR 0015); a Google account syncs through `google-calendar` and public feeds through `calendar-subscriptions` instead. Events added on the Calendar Surface are kept apart, in `$XDG_DATA_HOME/kanade/events`, one `.ics` file each, always read, and the only ones the Surface changes or removes (ADR 0045).
- `calendar.reminder`: a reminder for each timed event that has none of its own, this many minutes before it starts; `0`, the default, gives none. An event's own reminders (its `VALARM`s that display or play a sound) always go off, and a change to this key never sets off again a reminder that already went off (ADR 0043).
- `calendar.subscriptions`, `calendar.subscription_interval`: public iCalendar feeds, up to 16, as `https://` addresses, `webcal://` read as `https://`; `http://` is refused, and so is a redirect to it. Needs the `calendar-subscriptions` Module (ADR 0062). Each feed is fetched at start, every `subscription_interval` minutes (30 to 1440, 60 by default), on `kanade calendar-subscriptions sync` and when the list changes, asking only for a change (`If-None-Match`, `If-Modified-Since`). An answer over 5 MiB, one that does not start as an iCalendar file or one after 30 s is refused; a feed that fails is tried again after 5 minutes, doubling up to the interval, and keeps its last good copy. Copies are read-only files, named by a hash, under `$XDG_CACHE_HOME/kanade/calendar-subscriptions` (readable by you only), which the calendar reads; turning the Module off stops reading them but leaves the files, and a feed taken out of the list has its file deleted. An address may hold a secret token, so it is in no log, no `kanade status` and no error; a feed is named by its place in the list, and the Calendar Surface says "Subscription offline" or "2 subscriptions failing". A feed is fetched with a User-Agent of `Kanade/<version>` and no cookie or login. The address is in your config file and the Settings window as you wrote it, so keep that file private when it holds a token.
- `weather.location`: where the weather is for. Kanade asks [Open-Meteo](https://open-meteo.com/) for it, sending only these coordinates, at start, every 30 minutes, on `kanade weather refresh`, when this key changes, and when the Weather Surface opens on a forecast 30 minutes old; a failed fetch tries again after a minute, doubling up to 30. Unset, nothing is fetched (ADR 0017). Weather data by Open-Meteo.com, under CC BY 4.0.
- `controls.tiles`: the Controls switches, in order, each `"name"` or `"name:size"`: `wifi`, `bluetooth`, `caffeine`, `dnd`, `night-light`, `focus`, `charge-limit` or `power-profile` (pressed, it takes the next profile the machine has, the Controls profile bar still choosing one; it needs `power`), in `half` (the default), `full` or `icon` (a round button; not for `wifi` and `bluetooth`). Each tile once, at least one, and no more than fit three rows (two halves share a row with one round button, or a `full` tile with up to three; a half beside more would be too narrow to read); a list that needs more is refused whole. A tile this machine cannot use is left out. The pencil in the Controls header arranges them and writes this key; Settings lists them too (ADR 0067).
- `focus.dnd`, `focus.caffeine` and `focus.night_light`: what `kanade focus on` and the Controls Focus button turn on, each only while its Module (`notifications`, `caffeine`, `night-light`) is on; `off` turns off each piece Focus turned on, but only while it is still as Focus left it: a piece that was on already, a caffeine you started or stopped yourself since, or a night light mode you changed since is left as it is. The night light, kept in `settings.toml`, stays on if Kanade restarts while Focus is on; the others end with Kanade or reset.
- `focus.auto`: a list of triggers that turn Focus on by themselves (config file only, empty by default). `"cast"`: a screen cast running turns Focus on, and its end turns it off, but only the Focus the cast turned on: an `on`, `off` or `toggle` of yours meanwhile makes it yours and it stays. A Focus that was on already is left alone. Needs the `privacy` Module, which sees casts; a cast already running when the setting changes is seen from the next change. There is no calendar trigger, since it would need a timer for every event boundary. See [ADR 0078](../adr/0078-focus-turns-on-for-a-screen-cast.md).
- `night-light.mode`: `on` warms the screen all day, `schedule` from `night-light.sunset` to `night-light.sunrise` when both are set, else from sunset to sunrise at `weather.location`, which the `weather` Module need not be on for; a schedule with neither runs nothing and says so. The Controls Night Light switch and `kanade night-light on|off` write this key to `settings.toml`; turning it on restores the mode your config file gives, else `schedule` when both times are set, else `on`. Kanade runs `wlsunset` while it is not off and restarts it when these keys change; one that ends on its own runs again after 1 s, doubling to 60 s, and its failure shows once on the island (ADR 0046).
- `battery.low`, `battery.critical`, `battery.critical_action`: a draining battery shows on the island at or below `low` %, and at or below `critical` % turns red and interrupts; both stay until a charger goes in. At critical, an action other than `none` counts down 60 s on the island, with Cancel and Now, then asks logind to suspend, hibernate or power off; plugging in cancels it, and after Cancel it does not count down again until a charger has gone in. It needs the `session` Module. Hibernation needs swap and a resume setup logind accepts; a refusal shows on the island.
- `dock.pinned`: the apps the Dock keeps, as `.desktop` file ids (the `.desktop` optional), left to right. A later file's list replaces the one below. A pinned id no `.desktop` file has, as of an app uninstalled or renamed, shows as a `?` the Dock's right-click menu unpins, and `kanade status` names it.
- `output."<name>"`: a key marked per output, for the monitor niri names so (`niri msg outputs`) only. It wins over the same key outside it in any file, and a later file's value for that output replaces an earlier one's. A key not marked per output is skipped there, with a warning.
- `KANADE_REDUCED_MOTION`: overrides `reduced_motion` and the desktop's preference (`appearance.accessibility`). `1` turns it on, `0` off.
- `modules`: turns a feature off. An off Module starts no thread or helper process, posts nothing and answers its verbs with `module <name> is off`. Turning one on or off takes a restart. The Modules are `island` (the island itself, cannot be turned off; a held Backspace or arrow repeats on it as the compositor repeats keys, read from the keyboards, masked in the kernel to those keys, which needs the `input` group, while letters type once, as on macOS; a key remapped to Backspace, like `caps:backspace`, does not repeat), `workspace`, `windows` (the running apps for the Dock, read from niri; `kanade status` names the app ids no `.desktop` file matched), `dock` (where `dock.edge` and `dock.align` put it on each monitor, the pinned apps, then the running ones not pinned, a dot between each running one and the edge; a click launches an app without windows as its `.desktop` file says, focuses its window, or the next one if it is focused already; an app with `Terminal=true` needs `xdg-terminal-exec`; requires `windows`), `privacy` (microphone, camera and screen cast; turning it off is warned about at every start), `battery` (the low and critical battery, `battery.critical_action` through `session`, UPower's time left or to full at the end of the Controls header, and the charge limit through UPower, its Controls switch and `kanade battery limit`), `media`, `timer` (a countdown or a pomodoro on the island, see `kanade timer`; the Timer Surface is `timer-surface`), `live` (live Activities other programs show with `kanade activity`; with it off, those verbs are refused and `activity run` runs its command without one), `launcher-entry` (an app's download or copy progress, which it reports on the session bus through the Unity `LauncherEntry` signal, as a live Activity; requires `live`; at most 2 at once, the app named by its `.desktop` entry), `audio` (volume and mute; with it off, nothing reads them), `brightness` (the backlight, or an external monitor over DDC/CI as `brightness.backend` says; likewise), `osd` (volume, brightness and microphone mute, the keyboard's backlight as UPower announces it, its firmware key included, Caps Lock, Num Lock, airplane mode and a keyboard layout switch (while `osd.layout` is on) on the focused output's island, over any Activity, over an open Surface as a capsule, and over fullscreen windows while it shows, and nothing while another session has the seat; shows what of `audio` and `brightness` is on. A level key shows the level even at its limit: Kanade reads the keyboards, masked in the kernel to the level keys and lock lights, which needs the `input` group; airplane mode needs the `rfkill` group; `kanade doctor` says which is missing), `notifications` (with it off, Kanade is not the notification daemon and Do Not Disturb is unavailable), `banners` (notification cards on the focused output, coming out of the island and hanging from it, past a Dock beside it; requires `notifications`), `network`, `bluetooth` and `power` (the Controls tiles), `tray` (Kanade hosts the apps' tray items, and is their StatusNotifierWatcher unless another program already is; hovering the island at rest shows them, and the Tray Surface lists them with their menus), `clipboard` (a history of copied text and images through `wl-paste` and `wl-copy`, kept only in memory and never logged; a copy its app marks sensitive, like a password manager's, is never read), `capture` (screenshots through niri, announced by niri's own notification, not on the island; a recording shows with Stop while it runs and, once saved, for 10 s with Copy path, which needs `wl-copy`, and Open, which needs `xdg-open`; recording needs `wf-recorder`), `lock` (the lock screen, over the wallpaper `awww query` says each output shows, solid without awww; at start it locks again a session logind still counts as locked, after a crash; before the machine sleeps it locks, holding sleep back through a `systemd-inhibit` delay inhibitor until niri holds the lock, at most logind's `InhibitDelayMaxSec`, 5 s by default, and until it wakes no password typed is checked, staying in the field to send again, while one being checked as sleep comes is dropped and typed again (one already accepted may still unlock for a moment before the lock is asked again); a system bus lost is connected to again, and a gate shut stays shut until logind says the machine woke; an inhibitor lost is taken again, and `kanade status` says how it stands; without `systemd-inhibit` or `setpriv`, sleep may come before the lock; the password goes only to PAM, the first hidden prompt of a check, and is never written to disk, logged or shown; PAM's own messages show under the field; with two or more keyboard layouts, the short label of the one in use ("US", from XKB's rules list as the layout OSD reads it) shows at the field's end beside Caps Lock, and is only a hint: a layout niri has not told leaves it out; `lock.pam_service` names another service, and `lock.grace` lets input unlock for a few seconds a lock the `idle` Module asked for, [ADR 0066](../adr/0066-a-short-grace-after-an-idle-lock.md)), `session` (the Session Surface and `kanade session`; Lock shows only while `lock` is on), `idle` (off unless set to true, so an idle daemon like hypridle keeps working: after `idle.dim` seconds idle a click-through layer fades every output toward dark until `idle.lock`, which locks as `kanade lock` does, then `idle.screen_off` has niri power the monitors off and `idle.suspend` asks logind to suspend; any input takes the dim away at once. The stages follow niri's idle, which an app's Wayland idle inhibitor holds, and a logind idle inhibitor or caffeine holds them too. A lock that fails is asked again every 10 s while idle, and with a lock stage the monitors stay on and the machine awake until it holds; the lock never waits on the dim. Reduced motion locks with no fade. Locking needs `lock` and suspending `session`; `kanade doctor` warns when hypridle or swayidle runs beside it), `caffeine` (holds a logind idle inhibitor through `systemd-inhibit` while on, shown on the island with Turn off; without `setpriv` it refuses to turn on), `night-light` (warms the screen through `wlsunset` as `night-light.mode` says; without `wlsunset` or `setpriv` its Controls switch is unavailable), `focus` (`kanade focus` and the Controls Focus tile: Do Not Disturb, caffeine and the night light together as `focus.*` say; works with whichever of `notifications`, `caffeine` and `night-light` are on), `wallpaper` (sets the wallpaper through awww, from `kanade wallpaper set` or the Wallpapers Surface; runs `awww-daemon` while Kanade runs, unless one runs already; without `setpriv` it runs none; an `awww img` that takes over 10 s is stopped), `calendar` (local iCalendar files read from `calendar.paths`, followed as they change; it fetches nothing is fetself), `google-calendar` (a Google account's calendars, synced every 15 minutes into files `calendar` reads; idle until signed in; requires `calendar`), `calendar-subscriptions` (public iCalendar feeds from `calendar.subscriptions`, fetched into read-only files `calendar` reads; nothing is fetched without a feed; requires `calendar`), `calendar-reminders` (an event's reminders on the island, from its `VALARM`s or `calendar.reminder`, each counting down to the event with Open and Dismiss, and Join when the event links to a Google Meet, Zoom, Teams, Webex, Jitsi or Whereby meeting, which needs `xdg-open`; which went off is kept in `$XDG_STATE_HOME/kanade/reminders`, so a restart never sets one off again; requires `calendar`), `weather` (the weather for `weather.location` from Open-Meteo; with no location, or off, nothing is fetched), `hooks` (the commands `hooks.<event>` list, run on lock, unlock, wallpaper, tone, charger, suspend and resume; nothing runs while none is listed), `settings` (the Settings window: task-based pages with search, Inspect and Undo; changes apply live unless marked Restart; its Health page shows `kanade doctor`'s checks, run again when the page opens or Check again is pressed. At the first start, 30 s after it, one notice on the island says how many need attention, and `settings.health_notice = false` skips it; that start is recorded in `$XDG_STATE_HOME/kanade/health-notice`, so it is told once, never again, delete the file to see it again), and the island's Surfaces: `controls` (with it off, a click at Rest does nothing), `launcher` (an app starts as its `.desktop` file says, as from the Dock, so one with `Terminal=true` needs `xdg-terminal-exec`; its own actions are rows too while `launcher.actions` is on; with `windows` on, open windows are found by title and focused; how often you start each app is kept in `$XDG_STATE_HOME/kanade/launcher-usage`, readable by you only; copying a sum's value or an emoji needs `wl-copy`; `kanade dmenu` is refused with it off), `notification-surface` (the notification list; requires `notifications`, and with it off a click on a notification opens Controls), `clipboard-surface` (the clipboard history; requires `clipboard`), `calendar-surface` (a month and the agenda of a day; requires `calendar`), `weather-surface` (the weather now and the next days; requires `weather`), `wallpaper-surface` (the wallpaper folder as a grid of pictures, one pressed to set it; requires `wallpaper`), `timer-surface` (tiles to start a countdown or a pomodoro, then its time left with Pause or Resume, Skip and Cancel; requires `timer`), `capture-surface` (tiles for a screenshot of an area, a window or the screen, and the screen recording with Stop while it runs; requires `capture`) and `keys-surface` (niri's keybinds, read from `$NIRI_CONFIG`, else `niri/config.kdl` in the config directory, and the files it includes, then the keys Kanade's Surfaces take). An off Surface never opens. A Module whose requirement is off turns off too, and stderr names why.
- `schema_version`: the config layout a file is written in, per file, `1` without one. Version 2 drops `timings.toast`; version 3 drops `privacy.style`. When a Kanade release changes the layout, it migrates older files in memory as it reads them and leaves them as they are on disk. A file with a version newer than this Kanade reads, or one that is not a version, is skipped whole, so a downgrade never applies settings it cannot read.
- `kanade config schema` prints a JSON Schema of every key, for an editor to check and complete a file; `kanade config get <key>` prints a key's value in effect and the file it comes from, `kanade config set <key> <value>` and `reset <key>` write the settings file as the Settings window does, and `kanade config validate <file>` checks a file without a shell. See [CLI](CLI.md).
- Problems are reported on stderr with file and line, an unknown key with the nearest known one when it is close. At start, a key that is unknown or a value out of range is skipped, keeping what the layers below gave it, and a file that is not valid TOML, like one that sets a key twice, is skipped whole.
- A change while running applies without a restart, except `modules`, which stays as it started and is reported pending restart. A change with any problem applies nothing: the config in effect stays whole, and stderr and `status` report why.

