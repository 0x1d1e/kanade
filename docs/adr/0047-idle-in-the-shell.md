# 47. Idle in the shell, with a dim grace

Status: accepted; amends [ADR 0011](0011-kanade-adapters-where-amane-lacks-a-capability.md) (caffeine) and [ADR 0025](0025-the-lock-screen-over-its-own-connection.md) (what asks for the lock).

## Context

Idle locking was hypridle's (`lock_cmd = kanade lock`). The lock then came without warning: a user reading a page who had not touched the keyboard for five minutes was locked out with no chance to move. hypridle cannot draw, so the warning has to come from the shell, and two programs counting idle on their own timers would race. noctalia's idle manager (one idle notification per behavior, a fade to black before its action) was the reference.

## Decision

- A new Module, `idle`, off by default so hypridle setups keep working. Turned on (`[modules] idle = true`), it does what hypridle did, in stages, each optional, in seconds (0 never): `idle.dim` (270), `idle.lock` (300), `idle.screen_off` (330, niri's `power-off-monitors`), `idle.suspend` (never, logind's `Suspend` through the `session` Module).
- The runtime binds `ext-idle-notify-v1` and exposes it as a primitive (`kanade_runtime::idle`): `watch(timeouts)` asks for one notification per timeout through `get_idle_notification`, which honors the Wayland idle inhibitors apps hold, and the `Idle` Service says per timeout when the user went idle, cleared on input. Kanade asks for one, at the earliest stage; the stages count from there.
- A pure scheduler (`src/idle/schedule.rs`) says which stage fires when, from the stages, idle and resumed events, inhibitors and the time it is given. It never reads a clock. A thread of the Module does what it says.
- The dim grace: from `idle.dim` until `idle.lock`, a click-through Overlay layer on every output fades toward dark (`theme::IDLE_DIM`), slow at first. Input takes it away at once. It stays dark through the lock, so the desktop never flashes between the two. With no lock after it, it fades only partly over 5 s. Reduced motion has no fade: the lock comes at its time.
- The lock runs the lock Module's path, as `kanade lock`, and waits for niri to hold it. It never waits on the dim, so a dim that cannot draw still locks on time. A lock that fails is asked again every 10 s while the user stays idle, so an error never leaves an idle session unlocked. A session already locked is kept as it is.
- Inhibitors hold every stage and take the dim away: logind's `BlockInhibited` naming `idle`, followed over Kanade's own zbus system connection, and caffeine, read directly too. Once none holds, a user still idle is counted from then. A logind that cannot be asked inhibits nothing, an idle lock lost to a wrong guess being worse.
- Lock before sleep stays the lock Module's, unchanged.
- `kanade idle status` says the stages, where the count stands and what inhibits it. `kanade doctor` warns when hypridle or swayidle runs while the Module is on.

## Consequences

- One idle owner per session: turning the Module on means stopping hypridle, which doctor says.
- The stages follow a config reload, counting from the start again.
- niri powers the monitors back on at input; the dim, if any, is already gone by then.
- With logind unreachable and caffeine turned off while the user stays idle, the stages count again only at the next idle or input event.
- Suspend needs the `session` Module, and a lock needs the `lock` Module; without them the stage only logs why.
