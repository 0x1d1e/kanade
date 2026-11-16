# CLI

`kanade` with no verb runs the shell. `kanade <verb> [args]` asks the running one. `kanade help` lists every verb, and `kanade help <verb>` or `kanade <verb> --help` gives the forms it takes and what their arguments are, for `debug` the `post` and `withdraw` forms that post test Activities.

## Launcher opening

A Launcher opening (`launcher open`, or `launcher toggle` when closed) waits for keyboard focus and its key handler before answering success, in both styles. Typing immediately after the command returns goes to the Launcher. If the Launcher cannot open (for example, during the compositor's overview), or that opening closes or is replaced first, the command refuses. A toggle that closes it succeeds immediately; an unconfirmed focus wait ends with exit 3 after at most 5 seconds.

## Surfaces

The first row lists every Surface and opens one; the two rows under it swap `open` for `close` or `toggle`.

| Verb | Does |
|---|---|
| `launcher\|controls\|media\|notifications\|tray\|clipboard\|calendar\|weather\|wallpaper\|keys\|timer\|capture\|window\|strip\|emoji\|screen-time\|files open` | opens that Surface on the focused output. `screen-time` is off unless turned on; `files` is the desktop's recent files, read-only |
| `... close` | collapses it when it is open there |
| `... toggle` | opens it, or collapses it when it is already open there |
| `dmenu [--prompt <text>]` | opens the Launcher on the focused output with only the lines read from stdin, searched as it searches apps, the prompt in its field. Prints the line pressed, exactly as read, and exits 0; Escape, a collapse or another Surface exits 1 with nothing printed. Empty lines are skipped; at most 5000 lines of up to 1000 characters. One at a time: a second `dmenu` replaces the first, which exits 1, while a Launcher you are typing in refuses it. It holds the keyboard until answered: unlike other verbs, it never collapses on its own after 5 s or when the pointer leaves. Part of the `launcher` Module |
| `island collapse` | collapses the open island on the focused output |
| `peek down\|up` | holds the focused island's Peek for a niri press/release bind: `down` shows what a hover would (the primary Activity, or the time Peek at Rest) and `up` returns to what that interrupted. An open Surface is left as it is, and `up` does not collapse a Peek the pointer is already holding. Kanade grabs no key |
| `settings [open] [<page>]\|close` | opens Settings on a task page (`appearance`, `shell`, `island`, `dock`, `motion`, `notifications`, `timer`, `modules`, `data`, `power`, `lock`, `health`, `niri`, or `umbriel`), or on its last page (`open` is optional); a Module with settings opens the page containing them. `close` closes the window |

## Notifications

| Verb | Does |
|---|---|
| `notifications clear` | dismisses every notification |
| `notifications dnd on\|off\|toggle\|status` | Do Not Disturb: notification Banners stop showing, Critical ones and those a rule lets bypass it (`notifications.rules`) still do |
| `notifications dnd on <duration>\|until <HH:MM>` | Do Not Disturb that turns itself off after a duration like `90s`, `25m` or `1h30m`, up to 24h, or at the next time the clock reads `HH:MM` (24 hour). `status`, `kanade status` and the Controls tile say when it ends. Anything else that sets Do Not Disturb, `off` or a press of the tile, cancels the end, so it never turns off a Do Not Disturb you turned on again. Waits on the wall clock, so a sleep over the moment ends it on wake; forgotten on a restart |

## Clipboard

| Verb | Does |
|---|---|
| `clipboard clear` | forgets the clipboard history except the entries you pinned, and says how many it kept (`kept 3 pinned`); the pins stay on disk as well, and what is on the clipboard stays |

## Capture

| Verb | Does |
|---|---|
| `capture screenshot area\|window\|output` | saves a screenshot under `~/Pictures/Screenshots` and prints its path: niri captures a picked area, focused window or focused output; Umbriel uses `slurp`/`grim` for an area and `grim` for the output, then offers the PNG to `wl-copy`. Window screenshots are unsupported under Umbriel; a clipboard helper failure leaves the saved file intact. With `capture.annotate` on, the saved shot opens in the annotation editor on the focused output instead of only showing its notice |
| `capture cast window [<id>]\|monitor [<output>]\|clear` | changes what a screen share with a dynamic cast target shows (niri's `set-dynamic-cast-window`, `set-dynamic-cast-monitor`, `clear-dynamic-cast-target`): a window by niri id, else the focused one; an output by name, else the focused one; or nothing. Refused while no running cast has a dynamic target, and by a niri without the actions |
| `capture color` | lets you pick a pixel of the screen with niri's picker, copies it as `#rrggbb` to the clipboard, shows it on the Island and prints it; waits for you, and exits 1 with "no color picked" if you leave the picker |
| `capture record start [screen\|area [<x,y WxH>]\|window [<id>]] [none\|system\|mic\|both\|mix]` | records the focused output (`screen`, the default), an area, or a window with `wf-recorder`, encoded on the GPU, to `~/Videos/Screencasts` |
| `capture record pause\|resume` | `pause` ends the recording's current segment and `resume` starts the next; it needs `ffmpeg`, and a pause is refused until the first frame was recorded. Locking the session while recording stops and saves it, and a start or resume is refused while locked |
| `capture record stop\|status` | `stop` saves the recording (joining the segments of one resumed after a pause, while the island says Saving) and prints its path; `status` says how it stands |

**capture record start**:

- An area is drawn over an overlay, or given as `x,y WxH`; each side is rounded down to an even number.
- `window` is the focused window, or `window <id>` a niri id from `niri msg windows`. It is cast by niri, encoded on the GPU through `gst-launch-1.0`, then written by `ffmpeg`.
- A recorded window keeps the size it had at start, and a resize is letterboxed. Closing it ends the recording and saves what exists.
- The sound is `none` (the default), `system` (what the speakers play), `mic` (the microphone), or `both` / `mix`.
- `both` and `mix` mix simultaneous system audio and microphone into a managed virtual sink. The microphone is recorded only when you ask for it here, never a default.
- Sound and picture are not guaranteed to be in sync.
- `start` prints the file's path once the first frame is written, and the file is there then.
- With an area to draw it prints nothing until you have drawn it; a failure to start shows on the island.
- A recording you never pause is that file and nothing is joined. One you resume after a pause is the whole at that path only after `stop`.

## Dictation

| Verb | Does |
|---|---|
| `dictate toggle\|down\|up\|cancel\|status` | the `dictation` Module (off by default): `toggle` starts recording, or stops it and transcribes with the Gemini API, putting the text on the clipboard or typing or pasting it with `dictate.insert` |
| `dictate key\|consent\|forget` | the setup, run in your terminal and never sent to the shell, so they work before the Module is on: `key` reads the Gemini API key from stdin (typed hidden at a terminal, else piped) into the Secret Service, never an argument; `consent` shows what dictation sends to Google and records your acceptance when you type `I agree`, and refuses anything but a terminal; `forget` deletes the key and withdraws the consent |

**dictate toggle**:

- `toggle` is refused while the session is locked, and until a key and your consent exist; the island shows each step.
- `down` starts only while idle, through the same gates, and `up` stops and transcribes only that hold.
- `up` with no `down` does nothing, and `up` does not stop a `toggle`.
- A recording stops by itself at 120 seconds.
- `cancel` before the stop drops the recording and sends nothing.
- Once you stopped it the upload is already under way and cannot be recalled. `cancel` (the island's Discard) then only throws the result away, uninserted.
- `status` says how it stands and whether the key and your consent are in place (`gate: ready`, `no key`, `no consent`).
- See [Dictation](Dictation.md).

## Gestures

| Verb | Does |
|---|---|
| `gesture` | opens a layer over the focused output that takes the pointer: draw one stroke with the left button held, and on release the recorded gesture it matches runs the Kanade verb `gestures.<name>` maps it to. Escape, a right click, 8 seconds without a stroke or a lock closes it; run again while it is open, it closes. A stroke that matches nothing shows "No gesture" on the Island and runs nothing. `gestures.<name>` takes only a Kanade verb (`"launcher open"`), never a shell command; to run your own command, use a [hook](Configuration.md) or a verb that does. Refused while locked, or before any gesture is recorded. Part of the `gesture` Module |
| `gesture record <name>` | draws `<name>` three times in the same overlay (each stroke within 8 seconds) and stores the three strokes in `$XDG_STATE_HOME/kanade/gestures.toml`, replacing a gesture of that name; waits for you, and prints what to set in the config when the gesture has no action yet. A name is 1 to 32 letters, digits, `-` or `_` |
| `gesture forget <name>` | removes a recorded gesture |
| `gesture list` | prints each recorded gesture with the verb the config gives it |

## Session and power

| Verb | Does |
|---|---|
| `caffeine on\|off\|toggle [<duration>]\|status` | keeps the session from going idle, so neither the `idle` Module nor an idle daemon like hypridle locks or suspends it, until turned off or for a duration like `90s`, `25m` or `1h30m`, up to 24h; the island shows it while on. `on` returns once the inhibitor is held, `status` says how it stands |
| `focus on\|off\|toggle\|status` | one switch for Do Not Disturb, caffeine and, if `focus.night_light` is set, the night light (`focus.*`); `off` turns off only what `on` turned on and you have not changed since, so a piece that was on before, or that you set yourself meanwhile, stays; `status` says `off` or `on` with the pieces it turned on, and `(screen cast)` when `focus.auto` turned it on. Focus is forgotten on a restart |
| `night-light on\|off\|toggle\|status` | warms the screen through `wlsunset`: `on` turns it back on as the config says (a schedule when one is set, else all day), `off` turns it off, both kept across restarts in `settings.toml`; `status` says the mode and why `wlsunset` is not running, if it should be |
| `session menu\|suspend\|reboot [now\|--now]\|poweroff [now\|--now]\|logout [now\|--now]\|cancel` | `menu` opens the Session Surface with lock, sleep, restart, power off and log out, which Escape closes; `reboot`, `poweroff` and `logout` count down 60 s on every island, with Cancel and a button to do it now |
| `lock` | locks the session: every monitor shows the date, the time, who is signed in and a password field over its wallpaper, checked by PAM through its `login` service, or the one `lock.pam_service` names, whose messages show under the field. Exits 0 once niri holds the lock for that call, 1 when niri refuses it, as while another locker holds the session, 3 if not within 5 s or when a password typed on it is accepted or being checked, see [Keyboard](Keyboard.md). Refused without the PAM service's file |
| `idle status` | the `idle` Module's stages, how long the user has been idle, and what holds the stages: a logind idle inhibitor or caffeine |
| `osd volume\|brightness` | shows the OSD for the volume or brightness as it is, on the focused output, for a keybind that changes it outside Kanade; refused while `osd`, or `audio` or `brightness`, is off |
| `module list` | each Module: whether it runs, why not, and what a restart would change |
| `module enable\|disable <name>` | turns a Module on or off in the settings file, applied at the next restart; prints what the restart will change, the Modules that turn off with it included. `island` cannot be turned off |

**session**:

- `menu` has no `toggle`. `suspend` asks logind to suspend at once, and `now` or `--now` ends a restart, power off or log out countdown at once.
- A newer countdown replaces the running one. `cancel` dismisses an active countdown and removes the island activity.
- The countdown lists up to three lines of what would be lost: a running recording or timer, live Activities, and programs holding a blocking `shutdown` inhibitor with logind (who and why, cut short).
- It only warns and prevents nothing.
- Under niri and Umbriel, Kanade answers polkit's password requests on the focused Island while the session is unlocked and the `polkit` Module is on (see [Installation](Installation.md)); live Umbriel Auth verification remains **NOT RUN**.
- A request that needs a password is answered there rather than refused ([ADR 0146](../adr/0146-polkit-auth-on-the-island.md), extended by [ADR 0147](../adr/0147-umbriel-compositor-support-and-config-editor.md)).
- It is still refused while the session is locked, with the Module off, or when another authentication agent owns the name. For a countdown, that happens once it runs out.
- The exit status says only that it was asked: a refusal shows later on the island with its reason, over a countdown too. A countdown's refusal stays until dismissed.
- A countdown starting collapses any other open Surface; under the overview it shows once that closes.
- Until logind answers a request, another sleep, restart, power off or log out is refused, and so is a countdown running out meanwhile.
- A request logind never answers may still happen: the island says so until dismissed, a countdown running is refused, and nothing more is asked until Kanade restarts.

## Hardware

| Verb | Does |
|---|---|
| `wifi on\|off\|toggle\|status` | switches the Wi-Fi radio through NetworkManager; returns once NetworkManager has it as asked. `status` prints `on` or `off`. Part of the `network` Module |
| `bluetooth on\|off\|toggle\|status` | switches the Bluetooth adapter through BlueZ, as `wifi` does. Part of the `bluetooth` Module |
| `power-profile set <profile>\|cycle\|status` | switches the power profile through power-profiles-daemon: `power-saver`, `balanced` or `performance`, where the machine has it; `cycle` takes the next one the machine has. `status` prints the profile and those available. Part of the `power` Module |
| `brightness set <percent>\|up\|down\|status` | sets the brightness 1-100, or 5 points up or down, of the backlight or, over DDC/CI (`brightness.backend`), of the monitor under the focused output; `status` prints it. Part of the `brightness` Module |
| `volume up [<step>]\|down [<step>]\|set <percent>\|mute [toggle\|on\|off]\|mic-mute [toggle\|on\|off]\|status` | sets speaker volume (1-100 or stepped by `<step>`, default 5), toggles or sets speaker mute, toggles or sets microphone mute, or queries status (`speaker: X% [muted\|unmuted]\nmicrophone: Y% [muted\|unmuted]`); triggers visual feedback on the Island when at Rest, suppressed over open surfaces or active banners. Part of the `audio` Module |
| `battery limit on\|off\|status` | asks UPower (1.90 or later) to stop charging the battery short of full, at the threshold UPower or the firmware sets, or to charge it full again; returns once UPower answered, printing the state or why it refused. `status` says whether it is on and where charging stops, or why it is unavailable |

## Media and usage

| Verb | Does |
|---|---|
| `media play-pause\|next\|previous` | asks the player the island follows, the one playing or else the one shown last, to play or pause, or to skip; refused when none is open or it says it cannot |
| `usage` | refreshes Claude plan utilization when the `usage` Module is enabled. The Island shows five-hour and seven-day rings; a missing window says `N/A`, and limit or fetch-error notices show their message. It opens no Surface |
| `usage key` | reads the API key from stdin, hidden at a terminal, and stores it at `$XDG_STATE_HOME/kanade/usage-key` with mode 0600; run in the terminal, never sent through shell IPC |

## Wallpaper

| Verb | Does |
|---|---|
| `wallpaper set <path>\|shuffle [--output <name>\|all]\|status` | sets the wallpaper to an image through awww, and prints its path once awww shows it; `shuffle` sets one from `wallpaper.directory` at random, other than the one showing on its output. `--output` names the monitor (`eDP-1`, as `niri msg outputs` names it): `set` shows the image there and leaves the others as they are, `shuffle` defaults to the focused monitor, and `all` (the default for `set`) is every monitor. `status` names the one set since start and what each monitor shows, marking the one the shell colors follow |

## Calendar and weather

| Verb | Does |
|---|---|
| `google-calendar sign-in <client.json>\|sign-out\|sync\|status` | signs in to Google Calendar through the browser, with a Desktop app OAuth client's JSON file, see [Google Calendar](Google-Calendar.md); `sign-out` revokes access and deletes the credentials and synced events, `sync` syncs now, `status` says how the account stands |
| `reminders dismiss\|status` | `dismiss` takes away the calendar reminders the island shows, as their Dismiss does; `status` says how many show and when the next goes off |
| `calendar-subscriptions sync\|status` | `sync` fetches every calendar subscription now, `status` says how each stands, by its place in `calendar.subscriptions`, never by address; see [Configuration](Configuration.md). Part of the `calendar-subscriptions` Module |
| `weather refresh\|status` | fetches the weather now, or says how it stands and when it was fetched |

## Timer

| Verb | Does |
|---|---|
| `timer start <duration>` | starts the timer, like `90s`, `25m` or `1h30m`, up to 24h; its Peek has Pause or Resume and Stop pills |
| `timer pomodoro [<focus> <break> <long-break> <rounds>]` | runs a pomodoro on the timer until cancelled: a focus, then a break, a long break after the last round's focus, then round 1 again. The island shows the phase and, on its Peek, the round and a Skip pill beside Pause and Stop. Lengths are like `25m`; those left off come from `timer.pomodoro.*` (25m, 5m, 15m, 4 rounds by default). A phase the timer moves on to by itself shows over Media for 6 s and collapses an open Surface |
| `timer pause\|resume\|skip\|cancel` | pauses, resumes or cancels it; `skip` moves a pomodoro to its next phase at once, paused if it was |

## Activities

| Verb | Does |
|---|---|
| `activity start <id> <title>` | shows a live Activity on every island for a script, a build or a download: its title, a pulse or how far along, a Satellite beside a higher one. `<id>` is 1 to 64 letters, digits, `.`, `_` or `-`, and does not begin with `notification.`, which Kanade keeps for a notification's progress; starting a live id again starts it afresh. At most 8 are live at once, and notifications take at most 3 of them |
| `activity progress <id> <amount>` | how far along it is: a percent (`40`, `40%`) or a fraction (`0.4`); a bar shows it |
| `activity detail <id> [<text>]` | a line under the title, cut to 160 characters; none clears it |
| `activity end\|fail <id> [<text>]` | ends it as done or failed, shown for 10 s on the focused output, with the text or its last detail |
| `activity touch <id>` | says it still runs, so it is not withdrawn after `live.stale` (30 min) without a word |
| `activity list` | each live one: its id, how far along (`-` when it does not say) and its title, tab-separated |
| `activity actions <id> [<key>=<label>]...` | puts up to 3 buttons on the live Activity, replacing those it had; none clears them. A key is 1 to 32 of `a-z`, `0-9`, `_`, `-`; a label is one line cut to 24 characters, never `Cancel`, `Now`, `Dismiss`, `Stop` or `Confirm` (the shell's own words, in any case, ignoring invisible characters and punctuation around them; a look-alike in other scripts can still pass, but the button is always the neutral one and a press only tells you). Shown on Peek only. Kanade runs nothing for a press: it queues the key for you to read with `pressed` or `wait`. They end with the Activity |
| `activity pressed <id>` | takes the oldest press waiting, prints its key and exits 0; exits 1 with no output when there is none. At most 4 wait, the oldest dropped. A pressed button shows `...` until your next `detail`, `progress`, `end`, `fail` or `actions` for the id, or 10 s |
| `activity wait <id> [--timeout <seconds>]` | asks `pressed` every 250 ms until there is a press, prints its key and exits 0; exits 1 after the timeout (up to 86400) or when the shell refuses. Kanade holds no call while it waits, but the client asks about four times a second, each ask a short call to the shell; one that fails for now (a timeout) is asked again |
| `activity run <id> <title> [--action <key>=<label>]... -- <command>...` | runs the command with a live Activity for it, ended as done on exit status 0 and failed otherwise, and exits as the command did |
| `activity hook claude` | reads one Claude Code hook event as JSON on stdin and shows the session as a live Activity, see [Agent hooks](Agent-hooks.md). Exits 0 when told, when the event needs nothing shown, and with no shell or `live` off; 1 with a line on stderr for anything else; never 2, which Claude Code takes as a block |
| `activity hook claude decide` | the opt-in permission decider, a replacement for the `PermissionRequest` entry only. Allow exits 0; Deny, no answer, or a prompt that was not shown exits 2. The default hook is unchanged |

**activity run**:

- It exits 127 when the command cannot start, and 128 + the signal when one ended it. Ctrl+C reaches only the command.
- Without a shell, or with `live` off, the command still runs and stderr says why it shows nothing.
- With `--action cancel=<label>`, a press sends SIGTERM to the command, which then ends as failed (143). Each further press while it runs sends it again.
- The signal goes to the command itself, not its process group: a `sh -c` wrapper dies, and what it started may live on.
- `cancel` is the only action `run` takes with it, since `run` reads the presses.

## Configuration

| Verb | Does |
|---|---|
| `config reload` | reads the config again now |
| `config validate` | says what a reload would find, without applying it |
| `config validate <file>` | says what a file alone would not apply, as `file:line: what`, exit 1 if anything; works without a shell |
| `config defaults` | prints every key with its default and help, as the wiki's Configuration page holds them; works without a shell |
| `config schema` | prints a JSON Schema of every key (type, range, choices, default, help; an `output` table for per output keys), for an editor; works without a shell |
| `config get <key>` | prints `key = value` as in effect, then `# from` the file that set it or `default`; a table lists every file that adds to it |
| `config set <key> <value>` | writes the key to the settings file as the Settings window does and prints what it comes to; never touches your own files. A key that takes text takes it as typed, any other a TOML value like `300`, `true` or `[1, 2]`. An unknown key or a value out of range is refused with the reason, exit 1, and nothing is written. Refused the same way: a key that runs a command (`hooks.*`), and `gestures`, which maps a gesture to a verb, as both are set in files only (ADR 0106). The shell, if running, reloads it |
| `config reset <key>` | removes the key from the settings file, so your files and the defaults give it again |

## Compositor config

`niri-config` edits niri's KDL config and `umbriel-config` Umbriel's TOML one, both without a running shell.

| Verb | Does |
|---|---|
| `niri-config validate` | runs `niri validate` on niri's config (`$NIRI_CONFIG`, else `$XDG_CONFIG_HOME/niri/config.kdl`); works without a shell |
| `niri-config get <path>` | prints the value niri will use at a dotted path like `layout.gaps` (a flag like `layout.focus-ring.off` is `true` or `false`), with `# from <file>` when an include sets it; exit 1 when it is not set. Works without a shell |
| `niri-config set <path> <value>` | changes one setting in niri's config, keeping every other byte: comments, `/-`, blank lines and line endings; a missing node is made at the end of its block |
| `niri-config remove <path>` | removes that node's own line (its trailing `//` comment goes with it, comments on lines of their own stay), by the same checks |
| `niri-config bindings` | lists the key bindings of niri's own config that Kanade may rename: the chord as the file writes it, a tab, then the action, one a line. An include's bind and a `/-` one are not listed |
| `niri-config rebind <old> <new>` | renames one bind's chord, keeping its action, properties, comments and the node's form; refused for an include's bind, a `/-` one, a chord KDL cannot write and one another bind already takes ([ADR 0161](../adr/0161-niri-bind-rename.md)) |
| `niri-config get\|set\|remove 'output["<name>"].<key>'` | the same for a key of niri's `output "<name>"` block, whose name is in double quotes |
| `niri-config rules [apply <id>\|remove <id>]` | lists the recommended installation rules (`blur-xray`, `screencast-banners`, `overview-backdrop`) and their status (`active`, `missing`, `included <file>`), or applies/removes one by id with `niri validate` check and backup ring |
| `niri-config undo` | puts back the bytes before Kanade's last change, only while the file is as that change left it. The Settings window's niri page ([Settings](Settings.md)) stages its changes and writes them with Save as one such change, and has this as its Undo |
| `niri-config backups` | lists the backups, newest first (the number is the one `restore` takes), then `orig`, the first bytes Kanade saw; kept in `$XDG_STATE_HOME/kanade/niri-config/`, a ring per config, the newest 20, readable by you only |
| `niri-config restore <n>\|orig` | puts a backup back (the result must validate; the current file need not), keeping what it replaces as a backup |
| `umbriel-config validate\|get <path>\|set <path> <value>\|remove <path>\|undo\|backups\|restore <n>\|orig` | edits Umbriel's TOML config with validation and backups, without a running shell; `$UMBRIEL_CONFIG`, else `$XDG_CONFIG_HOME/umbriel/config.toml`. `--config <file>` immediately after `umbriel-config` selects another file. `rules [apply\|remove <id>]` manages its recommended rules. See `kanade help umbriel-config` and [Settings](Settings.md) |

**niri-config set**:

- The value is `true`, `false`, a number, or text. Text that looks like a number goes in double quotes, `'"12"'`.
- `niri validate` must accept the file first and the result before it replaces the file; the old bytes are kept as a backup.
- Refused, nothing written, exit 1: a config niri rejects as it is (a v2 syntax too), a symlink or hard link, `/etc/niri/config.kdl`, a node an include sets (the message names the file, `kanade.kdl` among them), a node under `/-`.
- Also refused: a repeated or block node (`window-rule`, `spawn-at-startup`; `output` only as below), a result niri rejects (niri's first error, with file:line:col), and a file that changed on disk meanwhile.
- `--config <file>` right after `niri-config` names another file (for tests). It works without a shell, and niri reloads the file by itself.

**niri-config bindings|rebind**:

- A bind is the chord as the node's name; its action is its child and its properties sit on the node before it. Only the name token is written: `Mod+Return allow-when-locked=false { spawn "kitty"; }` renamed to `Mod+B` keeps the action, the property, the comments and the node's one-line form.
- Both verbs read the config itself. An include's bind is not listed, and `rebind` names the include if that chord is asked for; a `/-` bind is not listed and is refused.
- A new chord collides when it resolves to the same keys as another bind, with `Mod` read as the file's `input { mod-key }` (Super when it sets none): `Mod+Q` and `Super+Q` collide, `Mod+Q` and `Shift+Q` do not. `Enter` and `Return` are different spellings, as in niri.
- A new chord must be a bare KDL v1 node name: letters, digits, `-`, `_` and `+`, not starting with a digit (`Mod+B`, `Mod+Shift+q`). A space, a quote or anything else KDL cannot write bare is refused before anything is written. A bind the file writes quoted is listed and renamed by its name's value.
- `niri-config --config /tmp/config.kdl rebind 'Mod+Return' 'Mod+B'` works on a copy, with the same validation, backup ring and lock as `set`; Undo puts the old bytes back.

**niri-config get|set|remove 'output["<name>"].<key>'**:

- The name is in double quotes, with `\"` and `\\` inside, so `'output["Dell Inc. P1917S 0x1234"].scale'` works.
- Keys: `off`, `scale`, `transform` (always text: `90` is written `"90"`), `position` and `variable-refresh-rate`, `mode`.
- `position` and `variable-refresh-rate` take properties: `x=-1920 y=100`; `on-demand=true`; `""` for the bare node, `remove` for off.
- The block is the first one in the config, includes read where they stand, whose name matches the monitor as niri does (case-insensitively, the connector or `make model serial`).
- A later one is shadowed and `get` says so. With no match a block named as typed is made at the end of the file (before the appearance sync's include).
- The command line knows only the name as typed, not the monitor's EDID. A block named by `make model serial` for the same monitor is not matched by `DP-2`, and a new `DP-2` block beside it would be shadowed.
- The command says so when blocks with such names exist. Settings resolves by EDID, and is the better way to change a monitor.
- Refused: a match in an include (the message names it), `mode custom=true` and `modeline`.
- `get 'output["<name>"]'` prints the block as the file has it.
- `remove 'output["<name>"]'` removes the keys above and the block with them if nothing else is in it (`focus-at-startup`, `hot-corners` or a comment keep it).
- Removing the block is refused when another block of the same monitor follows, as niri would then use that one. The blank line a block was made with goes with it.
- Same safety flow as above.

## Status and modules

| Verb | Does |
|---|---|
| `status` | the config generation, the paths the last good config was read from, the last reload error and the keys pending restart |

## Login screen

| Verb | Does |
|---|---|
| `sddm status\|sync\|enable\|disable\|theme list\|theme info <name>\|theme use <name>` | the login screen's look: writes Kanade's SDDM theme, or another installed theme's links, and the picture, to an overlay under `/usr/local/share/kanade-sddm` you own, from the `sddm.*` keys. Works without a shell. Kanade never runs `sudo`: `status`, `enable`, `disable` and `theme use` print the root commands to run, and `sddm.sync` writes the picture again when the wallpaper changes. See [SDDM](SDDM.md) |

## Shell

| Verb | Does |
|---|---|
| `completions bash\|fish\|zsh` | prints shell completions, derived from the commands `kanade help` lists, see [Installation](Installation.md); works without a shell |
| `update` | builds `main` from GitHub with `cargo install --locked --git https://github.com/0x1d1e/kanade --branch main --root ~/.cargo --force kanade`, refreshes the user unit (the old unit is kept when it differs), and restarts `kanade.service` if that unit is running; a failed build leaves the installed binary as it was; requires Cargo and the build dependencies, and works without a shell |
| `doctor` | read-only diagnostics: Kanade and niri versions, niri's open windows and layer-shell surfaces, Wayland protocols, buses and sockets, the config, what each Module on runs worse without, like a missing `pw-dump` or another notification daemon, and, with `wallpaper` on, whether a niri layer rule puts awww in the overview backdrop; works without a shell, exits 1 on a failure; Settings' Health page shows the same checks |

## Inspection

`kanade ui` reads and drives the shell's windows from outside. `motion inspect` replays Island motion offline; the [Motion inspection](#motion-inspection) section below explains its flags and output.

| Verb | Does |
|---|---|
| `ui describe [--window <name>]` | prints the windows as a JSON tree of named nodes (`id`, `parent`, `role`, `bounds`, `label`, `value`, `focused`, `pressed`, `disabled`, `selected`, `checked`, `secret`), as of each window's last frame. Ids are stable paths like `dock/app` or `launcher/result#2`. With the default `ui.describe = "structure"`, text a user or the world wrote (a notification, a track, a window title, a network, the clipboard, text typed) is `<redacted>`, and a password is never given. `--window` is `kanade-dock` or `dock`. Part of the `ui` Module |
| `ui press\|focus <name>` | clicks the node, or focuses a text input, by name through the shell's own input path, not the pointer: your pointer and focus are not moved. `<name>` is an id from `describe`, the end of one when only one fits (`wifi`), or a label. Refused when the name fits none or several, when the node is disabled or covered, and while the session is locked |
| `ui type <name> <text>` | focuses the text input and types the text as keys to its window |
| `ui key <name> <key>` | sends one key to the node's window: `enter`, `escape`, `tab`, `backspace`, `space`, an arrow, `home`, `end`, `delete`, `pageup`, `pagedown` or a character, after any of `ctrl+`, `alt+`, `shift+`, `super+`. A ctrl chord reaches a Surface's own keys as the shortcut it is, so in the Launcher `ctrl+k` opens the selected answer's actions and `ctrl+1` through `ctrl+9` press the nth answer |
| `ui scroll <name> <lines>` | wheel lines over the node, positive down |
| `ui wait <name> [--timeout <seconds>]` | returns 0 once the node is in the tree, 1 after 5 s (up to 600 s with `--timeout`) |
| `motion inspect [interruptions\|satellites] [--script <file>] [--fps <1-240>] [--duration <ms>] [--at <ms>] [--speed <0.1-10>] [--edge top\|bottom\|left\|right] [--reduced-motion] [--format json\|html]` | local deterministic Island motion inspection with synthetic Activities; no running shell, compositor, source services or user config required. Defaults: interruptions, 60 fps, 2400 ms, speed 1, top edge, JSON |

## Exit status and output

`kanade --version` prints the version. Output is bold on a terminal unless `NO_COLOR` is set. A verb of a Module turned off in the config answers `module <name> is off`. With no shell running, a verb says so.

Titles and details of `activity` are one line of plain text, control characters becoming spaces, cut to 80 and 160 characters, and never logged; an id that is not live is refused. A change to a live Activity within 250 ms of its last is held and shown when that is up, and one that changes nothing shows nothing.

- 0: done.
- 1: refused, no shell, or a failed write to stdout (not a pipe its reader closed, as `head` does).
- 2: not a verb or not its arguments.
- 3: not known whether it was done: the shell, or niri for a screenshot, got the call but did not answer in time, so it may still be done; not worth repeating blindly.

## Motion inspection

```sh
kanade motion inspect interruptions --at 75 > /tmp/kanade-motion.json
kanade motion inspect satellites --fps 144 --format html > /tmp/kanade-motion.html
kanade motion inspect satellites --edge right --reduced-motion --format html > /tmp/kanade-reduced.html
```

Open the generated HTML in a browser. It starts paused, with play/pause, sample stepping, a keyboard-accessible sample slider and frame/time seek fields. Playback shows recorded poses without interpolation; time seek picks the nearest recorded sample. Repeat `--at <integer-ms>` to record exact instants beyond the frame grid. JSON includes before/after samples for every command, body geometry and analytic velocity, content and section opacity/lift, and Satellite presence, shape alpha and raw/effective mark alpha. The stage is diagnostic geometry, not a Surface screenshot; pane materials, GPU neck finish and real pointer/keyboard/privacy behavior need live verification.

`--script` accepts a bounded, local replay file (64 KiB, 256 events).

Each nonblank line is `<integer-ms> <command>`, ordered by time; equal-time commands keep file order, and `#` starts a comment.

Supported commands are Surface `open|close|toggle`, `island collapse`, `peek down|up`, the existing `debug post|withdraw` forms, `dnd on|off|toggle`, `hover in|out`, and `segment primary|satellite`. Nothing dispatches to IPC or external actions.

Times and `--duration` are limited to 60000 ms; later events are omitted from a shorter recording.

`--speed` changes normal body and Satellite responses while accessibility's brief fade and input deadlines keep their configured durations.

Synthetic identifiers and content come from the fixture or replay file, never the running desktop.

```text
0 debug post timer primary ongoing persistent global none
0 debug post live secondary ongoing persistent global none
500 peek down
650 peek up
800 controls open
835 island collapse
```
