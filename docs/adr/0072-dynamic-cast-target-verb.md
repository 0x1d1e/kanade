# 72. The dynamic cast target as a verb, and paused casts named

Status: accepted. Builds on [ADR 0005](0005-privacy-indicator-outside-arbiter.md) (privacy is never an Activity) and the Stop sharing wiring of the Controls header.

## Context

niri's IPC reports for each cast `is_dynamic_target` (actions like `SetDynamicCastWindow` act on it) and `is_active` (false when the consumer pauses it, as OBS does on another scene). It has three actions to retarget a dynamic cast: `SetDynamicCastWindow { id }`, `SetDynamicCastMonitor { output }` and `ClearDynamicCastTarget`, where no id or output means the focused one. Kanade used neither field.

## Decision

- `Seen` (the `Niri` Service) gains `dynamic` (some cast has a dynamic target) and `paused` (casts run and none is active). A field niri does not send counts as not dynamic and active.
- `kanade capture cast window [<id>]|monitor [<output>]|clear`, a verb of the `capture` Module, asks niri for the action. It is refused while no cast is dynamic, rather than ask niri for a no-op; a niri without the actions answers an error, which is the refusal, as for `stop-cast`.
- `Privacy.paused` copies `paused`, and the Controls header says `(paused)` after the casts it names. The dots are unchanged: a paused cast is still open and can resume, so it still counts as capture. Nothing enters the Arbiter.
- No button. The Capture Surface is one row of four tiles, and the Controls header has the apps, the casts and Stop sharing in one row; room for share-this-window, share-this-screen and clear in either means new geometry and a new focus level, for a feature few portals use. A bind can run the verb. A button is left for when a dynamic-target share is common enough to ask.

## Consequences

- The verb acts on niri's focused window or output, which is what the user means from a bind, but not from the Capture Surface if it ever takes focus.
- A cast niri lists with a target Kanade does not know is not named in the header, so its pause is not either.

Files: `src/sources/niri.rs` (`retarget`), `src/sources/capture.rs` (`cast`, `parse_cast`), `src/sources/privacy.rs`, `src/surfaces/controls/mod.rs`, `src/modules/catalog.rs`, `src/ipc.rs`, `src/cli.rs`.
