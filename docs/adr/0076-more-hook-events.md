# 76. More hook events: start, Do Not Disturb, power profile and leaving the session

Status: accepted. Amends [ADR 0055](0055-hooks-run-user-commands.md) (the closed set of events and the one wait).

## Context

ADR 0055 left a new event as a code change with an amendment. Users asked for a script on the shell starting, on Do Not Disturb changing, on the power profile changing (to match a refresh rate or a wallpaper to it), and before a log out, restart or power off (to stop a service or save a state).

## Decision

- **Six events join the closed set.** Each has `hooks.<event>` and the payload ADR 0055 allows, environment variables only:
  - `started`: the `hooks` Module's own start, last of the Modules, so every Module that is on has started. A restart of the shell fires it again.
  - `dnd`: `notifications::apply`, when Do Not Disturb goes on or off, not for a setting that leaves it as it was. `KANADE_DND` is `on` or `off`.
  - `power_profile`: `system`'s refresh of power-profiles-daemon, which already reads the active profile. It fires for a change from one profile to another, not for the first reading nor a daemon coming back. `KANADE_POWER_PROFILE` is `power-saver`, `balanced` or `performance`. No new source or poll.
  - `logout`, `reboot`, `poweroff`: `session::end_now`, just before logind is asked, after a countdown or at once (Now, `kanade session`, a critical battery action). Sleep and hibernate have `suspend`.
- **The session events wait, within a bound.** The action is asked of logind after the hook ended or `hooks.timeout` plus 2 s passed, whichever is first (`hooks::fire_and_wait`). The wait is on the thread that asks logind, never the draw thread. While it waits the session is `Waiting` as for any logind call, so another request is refused as busy. A hook cannot cancel or prevent the action, and a hook that does not exist or whose `hooks` Module is off costs nothing. A hook still running at the bound is killed by the child runner.
- **Names.** The status listing of failed and dropped events follows `Event::ALL`, so it covers the new ones. Doctor lists no hook.

## Consequences

- `reboot` fires when the restart is asked, not when it begins; a refusal by logind or polkit still shows later, and the hook has run.
- A countdown cancelled fires nothing.
- `started` runs before the windows are on screen.
