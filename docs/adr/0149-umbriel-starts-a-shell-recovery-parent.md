# 149. Umbriel starts a shell recovery parent

Status: accepted. Amends ADRs 0018, 0025 and 0147 for Umbriel. niri's service and logind recovery path remain as before.

## Context

Umbriel's autostart forks and executes `kanade` once; its SIGCHLD handling reaps a dead shell without restarting it. Umbriel keeps an opaque blank screen when a lock client dies and accepts a replacement lock client, but does not set logind's `LockedHint`. Restarting Kanade without surviving lock intent therefore cannot restore its lock UI. This is source evidence, not a live destructive reproduction.

The startup owner must remain Umbriel: `dist/umbriel/config.toml` retains `autostart = ["kanade"]`. Moving ownership to systemd would change the requested session model.

## Decision

- Bare `kanade`, when Umbriel IPC is selected, becomes a non-rendering recovery parent before allocator, runtime and Modules initialization. It executes the same binary through a private child entry, with the compositor's environment inherited. Normal CLI verbs and native messaging do not enter recovery. niri still starts the shell directly under its existing service.
- The parent takes a nonblocking exclusive flock on `$XDG_RUNTIME_DIR/kanade-recovery.owner`. This empty file carries ownership only, no lock state; it is never unlinked, avoiding split ownership across inodes. The runtime's existing shell singleton stays separate. A reachable existing shell aborts recovery rather than creating a loop of rejected children. The parent is named `kanade-recover`; `pgrep -x kanade` continues to identify the renderer, and `kanade status` reports its parent PID. Doctor verifies that parent instead of requiring the niri unit under Umbriel.
- The parent binds to the UID/PID returned by `SO_PEERCRED` on the selected Umbriel IPC socket and holds a pidfd for its lifetime, rechecking the listener after opening it. PPID is not the compositor identity: Umbriel may spawn via another launcher or a transient scope. No new Wayland connection or renderer is added.
- There is one child at a time, reaped before replacement. Any child exit restarts after 250 ms, doubling to at most 8 s; a run of at least 30 s resets the delay. There is no restart-count limit that could strand a locked session. Spawn/setup errors are diagnosed and stop the parent. SIGTERM/SIGINT of the parent or compositor exit interrupts both child watching and restart delays. Shutdown sends the child SIGTERM, waits up to 6 s for its existing cleanup, then kills and reaps it. `scripts/dev` allows 7 s for the parent to finish. An abrupt parent death kills the child through `PR_SET_PDEATHSIG`.
- The private inherited socket pair carries only lock-intent bytes and acknowledgements. Its child endpoint is duplicated with close-on-exec and stdin is reset to `/dev/null`, so shell helpers cannot consume or inherit the channel. Before every compositor lock request, under the lock request mutex, the shell sends intent and waits for the parent to record and acknowledge it. Failure exits the shell before the compositor can receive an untracked lock request. The parent passes its retained intent to each replacement, which relocks before running the shell even with a false or absent logind hint. Logind remains a secondary startup source and remains authoritative for niri.
- Intent persists through pending locks, a held lock, password acceptance and a queued unlock. The existing lock connection now issues `wl_display.sync` after `unlock_and_destroy` and reports `News::Unlocked` only on that callback. Flushing a request alone is insufficient confirmation. The shell clears intent only on this confirmation or definite `Finished` for its newest request. Older news and connection loss never clear newer intent.

## Verification

Disposable subprocess fixtures exercise actual process exit/restart, singleton refusal, a pre-existing shell socket, compositor PID death, parent termination and restart-delay cancellation, reaping, and acknowledged lock intent inherited by a replacement. Channel checks ensure a lock checkpoint waits for acknowledgement and rejects a closed parent channel. Lock request/state checks cover stale confirmations and the existing interleavings.

Live Umbriel crash/lock/replacement/authentication acceptance is a separate session gate. Automated checks neither kill nor lock the user's compositor or shell.

## Limits

The parent itself is not supervised. Killing it abruptly or a parent failure ends automatic recovery and loses in-memory intent. Umbriel remains fail-closed on its blank lock screen after a locker dies; starting a new parent cannot infer the old lock without `LockedHint`. Recover from a TTY with a compatible replacement locker or end the graphical session as a last resort (unsaved work is lost). Do not intentionally stop the recovery parent while locked expecting a new parent to restore its UI.

A hung child is not detected; this is process-exit recovery, with no heartbeat watchdog. A crash loop continues at the capped delay. An ambiguous crash after a successful unlock but before the parent clears intent can conservatively relock. There are no passwords and no lock-state files in the recovery protocol.
