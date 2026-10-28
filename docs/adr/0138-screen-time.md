# 138. Screen time

Status: accepted. Builds on [ADR 0004](0004-wake-sources-on-announcements.md) (no poll) and the withheld-Surface rule in `CONTEXT.md`.

## Context

A user wants today's focused time per app, and one notice when it crosses a limit. Polling the window list, or a thread that reads focus while the feature is off, would wake the session for something they did not ask for. Window titles are private; the app id is enough unless they turn titles on.

## Decision

- **A `screen-time` Module, off unless turned on.** Off, it starts no thread, registers no niri callback, reads no focus stream and posts nothing. The Surface is withheld, so `kanade screen-time` and the Launcher destination do not open it.
- **Time comes from events the shell already receives.** `WindowFocusChanged` and the window list on Kanade's existing niri stream. No second connection and no poll. Time is attributed to the app id, or `unknown` when niri gives none. A focus change closes the previous app's interval and opens the new one's. The day boundary is local.
- **Titles are a private switch, off.** `screen-time.titles` keeps the focused window's title in the state file and on the Surface. Off, the title is not copied off the event, not stored and not logged. App names are not logged either.
- **State.** `$XDG_STATE_HOME/kanade/screen-time`, written with `atomic::write` at `0600`, from the worker, never the draw thread. Seven days, 48 apps a day, ids and titles cut. A file over 256 KiB is ignored.
- **The limit is not a poll.** `screen-time.limit` is minutes, 0 for none. While an interval is open and today's closed total is under the limit, the worker sleeps until the crossing, the next local midnight, or the next focus event. The notice is one Transient on the focused output, the first time the total crosses the limit that day, and not when the day already started over it. It names no app. A later day can fire again.
- **The Surface** shows today's total and the apps by time. `kanade screen-time` and a Launcher destination open it. Up, Down, Tab, Home and End move a ring; the wheel scrolls. It reads the Service and writes neither it nor the file.

## Consequences

- Turning the Module on takes a restart, like every Module.
- A focused window the shell does not yet know is `unknown` until its app id arrives, then time moves to that app.
- The open Surface does not tick each second. A frame shows the closed total plus what has passed since the last event.
