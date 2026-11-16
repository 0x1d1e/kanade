# 160. Dragging a pinned icon reorders the Dock

Status: accepted. Builds on [ADR 0031](0031-the-island-and-the-dock-merge.md) (the Dock and the
Island share a window) and the settings file's one-write rule.

## Context

`dock.pinned` gives the pinned apps their order, and the Dock draws them in it before the running
apps none pins. The order could only be changed by editing the config or unpinning and pinning
again, which appends at the end. The runtime already reports a left press, every move until its
release, and the release (`Rectangle::on_drag` and `on_drag_end`), so a pinned icon can be moved
along the row without a new input path.

Two things must not move: the slot's hit area, or the press that starts a drag would be lost as
the icon slides away from the pointer, and the row's layout, strip and magnification, which are
what a click, the dots and the Dock's geometry are built on.

## Decision

- **Only pinned slots drag.** A slot with an `Item::pin` gains `on_drag` and `on_drag_end`; a
  running app that is not pinned has no place in `dock.pinned` and keeps its own. A pinned id no
  entry has (the `?` placeholder) drags as any other pin does.
- **The pointer's travel along the row is the drag.** The first local coordinate along the row (x
  for a bottom or top Dock, y for a side one) is the anchor; each move sets `axis` to the
  distance since. Below 8 px nothing moves and the press is still a click. `Pointers` holds the
  one drag in progress, as it holds the hover, so every Dock reading it redraws as it moves.
- **The hit area stays put and the icon moves.** The slot that takes the press keeps its place;
  its icon and dot are drawn in a child rectangle translated by `axis` (and by ±`pitch`, the
  rest distance between two icons, for the slots the drag has passed). The row, strip, dots,
  magnification and input region are unchanged, and the press keeps its target for the whole
  drag.
- **A release writes the order once.** Past 8 px, `shift = round(axis / pitch)` moves the pin
  from its place to the new one, clamped to the list, remove then insert. The list is read from
  `config::get().pinned` (the pin followed by id, since a reload may have changed it) and written
  by `settings::set_pinned_order`, one `dock.pinned` array, over the file as it stands so every
  other key stays. A failed write is logged and the Dock keeps the order it had.
- **A click after a drag launches nothing.** The drag records that it moved; the slot's click
  that ends the press checks it, resets it, and returns, so neither a launch nor a focus follows
  a drag. A press that never passes the threshold is an ordinary click. Letting go outside the
  window ends the drag with no reorder.

## Consequences

- Dragging a pinned icon past its neighbour parts the row by one slot and, on release, the new
  order is `dock.pinned`; the config reload draws it and it survives a restart.
- Running apps that are not pinned cannot be reordered, and a drag of one does nothing.
- The drag is one per session, not per output: a Dock on another monitor showing the same pins
  parts them with the dragged one. The order is shared, so this previews the result everywhere.
- **Not run live**: a drag over a real output, the parting gap and the written order (E2E NOT
  RUN, no session). The reorder and offset helpers and the settings write are unit-tested.
