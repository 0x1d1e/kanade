# 60. Focus is one switch over the Modules that already quiet the session

Status: accepted. Builds on ADR 0046 (night light), ADR 0007 (Banners), ADR 0030 (the Controls canvas).

## Context

Quieting the session takes three separate presses: Do Not Disturb, caffeine and the night light. Each is a verb and, for two, a Controls tile. Users want one switch that does the lot and puts it back.

## Decision

- **A `focus` Module owns `kanade focus on|off|toggle|status` and three settings,** `focus.dnd` (true), `focus.caffeine` (true) and `focus.night_light` (false). It starts no thread and reads no Service of its own.
- **It calls what the pieces' own verbs call:** `notifications::set_dnd`, `caffeine::request` with a detached `Later`, `night_light::turn`. It runs on the draw thread, as those verbs do.
- **`off` restores, it does not reset.** `on` turns on only a piece that is wanted, whose Module is on and that is off now, and remembers which. `off` turns off those alone, and Do Not Disturb only if it is still on. A session that had caffeine on keeps it.
- **The Modules it drives are optional** (`notifications`, `caffeine`, `night-light`): with one off, its piece is skipped. Focus itself can be turned off; its verb is then refused and its button is not drawn.
- **The state is in memory.** A restart forgets what Focus turned on. Caffeine and Do Not Disturb end or reset with Kanade; the night light is in `settings.toml` (ADR 0046) and stays on until turned off. This is stated in the wiki.
- **No Activity.** Caffeine already shows its own while on. A Focus Activity would need a new Kind and Detail in the island core and views for a state that the Controls button shows, and `island/` stays free of this.
- **The Controls button is round and icon-only,** at the end of the third row of switches, a new `Focus` icon, filled while on. The Controls shape is fixed and full (ADR 0030), so the row's tiles narrow to make room only while the charge limit tile and the button share it; otherwise Night Light keeps its half. It is a keyboard target (`At::Focus`).

## Alternatives

**A Focus tile like the others.** Controls has no room for a fourth tile row, and a laptop's third row holds two already.

**Remember the state on disk.** It would restore the night light across a restart, at the cost of a state file and a stale-state question after a crash; Focus stays a switch, not a mode.

**A policy engine of rules per app or time.** Not needed for one switch; the `notifications.rules` and the night light's schedule already carry those.
