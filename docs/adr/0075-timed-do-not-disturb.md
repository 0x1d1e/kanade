# 75. Do Not Disturb may end by itself

Status: accepted. Builds on ADR 0043 (the wall-clock timer), ADR 0004 (no polling) and ADR 0060 (turn off only what was turned on).

## Context

Do Not Disturb stays on until someone turns it off, so a user who wants it for a meeting or until morning must come back to it. Caffeine already takes a duration (ADR 0011).

## Decision

- **`notifications dnd on <duration>` and `on until <HH:MM>`** turn it on and off again by themselves. A duration is caffeine's (`timer::duration`: `90s`, `25m`, `1h30m`, up to 24h). A time of day is the next time the local clock reads it, so tomorrow's when today's has passed; across a daylight saving change it is an hour off. `dnd status` says `off`, `on` or `on until 08:00` (in the user's clock format), and so do `kanade status` and the Controls tile (`Until 08:00`) while it is timed.
- **The end is a wall-clock moment** (`notifications/timed.rs`), waited for on a `CLOCK_REALTIME` timer (`clock::WallTimer`) by a thread that the first timed `on` starts. Nothing polls. A suspend that passes the moment ends it on resume, and a clock set makes the thread look again.
- **It turns off only what it turned on.** Every other setting of DND (`set_dnd`: `on`, `off`, `toggle`, the Controls tile, Focus) clears the moment first, under the lock the end takes. A DND the user turned off, or off and on again, or that Focus set, meanwhile is never turned off by an old timer. A timed `on` over a DND that was on already replaces it with a timed one, as the user asked. Each setting counts in `dnd_revision`, so Focus (ADR 0060) leaves a DND alone that a timed `on` or its end changed.
- **No Activity.** The expiry is said in status and on the tile. A Transient would need a Kind and Detail in the island core for a fact the tile shows, and DND silences Banners, not the island.
- **No per-app timed mute.** `notifications.rules` are config-file tables with no runtime state; a mute that ends would need a state of its own.
- **The state is in memory.** A restart ends DND with Kanade, as before.

## Alternatives

**A monotonic timer, as caffeine's `sleep`.** It stops during suspend, so a DND until 08:00 would end late after a night's sleep, and a time of day has no monotonic meaning.

**Poll once a minute.** Against ADR 0004.

**Carry the duration in the island's `Command::SetDnd`.** `island/` takes no clock and the Arbiter reads none; the end is the source's.
