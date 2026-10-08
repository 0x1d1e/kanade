# 18. Lock stays in the shell

Status: accepted (roadmap 11 Session, #153). Answers the lock ship gate of `docs/design.md` Security/privacy: no `kanade-lock` split.

Validated: crash recovery in a temporary test build (#153), then again in #154 on the production binary and the installed `kanade.service` unit, with the start limit and its recovery.

## Context

The lock shows on Amane's `Lock` (`App::lock`, ext-session-lock, PAM through the `login` service), inside the shell process. A shell crash must not unlock the session or leave it unrecoverable. The gate: kill Kanade while locked → compositor stays locked → systemd restart → lock UI reacquired → auth succeeds. Any failure → split the lock into its own process.

Measured in #153 on niri 26.04 (8ed0da4) and Amane `6ace43e`, with a throwaway build: Kanade plus `App::lock` and a minimal password screen, run as a transient user unit (`systemd-run --user -p Restart=on-failure -p RestartSec=1`), which at start called `Lock::start()` when logind's `LockedHint` was true.

| Step | Seen |
|---|---|
| lock | niri `locking session`; `LockedHint` true |
| `kill -9` while locked | niri keeps the session locked and shows its solid red screen, no content; `LockedHint` stays true |
| restart | systemd restarts the unit 1 s later; the new process reads `LockedHint` true and calls `Lock::start()` before `App::run` |
| reacquire | 65 ms after start niri logs `locking session (replacing existing dead lock)`; the lock screen is back on every output |
| auth | the right password unlocks the reacquired lock; `LockedHint` false. A wrong one, tried in an earlier run, stayed locked |

niri sets `LockedHint` itself on lock and unlock, so it outlives the shell. `/org/freedesktop/login1/session/auto` resolves to the graphical session from a user unit, which has no `XDG_SESSION_ID` of its own. niri refuses a new lock while a live client holds one, so a restart while another locker (swaylock) holds the session does nothing.

## Decision

All three land in #154.

- **Lock stays in the shell process.** No `kanade-lock`.
- **Kanade runs as a systemd user unit** tied to niri's graphical session, with `Restart=always` and a start limit: `on-failure` leaves a shell ended by `SIGTERM` or a clean exit stopped, which strands a locked session as a crash does; `systemctl --user stop` still stops it. The README starts Kanade through it instead of `spawn-at-startup`.
- **At start, a true `LockedHint` locks at once.** The `lock` Module reads it from `session/auto` before `App::run` and calls `Lock::start()`; a lock the compositor refuses is left alone. No Kanade-owned marker: niri's hint is the only state, and it cannot go stale on a Kanade crash.

## Alternatives

**Separate `kanade-lock` process.** Isolates the lock from shell panics, but adds a second Amane app, its own IPC and theme loading, and still needs the same restart and reacquire path for its own crashes. The gate passed without it.

**A marker file in `XDG_RUNTIME_DIR`.** Kanade's own record of being locked. It goes stale when another locker unlocks or niri restarts, and duplicates what niri already tells logind.

## Consequences

- `LockedHint` is only for the restart: right after an unlock it may still say the last lock, so `kanade lock` waits on its own lock screen drawing instead (#196).
- Kanade checks a password with PAM itself before Amane's `Lock::unlock`, which alone ends the lock, checks it again (#155): Kanade's check can be dropped when sleep comes, Amane's cannot. So every unlock runs the `login` stack twice; a stack with `pam_fprintd` or `pam_u2f` asks for a finger or key twice, and one that refuses a second success in a row says Wrong password after a right one.
- A crash while locked shows niri's red screen for about the restart delay (1 s here), then the lock screen. A password typed into the dead process is lost.
- Without the user unit, as with `spawn-at-startup`, nothing restarts the shell: the session stays on the red screen until another locker or a TTY ends it. `kanade doctor` warns about a unit that is not enabled or whose `MainPID` is not the shell it talks to.
- A lock that crashes on every start hits the unit's start limit and stays on the red screen. Recovery, from another TTY (`Ctrl+Alt+F3`): first fix the cause and restart the unit (`kanade.service`, `dist/kanade.service`), which locks again and keeps the session:

  ```sh
  systemctl --user reset-failed kanade.service
  systemctl --user restart kanade.service
  ```

  Only if that fails, end the graphical session. This closes every app in it, and unsaved work is lost:

  ```sh
  loginctl list-sessions
  loginctl terminate-session <niri-session-id>
  ```
- A hung, not crashed, shell is not covered: niri sees a live client. `WatchdogSec` stays deferred: it needs real heartbeats from the shell (`sd_notify WATCHDOG=1` from a loop that proves it is alive), not only the unit setting.
- `SIGKILL` and `SIGTERM` while locked were measured (#154); a panic, an abort or a clean exit restarts the same way under `Restart=always`.
