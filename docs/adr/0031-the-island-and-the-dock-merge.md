# 31. The Island and the Dock merge where they share an edge and a side

Status: accepted. Amends ADR 0001 (one window, the body morphs in a fixed canvas), ADR 0024 (a pane of glass per body) and ADR 0030 (the Dock beside the Island).

## Context

An Island and a Dock on the same edge and side were two windows with two panes of glass: the Dock sat beside the Island, further in by the room its small forms and hover take, and faded aside while the Island grew over it. Two objects in one place read as a clash, not as one shell.

## Decision

- **`dock.merge`** picks how they join: `crown`, `keystone`, `fold`, or `off` (the old beside). It applies to a top or bottom edge, where both sit on the same edge and side, with the Dock on, showing an app, and the Island not autohiding. A Dock that autohides merges as a Fold, whatever `dock.merge` names: it is out only while the pointer holds it, and an always-out Crown or Keystone would not hide. Across a side edge, with another side, or on another edge they stay apart as before.
- **One window.** Merged, the Island's window draws both and the Dock's own window is hidden, kept up only to let go the glass pictures it left (`dock::merged`). The window is the shell: the Island's canvas and the Dock's, each at an origin (`merge::Shell`), the Island standing in from the Dock by the offset (`merge::offset_of`). ADR 0001's one window and per-frame input region stand: the input region is the Island's area and the Dock's, each moved to its place.
- **One pane of glass.** The Island's body and the Dock's plate are two rounded rects united by a smooth minimum in the shader (`glass.wgsl`, `glass::Pane::united`): the rim, the tint and the highlights follow one outline, and the glass captures the bounds of both. The body's own content is drawn over it; the shader cuts the tint to the outline, as no rounded rect clips it.
- **Crown** (this ADR's first form): the Dock stays a plate on the edge and the Island hangs from its inner side, reaching `merge::OVERLAP` into it, the two edges pulling together into a neck. The Island's body stands the offset further in, and what hangs from it (the Banners) and what is reserved for it stand `merge::apart`, the offset and the clearance. The Dock's icons grow under the pointer as alone, over the Island's body where they reach it; `dock.reserve` counts toward the Island's.
- **Keystone**: one pane, the Island the middle piece of the Dock's row, its icons on either side of the body (all on the inner side along a side of the edge). The plate is the strip around them; the body stands in the middle of its thickness and grows out of it as a T. The row is laid out again each frame from the body's width (`dock::between`, `merge::Shell::plate`), so the icons slide as it morphs. The window holds the icons as far as they reach with the body at its largest and the icons grown (`Extent::swell`, `rise`). Icons grow under the pointer on each side separately (`dock::swell`), laid out outward from the body, which stays where it is, so the plate widens outward. The Dock's target forwards the moves over the Island's area to the Island's own, as it overlaps it. Icons and dots stand half the difference between the gap and the dot's room away from the edge (`dock::balance`), so a plate of them looks level, flipped for the bottom.
- **Fold**: a Keystone that is out only while the pointer holds it, as an autohidden Dock is (the Dock's own `shown` spring, its delay on leaving). Folded, the plate is the body and the icons are in it; they come out nearest the Island first, growing from a little smaller, and fold back as the body grows past the small forms, so a Surface opening folds them away. The pointer on the Island brings the Dock out instead of peeking or showing the Tray (`IslandService::hover`'s `peeks`); a right click still peeks, and a click opens a Surface.
- The Dock's laying out is `dock::lay`, which both its window and the Island's use, so the plate and the icons are one code path whether merged or not. The Dock's target (hover, pointer along the row) is as big as the area it reaches, not its window, so it never takes the pointer from the Island's body.

## Consequences

- The Island's window grows by the plate's depth and the Dock's width while merged, and redraws with the Dock's pointer and the running apps.
- The Dock's own windows, `dock::strip` for the Banners and `dock::reserve`, give way while merged.
- Fold gives up hover Peek and the Tray. A Crown's grown icons cover the Island's body under them while the pointer is on the Dock.
- A side edge stays beside: the Dock's row would stand in the way of the Island's upright forms.
- The Dock's targets come after the Island's, so a press on a body's button keeps its place when an app opens.
