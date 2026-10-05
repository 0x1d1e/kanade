# Kanade domain language

Use these terms in code, tests, issues and docs. Avoid the listed synonyms. Terms for parts not built yet (Activity, Arbiter, Frame, Satellite) are fixed here ahead of the code.

## Island
The single physical surface on one monitor. One per monitor.
- Monitor and output name the same thing: Amane says monitor, niri says output.
- **Avoid:** pill (as a type name; fine in prose for the small shape), widget, popup, dropdown

## Activity
Something happening that may deserve attention. Has identity, Kind, Priority, Lifetime, Scope, Actions.
- **Invariant:** posting an Activity with an existing id replaces it and refreshes its Lifetime. A repeated volume key extends one Transient, not a queue of them.
- **Avoid:** event, notification (a Notification is one Kind of Activity), OSD (use Transient)

## Lifetime
`Persistent` lives until withdrawn: media playing, screen cast, timer, privacy, low battery.
`Transient(duration)` expires on its own: volume, brightness, workspace switch, notification toast.

## Arbiter
Pure function of (registered Activities, now, focused output) to a Frame. Owns priority, preemption, expiry and Satellite selection.
- **Invariant:** time is an input. The Arbiter never reads a clock.
- **Invariant:** preemption never destroys. A Persistent Activity hidden by a Transient shows again when the Transient expires, with no re-post.

## Frame
The Arbiter's output: `primary: Option<Activity>`, `satellites: Vec<Activity>` (bounded), `transient: Option<Activity>`. Global, then filtered per island by Scope.
- Not a rendered frame (Amane's `request_frame`, `AMANE_FRAMES`).

## Scope
`Global` shows on every island. `FocusedOutput` shows only on the island of the focused output. Transients are FocusedOutput.

## Presentation
An island's visual level: `Rest | Compact | Peek | Expanded(Surface)`.
- **Invariant:** `Expanded` always carries a Surface. There is no surface-less expanded form.
- **Invariant:** which Surface opens is a pure function of (Presentation, primary Activity, request). No remembered last-used Surface: a click at Rest opens Controls.
- **Invariant:** at most one island is Expanded at a time.
- **Rule:** a Presentation change is geometry plus content crossfade in one motion, never collapse-then-grow.
- **Avoid:** state (Amane uses state for Services)

## Surface
Full interactive content of an Expanded island: `Media | Notifications | Controls | Launcher`.
- Media and Notifications are also Activity Kinds. Compact and Peek are the Activity's own small form, and Expanded is its Surface.
- Controls and Launcher have no Activity. They open only by user action.
- **Avoid:** panel, page, view (a view is Amane's build function)

## Satellite
Small secondary indicator beside the primary island: screen cast, mic/camera, timer, VPN, critical battery. Persistent Activities only.

## Hold
The keyboard an island keeps (`Keyboard::Exclusive`) after an IPC or keybind open, so Escape reaches it without a press.
- **Invariant:** only an island opened without a press holds. The hold ends when the island collapses.
- **Invariant:** the hold is bounded. An ignored island collapses on its own; keys never extend it. The bound is `HOLD` in `src/island/service.rs`; the niri measurements behind it are in `docs/plan.md` §6.1.
- **Avoid:** hold for a Peek or Surface the user opened. That is the island's Presentation.
