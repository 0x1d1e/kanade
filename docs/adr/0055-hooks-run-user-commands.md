# 55. Hooks run user commands on a closed set of events

Status: accepted. Settles the "Hooks/event automation" deferred decision in `docs/design.md`; plugins stay deferred.

## Context

Users want a script on a few moments of the session: a wallpaper to theme something Kanade does not, a lock to pause a player, a charger to change a power setting. The deferred questions were the event model, permissions, execution isolation and failure or backpressure semantics, with the warning against shell-script event soup.

## Decision

- **Event model.** A closed set, no subscriptions and no user-defined events: `lock`, `unlock`, `wallpaper`, `tone`, `charger`, `suspend` (before sleep) and `resume`. Each has one command, `hooks.<event>`, an argv list in config (`["notify-send", "Kanade", "hook"]`), run without a shell; `["sh", "-c", "..."]` is the user's explicit choice. An empty list is no hook. A new event is a code change and an ADR amendment.
- **Payload.** Environment variables of the child only: `KANADE_HOOK` (the event name), and `KANADE_WALLPAPER` (the path), `KANADE_TONE` (`dark` or `light`) and `KANADE_CHARGER` (`plugged` or `unplugged`) for their events. Never argv, never stdin (null).
- **Who fires.** The Module that owns the fact, from its own watcher or source: the `lock` Module the lock, unlock, suspend and resume (its logind sleep source), the `wallpaper` Module its wallpapers, the `battery` Module the charger, a config reload the tone. A Module that is off fires nothing, and neither does an off `hooks` Module (no commands run, nothing else changes). `charger` and `wallpaper` take their first observation as a baseline and fire only on a change after it, so starting Kanade fires neither.
- **Permissions.** The config is the user's own files, so a hook is the user running their own script; no more is granted than the user's account has. Hooks are set in files only: `kanade config set`, Settings and `settings::set` refuse a `Kind::Argv` key, so a remote caller of the IPC cannot make Kanade run a command. A hand edit of `settings.toml` in the state directory still applies, as any file does.
- **Isolation.** The bounded child runner (`appearance::child`): `hooks.timeout` seconds (default 10, 1 to 300), 1 MiB output cap, killed on timeout, under `setpriv --pdeathsig KILL` where found, so a hook dies with Kanade. Only the direct child is killed: a `sh -c` command's own children may outlive the timeout, and a hook that must clean up should `exec`. The command runs on a thread of its own, so no Service, watcher or the draw thread ever waits for one.
- **Backpressure.** At most one run of an event is in flight. A fire while it runs is dropped, counted and said once on stderr, so a slow command never piles up. Events do not wait on each other. `suspend` is the one wait: sleep is let go when the hook ended or after logind's delay, whichever is first, started before the lock is asked, and waited for only within logind's delay, so a slow hook can hold sleep until then even when the lock was quick.
- **Failures.** A failing command (spawn error, non-zero exit, timeout, panic) is kept for `kanade status` with the event, the streak and the first line of what it said, and logged once per streak on stderr. It is never an Activity, so a broken hook never reaches the island.
- **Logging.** Only event names are logged; a wallpaper path, like any payload, is not.

## Consequences

- No hook can observe a failed sleep: `resume` fires after any `PrepareForSleep(true)` ending, also one that did not suspend.
- Two wallpaper paths for the same file, spelled differently, may fire twice.
- A hook edited while running applies from the next fire; a `modules` change takes a restart as always.
- The naming overlaps `modules::Hooks` (a Module's `status` and `reload` callbacks); the user-facing term is Hook, as in `CONTEXT.md`.
- Plugins, an ABI and subscriptions to arbitrary events remain open.
