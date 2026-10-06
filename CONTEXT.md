# Kanade domain language

Use these terms in code, tests, issues and docs. Avoid the listed synonyms. Terms for parts not built yet (Activity, Arbiter, Frame, Satellite) are fixed here ahead of the code.

## Island
The single physical surface on one monitor. One per monitor.
- Monitor and output name the same thing: Amane says monitor, niri says output.
- **Avoid:** pill (as a type name; fine in prose for the small shape), widget, popup, dropdown

## Activity
Something happening that may deserve attention. Has identity, Kind, Priority, Lifetime, Scope, Interrupt, Actions, Detail.
- Identity is Kind plus a key, so keys from different Kinds never collide.
- Detail is what its small form draws, typed per Kind (a Media Activity's track, a Volume's level). It is not identity: a repost with new Detail replaces the Activity. A new track dissolves in place, the new art and text rising over the old without the form moving; a level that moves redraws in place.
- **Invariant:** Lifetime, Priority, Scope and Interrupt are independent: the source sets each, and none implies another (ADR 0009). `Activity::new` refuses only what cannot be carried out: `Transient(0)`, and `AutoExpand` for a Kind with no Surface of its own (only Media and Notification have one). Unusual combinations, like a Persistent FocusedOutput or a Critical that interrupts nothing, stand.
- **Invariant:** posting an Activity with an existing id replaces it and refreshes its Lifetime. A repeated volume key extends one Transient, not a queue of them.
- **Avoid:** event, notification (a Notification is one Kind of Activity), OSD (the Overlay window, not an Activity; the island's is a Transient)

## Privacy cluster
The microphone, camera and screen capture in use, in its own Overlay window per monitor, so no fullscreen window covers it (ADR 0005). It takes no pointer; the Controls Surface names the apps.
- **Invariant:** never an Activity and never in the Arbiter. It shows whatever the island shows.
- **Avoid:** privacy Activity, privacy Satellite, indicator (as a type name)

## Banner
A notification card in the top-right corner of the focused output, in its own Top-layer window per monitor (ADR 0007). The `banners` Module draws it over what `notifications` keeps: the bus, state and history stay there.
- Low shows 4 s, normal 6 s, Critical until closed. The pointer on any Banner pauses every timer. At most `MOST` (`src/banners/stack.rs`) show, newest on top; the rest queue, Critical first.
- **Invariant:** an action runs only on a click, and closes the notification unless it is resident. A Banner closed with no action run (its X, its timer, a click with no default action) leaves the notification in history.
- **Invariant:** under DND only Critical Banners show; DND coming on drops the rest, queued too.
- Until #109 a notification also shows as an island toast.
- **Avoid:** toast (the island's Transient), popup

## OSD
A volume, brightness or microphone mute change, bottom centre of the focused output, in its own Overlay window per monitor (ADR 0007). The `osd` Module reads `audio` and `brightness` only while they are on, and shows nothing for one that is off.
- **Invariant:** one OSD at a time: a change while one shows takes its place and starts its time (`osd` in the config) again, so a held key extends one OSD.
- Until #109 a change also shows as an island Transient.
- **Avoid:** popup, toast

## Lifetime
`Persistent` lives until withdrawn: media playing, timer, low battery.
`Transient(duration)` expires on its own: volume, brightness, workspace switch, notification toast. It implies no Scope and no Interrupt.

## Arbiter
Function of (registered Activities, the primary shown and since when, now, focused output) to a Frame. Owns priority, preemption, primary dwell, expiry and Satellite selection.
- **Invariant:** time is an input. The Arbiter never reads a clock.
- **Invariant:** preemption never destroys. A Persistent Activity hidden by a Transient shows again when the Transient expires, with no re-post.

## Frame
The Arbiter's output: `primary: Option<Activity>`, `satellites: Vec<Activity>` (bounded), `overflow` (the Satellites past the bound, as a count), `transient: Option<Activity>` (the `Interrupt::Transient` one over the primary), `queued` (those kept off an open Surface, shown as a badge). One per island: Scope, an open Surface and DND decide what it leaves out.
- Not a rendered frame (Amane's `request_frame`, `AMANE_FRAMES`).

## Scope
`Global` shows on every island. `FocusedOutput` shows only on the island of the focused output.

## Interrupt
How an Activity interrupts: `None | Transient | Preempt | AutoExpand(duration)`.
- `None` only competes for the primary. `Transient` shows over the primary instead (goes with the Frame's transient slot, #109). `Preempt` collapses the open Surface it shows on when it arrives. `AutoExpand` opens the Activity's own Surface on the focused island for `duration`, then gives every island it changed back its Presentation and Surface.
- A repost with the same Interrupt is no new arrival, so it neither preempts nor expands again. One DND drops never arrives, so it interrupts nothing.
- **Invariant:** an explicit user action while an AutoExpand is open (click or press, open or toggle, collapse, pin, a key the Surface consumes) cancels the restore: the user's choice owns the islands. The pointer entering or leaving does not, but the pointer still on the Surface at its deadline keeps it open, like Hold, until it leaves and the grace runs out.
- **Invariant:** a Preempt arriving is policy, not choice: a pending AutoExpand gives the islands back first, then it preempts as it would have without one.

## Presentation
An island's visual level: `Rest | Compact | Peek | Expanded(Surface)`.
- Rest has no primary Activity and shows the local time.
- **Invariant:** `Expanded` always carries a Surface. There is no surface-less expanded form.
- **Invariant:** which Surface opens is a pure function of (Presentation, primary Activity, request). No remembered last-used Surface: a click at Rest opens Controls.
- **Invariant:** at most one island is Expanded at a time.
- **Rule:** a Presentation change is geometry plus content crossfade in one motion, never collapse-then-grow.
- **Avoid:** state (Amane uses state for Services)

## Surface
Full interactive content of an Expanded island: `Media | Notifications | Controls | Launcher`.
- Media and Notifications are also Activity Kinds. Compact and Peek are the Activity's own small form, and Expanded is its Surface.
- Controls and Launcher have no Activity. They open only by user action.
- **Invariant:** an `Interrupt::Transient` Activity the open Surface already shows is dropped, not queued (Volume while Media or Controls is open, Brightness, Bluetooth or Wi-Fi Network while Controls is open, a non-Critical notification while Notifications is open, `Surface::shows`).
- **Avoid:** panel, page, view (a view is Amane's build function)

## Satellite
Small secondary indicator beside the primary island: timer, VPN, low battery.
- Capture is no Satellite: the privacy cluster shows it.
- **Invariant:** only Persistent Ongoing or Critical Activities that are not the primary. At most `SATELLITES` (`src/island/arbiter.rs`) show, highest first; the rest are a count.
- **Rule:** a Satellite comes out from under the body and tucks back under it, fading; one that changes place slides. None pops.

## Hold
The keyboard an island keeps (`Keyboard::Exclusive`) after an IPC or keybind open, so Escape reaches it without a press.
- **Invariant:** only an island opened without a press holds. The hold ends when the island collapses.
- **Invariant:** an unattended hold is bounded. An ignored island collapses on its own; only keys the open Surface consumes restart the bound, and typing into a held Surface that does not type collapses it. Once the pointer enters, the hold lasts until the pointer leaves and the grace runs out. A Pin ends it. The bound is `HOLD` in `src/island/service.rs`; the niri measurements behind it are in `docs/archived/plan.md` §6.1.
- **Avoid:** hold for a Peek or Surface the user opened. That is the island's Presentation.

## Pin
A right click keeps an island's Peek or open Surface up after the pointer leaves, with no leave grace. The body shows a ring while pinned.
- Right click on Compact peeks pinned, on a Peek or open Surface pins or unpins it, at Rest does nothing (a click already opens Controls). On a Surface's own control it pins too and never presses it.
- Escape ends a pin only while the island has keyboard focus (a pinned island gives it back, see below); `kanade island collapse` and `toggle` close a pinned Surface.
- **Invariant:** a pin lasts only for its current raised Presentation. Collapsing or replacing it clears the pin: collapse, a click expanding the Peek, another Surface opening, the overview, another island expanding. The primary's withdrawal clears a pinned Peek; Preempt clears a pinned Surface. Nothing pinned is remembered.
- **Invariant:** a pinned island never holds. Pinning a held island gives the keyboard back; a press on the island takes it again.
- No per-Activity context action on right click: a click already opens the Activity's Surface, where its actions are, and a hidden action would run before it could be seen.
- **Avoid:** sticky, lock (lock is the screen locker)

## Module
A feature the user can turn off in `[modules]` of the config: `src/modules.rs` lists every one, with the Modules it requires and those it can do without.
- `island` is the core and cannot be turned off; every other Module requires it.
- **Invariant:** a Module that is off starts no thread or helper process, opens no window, reads no Service, posts nothing and answers its verbs with `module <name> is off`. An Amane Service only it reads stays cold.
- **Invariant:** a Module whose requirement is off, or whose requirements loop, is off too, and stderr names why. One missing an optional Module runs without it.
- A Module whose loss is easy to miss says so at every start while off (`privacy`: no capture indicators).
- Do Not Disturb belongs to `notifications`: with it off, the verb is refused and the Controls switch is unavailable.
- Which Modules run is decided once at start; a change takes a restart.
- **Avoid:** plugin (not built), feature flag, Service (an Amane `Service` is shared state a Module reads)
