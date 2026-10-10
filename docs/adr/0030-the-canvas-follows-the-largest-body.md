# 30. The canvas follows the largest body, and list Surfaces their content

Status: accepted. Amends ADR 0001, whose canvas was a fixed size.

## Context

ADR 0001 gave each Island one fixed canvas, sized for the largest Presentation, and every Surface one fixed body. A long list was cut to the same rows on any screen, and a short one, a search with one match or one notification, still took the whole body. The Island and Dock sat only on the top or bottom edge.

## Decision

- **The largest body** is `island.width` by `island.height` (520-800 by 330-600), live, made smaller where an output has no room for it (`view::largest`, over the runtime's `Monitors` Service, which it watches). The Island service, the views, the list Surfaces, the Dock and Banners all read that one size. The canvas is `Canvas::around(largest).within(output)`: it changes only with the largest body, the output or the edge, never during a morph, so ADR 0001's one window and per-frame input region stand.
- **Side edges**: `island.edge` and `dock.edge` take `left` and `right`, centered on the edge; the Settings grid shows the eight places. On a side edge the body grows from the edge (`Anchor::thickness` is its width there) and the small forms stand upright (`geometry::upright`): Rest, Compact and Split turn into tall pills with their content stacked, Split's segments down it, and Tray is a column of the time over its slots. Peek stays an upright card reaching into the screen, to be read, and Surfaces keep their bodies. A small form growing elsewhere along the edge (Peek, Tray) keeps the place it stood in the input region until the pointer moves onto the new body or leaves, so a still pointer is never left out. Satellites line up below the body, along the edge, and the canvas is tall enough for them past an upright Tray. `island.reserve` reserves a strip as wide as an upright Split. Banners line up beside the Island, level with its top and down from there, moving out as its body grows; their window is as tall as the output and wide enough for them past the largest body.
- **List Surfaces fit their content**: Launcher, Clipboard and Notifications ask `IslandService::fit` for the height their content takes, chrome included. The service keeps the asks (`geometry::Sizes`), clamps each to between the Peek and the largest body, and re-aims the body as one changes, its content showing on without a second crossfade. A body never shrinks from under the pointer, which would leave it and collapse the Surface; the shrink waits for the pointer to leave. An ask can belong to one visit, so a search's height applies only while that visit lasts and the next opening starts from the fresh one. Opening a Surface again starts a visit and takes its fresh ask.
- **The Dock on another edge** steps aside at once where the Island's body or Satellites reach its strip on the output, as on corners an edge shares with a side edge.
- **Banners keep clear of the Dock's strip** (`dock::strip`) wherever their column, from the Island at rest to its end, would cover it: past it on the Island's edge, short of it on the far one, aside of it on an edge across the column. A Dock sharing a side edge with the Island sits past the Banners beside it. Those that would hang past the column's end, past the body as far as it reaches now or where it heads, wait in the stack, so a tall Surface never pushes them off the output.
- **`service::watch::<S>(f)`** (`crates/runtime`): `f` runs after each write of `S` that changed something (ADR 0035: on the runtime's thread before the next draw, not on the writer's). A Surface recomputes its ask there from its Sources, so a view never writes a Service.

## Consequences

- A watcher runs on the runtime's thread (ADR 0035), where no view's read is held; it must not block. The watchers write only `IslandService`.
- The largest body is one for every output, so a small output shrinks it on a large one too.
- A Banner waiting while a Surface is open keeps its timer, so a short one may run out unseen; the notification stays in history.
- The column steps aside of a Dock by its length at rest, so it may move as an app opens or closes.
- Other Surfaces keep their fixed bodies; one joins by asking `fit`.
- An upright body's shadow takes corners turned on their diagonal, prepared beside the wide ones; a body morphing through about square overlaps them for a frame or two.
- An upright Compact shows less than a wide one: a title becomes its app's tile, a level its icon and bar.
