# CLI

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
| `google-calendar sign-in <client.json>\|sign-out\|sync\|status` | signs in to Google Calendar through the browser, with a Desktop app OAuth client's JSON file, see [Google Calendar](Google-Calendar.md); `sign-out` revokes access and deletes the credentials and synced events, `sync` syncs now, `status` says how the account stands |
| `weather refresh\|status` | fetches the weather now, or says how it stands and when it was fetched |
| `osd volume\|brightness` | shows the OSD for the volume or brightness as it is, on the focused output, for a keybind that changes it outside Kanade; refused while `osd`, or `audio` or `brightness`, is off |
| `timer start <duration>` | starts the timer, like `90s`, `25m` or `1h30m`, up to 24h |
| `timer pause\|resume\|cancel` | pauses, resumes or cancels it |
| `config reload` | reads the config again now |
| `config validate` | says what a reload would find, without applying it |
| `status` | the config generation, the last reload error and the keys pending restart |
| `module list` | each Module: whether it runs, why not, and what a restart would change |
| `module enable\|disable <name>` | turns a Module on or off in the settings file, applied at the next restart; prints what the restart will change, the Modules that turn off with it included. `island` cannot be turned off |
| `lock` | locks the session: every monitor shows the date, the time, who is signed in and a password field over its wallpaper, checked by PAM through its `login` service. Exits 0 once niri holds the lock for that call, 1 when niri refuses it, as while another locker holds the session, 3 if not within 5 s or when a password typed on it is accepted or being checked, see [Keyboard](Keyboard.md). Refused without the `login` PAM service |
| `session menu\|suspend\|reboot\|poweroff\|logout` | `menu` opens the Session Surface: lock, sleep, restart, power off and log out; it has no `toggle`, and Escape closes it. `suspend` asks logind to suspend at once; `reboot`, `poweroff` and `logout` count down 60 s on every island first, with Cancel and a button to do it now, and a newer one replaces the countdown. Kanade does not let polkit ask for a password, so a request that needs one is refused; for a countdown, that happens once it runs out. The exit status says only that it was asked: a refusal shows later on the island with its reason, over a countdown too. A countdown's refusal stays until dismissed. A countdown starting collapses any other open Surface; under the overview it shows once that closes. Until logind answers a request, another sleep, restart, power off or log out is refused, and so is a countdown running out meanwhile. A request logind never answers may still happen: the island says so until dismissed, a countdown running is refused, and nothing more is asked until Kanade restarts |
| `doctor` | read-only diagnostics: Kanade and niri versions, Wayland protocols, buses and sockets, the config, and what each Module on runs worse without, like a missing `pw-dump` or another notification daemon; works without a shell, exits 1 on a failure |

`kanade help` lists every verb with a line on what it does; `kanade help <verb>` or `kanade <verb> --help` gives the forms it takes and what their arguments are, for `debug` the `post` and `withdraw` forms that post test Activities. `kanade --version` prints the version. Output is bold on a terminal unless `NO_COLOR` is set. A verb of a Module turned off in the config answers `module <name> is off`. With no shell running, a verb says so. Exit status: 0 done, 1 refused, no shell or a failed write to stdout (not a pipe its reader closed, as `head` does), 2 not a verb or not its arguments, 3 not known whether it was done: the shell, or niri for a screenshot, got the call but did not answer in time, so it may still be done; not worth repeating blindly.

