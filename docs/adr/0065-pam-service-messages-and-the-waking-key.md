# 65. The PAM service, PAM's messages and the waking key

Status: accepted. Amends [ADR 0025](0025-the-lock-screen-over-its-own-connection.md) (what the password is checked with, what the lock screen shows, what its field types) and [ADR 0047](0047-idle-in-the-shell.md) (the key that lights the monitors).

## Context

The lock checked the password with PAM's `login` service only and showed nothing of what PAM said, so a fingerprint or a smart card module that talks (`Place your finger`, `Too many attempts`) looked like a hang. A conversation that asks for anything but the password was answered with the password anyway. And the key that wakes monitors the idle Module powered off was typed into the field as its first character.

## Decision

- **`lock.pam_service`** (default `login`, unset uses it) names the service, a file in `/etc/pam.d`, `/usr/lib/pam.d` or `/usr/etc/pam.d`. A name is letters, digits, dots, dashes and underscores, up to 64, not starting with a dot or a dash, so it is a file name and never a path. A name that is invalid or has no file means no lock is asked and no password unlocks: the same refusal as a missing `login` file, because a lock no password can open must not be asked. `kanade doctor` fails on it, using the config it is given and not the shell's. It takes effect on the next check, with no restart.
- **A prompt is not a password prompt unless it is the first hidden one.** In one PAM transaction the password answers the first `PAM_PROMPT_ECHO_OFF` only. An echoed prompt (a name, a code), a second hidden one (a PIN, a one-time code) or a style Kanade does not know fails the conversation with `PAM_CONV_ERR`. This is decided over the whole message list before anything is allocated or answered, so a list that cannot be answered answers nothing. The password never reaches a second prompt, and nothing but the first hidden prompt reads it. It is never logged or shown, as before.
- **PAM's messages show.** `PAM_TEXT_INFO` and `PAM_ERROR_MSG` are cleaned (whitespace, newlines included, turned into spaces, other control characters dropped, trimmed, at most 64 characters with a trailing ellipsis), sent to the lock screen and drawn in the status line under the field for the check under way. After the check an info note is gone; an error note stays, in the critical color, until the next try, a new lock request or a sleep. A message of a check that was dropped (sleep, a new lock request) is not shown. The status text never holds what was typed.
- **One check at a time stays** (ADR 0025): the conversation callback does not change which password goes to which check, nor allow a second PAM.
- **The waking key.** When the idle Module powers the monitors off (niri's `PowerOffMonitors` answered `Done`) while a lock holds, it tells the lock screen. The first key pressed after that, and its repeats while it is held, are not typed. A pointer motion, click or scroll after it wakes without swallowing any key, and ends the wait. The idle Module also says when input lit the monitors, and a key arriving up to 250 ms after that is still the waking one, because the compositor's idle notification and the key event reach Kanade on two connections in either order. After that, or once any key was swallowed, keys type normally. A swallowed key still counts as input for ADR 0066.

## What the lock can know, and the limits

The lock screen sees only what niri sends its surfaces and what the idle Module says. Hence:

- Only a screen-off by the idle Module arms the wait. A manual `niri msg action power-off-monitors`, niri's own idle config, a lid close, a hotplug and a resume from suspend are not known, so their waking key is typed. That is the old behavior, not a new fault.
- Armed without the monitors waking by input (a mouse nudge that sends no event here, an app waking them) the next key typed is swallowed once. The cost is one character lost, and the user sees the field empty.
- Assumption not checked here: niri delivers the waking key to the lock surface. If it consumes that key, nothing is swallowed wrongly except that a password typed within 250 ms after the lit message loses its first character. Live E2E decides; the window is short on purpose.
- A dimmed display with a held lock does not exist: the dim is never drawn over a held lock (ADR 0047), and a lock surface covers an earlier dim. So the dim needs no such handling.

## Consequences

- The wiki documents `lock.pam_service`; Settings shows it under Unlocking.
- A service that sends no text messages behaves as before. A service that asks anything but one hidden prompt now fails every check, which is the safe side: the user sees the wrong-password status and the doctor line names the service.
- The wake handling lives in `crates/lock` as a pure state (`wake.rs`) with time as an input, tested; the shell only says dark and lit.
