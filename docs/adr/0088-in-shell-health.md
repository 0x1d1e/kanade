# 88. Health in Settings, from the doctor's checks run in the shell

Status: accepted. Extends `kanade doctor` (design.md CLI) and Settings (ADR 0029).

## Context

`kanade doctor` finds what is missing (a helper, a bus name, a protocol), but only a user who knows to run it sees it. A fresh install shows a shell that quietly does less.

## Decision

- `doctor::in_shell()` returns the same `Check`s as `doctor::run()`; both call one `gather`. The shell's own line is not asked of its IPC socket but built from its pid, since it is the shell. `Check` and `Verdict` are public; the checks stay in `src/doctor/`, so `wayland-client` stays there too.
- Settings has a Health page (group System) with a `Row::Health` that draws the checks read-only: verdict, text, detail lines. The checks run on a worker thread when the page first shows, when the window opens on it, and on Check again; a draw never waits on them. Health fixes nothing: doctor has no fix mode and Kanade owns no state a fix could touch.
- At the first start of a shell with the `settings` Module on and `settings.health_notice` true (default), a worker waits 30 s so the session and the shell's own names have settled, then runs the checks and, if any warns or fails, posts one failed-form Live Activity ("N checks need attention", "open Settings, Health") on the focused output. No custom click: the island layer does not know Settings.
- Told once: `$XDG_STATE_HOME/kanade/health-notice` is written (`atomic::write`, owner only) before the checks run, whatever they find. If it cannot be written nothing is told, rather than at every start. After a first start that found nothing, no later problem is announced; the Health page and doctor show it.
- Off: `settings.health_notice = false`, or the `settings` Module off, runs no thread. The key takes effect at the next start and is not marked Restart, as nothing is left pending.

## Consequences

- A first start that finds a service not yet up (a slow session) tells something that is gone by the time Settings opens. The page then shows all well.
- The checks open a Wayland connection and run `pw-dump`/`pactl` in the shell's process; they run a few times a session at most.
- Another doctor check shows on the page the day it is added; no other code changes.

Files: `src/doctor/mod.rs`, `src/settings/health.rs`, `src/settings/layout.rs`, `src/settings/view.rs`, `src/sources/live.rs`.
