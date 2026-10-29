# Kanade domain language

Use these terms in code, tests, issues and docs. Avoid the listed synonyms. Terms for parts not built yet (Activity, Arbiter, Frame, Satellite) are fixed here ahead of the code.

## Island
The single physical surface on one monitor. One per monitor.
- Monitor and output name the same thing: the runtime says monitor, niri says output.
- An autohidden Island (`island.autohide`), or one whose Rest has nothing to show (`rest.clock` and `rest.battery` off), slides past its edge once at Rest a moment, with no Satellite, nothing pinned, outside the overview and the pointer away. It comes back as the pointer touches the edge under it, where a click opens it as on the body, or as anything shows on it.
- **Avoid:** pill (as a type name; fine in prose for the small shape), widget, popup, dropdown

## Desk widget
A face of an existing Service, drawn as plain text on the desktop in a window of its own on the `Bottom` layer, one per monitor, by the `desk` Module (ADR 0137). Not an Activity, Surface, Satellite or Pane: it has no Arbiter, Lifetime or Presentation, and writes no Service from a view. It belongs to the shell whatever core is chosen. The faces, each off by default: the clock (`desk.clock`); the weather (`desk.weather`, temperature and condition from the weather Service, nothing fetched of its own); the next event (`desk.agenda`, time only unless `desk.agenda_titles` is on); the player's previous, play-pause and next buttons (`desk.media`); the track's title and artist (`desk.media_titles`); the notes card (`desk.notes`).
- **Invariant:** a Desk face with its switch off reads no Service. With every face off the window is hidden and reads nothing; the Module off opens no window. The Desk is hidden while niri's overview is open. The clock's one frame a minute is the Desk's idle frame. Weather and the agenda add no poll; a shown agenda uses that same minute tick. The track line redraws when the player changes, and only while its switch is on.
- **Invariant:** private content is its own switch, off by default: `desk.agenda_titles` (an event's title) and `desk.media_titles` (a track's title and artist). Off, neither is drawn.
- Interaction is the media buttons and the notes card: the pointer only inside an input region of its drawn controls (empty with no player and under the overview), and no key focuses the Desk's window but the notes card's (`Keyboard::OnDemand`, ADR 0140); every other effect is a Kanade verb. "Every target is a key away" covers Surfaces, not Desk widgets.
- **Invariant:** the notes card is the one face typed into in place, and its text is written off the draw thread by the `desk` Module's own worker, coalesced (at most about once a second while typing, at once when the card is left); a view never touches the file. The file is `$XDG_STATE_HOME/kanade/desk-notes`, one note per line, owner-only; note text is never logged. With `desk.notes` off there is no card, no input region for it and no read of the file (ADR 0140).
- In code it is a *Desk* item (`src/desk.rs`); the runtime's `widgets/` are layout primitives.
- **Avoid:** widget (in code, for this meaning), gadget, complication

## Activity
Something happening that may deserve attention. Has identity, Kind, Priority, Lifetime, Scope, Interrupt, Actions, Detail.
- Identity is Kind plus a key, so keys from different Kinds never collide.
- Detail is what its small form draws, typed per Kind (a Media Activity's track, a workspace switch's pager). It is not identity: a repost with new Detail replaces the Activity. A new track dissolves in place, the new art and text rising over the old without the form moving; a level that moves redraws in place.
- **Invariant:** Lifetime, Priority, Scope and Interrupt are independent: the source sets each, and none implies another (ADR 0009). `Activity::new` refuses only what cannot be carried out: `Transient(0)`, and `AutoExpand` for a Kind with no Surface of its own (only Media, Notification, Session and Reminder, whose is Calendar, have one). Unusual combinations, like a Persistent FocusedOutput or a Critical that interrupts nothing, stand.
- **Invariant:** posting an Activity with an existing id replaces it and refreshes its Lifetime. Switching on through workspaces extends one Transient, not a queue of them.
- **Avoid:** event, notification (a Notification is one Kind of Activity)

## Live Activity
An Activity of Kind `Live` another program posts through `kanade activity` (ADR 0042): a build, a download, a script. It shows a title, a pulse or how far along it is, and on Peek a detail line and a bar. The `live` Module owns it; it has no Surface, so a click opens Controls.
- While live it is Persistent Ongoing and Global, so it can be a Satellite; ended, as done or failed, it is a Transient Actionable on the focused output, as a saved recording is.
- **Invariant:** what comes in is bounded: at most `MOST` (`src/sources/live.rs`) live at once, ids from a small alphabet, titles and details one line of plain text cut to a length. A verb for an id that is not live is refused. Titles and details are never logged.
- An app's progress through its Unity launcher entry (`com.canonical.Unity.LauncherEntry`, ADR 0083) is one too, of the `launcher-entry` Module, id `launcher.<desktop file id>`, which no client can have; at most `LAUNCHED` of the live ones, the oldest giving way.
- niri's own failed config load (`ConfigLoaded { failed: true }`, ADR 0071) shows as the failed form of one, id `niri config`, which no client can have: a Transient on the focused output, with no live phase. It follows the `live` Module; a load that succeeded shows nothing.
- **Invariant:** one its program stops talking to is withdrawn after `live.stale`; `activity run` keeps its own alive while its command runs.
- **Invariant:** a repost is cheap: a change within `REPOST` (250 ms) of the Activity's last post is held and posted when due, the newest of those held, and one that changes nothing is never posted (ADR 0058).
- `activity hook claude` is a client of these verbs, not another path to the Island: a Claude Code session's events become one Live Activity, and a late event never brings an ended one back (ADR 0058).
- A notification with a `value` hint under 100 is also a Live Activity of the `notifications` Module, id `notification.<app>.<id>`: the same policy, in the same 8 but at most 3 of them, the oldest giving way, kept while the notification lasts, ended as done at 100, and withdrawn when it closes or is replaced without the hint. It follows the notification's rule and Do Not Disturb as its Banner does (ADR 0059).
- **Live action:** up to 3 buttons the poster attaches (`kanade activity actions`, `run --action`), drawn as neutral pills on Peek (ADR 0110). A press is not run by the shell: it is queued, at most 4, for the poster, which reads it back with `pressed` or `wait`.
- **Invariant:** a press names its start (`Job::serial`) and is ignored when that start is not the one live; actions and the mailbox end with the Activity (`end`, `fail`, `live.stale`, a new `start`). A pressed pill is pending until the poster's next word for the id or `PENDING` (10 s), given as time. Labels are one line cut to 24, never `Cancel`, `Now`, `Dismiss`, `Stop` or `Confirm` (best effort: invisible characters and edge punctuation are ignored, a look-alike in other scripts can pass; the pill is always neutral and a press only messages the poster), never logged; a gesture runs none of these verbs.
- **Avoid:** job, task (as the type; `Job` is its Detail), progress notification

## Device event
A Bluetooth device connecting or going, the charger plugged in or out, the default sound output changing, or a Wi-Fi network joined, shown as a Transient Glance Activity on the focused output (the Detail is `Hardware`, posted under the Kind of its source: Bluetooth, Battery, Volume or Network). It shows a short title for Compact and a line for Peek, and nothing to press. `island.device_events` turns them off.
- **Invariant:** each is told from a change a Module's own Service shows, so a Module that is off shows none. What a watcher first sees is a baseline, never an event, so starting the shell, a daemon or an adapter, or a battery or Wi-Fi device turning up, shows nothing; an adapter turned off disconnects its devices without news.
- **Invariant:** one Activity per class, so a burst takes one place; the same event again within 10 s shows nothing.
- **Avoid:** notification, toast

## Privacy cluster
The microphone, camera and screen capture in use, a dot of its color and shape each (round microphone, square camera, ring screen cast; `theme::signals`) at the trailing end of the Island's body, inside it: in the small forms (Rest, Compact, Split, Peek) and, rising to the top corner as the body grows, every Surface (ADRs 0005, 0033). They fade in and out. In the small forms what is centered stands toward the start while they show. It takes no pointer; the Controls Surface names the apps and says whether a screen cast captures a window or a screen, never whose, says `(paused)` when every cast has stopped streaming frames (the dots stay: the cast is still open), and ends a PipeWire cast with Stop sharing at the header's end, through niri's `stop-cast` by session (a wlr-screencopy cast, like a recording, has none); `privacy.indicators` turns it off.
- **Invariant:** never an Activity and never in the Arbiter. Capturing, the Island does not hide; under a fullscreen window it does not show. A Tray strip shows them past its last slot.
- **Avoid:** privacy Activity, privacy Satellite, indicator (as a type name)

## Banner
A notification card on the focused output, hanging from the Island on its edge and side and clear of the Dock, in its own Top-layer window per monitor (ADRs 0007, 0023). It comes out of the Island and goes back into it, as `banners.entrance` says. The `banners` Module draws it over what `notifications` keeps: the bus, state and history stay there.
- Low shows 4 s, normal 6 s, Critical until closed. The pointer on any Banner pauses every timer. At most `MOST` (`src/banners/stack.rs`) show, newest nearest the Island; the rest queue, Critical first.
- A notification rule (`notifications.rules`) matches a notification as it arrives, by app, desktop entry, category or a piece of its summary, ignoring case, every one it gives; the first matching wins, and none keeps the above. It may show no Banner, play no sound or its own sound at its own volume, keep it out of history, bypass DND, give its Banner its own time (a Critical one too) or change its urgency.
- **Invariant:** a notification plays a sound only with `notifications.sound` on, only when new to the list (a replacement plays none), only if its rule allows (`sound = false`; a rule's own sound beats `suppress-sound`, which otherwise silences all below the sender), and never under DND, a Critical one included, and not while `notifications.fullscreen_silence` holds and a fullscreen window covers the focused output. A sound skipped then is not played later. Its sound is the first of the rule's, the sender's hint, the urgency's (`sound_low`/`_normal`/`_critical`), `sound_file`, `message-new-instant` (ADR 0118); a layer of the user's whose file is gone is skipped with a note, never an error. A sound is a helper (`pw-play`, else `canberra-gtk-play`) started without a shell, one at a time, for at most 5 s, from an absolute existing regular file or a plain theme name, at `sound_volume` on its own stream (never the system volume); Kanade links no audio library.
- A notification whose sender asked for an inline reply (the KDE `inline-reply` action) gets a reply field on its Banner while the `banners` Module is on (ADR 0079). Enter sends the text to that sender only and closes the notification unless it is resident. A Banner with text typed in its field stays until sent or closed.
- **Invariant:** an action runs only on a click, and closes the notification unless it is resident. A Banner closed with no action run (its X, its timer, a click with no default action) leaves the notification in history, unless its rule keeps it out: then it closes as expired, as one whose rule shows no Banner does at once.
- **Invariant:** the history is bounded: 20 notifications of one app and 100 in all, the oldest of the app, then of all, closing as expired when one arrives past them. The Notifications Surface shows an app's notifications as a stack, the newest with a count, once it has more than one; a stack is closed each time the Surface opens, opened by click, Enter or Space, and its cross or Backspace dismisses all it holds. Banners are not stacked.
- **Invariant:** under DND only Critical Banners show, and those whose rule says `bypass_dnd`; DND coming on drops the rest, queued too.
- **Invariant:** app silence is a control on the Notifications Surface and a Dock menu row, kept in `$XDG_STATE_HOME/kanade/notification-silence`, not in settings and not a `notifications.rules` entry. A matching rule still wins. A silenced app with no matching rule gets no Banner and no sound, Critical included; the history keeps it. When both sides have a desktop id, that id is the match; the name is used only when one side has none. Unsilence restores the normal path for the next notification (ADR 0124).
- **Invariant:** with `notifications.fullscreen_silence` on (the default), a non-Critical Banner waits while a fullscreen window covers the focused output, and shows when it no longer does. A Critical Banner, and one whose rule says `bypass_dnd`, still shows. Waiting Banners are not dropped. History is unchanged (ADR 0124).
- **Invariant:** a timed DND (`notifications dnd on 15m`, ADR 0075) ends only the DND it turned on: any other setting of DND cancels its end.
- **Avoid:** toast (as the window; `Toast` is the text a Banner shows), popup

## OSD
A volume, brightness, keyboard backlight or microphone mute level, a mode (Caps Lock, Num Lock, airplane mode), or a keyboard layout switch, shown as a Transient Activity of `Feedback` Priority on the focused output's Island, over any other Activity and its Satellites, and over an open Surface or a Peek as a capsule (ADR 0021). It shows on a change, and on a level key pressed even at a limit. The Island shows over a fullscreen window only while an OSD or a Surface does. The `osd` Module reads `audio` and `brightness` only while they are on, and shows nothing for one that is off. Brightness is one level: the backlight's, else that of the external monitor on the focused output over DDC/CI (ADR 0082).
- A keyboard layout shows in a mode's place, as a lamp with its short label (like "US") and the name XKB gives it, while `osd.layout` is on: only when niri switches between two or more layouts, never for the list niri gives at start or on a change of its config.
- **Invariant:** one OSD at a time: a change while one shows takes its place and starts its time (`osd` in the config) again, so a held key extends one OSD.
- **Avoid:** popup, toast

## Lifetime
`Persistent` lives until withdrawn: media playing, timer, low battery.
`Transient(duration)` expires on its own: a workspace switch, an OSD. It implies no Scope and no Interrupt.

## Arbiter
Function of (registered Activities, the primary shown and since when, now, focused output, the output the frame is for) to a Frame. Owns priority, preemption, primary dwell, expiry and Satellite selection.
- **Invariant:** time is an input. The Arbiter never reads a clock.
- **Invariant:** preemption never destroys. A Persistent Activity hidden by a Transient shows again when the Transient expires, with no re-post.

## Frame
The Arbiter's output: `primary: Option<Activity>`, `satellites: Vec<Activity>` (bounded), `overflow` (the Satellites past the bound, as a count). One per island: Scope and DND decide what it leaves out. Notification toasts are never in it: Banners show them (ADR 0007).
- Not a rendered frame (the runtime's `request_frame`, `KANADE_FRAMES`).

## Scope
`Global` shows on every island. `FocusedOutput` shows only on the island of the focused output. `Output` shows only on the named output, as a Satellite, never as that island's primary; without a name it shows nowhere.

## Interrupt
How an Activity interrupts: `None | Preempt | AutoExpand(duration)`.
- `None` only competes for the primary: a workspace switch wins it while it lives, then the previous primary returns. `Preempt` collapses the open Surface it shows on when it arrives, unless that is its own Surface. `AutoExpand` opens the Activity's own Surface on the focused island for `duration`, then gives every island it changed back its Presentation and Surface.
- A repost with the same Interrupt is no new arrival, so it neither preempts nor expands again. One DND drops never arrives, so it interrupts nothing.
- A Persistent Activity takes attention again by a repost turned Preempt, then one back to None: a pomodoro's phase the timer moved on to by itself is Actionable and Preempt for a few seconds, then Ongoing again (`src/sources/timer.rs`).
- **Invariant:** an explicit user action while an AutoExpand is open (click or press, open or toggle, collapse, pin, a key the Surface consumes) cancels the restore: the user's choice owns the islands. The pointer entering or leaving does not, but the pointer still on the Surface at its deadline keeps it open, like Hold, until it leaves and the grace runs out.
- **Invariant:** a Preempt arriving is policy, not choice: a pending AutoExpand gives the islands back first, then it preempts as it would have without one.

## Presentation
An island's visual level: `Rest | Compact | Split | Peek | Tray | Expanded(Surface)`.
- Rest has no primary Activity and shows the local time (`rest.clock`), and the battery's percent as set (a bolt before it while charging; a bare Rest crowded by dots drops it). A machine with no battery counts `rest.battery` as off. With neither, nothing shows and the island hides as autohide does.
- Tray is Rest's own small form: the time's Peek (the time, with the battery as set, over the date, and the weather and next event as set), then the apps' tray items, raised by the hover delay and gone with the pointer, as a Peek is (ADR 0012). A slot is the item's own target: left activates it (an item that is only a menu shows its menu instead), middle is its secondary activation, the wheel scrolls it, right opens its menu.
- Split is Compact while the Frame has a Satellite: one body in two segments, the primary leading and the top Satellite trailing. It follows the Frame, never input (ADR 0010).
- A Peek shows one Activity, by identity: the one under the pointer, primary or top Satellite. It lasts while that Activity still shows on the island as either.
- The wheel over an island showing Media or the volume (the Compact or Split segment, or the Peek, under the pointer) sets the speaker's level, 5% a line as the Surface's own bar does, and changes no Presentation. It does nothing for any other Activity or when `island.wheel` is off; at Rest, on the Tray or on a Surface it stays with what is there.
- **Invariant:** a body is never larger than the largest (`island.width` by `island.height`), made smaller on an output with no room for it. A list Surface is as tall as its content asks, down to the Peek, shrinking under the pointer while holding the previous bounds in the input region until the pointer moves inside or leaves; the canvas never resizes during a morph (ADR 0030).
- **Invariant:** `Expanded` always carries a Surface. There is no surface-less expanded form.
- **Invariant:** which Surface opens is a pure function of (Presentation, the Activities shown, the segment under the pointer, request, the Surfaces withheld). No remembered last-used Surface: a click at Rest opens Controls.
- **Invariant:** at most one island is Expanded at a time.
- **Rule:** a Presentation change is geometry plus content crossfade in one motion, never collapse-then-grow. A Split whose segments trade places slides each to its new place.
- **Rule:** content takes pointer input only once it fades in at half strength or more; what fades out takes none, so a click never lands on a control the eye has lost, and the click is taken and ignored, so it neither expands, pins nor claims an AutoExpand Surface. A press that spans the fade is dropped too. Keys go to the open Surface, whatever shows.
- **Avoid:** state (state is what Services hold)

## Surface
Full interactive content of an Expanded island: `Media | Notifications | Controls | Launcher | Tray | Clipboard | Calendar | Weather | Wallpapers | Keys | Session | Timer | Capture | Window | Strip | Emoji | Screen time | Files`.
- Media, Notifications and Session are also Activity Kinds. Compact and Peek are the Activity's own small form, and Expanded is its Surface. A Session Activity is a restart, power off or log out counting down, or the sleep, hibernation or power off `battery.critical_action` counts down at a critical battery, a lock, sleep, restart, power off or log out that was refused or failed, or one logind never answered, which may still happen; the Session Surface opens by user action too, and shows a countdown but no refusal, so a refusal or no answer closes it.
- Media's timeline is seekable only in the Surface, never in the Island's forms, and only for a player that says `CanSeek` and gives a track id and a length; a seek never leaves the track.
- Emoji lists every emoji in one scrolling grid, a title over each of Unicode's groups and the ones used lately in a row above the first, searched as it is typed. A press copies the emoji and closes the island once it is on the clipboard, and an emoji copied here is kept in the recents (`$XDG_STATE_HOME/kanade/emoji-recents`, newest first, readable by you only). The Launcher's `:` key opens this Surface at once while `emoji-surface` is on (ADR 0135), so a `:` typed in the Launcher is never a query of its own; with the Module off it is an ordinary search. Space types, so only Enter presses, as in the Launcher and the Clipboard.
- Screen time lists today's focused time per app, from focus changes the shell already receives. It attributes time to the app id, and a window title only while `screen-time.titles` is on. `screen-time.limit` (minutes, 0 for none) posts one Transient the first time today's total crosses it, and that notice names no app. The Module is off unless turned on (ADR 0138).
- Files lists the desktop's own recent documents, from `$XDG_DATA_HOME/recently-used.xbel`, as a shelf: newest first by the file's own mtime, at most 64, only files that still exist and that this user can open, and never one under Kanade's own config, data or state directories. It shows a name and the folder it is in, filtered by both as it is typed; Enter opens the file and Right its folder, through the Launcher's `xdg-open` path. Nothing is deleted, renamed, moved, copied or dragged out, and no file's contents are read. The list is read when the Surface opens and on Ctrl+R, off the draw thread, never on a frame; a list that is missing, over 2 MiB, or names nothing that exists is an empty shelf, and a path or file name is never logged (ADR 0141).
- Controls, Launcher, Tray, Clipboard, Calendar, Weather, Wallpapers, Keys, Timer, Capture, Window, Strip, Emoji, Files and Screen time open only by user action; a click on a Reminder opens Calendar. Screen time's Kind is the limit notice, not the Surface.
- Each is drawn by a Module (`media`, `notification-surface`, `controls`, `launcher`, `tray`, `clipboard-surface`, `calendar-surface`, `weather-surface`, `wallpaper-surface`, `keys-surface`, `timer-surface`, `capture-surface`, `session`, `window-surface`, `strip-surface`, `emoji-surface`, `screen-time`, `files-surface`). One whose Module is off is withheld: it never opens, by click, verb or AutoExpand. A click on an Activity whose own Surface is withheld opens Controls, as for a Kind without one; with Controls withheld too, or at Rest, a click opens nothing.
- **Avoid:** panel, page, view (a view is the runtime's build function)

## Sub-surface
A level inside a Surface, entered from one of its targets and left by a back control or Escape. Controls has five: Wi-Fi, from the Wi-Fi tile's chevron, its password entry, from a network that needs one, Bluetooth, from the Bluetooth tile's chevron, Audio, from the speaker level's chevron (an app that plays has a Route row to move it to another output, ADR 0081), and the arrangement of its tiles (ADR 0067), from the header's pencil. In the Tray Surface an item's menu is one, from its chevron or its slot, and each submenu another below it.
- **Invariant:** every target in a Surface and its sub-surfaces is a key away. Arrows move a ring between targets, Enter or Space presses, Escape leaves a sub-surface for the level it came from, ringed on the target that entered it, and collapses the island only from the top level.
- **Invariant:** a hidden ring (a Surface opened by pointer, or its target gone) is shown by the first key that would act or move, which does nothing else; any other key does nothing. Session, Timer, Capture, Tray and Controls share it through `surfaces/ring.rs` (the first three through `surfaces/pad.rs`); Notifications keeps an equivalent local one.
- **Invariant:** a sub-surface lasts one opening of its Surface. The Surface opens at its top level.
- **Invariant:** a password never prints, in a log or a `Debug`; it lives only until NetworkManager has it.
- **Avoid:** detail (an Activity's Detail), page, view

## LauncherProvider
One kind of answer the Launcher finds for a query: apps, an app's own Desktop Actions (like New Private Window, from two letters, while `launcher.actions` is on), open windows (by title or app, from two letters, with the `windows` Module on), the calculator, destinations (Wi-Fi, Bluetooth, each Surface, Settings), quicklinks (your own rows from `launcher.quicklinks`), and commands (Kanade's own verbs, like Toggle Caffeine, from two letters of their name, while `launcher.commands` is on and the verb's Module is on; commands, windows, destinations, quicklinks and apps also answer an empty query, as the sections below say). Each answer has a Fit, what its row shows, and its action: start an app or one of its actions, focus a window, copy text, open a Surface or Settings, run a command, run a quicklink's argv, or open an address in the browser.
- **Invariant:** every provider ranks on one Fit scale, so their answers interleave; a tie keeps the provider order (calculator, apps, app actions, windows, destinations, quicklinks, commands), then each provider's own.
- **Invariant:** a *typed* query of exactly `@`, `:` or `>` opens that Surface at once, and the key is consumed, when the Surface's Module is on: Wallpapers, Emoji or Clipboard. A prefix pasted, or one a deletion leaves as the whole query, is an ordinary search: only the key that typed it hands off. With the Module off the character is an ordinary search, following the withheld-Surface invariant, so a Surface that is withheld never opens this way either (ADR 0135).
- **Invariant:** a query starting with `?` lists every command, filtered by what follows it, while `launcher.commands` is on; it never answers from another provider and is never offered to the web (ADR 0135).
- **Invariant:** an app's, a command's or a destination's launches are kept as a decayed score (`launcher-usage`, ADR 0127), and the queries that launched it for seventeen days. Both order only answers that fit a query alike; neither lifts one over a better Fit, so an exact name always leads.
- **Invariant:** with nothing typed the Launcher lists sections, in this order: the Recents (the six highest scores), the quicklinks, the commands, the open windows and the apps. Each section has a heading over it, and one with nothing to list shows none. An answer the Recents show is taken out of the section it would list in, so nothing lists twice, and a Recent that no longer exists is left out; every other answer keeps its provider's own order. Typing a character returns to the ranked search.
- **Invariant:** a quicklink is a title and a target of exactly one kind: a URL opened by `xdg-open` alone, or an argv list started directly, each by argv with no shell. With `passes_query = true` an argv quicklink receives the typed query as one final word after the `~/` expansion, is hidden while the query is empty, and is offered for any typed text, ranked last when the text does not fit its title; such a row is a filler, not an answer, so it does not keep the web fallback away. A pass on a URL is refused. An empty `launcher.quicklinks` (the shipped default) contributes no rows and starts no work. A quicklink's title, target and passed query are never logged or kept: it has no learned score, so within a Fit it keeps the config file's order. A layer replaces the list below (ADR 0139).
- Window titles are asked of niri off the view thread each time the Launcher opens and held in the Launcher's memory; `windows` still holds ids only.
- **Invariant:** a command is a static Kanade verb from a closed table in the binary and takes no typed text; it runs through the shell's own socket (`cli::call_within`), so its Module gate applies, and never by calling a source (ADR 0112). The table is not the gesture allow-list.
- **Invariant:** a typed query the providers find nothing for is offered to the web instead: `Search Google for <query>` and `Open <query>` in the default browser, `Action::Browse` with the address the row opens. `xdg-open` alone opens it, by argv with no shell, and the search's address is fixed in the binary, never a setting. The fallback is not a provider, so it can never outrank an answer, and neither the query nor the address is logged or kept; without `xdg-open` neither row shows and one row says so (ADR 0130).
- Kanade's own seam; no plugin API.
- In a visit opened by `kanade dmenu`, the lines read from stdin are the only provider (choose mode); a press answers the waiting client instead of acting.
- **Avoid:** plugin, source (a source posts to the island)

## Hook
A command the user lists in config (`hooks.<event>`, an argv list) that Kanade runs on one of a closed set of events: lock, unlock, wallpaper, tone, charger, suspend, resume, started, dnd, power_profile, logout, reboot, poweroff (the last three fired just before logind is asked, and waited for up to `hooks.timeout`). The Module that owns the fact fires it.
- **Invariant:** a hook never blocks a Service, a watcher or the draw thread; it runs as a bounded child that dies with Kanade, at most one per event at a time, and a fire while it runs is dropped.
- **Invariant:** hooks are set in files, never through `kanade config set` or Settings. The payload is environment variables only; a failure logs the first line of the command's own error.
- **Invariant:** a failing hook shows in `kanade status`, never as an Activity.
- Not `modules::Hooks`, the `status` and `reload` callbacks of a Module's descriptor.
- **Avoid:** plugin, script event soup, subscription

## Clipboard
The history of what was copied, text and images, kept by the `clipboard` Module and shown by the Clipboard Surface (ADR 0011, ADR 0019, ADR 0114, ADR 0115). Newest first, the pinned entries before the rest.
- **Entry**: one item of the history: an `id`, its content (text, or an image with its type) and whether it is pinned. The id is never reused, and a copy of its content again keeps it. **Avoid:** item, clip, snippet (a snippet is text kept on purpose and filled in, not built and declined, ADR 0114, ADR 0115).
- **Pin**, of an entry (not the Island's Pin above): marks an entry to stay. It sorts first and the bounds (50 entries, 64 MiB) never evict it, though it counts in them; pins have their own caps, 20 and 16 MiB together, and one past them is refused with a line in the footer. Pins, and only pins, persist across restarts as plaintext in `$XDG_STATE_HOME/kanade/clipboard-pins` (0600), ADR 0115; the rest of the history is memory only.
- **Invariant:** a pin marks an entry that is already in the history, so one that passed the sensitive gate (ADR 0019): a selection marked sensitive is never read, never kept and so never pinned. A pin adds no filter of its own and no heuristic.
- **Invariant:** Clear all (the held button, `kanade clipboard clear`) keeps the pins and says how many; removing an entry removes it, pinned or not. Nothing of the history but the pins is written to disk, and an unpinned or removed pin is gone from the file at once. A stored pin is loaded at start under a fresh id, within the pins' caps and the entry limit, and a copy of its content is that pinned entry, not a new one.
- **Invariant:** a picture's preview is a file under `$XDG_RUNTIME_DIR/kanade-clipboard/<pid>` (tmpfs, the directories 0700, the file 0600), named by the entry's id and an extension from a fixed table, written only for a row that shows, deleted when its entry is removed, evicted or cleared, and at the next start swept only for the Kanades whose pid is gone (never a running one's), so they also stay after a clean exit or SIGTERM until then (tmpfs, 0600, same user). With no runtime directory there is no preview. What was copied and where a preview is are never logged.
- **Avoid:** favorite, star, sticky (for a pin of an entry)

## Dictation
Speech turned into text by the `dictation` Module: the user starts a recording, stops it, and one request to the Gemini API returns the text, which is then inserted (copied by default).
- **Invariant:** nothing records or is sent unless a key is in the Secret Service and the user's consent to the disclosure is recorded (audio goes to Google; on the free tier it may be used to improve Google's products). Audio is kept in memory only and capped at 120 s (ADR 0111).
- The microphone indicator is the privacy cluster's, read from the PipeWire graph; dictation adds none.
- Text is typed or pasted only into the window the recording started in, not in `dictate.never_in`, and not while the session is locked, as far as a check just before the keys can tell (focus can still move after it); any doubt falls back to a sensitive copy, and the Activity says which. The audio cannot be recalled once the recording is stopped: Cancel then only discards the result. A recording not yet stopped is cancelled when the session locks.
- A transcript is not a Notification and not a clipboard entry until inserted. Its Activity (Kind `Dictation`) shows the seconds heard, that it transcribes, then the text or why it failed; it has no Surface of its own.
- **Invariant:** the key and the consent are handled only by `kanade dictate key|consent|forget`, in the user's terminal: the key by stdin, never an argument, and the consent only from a typed phrase at a terminal. Neither passes through the shell's socket.
- **Avoid:** ASR, STT, voice input, voice typing, speech widget

## Semantic tree
The windows' interactive controls as named nodes (`id`, `parent`, `role`, `label`, `value`, states, `bounds`), read and acted on from outside with `kanade ui` (ADRs 0095 to 0099). It is the input targets of each window's last frame, with the role and name a view says where it builds them, not a second tree.
- **Invariant:** a node's name is source text; what the user or the world wrote (a notification, a title, a track, the clipboard, a network, typed text) is content, `<redacted>` unless `ui.describe = "content"`, and a password is never stored in any mode.
- **Invariant:** the lock screen is never in it and `kanade ui` is refused while the session is locked; `ui.describe = "off"` keeps no tree and refuses everything.
- **Invariant:** an act goes through the runtime's pointer and keyboard path to the named node, never by coordinates, and leaves the user's pointer where it was.
- Not an accessibility tree: shaped for an AccessKit or AT-SPI adapter, which does not exist.
- **Avoid:** DOM, accessibility tree, UI automation API

## Owned event
A calendar event Kanade wrote from the Calendar Surface: a file `<name>.ics` in `$XDG_DATA_HOME/kanade/events` whose one event has UID `<name>@kanade` (ADR 0045). Only an owned event is changed or removed from the Surface, and every other event is read-only there. A change rewrites only its SUMMARY, DTSTART and DTEND (ADR 0113); its times are floating.
- **Invariant:** a name or UID that is not of this form never names a path, so a copy of a file elsewhere, a vdir's file or Google's mirror is never owned.
- **Invariant:** a change or removal is written only if the file still holds the title, start and end the Surface showed; otherwise it says "Changed elsewhere" and writes nothing. A file Kanade cannot change without losing something is refused, not rewritten: another UID's event in it, a repeat, a zoned time, or an event the typed form cannot say (several days, an end on a later day, a timed event with no end, seconds, an all-day title that starts like a time). Removal also needs every event in the file to be this one's.
- **Avoid:** local event, own calendar

## Calendar subscription
A public iCalendar feed, an `https://` address in `calendar.subscriptions` (`webcal://` read as `https://`), fetched into a read-only file the calendar reads beside the local ones (ADR 0062). A feed is named by its place in the list, never by address.
- **Invariant:** the address may hold a secret: it is never logged, put in `kanade status` or an error, or named in a file name. Only https is spoken, redirects included.
- **Invariant:** a failed feed keeps its last good copy and backs off; nothing is fetched without a feed or with `calendar-subscriptions` off.
- Not a Hook (`hooks.<event>`), nor a Service's `watch`.

## Reminder
An Activity saying a calendar event is coming: its title and how long until it starts, or ends for an alarm on its end, with Open and Dismiss, and Join while the event links to a video meeting (ADR 0043). Join opens the first `https` link of its URL, LOCATION or DESCRIPTION whose host `sources/meeting.rs` lists (Google Meet, Zoom, Teams, Webex, Jitsi, Whereby) with `xdg-open`, by argv with no shell, never logging it. It comes from the event's own `VALARM`s that display or play a sound, or `calendar.reminder` minutes before a timed event with none. Persistent, Ongoing, Global, interrupting nothing; it goes 5 minutes after its event starts, an hour after it went off at most, or on Open, Join or Dismiss.
- **Invariant:** a reminder goes off once. Which went off is kept in a file under the state directory, keyed by event, occurrence and alarm, the default lead by a mark of its own, so a restart, a clock set or a new `calendar.reminder` never fires one again.
- **Invariant:** a missed reminder goes off only until 5 minutes after its event starts; past that it is dropped, never shown late.
- **Avoid:** alarm (the `VALARM` it comes from), alert, notification

## Recording
Kanade's own screen recording (`wf-recorder` through niri's wlr-screencopy, ADR 0013, 0116): the focused output, an area or a window (a niri screen cast read through `gst-launch-1.0`, ADR 0117), with no sound, the system's or the microphone's, as an Activity of Kind Recording while it runs and a saved file in `~/Videos/Screencasts`.
- A **Segment** is one run of the recorder: `wf-recorder`, or for a window the pair `gst-launch-1.0` (the encoder) and `ffmpeg` (the writer) with the niri screen cast session they read. Pausing ends the segment (SIGINT, a playable mp4) and resuming starts the next; Stop joins them (`ffmpeg`, copying) into the one file. The first segment is written to the file itself, so a recording never paused is the file from its first frame, with nothing to move or join; the segments after a pause live in the hidden folder `.Screencast from <stamp>.parts/` beside the file until saved. While they are joined the Activity says Saving.
- **Invariant:** the microphone is recorded only when the user chose it for that start; it is never a config default or a persisted choice (the Capture Surface remembers the last choice in memory only), and the Launcher cannot start it.
- **Invariant:** locking the session while recording stops and saves it, and a start or resume is refused while locked.
- **Invariant:** Kanade deletes only the segments, list and joined file it made, and only after the recording was saved (an empty file the recorder left is no recording and goes at once); on a failure they are kept and the Activity names the file and the folder. A crash after a resume leaves the file with the first segment and the folder with the rest, which Kanade never deletes.
- **Invariant:** saving never replaces a file: a name taken meanwhile keeps its file and the recording is left in the folder.
- **Avoid:** screencast (the file's name, not the term), clip (a Clip is an Activity's detail)

## Satellite
Small secondary indicator beside the primary island: timer, VPN, low battery, a Live Activity.
- Capture the privacy cluster sees is no Satellite: the cluster shows it, as dots inside the body. Kanade's own recording is an Activity like any other, so it can be one, carrying Pause or Resume and Stop (a paused one shows a Pause icon).
- **Invariant:** only Persistent Ongoing or Critical Activities that are not the primary, and a playing Media Activity beside such a primary, so a song keeps its place beside a timer; it counts after the others and shows its cover. At most `SATELLITES` (`src/island/arbiter.rs`) show, highest first; the rest are a count.
- **Rule:** a Satellite comes out from under the body and tucks back under it, fading; one that changes place slides. None pops. Coming out or going, it is one liquid shape with the body: a neck stretches and snaps as it parts (ADR 0101), except under reduced motion.
- While the island is Split, or a Peek out of one, the top Satellite is the trailing segment, not a dot; it counts toward `SATELLITES`. The other dots and the count take no pointer.

## Weather location
Where the Weather is for: `weather.location` (latitude, longitude) with `weather.place` as the name the Surface titles itself with. Settings sets both for you from a place name ("City, Country", searched by Open-Meteo's geocoding, several matches listed to pick from) or from the machine's IP address through ipwho.is (ADR 0136); the coordinates stay writable by hand.
- **Invariant:** the weather location is never found by itself. The place search and the IP lookup run only on a user's press in Settings, never at start, on a reload or on a timer; the IP address leaves the machine only for the IP lookup, and neither what was typed nor what came back is logged.
- **Invariant:** a name that matches several places is never settled silently: only a lone match, or "Name, Qualifier" fitting exactly one place, is set without a pick.
- **Avoid:** geolocation (it reads as GeoClue or the browser's), auto-location

## Hold
The keyboard an island keeps (`Keyboard::Exclusive`) after an IPC or keybind open, so Escape reaches it without a press.
- **Invariant:** only an island opened without a press holds. The hold ends when the island collapses.
- **Invariant:** an unattended hold is bounded. An ignored island collapses on its own; only keys the open Surface consumes restart the bound, and typing into a held Surface that does not type collapses it. Once the pointer enters, the hold lasts until the pointer leaves and the grace runs out. A Pin ends it. The one exception is a visit a `kanade dmenu` waits on: as with any dmenu, its hold lasts until the user answers with Escape, a choice or another Surface, since Escape always reaches it. The bound is `HOLD` in `src/island/service.rs`; the niri measurements behind it are in `docs/archived/plan.md` §6.1.
- **Invariant:** the first-start tour never holds (ADR 0123). It opens at a start that already has a window focused, so it takes no keyboard until it is pressed, and it stays until the user closes it: no hold, no grace, no self-close. Escape, the final advance, a press on it, another Surface opening over it, a typed character, or `kanade island collapse` ends it; so does niri's overview, which ends every open Surface.
- **Avoid:** hold for a Peek or Surface the user opened. That is the island's Presentation.

## Pin
A right click keeps an island's Peek or open Surface up after the pointer leaves, with no leave grace. The body shows a ring while pinned.
- Right click raises the island to what the pointer would raise it to, pinned: on Compact or a Split segment it peeks that Activity pinned, at Rest it opens Controls pinned (no context menu), on a tray slot it opens that item's menu, not pinned (ADR 0012), on a Peek or open Surface it pins or unpins it. On a Surface's own control it pins too and never presses it.
- Escape ends a pin only while the island has keyboard focus (a pinned island gives it back, see below); `kanade island collapse` and `toggle` close a pinned Surface.
- **Invariant:** a pin lasts only for its current raised Presentation. Collapsing or replacing it clears the pin: collapse, a click expanding the Peek, another Surface opening, the overview, another island expanding. The peeked Activity leaving both the primary and the top Satellite clears a pinned Peek; Preempt clears a pinned Surface, unless it is its own. Nothing pinned is remembered.
- **Invariant:** a pinned island never holds. Pinning a held island gives the keyboard back; a press on the island takes it again.
- No per-Activity context action on right click: a click already opens the Activity's Surface, where its actions are, and a hidden action would run before it could be seen.
- **Avoid:** sticky, lock (lock is the screen locker). A pinned Clipboard entry is another thing (see Clipboard).

## Confirm hold
How a Surface confirms a destructive press: the target fills while the pointer button or the key is held, and acts only when it is full after 700 ms (ADR 0085). Calendar remove (`d`) and the Clipboard's Clear all.
- **Invariant:** a release before it is full acts on nothing, and drains the fill. Any key coming up, the next key going down, the pointer leaving the target or the window, or the Surface being visited anew ends it. It acts on a thread of its own, never in a view.
- **Invariant:** reduced motion has no fill or drain, only the same wait.
- **Avoid:** Hold (that is the island's keyboard), double press, arm

## Running app
One app's open windows, as `windows` groups them for the Dock: those whose `app_id` matches one desktop entry (ADR 0014). Windows that match none group by `app_id`; a window without one stands alone.
- **Invariant:** nothing of niri leaves `windows` but window ids. A window's title is asked of niri each time the Dock's menu or its hover list opens, and lives only as long as that list.
- **Invariant:** an override in `windows.apps` is final: one naming a missing entry leaves the app unmatched, never guessed.
- **Avoid:** client, toplevel, task, program

## Dock
Strip per monitor on the edge and side `dock.edge` and `dock.align` give it: pinned apps (`dock.pinned`), then Running apps not pinned, a dot between each running one and the edge. A left click launches an app without windows, else focuses its window, cycling while it has the focus. A middle click starts a new instance of the app through the same launch as an app without windows, running or not. A right click opens the app's menu: pin or unpin, Silence or Unsilence, one row per open window (its title, most recently focused first where niri says when; Enter focuses it, its close target or Delete closes it), then the app's own actions. The pointer on a running icon opens that window list alone, the same rows, and leaves it when the pointer leaves the icon and the list; an app with no windows does not. The right-click menu wins if it is open. Neither list warps the pointer. Files dragged onto an icon start that app with them, each as an argument of its own and never through a shell, when its `Exec` has a standing `%f`, `%F`, `%u` or `%U` and no `Path`; an app that takes one file at a time starts once per file, up to 8. Icons sized by `dock.size` grow near the pointer (`dock.magnification`); an autohidden Dock comes out when the pointer touches the edge under it. It steps aside while the Island grows over it, as with a Surface open beside it. Where it shares the Island's edge and side, `dock.merge` joins them instead (ADR 0031): one window, one pane: the Dock a plate on the edge with the Island crowning it (`crown`), the Island the middle piece of its row (`keystone`), or the Dock folded into the Island and out under the pointer (`fold`), its running apps' dots kept in the Island's body while the icons are in it.
- **Invariant:** app data only from `windows`; never reads `workspace` or raw niri objects, the niri source hands it the output names.
- **Avoid:** taskbar, launcher (the Launcher is a Surface), panel

## Pane
One opaque piece of the shell, tinted toward the wallpaper's hue, with a top light edge and a rim: the Island's body, the Dock, or one Banner (ADR 0050). Its finish, `appearance.finish`, is the tinted solid (dark, light text), paper (light, dark ink) (ADR 0051), or Blur (the tinted solid, or paper's ramp while `appearance.tone` is light, translucent over what the compositor blurs behind the pane's outline) (ADR 0057). A merged Island and Dock are one pane with one outline (ADR 0031).
- **Invariant:** tinted from the wallpaper's hue (a cool slate until Matugen answers), with text that suits its finish, whatever `appearance.tone` says (except Blur, which is paper's ramp while the tone is light); opaque and reads nothing behind it. Blur is translucent, the compositor blurs behind it, and it is opaque where the compositor cannot.
- **Invariant:** the desktop's higher contrast (settings portal, `appearance.accessibility`) makes a Blur Pane the solid finish of `appearance.tone` for as long as it asks, without changing the saved `appearance.finish` (ADR 0080).
- **Avoid:** frost, material (the choice is the finish), glass, frosted (the Blur finish has no frost of its own), layer (a `pane::layer` widget), surface (a Surface is the Island's)

## Idle stage
What the `idle` Module does after the user has been idle for so long (ADR 0047): `idle.dim`, `idle.lock`, `idle.screen_off`, `idle.suspend`, each optional. The Dim grace comes first: a click-through Overlay layer on every output fading toward dark until the lock, taken away at once by any input.
- **Invariant:** the lock never waits on the Dim grace: a dim that cannot draw still locks on time, and a lock that fails is asked again while the user stays idle. An error never leaves an idle session unlocked, and with a lock stage the monitors going off and the suspend wait until it holds.
- **Invariant:** an inhibitor holds every stage: an app's Wayland idle inhibitor (niri never says idle), a logind idle inhibitor (`BlockInhibited` naming `idle`), or caffeine. A logind that cannot be asked inhibits nothing.
- **Invariant:** the stage scheduler takes time as an input, never reads a clock.
- **Avoid:** screensaver, DPMS (niri powers monitors off), idle inhibit (an inhibitor is what holds the stages)

## Gesture
A stroke the user recorded under a name (`kanade gesture record NAME`, three strokes, kept normalized in the state file `gestures.toml`, ADR 0106) and mapped, in `gestures.<name>` of the config, to a Kanade verb. `kanade gesture` covers the focused output with an overlay that takes the pointer; the stroke drawn is matched by a $1 recognizer and a match runs its verb (ADR 0105 to 0109).
- **Invariant:** a Gesture runs only a Kanade verb, through the shell's own socket, never a shell command; user commands go through a Hook. Verbs that wait on the user, run a program or end the session are refused when the config is read.
- **Invariant:** the overlay always lets go of the pointer and keyboard: Escape, a right click, the end of the stroke, 8 s and a lock close it, and it never opens while locked.
- **Invariant:** no match, or two Gestures too close to tell apart, runs nothing and shows a Transient "No gesture".
- The templates are state (rewritten by `record`), the verbs are config (user-owned); neither holds the other.
- **Avoid:** macro, shortcut, mouse gesture (a gesture here is drawn on an overlay, not a global pointer motion), Hook (runs a user command on an event)

## niri config
niri's `config.kdl`, which `kanade niri-config` changes on request (ADR 0119); the one user file Kanade edits. Its path is `$NIRI_CONFIG`, else `$XDG_CONFIG_HOME/niri/config.kdl`, else `~/.config/niri/config.kdl` (`sources::binds::config`; `/etc/niri/config.kdl` is only read).
- A setting is a single-valued node at a dotted path (`layout.gaps`), changed by splicing text into the file's bytes at the node's span.
- An **include owner** is the file a node comes from when `config.kdl` includes others (niri reads includes in order and the last setting wins). Kanade edits a node only when `config.kdl` itself owns it, and no included file sets it, before or after; `appearance.sync`'s `kanade.kdl` owns the window colors.
- **Invariant:** nothing outside the edited node's bytes changes (comments, `/-`, line endings); the file is replaced only after `niri validate` accepted the staged result, and only if its hash still equals the one read; a `set` or `remove` validates the original first (`undo` and `restore` do not, as a broken config is when they are needed, and need only the result to validate); a symlink or hard link is never written; a leading BOM stays.
- A **backup** is the file's bytes before a change, in `$XDG_STATE_HOME/kanade/niri-config/` (the newest 20, and the pinned first bytes, `.orig`), a ring per config, keyed by a hash of its canonical path, so configs of one file name do not mix. `undo` needs the file as the last change left it.
- A **trial** is a change to a connected monitor (`niri msg output <name> ...`) made in niri's memory only, before it is written: the Settings Outputs section starts one at the first change, restarts its 15 s at each further change, and ends it by **Keep** (the changes are written to the monitor's `output` block in one validated change) or **Revert** (the monitor goes back to **Previous**, what niri's `Outputs` IPC reported before the first change, never what the file says). One trial runs at a time. **Invariant:** `$XDG_STATE_HOME/kanade/outputs-pending.json` holds Previous before niri is told of the first change and is deleted when the trial ends; a Kanade that finds a file at start clears it without touching niri when its `saved` marker is set (Keep wrote the config), and applies only an unsaved one, unless it is older than 5 minutes: then it puts the output back to Previous before deleting the file when niri still shows exactly the trial's applied edits, and deletes it otherwise. The last enabled monitor is never turned off by a trial.
- **Avoid:** niri settings file, compositor config (this one only is edited), rewrite (an edit splices)

## SDDM overlay
The directory Kanade owns (`/usr/local/share/kanade-sddm/`: `themes/<theme>/` for SDDM to load, root-owned) that SDDM reads the login screen's theme from, after root makes it once and writes `/etc/sddm.conf.d/zz-kanade.conf`. The picture behind the clock is not in it: it is `/usr/local/share/kanade-sddm-data/backdrop.png`, owned by the desktop user, the one path the theme loads from outside its own tree (ADR 0120). Written by `kanade sddm`; shown, never changed, by `kanade doctor` and Settings' Login screen page.
- A theme in it has a **class** from its files: A reads a picture from a `theme.conf` setting (Kanade writes the picture there), B draws a fixed image, C has none, D plays a video; only A takes a picture.
- **Invariant:** Kanade never runs a command as root and never writes outside the overlay, its state and its settings file; it prints the root commands.
- **Invariant:** nothing is written through a symbolic link, to a non-file, or to a file with another hard link; every file is 0644 and directory 0755 whatever the umask.
- **Invariant:** `sddm.sync` is off by default, because the picture is readable by `sddm` and anyone who can read the data directory.
- **Avoid:** greeter (Kanade ships a theme, not a greeter), login manager

## Module
A feature the user can turn off in `[modules]` of the config: `src/modules/catalog.rs` lists every one, with the Modules it requires and those it can do without.
- `island` is the core and cannot be turned off; every other Module requires it.
- Every Module is on unless the config turns it off, but `idle`, which is off unless turned on, so an idle daemon like hypridle keeps working (ADR 0047), and `screen-time`, which is off unless turned on (ADR 0138).
- **Invariant:** a Module that is off starts no thread or helper process, opens no window, reads no Service, posts nothing and answers its verbs with `module <name> is off`. A Service only it reads stays cold.
- **Invariant:** a Module whose requirement is off, or whose requirements loop, is off too, and stderr names why. One missing an optional Module runs without it.
- A Module whose loss is easy to miss says so at every start while off (`privacy`: no capture indicators).
- Do Not Disturb belongs to `notifications`: with it off, the verb is refused and the Controls switch is unavailable. The Night Light switch likewise belongs to `night-light`, and is unavailable without `wlsunset`; the Caffeine switch belongs to `caffeine`. The microphone's mute is in the Audio sub-surface, not a switch. Focus turns on Do Not Disturb, caffeine and optionally the night light together, through the Modules that own them, and turns off what it turned on; it belongs to `focus`, which runs with whichever of them are on. The Limit Charge switch belongs to `battery`, and shows only while UPower says the battery supports a charge limit. The Power Profile switch belongs to `power`, and cycles the profiles the machine has. `kanade wifi`, `bluetooth` and `power-profile` belong to `network`, `bluetooth` and `power`, and `media play-pause|next|previous` to `media`; each answers once the daemon or player has it as asked, or refuses saying why.
- Which Modules run is decided once at start; a change takes a restart.
- **Avoid:** plugin (not built), feature flag, Service (a `Service` is shared state a Module reads)
