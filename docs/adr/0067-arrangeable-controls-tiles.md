# 67. Arrangeable Controls tiles

Status: accepted. Amends [ADR 0060](0060-focus-one-switch-over-existing-modules.md) (Focus is a labelled tile like the others, not a round button at the end of a row) and [ADR 0030](0030-the-canvas-follows-the-largest-body.md) (the canvas stays fixed; what fills it is configured).

## Context

The Controls switches were a fixed list: Wi-Fi, Bluetooth, caffeine, Do Not Disturb, the night light, the charge limit where supported, and Focus as an icon-only button squeezed in beside them. A user who never charges to a limit still lost room to it, and one who wants Focus larger, or the night light as a bare button, had no say.

## Decision

- `controls.tiles` is an ordered list of `"name"` or `"name:size"`: `wifi`, `bluetooth`, `caffeine`, `dnd`, `night-light`, `focus`, `charge-limit`, in `half` (the default), `full` or `icon` (a round button). Wi-Fi and Bluetooth have no `icon`: their chevron opens a sub-surface. The default is the first six, so the charge limit is off until added, and Focus is a half tile with its name and state.
- `src/tiles.rs` is the pure model: parsing, packing and the edits. Tiles pack left to right; a row holds two halves' worth of room and up to three round buttons (one beside a half, which would otherwise be under 160 px and unreadable), and a tile that does not fit starts the next row. A list is refused (config, editor and Settings alike) when it needs more than the three rows the canvas has, so what is stored always fits. A tile that has no place on this machine (Focus with its Module off, the charge limit without support) is left out and the rest repack.
- Two ways to edit, one value. Settings lists the tiles with move, size and remove buttons and a field to add one. The pencil at the Controls header's end opens an arrangement as a sub-surface: the tiles as they are, the chosen one filled; a toolbar to move it earlier or later, give it another size, or hide it; a tray of hidden tiles to bring back. Done writes `controls.tiles` (or removes the override when it is the default) and reloads; Escape or the back chevron drops the draft, as does leaving the Surface, since the draft lives in the Surface's focus. An edit that would not fit is not offered. The ring reaches every control.

## Consequences

- No drag to reorder yet: the runtime reports a drag only as moves with no release, so a drag editor needs a pointer-capture primitive first. Move buttons do the same by keyboard and pointer.
- Hidden tiles keep their actions on the CLI (`kanade focus`, `kanade night-light`, `kanade battery limit`).
- The keyboard grid's tile rows come from the same layout as the drawing, so the ring cannot point at a tile that is not there.
