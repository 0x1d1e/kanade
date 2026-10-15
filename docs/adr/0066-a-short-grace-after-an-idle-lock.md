# 66. A short grace after an idle lock

Status: accepted. Amends [ADR 0047](0047-idle-in-the-shell.md) (what the idle lock does once it holds) and [ADR 0025](0025-the-lock-screen-over-its-own-connection.md) (what ends a lock). The ship gate of the design stands.

## Context

An idle lock that fires a few seconds before the user comes back to the keyboard costs a password for a session they never left. macOS and others give a "require password after" delay for that. A grace is also a password bypass, so it is allowed only where it cannot outlive its cause and cannot be reached except by input at the machine: the user's, or an attacker's at the keyboard, or a process that injects input (`wtype`, uinput) while the window is open, which is why it is off by default and short.

## Decision

`lock.grace` is seconds, default 0 (none), at most 30 (a larger value is capped to 30 where it is read). While it is nonzero, the lock the idle Module asks for opens a grace once niri holds it. Within it, any of these on the lock screen unlocks the session through the same steps as an accepted password (`unlocking`, the unlock asked of `kanade-lock`, the lock screen told):

- a key pressed (not a repeat; the key that woke the monitors counts, though it is not typed, ADR 0065),
- a pointer button pressed,
- a scroll of nonzero amount,
- a pointer motion of at least 4 logical pixels from where the pointer was. A pointer arriving on a surface sets where it was and counts for nothing, so a lock surface appearing under a still pointer, or a pointer crossing to another output, does not unlock.

### Where it is impossible

- **Not for `kanade lock`, not for a lock before sleep, not at a restart.** Only `lock::hold_idle`, called by the idle thread's `Lock` action, can ask for a grace. `request_lock` at start (the relock after a crash), `kanade lock`, the Session Surface and the sleep path ask without one.
- **Not for a lock asked again after a failure.** A lock the idle Module asks again because the first one failed (`SEEN.failed` set) gets none.
- **Not when a lock is already there.** The grace is decided under the requests lock before the ask, and only if no lock is held, on its way, ended but for its unlock, or in the way of a sleep (`graceable`). An idle lock firing again while one holds does not re-arm.
- **Only for the request it was given for.** It is set only if that request is the newest and the confirmed one and the lock is Held.
- **Gone at once** with any later request (`kanade lock` included), a sleep, an unlock or end news from `kanade-lock` (`Unlocked`, `Finished`, `Gone`), an accepted password and the first input that uses it. A password being checked is not interrupted by it.
- **Gone at expiry.** The decision to unlock reads `Instant::now()` at that moment, under the requests lock, not the time the key was pressed: input handled late finds the grace gone. After expiry no input unlocks. The deadline is bounded at 30 s from when niri held the lock.

### Ship gate

The gate: kill Kanade while locked, niri stays locked, systemd restarts it, the lock UI is reacquired and a password unlocks.

- A killed Kanade leaves niri locked on its red surface. There is no unlock path without the process: the grace is a decision in the shell's memory and `kanade-lock`'s thread, and ends with them. Nothing is written to disk or sent to niri that could unlock.
- The restart relocks through `request_lock`, which never grants a grace. A grace is not carried across a restart, so killing Kanade does not restart a grace window, and cannot make a later input unlock.
- A grace exists only for a lock the idle Module asked for from the fully unlocked state, in the seconds after niri held it. A user at the keyboard has the same ability they have by typing the password; an attacker at the keyboard in that window is who the grace trades away, which is why it is off by default and capped at 30 s.
- The failure mode is closed: when the unlock cannot be asked (`kanade-lock` gone) the lock stays; when the grace is unknown, absent or expired, input does nothing.

## Visible

`kanade doctor` warns while `lock.grace` is nonzero, and `kanade status` shows `lock: grace N s: ...` while it is. Settings places it under Unlocking, with `lock.pam_service`.

## Not decided here

- Per-input kinds (keys only, no pointer). One setting, one rule.
- A grace for the lock before sleep, where the user is gone by definition.
- Putting the grace in the lock crate. The crate reports touch; the shell decides, as with passwords.

The window is measured in time since boot, suspend included (`CLOCK_BOOTTIME`), not in `Instant`, so a machine that slept through a missed sleep signal has no grace left on waking. The grace is also not granted to a lock asked again after the first ended.
