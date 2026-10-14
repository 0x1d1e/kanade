# Kanade domain language

Use these terms in code, tests, issues and docs. Avoid the listed synonyms. Terms for parts not built yet (Activity, Arbiter, Frame, Satellite) are fixed here ahead of the code.

## Island
The single physical surface on one monitor. One per monitor.
- Monitor and output name the same thing: the runtime says monitor, niri says output.
- An autohidden Island (`island.autohide`), or one whose Rest has nothing to show (`rest.clock` and `rest.battery` off), slides past its edge once at Rest a moment, with no Satellite, nothing pinned, outside the overview and the pointer away. It comes back as the pointer touches the edge under it, where a click opens it as on the body, or as anything shows on it.
- **Avoid:** pill (as a type name; fine in prose for the small shape), widget, popup, dropdown

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
- **Invariant:** one its program stops talking to is withdrawn after `live.stale`; `activity run` keeps its own alive while its command runs.
- **Invariant:** a repost is cheap: a change within `REPOST` (250 ms) of the Activity's last post is held and posted when due, the newest of those held, and one that changes nothing is never posted (ADR 0058).
- `activity hook claude` is a client of these verbs, not another path to the Island: a Claude Code session's events become one Live Activity, and a late event never brings an ended one back (ADR 0058).
- A notification with a `value` hint under 100 is also a Live Activity of the `notifications` Module, id `notification.<app>.<id>`: the same policy, in the same 8, kept while the notification lasts, ended as done at 100, and withdrawn when it closes or is replaced without the hint. It follows the notification's rule and Do Not Disturb as its Banner does (ADR 0059).
- **Avoid:** job, task (as the type; `Job` is its Detail), progress notification

## Device event
A Bluetooth device connecting or going, the charger plugged in or out, the default sound output changing, or a Wi-Fi network joined, shown as a Transient Glance Activity on the focused output (the Detail is `Hardware`, posted under the Kind of its source: Bluetooth, Battery, Volume or Network). It shows a short title for Compact and a line for Peek, and nothing to press. `island.device_events` turns them off.
- **Invariant:** each is told from a change a Module's own Service shows, so a Module that is off shows none. What a watcher first sees is a baseline, never an event, so starting the shell, a daemon or an adapter, or a battery or Wi-Fi device turning up, shows nothing; an adapter turned off disconnects its devices without news.
- **Invariant:** one Activity per class, so a burst takes one place; the same event again within 10 s shows nothing.
- **Avoid:** notification, toast

## Privacy cluster
The microphone, camera and screen capture in use, a dot of its color and shape each (round microphone, square camera, ring screen cast; `theme::signals`) at the trailing end of the Island's body, inside it: in the small forms (Rest, Compact, Split, Peek) and, rising to the top corner as the body grows, every Surface (ADRs 0005, 0033). They fade in and out. In the small forms what is centered stands toward the start while they show. It takes no pointer; the Controls Surface names the apps and says whether a screen cast captures a window or a screen, never whose, and ends a PipeWire cast with Stop sharing at the header's end, through niri's `stop-cast` by session (a wlr-screencopy cast, like a recording, has none); `privacy.indicators` turns it off.
- **Invariant:** never an Activity and never in the Arbiter. Capturing, the Island does not hide; under a fullscreen window it does not show. A Tray strip shows them past its last slot.
- **Avoid:** privacy Activity, privacy Satellite, indicator (as a type name)

## Banner
A notification card on the focused output, hanging from the Island on its edge and side and clear of the Dock, in its own Top-layer window per monitor (ADRs 0007, 0023). It comes out of the Island and goes back into it, as `banners.entrance` says. The `banners` Module draws it over what `notifications` keeps: the bus, state and history stay there.
- Low shows 4 s, normal 6 s, Critical until closed. The pointer on any Banner pauses every timer. At most `MOST` (`src/banners/stack.rs`) show, newest nearest the Island; the rest queue, Critical first.
- A notification rule (`notifications.rules`) matches a notification as it arrives, by app, desktop entry, category or a piece of its summary, ignoring case, every one it gives; the first matching wins, and none keeps the above. It may show no Banner, play no sound, keep it out of history, bypass DND, give its Banner its own time (a Critical one too) or change its urgency.
- **Invariant:** a notification plays a sound only with `notifications.sound` on, only when new to the list (a replacement plays none), only if its rule and `suppress-sound` allow, and never under DND, a Critical one included. A sound is a helper (`pw-play`, else `canberra-gtk-play`) started without a shell, one at a time, for at most 5 s, from a `sound-file` hint that is an absolute existing regular file or a plain `sound-name`; Kanade links no audio library.
- **Invariant:** an action runs only on a click, and closes the notification unless it is resident. A Banner closed with no action run (its X, its timer, a click with no default action) leaves the notification in history, unless its rule keeps it out: then it closes as expired, as one whose rule shows no Banner does at once.
- **Invariant:** the history is bounded: 20 notifications of one app and 100 in all, the oldest of the app, then of all, closing as expired when one arrives past them. The Notifications Surface shows an app's notifications as a stack, the newest with a count, once it has more than one; a stack is closed each time the Surface opens, opened by click, Enter or Space, and its cross or Backspace dismisses all it holds. Banners are not stacked.
- **Invariant:** under DND only Critical Banners show, and those whose rule says `bypass_dnd`; DND coming on drops the rest, queued too.
- **Avoid:** toast (as the window; `Toast` is the text a Banner shows), popup

## OSD
A volume, brightness, keyboard backlight or microphone mute level, a mode (Caps Lock, Num Lock, airplane mode), or a keyboard layout switch, shown as a Transient Activity of `Feedback` Priority on the focused output's Island, over any other Activity and its Satellites, and over an open Surface or a Peek as a capsule (ADR 0021). It shows on a change, and on a level key pressed even at a limit. The Island shows over a fullscreen window only while an OSD or a Surface does. The `osd` Module reads `audio` and `brightness` only while they are on, and shows nothing for one that is off.
- A keyboard layout shows in a mode's place, as a lamp with its short label (like "US") and the name XKB gives it, while `osd.layout` is on: only when niri switches between two or more layouts, never for the list niri gives at start or on a change of its config.
- **Invariant:** one OSD at a time: a change while one shows takes its place and starts its time (`osd` in the config) again, so a held key extends one OSD.
- **Avoid:** popup, toast

## Lifetime
`Persistent` lives until withdrawn: media playing, timer, low battery.
`Transient(duration)` expires on its own: a workspace switch, an OSD. It implies no Scope and no Interrupt.

## Arbiter
Function of (registered Activities, the primary shown and since when, now, focused output) to a Frame. Owns priority, preemption, primary dwell, expiry and Satellite selection.
- **Invariant:** time is an input. The Arbiter never reads a clock.
- **Invariant:** preemption never destroys. A Persistent Activity hidden by a Transient shows again when the Transient expires, with no re-post.

## Frame
The Arbiter's output: `primary: Option<Activity>`, `satellites: Vec<Activity>` (bounded), `overflow` (the Satellites past the bound, as a count). One per island: Scope and DND decide what it leaves out. Notification toasts are never in it: Banners show them (ADR 0007).
- Not a rendered frame (the runtime's `request_frame`, `KANADE_FRAMES`).

## Scope
`Global` shows on every island. `FocusedOutput` shows only on the island of the focused output.

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
- **Invariant:** a body is never larger than the largest (`island.width` by `island.height`), made smaller on an output with no room for it. A list Surface is as tall as its content asks, down to the Peek, never shrinking from under the pointer; the canvas never resizes during a morph (ADR 0030).
- **Invariant:** `Expanded` always carries a Surface. There is no surface-less expanded form.
- **Invariant:** which Surface opens is a pure function of (Presentation, the Activities shown, the segment under the pointer, request, the Surfaces withheld). No remembered last-used Surface: a click at Rest opens Controls.
- **Invariant:** at most one island is Expanded at a time.
- **Rule:** a Presentation change is geometry plus content crossfade in one motion, never collapse-then-grow. A Split whose segments trade places slides each to its new place.
- **Rule:** content takes pointer input only once it fades in at half strength or more; what fades out takes none, so a click never lands on a control the eye has lost, and the click is taken and ignored, so it neither expands, pins nor claims an AutoExpand Surface. A press that spans the fade is dropped too. Keys go to the open Surface, whatever shows.
- **Avoid:** state (state is what Services hold)

## Surface
Full interactive content of an Expanded island: `Media | Notifications | Controls | Launcher | Tray | Clipboard | Calendar | Weather | Wallpapers | Keys | Session | Timer | Capture`.
- Media, Notifications and Session are also Activity Kinds. Compact and Peek are the Activity's own small form, and Expanded is its Surface. A Session Activity is a restart, power off or log out counting down, or the sleep, hibernation or power off `battery.critical_action` counts down at a critical battery, a lock, sleep, restart, power off or log out that was refused or failed, or one logind never answered, which may still happen; the Session Surface opens by user action too, and shows a countdown but no refusal, so a refusal or no answer closes it.
- Media's timeline is seekable only in the Surface, never in the Island's forms, and only for a player that says `CanSeek` and gives a track id and a length; a seek never leaves the track.
- Controls, Launcher, Tray, Clipboard, Calendar, Weather, Wallpapers, Keys, Timer and Capture have no Activity of their own Kind. They open only by user action; a click on a Reminder opens Calendar.
- Each is drawn by a Module (`media`, `notification-surface`, `controls`, `launcher`, `tray`, `clipboard-surface`, `calendar-surface`, `weather-surface`, `wallpaper-surface`, `keys-surface`, `timer-surface`, `capture-surface`, `session`). One whose Module is off is withheld: it never opens, by click, verb or AutoExpand. A click on an Activity whose own Surface is withheld opens Controls, as for a Kind without one; with Controls withheld too, or at Rest, a click opens nothing.
- **Avoid:** panel, page, view (a view is the runtime's build function)

## Sub-surface
A level inside a Surface, entered from one of its targets and left by a back control or Escape. Controls has four: Wi-Fi, from the Wi-Fi tile's chevron, its password entry, from a network that needs one, Bluetooth, from the Bluetooth tile's chevron, and Audio, from the speaker level's chevron. In the Tray Surface an item's menu is one, from its chevron or its slot, and each submenu another below it.
- **Invariant:** every target in a Surface and its sub-surfaces is a key away. Arrows move a ring between targets, Enter or Space presses, Escape leaves a sub-surface for the level it came from, ringed on the target that entered it, and collapses the island only from the top level.
- **Invariant:** a hidden ring (a Surface opened by pointer, or its target gone) is shown by the first key that would act or move, which does nothing else; any other key does nothing. Session, Timer, Capture, Tray and Controls share it through `surfaces/ring.rs` (the first three through `surfaces/pad.rs`); Notifications keeps an equivalent local one.
- **Invariant:** a sub-surface lasts one opening of its Surface. The Surface opens at its top level.
- **Invariant:** a password never prints, in a log or a `Debug`; it lives only until NetworkManager has it.
- **Avoid:** detail (an Activity's Detail), page, view

## LauncherProvider
One kind of answer the Launcher finds for a query: apps, an app's own Desktop Actions (like New Private Window, from two letters, while `launcher.actions` is on), open windows (by title or app, from two letters, with the `windows` Module on), the calculator, emoji, destinations (Wi-Fi, Bluetooth, each Surface, Settings; anything after `@` is the Wallpapers destination). Each answer has a Fit, what its row shows, and its action: start an app or one of its actions, focus a window, copy text, or open a Surface or Settings.
- **Invariant:** every provider ranks on one Fit scale, so their answers interleave; a tie keeps the provider order (calculator, apps, app actions, windows, destinations, emoji, clipboard history), then each provider's own.
- **Invariant:** how often an app started (`launcher-usage`, bounded, counts halving at a cap) orders only apps that fit a query alike; it never lifts one over a better Fit, so an exact name always leads. With nothing typed, the last six started list first.
- Window titles are asked of niri off the view thread each time the Launcher opens and held in the Launcher's memory; `windows` still holds ids only.
- Emoji answer only a query starting with `:`, and apps answer none of those.
- Kanade's own seam; no plugin API.
- In a visit opened by `kanade dmenu`, the lines read from stdin are the only provider (choose mode); a press answers the waiting client instead of acting.
- **Avoid:** plugin, source (a source posts to the island)

## Hook
A command the user lists in config (`hooks.<event>`, an argv list) that Kanade runs on one of a closed set of events: lock, unlock, wallpaper, tone, charger, suspend, resume. The Module that owns the fact fires it.
- **Invariant:** a hook never blocks a Service, a watcher or the draw thread; it runs as a bounded child that dies with Kanade, at most one per event at a time, and a fire while it runs is dropped.
- **Invariant:** hooks are set in files, never through `kanade config set` or Settings. The payload is environment variables only; a failure logs the first line of the command's own error.
- **Invariant:** a failing hook shows in `kanade status`, never as an Activity.
- Not `modules::Hooks`, the `status` and `reload` callbacks of a Module's descriptor.
- **Avoid:** plugin, script event soup, subscription

## Reminder
An Activity saying a calendar event is coming: its title and how long until it starts, or ends for an alarm on its end, with Open and Dismiss, and Join while the event links to a video meeting (ADR 0043). Join opens the first `https` link of its URL, LOCATION or DESCRIPTION whose host `sources/meeting.rs` lists (Google Meet, Zoom, Teams, Webex, Jitsi, Whereby) with `xdg-open`, by argv with no shell, never logging it. It comes from the event's own `VALARM`s that display or play a sound, or `calendar.reminder` minutes before a timed event with none. Persistent, Ongoing, Global, interrupting nothing; it goes 5 minutes after its event starts, an hour after it went off at most, or on Open, Join or Dismiss.
- **Invariant:** a reminder goes off once. Which went off is kept in a file under the state directory, keyed by event, occurrence and alarm, the default lead by a mark of its own, so a restart, a clock set or a new `calendar.reminder` never fires one again.
- **Invariant:** a missed reminder goes off only until 5 minutes after its event starts; past that it is dropped, never shown late.
- **Avoid:** alarm (the `VALARM` it comes from), alert, notification

## Satellite
Small secondary indicator beside the primary island: timer, VPN, low battery, a Live Activity.
- Capture the privacy cluster sees is no Satellite: the cluster shows it, as dots inside the body. Kanade's own recording is an Activity like any other, so it can be one, carrying Stop.
- **Invariant:** only Persistent Ongoing or Critical Activities that are not the primary, and a playing Media Activity beside such a primary, so a song keeps its place beside a timer; it counts after the others and shows its cover. At most `SATELLITES` (`src/island/arbiter.rs`) show, highest first; the rest are a count.
- **Rule:** a Satellite comes out from under the body and tucks back under it, fading; one that changes place slides. None pops.
- While the island is Split, or a Peek out of one, the top Satellite is the trailing segment, not a dot; it counts toward `SATELLITES`. The other dots and the count take no pointer.

## Hold
The keyboard an island keeps (`Keyboard::Exclusive`) after an IPC or keybind open, so Escape reaches it without a press.
- **Invariant:** only an island opened without a press holds. The hold ends when the island collapses.
- **Invariant:** an unattended hold is bounded. An ignored island collapses on its own; only keys the open Surface consumes restart the bound, and typing into a held Surface that does not type collapses it. Once the pointer enters, the hold lasts until the pointer leaves and the grace runs out. A Pin ends it. The one exception is a visit a `kanade dmenu` waits on: as with any dmenu, its hold lasts until the user answers with Escape, a choice or another Surface, since Escape always reaches it. The bound is `HOLD` in `src/island/service.rs`; the niri measurements behind it are in `docs/archived/plan.md` §6.1.
- **Avoid:** hold for a Peek or Surface the user opened. That is the island's Presentation.

## Pin
A right click keeps an island's Peek or open Surface up after the pointer leaves, with no leave grace. The body shows a ring while pinned.
- Right click raises the island to what the pointer would raise it to, pinned: on Compact or a Split segment it peeks that Activity pinned, at Rest it opens Controls pinned (no context menu), on a tray slot it opens that item's menu, not pinned (ADR 0012), on a Peek or open Surface it pins or unpins it. On a Surface's own control it pins too and never presses it.
- Escape ends a pin only while the island has keyboard focus (a pinned island gives it back, see below); `kanade island collapse` and `toggle` close a pinned Surface.
- **Invariant:** a pin lasts only for its current raised Presentation. Collapsing or replacing it clears the pin: collapse, a click expanding the Peek, another Surface opening, the overview, another island expanding. The peeked Activity leaving both the primary and the top Satellite clears a pinned Peek; Preempt clears a pinned Surface, unless it is its own. Nothing pinned is remembered.
- **Invariant:** a pinned island never holds. Pinning a held island gives the keyboard back; a press on the island takes it again.
- No per-Activity context action on right click: a click already opens the Activity's Surface, where its actions are, and a hidden action would run before it could be seen.
- **Avoid:** sticky, lock (lock is the screen locker)

## Running app
One app's open windows, as `windows` groups them for the Dock: those whose `app_id` matches one desktop entry (ADR 0014). Windows that match none group by `app_id`; a window without one stands alone.
- **Invariant:** nothing of niri leaves `windows` but window ids. A window's title is asked of niri each time the Dock's menu opens and lives only as long as the menu.
- **Invariant:** an override in `windows.apps` is final: one naming a missing entry leaves the app unmatched, never guessed.
- **Avoid:** client, toplevel, task, program

## Dock
Strip per monitor on the edge and side `dock.edge` and `dock.align` give it: pinned apps (`dock.pinned`), then Running apps not pinned, a dot between each running one and the edge. A click launches an app without windows, else focuses its window, cycling while it has the focus. A right click opens the app's menu: pin or unpin, one row per open window (its title, most recently focused first where niri says when; Enter focuses it, its close target or Delete closes it), then the app's own actions. Files dragged onto an icon start that app with them, each as an argument of its own and never through a shell, when its `Exec` has a standing `%f`, `%F`, `%u` or `%U` and no `Path`; an app that takes one file at a time starts once per file, up to 8. Icons sized by `dock.size` grow near the pointer (`dock.magnification`); an autohidden Dock comes out when the pointer touches the edge under it. It steps aside while the Island grows over it, as with a Surface open beside it. Where it shares the Island's edge and side, `dock.merge` joins them instead (ADR 0031): one window, one pane: the Dock a plate on the edge with the Island crowning it (`crown`), the Island the middle piece of its row (`keystone`), or the Dock folded into the Island and out under the pointer (`fold`).
- **Invariant:** app data only from `windows`; never reads `workspace` or raw niri objects, the niri source hands it the output names.
- **Avoid:** taskbar, launcher (the Launcher is a Surface), panel

## Pane
One opaque piece of the shell, tinted toward the wallpaper's hue, with a top light edge and a rim: the Island's body, the Dock, or one Banner (ADR 0050). Its finish, `appearance.finish`, is the tinted solid (dark, light text), paper (light, dark ink) (ADR 0051) or Liquid Glass (clear glass, the screen showing through, its edge reflecting what lies around it, light text that turns to dark ink over a bright page) (ADRs 0052, 0054). A merged Island and Dock are one pane with one outline (ADR 0031).
- **Invariant:** tinted from the wallpaper's hue (a cool slate until Matugen answers), with text that suits its finish, whatever `appearance.tone` says; opaque, and nothing behind it read, except under Liquid Glass, the only finish that captures the screen, which stops when the finish changes or the session locks.
- **Avoid:** frost, material (the choice is the finish), glass for anything but the Liquid Glass finish, layer (a `pane::layer` widget), surface (a Surface is the Island's)

## Idle stage
What the `idle` Module does after the user has been idle for so long (ADR 0047): `idle.dim`, `idle.lock`, `idle.screen_off`, `idle.suspend`, each optional. The Dim grace comes first: a click-through Overlay layer on every output fading toward dark until the lock, taken away at once by any input.
- **Invariant:** the lock never waits on the Dim grace: a dim that cannot draw still locks on time, and a lock that fails is asked again while the user stays idle. An error never leaves an idle session unlocked, and with a lock stage the monitors going off and the suspend wait until it holds.
- **Invariant:** an inhibitor holds every stage: an app's Wayland idle inhibitor (niri never says idle), a logind idle inhibitor (`BlockInhibited` naming `idle`), or caffeine. A logind that cannot be asked inhibits nothing.
- **Invariant:** the stage scheduler takes time as an input, never reads a clock.
- **Avoid:** screensaver, DPMS (niri powers monitors off), idle inhibit (an inhibitor is what holds the stages)

## Module
A feature the user can turn off in `[modules]` of the config: `src/modules/catalog.rs` lists every one, with the Modules it requires and those it can do without.
- `island` is the core and cannot be turned off; every other Module requires it.
- Every Module is on unless the config turns it off, but `idle`, which is off unless turned on, so an idle daemon like hypridle keeps working (ADR 0047).
- **Invariant:** a Module that is off starts no thread or helper process, opens no window, reads no Service, posts nothing and answers its verbs with `module <name> is off`. A Service only it reads stays cold.
- **Invariant:** a Module whose requirement is off, or whose requirements loop, is off too, and stderr names why. One missing an optional Module runs without it.
- A Module whose loss is easy to miss says so at every start while off (`privacy`: no capture indicators).
- Do Not Disturb belongs to `notifications`: with it off, the verb is refused and the Controls switch is unavailable. The Night Light switch likewise belongs to `night-light`, and is unavailable without `wlsunset`. The Limit Charge switch belongs to `battery`, and shows only while UPower says the battery supports a charge limit. `kanade wifi`, `bluetooth` and `power-profile` belong to `network`, `bluetooth` and `power`, and `media play-pause|next|previous` to `media`; each answers once the daemon or player has it as asked, or refuses saying why.
- Which Modules run is decided once at start; a change takes a restart.
- **Avoid:** plugin (not built), feature flag, Service (a `Service` is shared state a Module reads)
