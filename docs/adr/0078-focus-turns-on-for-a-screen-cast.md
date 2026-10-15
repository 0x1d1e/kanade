# 78. Focus may turn on by itself for a screen cast

Status: accepted. Builds on ADR 0060 (Focus restores, it does not reset) and ADR 0005 (the privacy cluster).

## Context

A user sharing the screen wants Do Not Disturb on for the whole cast and back after it, without a press. Focus already knows how to turn pieces on and off only as it left them.

## Decision

- **`focus.auto`** is a list of triggers, empty by default, so nothing changes unless asked. It is set in the config file only. One trigger exists: `cast`.
- **`cast`** is the `Privacy` Service's `casting()`, which the `privacy` Module already writes from niri's cast list. The `focus` Module watches `Privacy` (`service::watch`) on the draw thread; no thread, timer or poll is added. With `privacy` off nothing is written and the trigger never fires.
- **Only its own Focus.** A Focus turned on by a trigger is marked. A cast ending turns off a marked Focus only; the pieces' own rules from ADR 0060 still apply, so a piece the user changed meanwhile stays. Any `on`, `off` or `toggle` by the user replaces the Focus or, for `on`, clears the mark, so it is theirs and a cast ending leaves it. A Focus on already when the cast starts is not marked. The rules are the pure `next` in `src/sources/focus.rs`, unit tested.
- **No `calendar-busy`.** Whether an event is busy now changes at event boundaries, so it needs a wall-clock timer on every start and end of every event. That is a new source, which the task excluded.
- **Not read back.** A cast already running when Kanade starts or `focus.auto` changes is seen from the next change of `Privacy`.
- The state is in memory, as Focus's is. `focus status` says `on (screen cast)` for a marked Focus.

## Alternatives

**A separate automation Module with rules.** A policy engine for one trigger; Focus is already the switch.

**Epoch counters for the user's changes.** The marked Focus value already encodes it: every user request replaces or clears it.
