# Keyboard

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
Mod+Alt+B hotkey-overlay-title="Island: Wallpapers" { spawn "kanade" "wallpaper" "toggle"; }
Mod+Alt+Slash hotkey-overlay-title="Island: Keys" { spawn "kanade" "keys" "toggle"; }
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

Kanade can lock on idle itself: turn on the `idle` Module (`kanade module enable idle`, then restart Kanade) and stop any idle daemon. After `idle.dim` seconds the screens fade toward dark, and any input brings them back; at `idle.lock` the session locks, then `idle.screen_off` turns the monitors off and `idle.suspend` suspends, each 0 for never, see [Configuration](Configuration.md). A video playing, caffeine or any logind idle inhibitor holds every stage. `kanade idle status` says how it stands, and `kanade doctor` warns when hypridle runs beside it.

With the Module off, as by default, an idle daemon locks the same way as the keybind. For hypridle, in `~/.config/hypr/hypridle.conf`:

```ini
general {
    lock_cmd = kanade lock
}

listener {
    timeout = 300
    on-timeout = loginctl lock-session
}
```

Kanade locks before sleep on its own, so no `before_sleep_cmd` is needed, and caffeine keeps the `idle` Module or hypridle from locking or suspending. An explicit suspend still locks while caffeine is on. `kanade lock` exits 0 only once niri holds the lock for that call and no password typed since was accepted, never for a lock ending right before it, so a suspend hook can wait on it. It exits 3 at once when a password typed on the lock screen unlocks it or is being checked as it is called, and with 1 when niri refuses the lock, as while another locker holds the session. On the lock screen, type the password and press Enter.

An island opened this way takes the keyboard. If nothing on it is used for 5 s and the pointer never comes onto it, it collapses and gives the keyboard back. An island opened with a click gets keys after the click.

| Where | Keys |
|---|---|
| Any Surface | Escape collapses. A pinned island gives the keyboard back, so the `collapse` bind closes it |
| Notifications | arrows and Tab move the ring, Enter or Space presses what it is on, Backspace dismisses the card |
| Launcher | typing searches apps (an empty query lists the last few started first; among alike matches the ones you start most come first), an app's own actions (`private` finds a browser's New Private Window; `launcher.actions`), open windows by title or app (Enter focuses it; needs the `windows` Module), a sum like `2+2*3`, emoji after `:` (`:smile`), or what you copied after `>` (`>invoice`, newest first, with the `clipboard` Module on), or where Kanade goes (`wifi`, `bluetooth`, `notifications`, `controls`, `weather`, `tray`, `session`, `timer`, `capture`, `clipboard`, `calendar`, `wallpapers` or `@`, `keys`, a setting like `magnification` or a page like `dock settings`); Up/Down/Home/End move the selection, Enter starts the app, copies the value or emoji, puts a copied entry back on the clipboard, or opens Controls, another Surface or Settings |
| Clipboard | typing searches, arrows, Tab, Home and End move the ring, Enter copies the entry, deletes it or clears the history, whichever the ring is on; clearing asks a second Enter or click to confirm |
| Wallpapers | typing searches file names, arrows, Tab, Home and End move the ring over the pictures and Shuffle, Enter sets the one the ring is on, or one at random; it stays open to try another |
| Keys | typing searches niri's keybinds and the keys on this page, Up/Down, Tab, Home and End move the ring through them |
| Calendar | Left/Right move a day, Up/Down a week, `n`/`p` a month, Home or `t` go to today; Tab or Enter enters the day's agenda, where Up/Down move through its events; Tab, Enter or Escape leave it; `a` adds an event to the day: type `14:30 Title`, `9:00-10:30 Title` or just a title for all day, Enter saves, Escape drops it; with the ring on an event you added, `e` changes it and `d` twice removes it (others are read-only) |
| Timer, Capture | arrows and Tab move the ring over the tiles (a running timer: Pause or Resume, Skip, Cancel), Enter or Space presses what it is on; a screenshot or recording starts as the island closes |
| Session | arrows and Tab move the ring, Enter or Space presses what it is on; a restart, power off or log out keeps it open on its countdown, the ring on Cancel |
| Settings | Tab and Shift+Tab move the ring through the pages, the controls and Undo, Enter or Space presses what it is on, arrows step a choice, a number or the page list (Up/Down in a number field), Escape leaves a field or the ring |
| Tray | arrows and Tab move the ring, Enter or Space presses what it is on, Right enters a submenu, Left or Escape goes back a level |
| Controls | Up/Down and Tab move the ring, Left/Right move it or adjust a slider, Enter or Space presses what it is on |
| Media, Controls, Weather, Session | a typed character collapses an island opened from a keybind, so it goes to the window beneath |

