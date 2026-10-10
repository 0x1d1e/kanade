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
- **Invariant:** Lifetime, Priority, Scope and Interrupt are independent: the source sets each, and none implies another (ADR 0009). `Activity::new` refuses only what cannot be carried out: `Transient(0)`, and `AutoExpand` for a Kind with no Surface of its own (only Media, Notification and Session have one). Unusual combinations, like a Persistent FocusedOutput or a Critical that interrupts nothing, stand.
- **Invariant:** posting an Activity with an existing id replaces it and refreshes its Lifetime. Switching on through workspaces extends one Transient, not a queue of them.
- **Avoid:** event, notification (a Notification is one Kind of Activity)

## Privacy cluster
The microphone, camera and screen capture in use, a dot of its color each at the trailing end of the Island's body, inside it, in the small forms (Rest, Compact, Split, Peek; ADRs 0005, 0033). What is centered stands toward the start while they show. It takes no pointer; the Controls Surface names the apps; `privacy.indicators` turns it off.
- **Invariant:** never an Activity and never in the Arbiter. Capturing, the Island does not hide; under a fullscreen window it does not show, and a Tray or Surface has none.
- **Avoid:** privacy Activity, privacy Satellite, indicator (as a type name)

## Banner
A notification card on the focused output, hanging from the Island on its edge and side and clear of the Dock, in its own Top-layer window per monitor (ADRs 0007, 0023). It comes out of the Island and goes back into it, as `banners.entrance` says. The `banners` Module draws it over what `notifications` keeps: the bus, state and history stay there.
- Low shows 4 s, normal 6 s, Critical until closed. The pointer on any Banner pauses every timer. At most `MOST` (`src/banners/stack.rs`) show, newest nearest the Island; the rest queue, Critical first.
- **Invariant:** an action runs only on a click, and closes the notification unless it is resident. A Banner closed with no action run (its X, its timer, a click with no default action) leaves the notification in history.
- **Invariant:** under DND only Critical Banners show; DND coming on drops the rest, queued too.
- **Avoid:** toast (as the window; `Toast` is the text a Banner shows), popup

## OSD
A volume, brightness, keyboard backlight or microphone mute level, or a mode (Caps Lock, Num Lock, airplane mode), shown as a Transient Activity of `Feedback` Priority on the focused output's Island, over any other Activity and its Satellites, and over an open Surface or a Peek as a capsule (ADR 0021). It shows on a change, and on a level key pressed even at a limit. The Island shows over a fullscreen window only while an OSD or a Surface does. The `osd` Module reads `audio` and `brightness` only while they are on, and shows nothing for one that is off.
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
- **Invariant:** an explicit user action while an AutoExpand is open (click or press, open or toggle, collapse, pin, a key the Surface consumes) cancels the restore: the user's choice owns the islands. The pointer entering or leaving does not, but the pointer still on the Surface at its deadline keeps it open, like Hold, until it leaves and the grace runs out.
- **Invariant:** a Preempt arriving is policy, not choice: a pending AutoExpand gives the islands back first, then it preempts as it would have without one.

## Presentation
An island's visual level: `Rest | Compact | Split | Peek | Tray | Expanded(Surface)`.
- Rest has no primary Activity and shows the local time (`rest.clock`), and the battery's percent as set. With neither, nothing shows and the island hides as autohide does.
- Tray is Rest's own small form: the time's Peek (the time, with the battery as set, over the date, and the weather and next event as set), then the apps' tray items, raised by the hover delay and gone with the pointer, as a Peek is (ADR 0012). A slot is the item's own target: left activates it (an item that is only a menu shows its menu instead), middle is its secondary activation, the wheel scrolls it, right opens its menu.
- Split is Compact while the Frame has a Satellite: one body in two segments, the primary leading and the top Satellite trailing. It follows the Frame, never input (ADR 0010).
- A Peek shows one Activity, by identity: the one under the pointer, primary or top Satellite. It lasts while that Activity still shows on the island as either.
- **Invariant:** a body is never larger than the largest (`island.width` by `island.height`). A list Surface is as tall as its content asks, down to the Peek, never shrinking from under the pointer; the canvas never resizes during a morph (ADR 0030).
- **Invariant:** `Expanded` always carries a Surface. There is no surface-less expanded form.
- **Invariant:** which Surface opens is a pure function of (Presentation, the Activities shown, the segment under the pointer, request, the Surfaces withheld). No remembered last-used Surface: a click at Rest opens Controls.
- **Invariant:** at most one island is Expanded at a time.
- **Rule:** a Presentation change is geometry plus content crossfade in one motion, never collapse-then-grow. A Split whose segments trade places slides each to its new place.
- **Avoid:** state (state is what Services hold)

## Surface
Full interactive content of an Expanded island: `Media | Notifications | Controls | Launcher | Tray | Clipboard | Calendar | Weather | Session`.
- Media, Notifications and Session are also Activity Kinds. Compact and Peek are the Activity's own small form, and Expanded is its Surface. A Session Activity is a restart, power off or log out counting down, a lock, sleep, restart, power off or log out that was refused or failed, or one logind never answered, which may still happen; the Session Surface opens by user action too, and shows a countdown but no refusal, so a refusal or no answer closes it.
- Controls, Launcher, Tray, Clipboard, Calendar and Weather have no Activity. They open only by user action.
- Each is drawn by a Module (`media`, `notification-surface`, `controls`, `launcher`, `tray`, `clipboard-surface`, `calendar-surface`, `weather-surface`, `session`). One whose Module is off is withheld: it never opens, by click, verb or AutoExpand. A click on an Activity whose own Surface is withheld opens Controls, as for a Kind without one; with Controls withheld too, or at Rest, a click opens nothing.
- **Avoid:** panel, page, view (a view is the runtime's build function)

## Sub-surface
A level inside a Surface, entered from one of its targets and left by a back control or Escape. Controls has four: Wi-Fi, from the Wi-Fi tile's chevron, its password entry, from a network that needs one, Bluetooth, from the Bluetooth tile's chevron, and Audio, from the speaker level's chevron. In the Tray Surface an item's menu is one, from its chevron or its slot, and each submenu another below it.
- **Invariant:** every target in a Surface and its sub-surfaces is a key away. Arrows move a ring between targets, Enter or Space presses, Escape leaves a sub-surface for the level it came from, ringed on the target that entered it, and collapses the island only from the top level.
- **Invariant:** a sub-surface lasts one opening of its Surface. The Surface opens at its top level.
- **Invariant:** a password never prints, in a log or a `Debug`; it lives only until NetworkManager has it.
- **Avoid:** detail (an Activity's Detail), page, view

## LauncherProvider
One kind of answer the Launcher finds for a query: apps, the calculator, emoji, wallpapers, destinations (Wi-Fi, Bluetooth, Clipboard, Settings). Each answer has a Fit, what its row shows, and its action: start an app, copy text, set a wallpaper, or open a Surface or Settings.
- **Invariant:** every provider ranks on one Fit scale, so their answers interleave; a tie keeps the provider order (calculator, apps, emoji), then each provider's own.
- Emoji answer only a query starting with `:`, and apps answer none of those.
- Kanade's own seam; no plugin API.
- **Avoid:** plugin, source (a source posts to the island)

## Satellite
Small secondary indicator beside the primary island: timer, VPN, low battery.
- Capture the privacy cluster sees is no Satellite: the cluster shows it, as dots inside the body. Kanade's own recording is an Activity like any other, so it can be one, carrying Stop.
- **Invariant:** only Persistent Ongoing or Critical Activities that are not the primary. At most `SATELLITES` (`src/island/arbiter.rs`) show, highest first; the rest are a count.
- **Rule:** a Satellite comes out from under the body and tucks back under it, fading; one that changes place slides. None pops.
- While the island is Split, or a Peek out of one, the top Satellite is the trailing segment, not a dot; it counts toward `SATELLITES`. The other dots and the count take no pointer.

## Hold
The keyboard an island keeps (`Keyboard::Exclusive`) after an IPC or keybind open, so Escape reaches it without a press.
- **Invariant:** only an island opened without a press holds. The hold ends when the island collapses.
- **Invariant:** an unattended hold is bounded. An ignored island collapses on its own; only keys the open Surface consumes restart the bound, and typing into a held Surface that does not type collapses it. Once the pointer enters, the hold lasts until the pointer leaves and the grace runs out. A Pin ends it. The bound is `HOLD` in `src/island/service.rs`; the niri measurements behind it are in `docs/archived/plan.md` §6.1.
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
- **Invariant:** nothing of niri leaves `windows` but window ids.
- **Invariant:** an override in `windows.apps` is final: one naming a missing entry leaves the app unmatched, never guessed.
- **Avoid:** client, toplevel, task, program

## Dock
Strip per monitor on the edge and side `dock.edge` and `dock.align` give it: pinned apps (`dock.pinned`), then Running apps not pinned, a dot between each running one and the edge. A click launches an app without windows, else focuses its window, cycling while it has the focus. Icons sized by `dock.size` grow near the pointer (`dock.magnification`); an autohidden Dock comes out when the pointer touches the edge under it. It steps aside while the Island grows over it, as with a Surface open beside it. Where it shares the Island's edge and side, `dock.merge` joins them instead (ADR 0031): one window, one pane of glass: the Dock a plate on the edge with the Island crowning it (`crown`), the Island the middle piece of its row (`keystone`), or the Dock folded into the Island and out under the pointer (`fold`).
- **Invariant:** app data only from `windows`; never reads `workspace` or raw niri objects, the niri source hands it the output names.
- **Avoid:** taskbar, launcher (the Launcher is a Surface), panel

## Pane
One piece of liquid glass: the Island's body, the Dock, or one Banner, named by a `Spot`, its output and `island`, `dock` or `banner-<i>` there (ADR 0024). Each is captured and lit apart: the runtime copies what is behind it and its shader bends the rim from the copy (ADR 0037).
- **Invariant:** a pane nothing draws in captures nothing; nothing captures while Kanade holds the lock, from niri saying it locked until the lock ends (ADR 0025).
- **Avoid:** layer (a `glass::layer` widget), surface (a Surface is the Island's), rim picture

## Module
A feature the user can turn off in `[modules]` of the config: `src/modules/catalog.rs` lists every one, with the Modules it requires and those it can do without.
- `island` is the core and cannot be turned off; every other Module requires it.
- **Invariant:** a Module that is off starts no thread or helper process, opens no window, reads no Service, posts nothing and answers its verbs with `module <name> is off`. A Service only it reads stays cold.
- **Invariant:** a Module whose requirement is off, or whose requirements loop, is off too, and stderr names why. One missing an optional Module runs without it.
- A Module whose loss is easy to miss says so at every start while off (`privacy`: no capture indicators).
- Do Not Disturb belongs to `notifications`: with it off, the verb is refused and the Controls switch is unavailable.
- Which Modules run is decided once at start; a change takes a restart.
- **Avoid:** plugin (not built), feature flag, Service (a `Service` is shared state a Module reads)
