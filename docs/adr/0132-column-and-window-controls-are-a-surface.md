# 132. Column and window controls are a Surface of their own

Status: accepted. Builds on ADR 0028 (Kanade's own runtime), ADR 0030 (the largest body), ADR 0038 (Services), ADR 0071 (niri's stream reconnects after a loss).

## Context

The focused window is the thing a scrolling layout user acts on most, and niri's actions for it are keyboard-only: consume a window into the column, expel one from it, set the column's width, move the window to a workspace. The backlog asks for them "from the Island, tied to the focused window" (`TODO.md`, "Backlog (user, 2026-10)").

Three placements were possible and two are excluded by `CONTEXT.md`, not by taste:

- **A Satellite** is for Persistent Ongoing or Critical Activities that are not the primary (`CONTEXT.md`, "Satellite", its invariant). The focused window is not an Activity and never becomes one.
- **A Peek** shows one Activity by identity, and the pin rule makes a Peek of a non-Activity undefined (`CONTEXT.md`, "Peek" and the pin invariant).
- **A Controls section** would fit, but Controls is a fixed 440x330 body already carrying a tile grid, three levels and profiles, arrangeable per ADR 0067. A fourth block is a permanent tax on every user for a feature only some use.

## Decision

- **`Surface::Window`, drawn on `geometry::PAD` (440x236)** beside `Surface::Keys`, `Session`, `Timer` and `Capture` (`src/island/geometry.rs`). "From the Island" is satisfied by being one of the Island's own Surfaces: it is not a detached window, which ADR 0131 permits only for the Launcher.
- **No new press-target at Rest.** It opens by verb, keybind and Launcher destination as the other pad Surfaces do, so the click-at-Rest-opens-Controls invariant (`CONTEXT.md`, "Presentation") is untouched.
- **The actions are niri's own, unrenamed where niri's meaning is surprising.** `ConsumeWindowIntoColumn` and `ExpelWindowFromColumn` act on the focused *column*: consume takes the first tile of the column to its right, and **expel takes the bottom tile, not the focused window**. The button says "Expel bottom" and is disabled with one tile, because a control that silently does something else is worse than no control.
- **Widths are proportions, not the user's presets.** Kanade cannot read `preset-column-widths` over IPC, so the row offers ⅓, ½ and ⅔ through `SetColumnWidth`, plus `SwitchPresetColumnWidth` for the user's own list.
- **Moving a window passes its id**, unlike the column actions. The id is the one the header showed, so a focus change between the draw and the press cannot move the wrong window.
- **It reads `Niri`, not `Windows`.** `Windows` publishes id, app and focus only; `Niri`'s `Known` already applies `WindowLayoutsChanged` and so already holds the tile size. `Seen` gains the focused window's id, its `pos_in_scrolling_layout`, its column width and its workspace; focus changes already post. Nothing polls.
- **The last non-null focused window is held across the Surface opening.** niri clears the layout window's `is_focused` when the keyboard goes to a layer surface, so reading "the focused window" after opening reads none. The actions still act on niri's own active column, so holding the last id is what keeps the header honest.

## Alternatives

**A Satellite form, or a Peek.** Excluded by the Satellite invariant and the pin rule above; both would also have to invent a meaning for a pin on something that is not an Activity.

**A Controls section (ADR 0067).** Rejected: it would charge every user of Controls for a power feature, and Controls' body has no room for a row of six tiles plus workspace pills without displacing what is there.

**Adding the fields to `Seen` versus a new Service.** Six watchers wake on any `Seen` change, so every window move would wake fullscreen, workspace, banners and privacy for nothing. The fields here change on resize and focus only, which those watchers already wake for.

## Consequences

- `CONTEXT.md`'s Surface list gains `Window`; the Module is `window-surface`; `docs/wiki/Keyboard.md` gains its row, which a test parses.
- The `Window` fields and the `Heard` extension touch the seam the Dock reads, which is this change's blast radius. A floating window and an empty column have no `pos_in_scrolling_layout`, so both are skipped and marked rather than drawn at a guessed place.
- Moving to a workspace is the one action that cannot be verified from this VM's single output; the pills read the focused window's output's workspaces.
- After a niri restart the Surface shows "No focused window" until the reconnected stream lists the windows again, because ADR 0071 reconnects with a bounded backoff. That is the same brief silence every niri-reading Surface has, not a new failure.
