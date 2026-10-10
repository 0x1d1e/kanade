# 32. The privacy cluster rides the Island

Status: superseded by ADR 0033 (dots inside the body, none over a fullscreen window). Was: amends ADR 0005: the cluster stays outside the Arbiter, but is no window of its own.

## Context

ADR 0005 gave the cluster its own `Layer::Overlay` window in the top right corner of every output, so a fullscreen window never covers it. There it was far from the Island and the place the Island was, and ignored the Island's edge, side and autohide.

## Decision

- The cluster is a pill drawn by the Island's window (`view::island`), where the first Satellite would stand: beside the body's end toward the middle of the output, centered on its small form, and as far as the body goes when it grows (`geometry::cluster`). A dot of the capture's color each (`privacy.style = "dots"`), or glyphs. Satellites stand past it (`geometry::cluster_room`).
- Still never an Activity, never in the Arbiter, and no pointer: it is outside the Island's input area.
- Capturing, the Island does not autohide and a bare Rest shows, so the cluster is never off screen.
- Capturing, the window is on the Overlay layer whatever `Fullscreen::covers` says, as that is a guess (an app id, a tile size) that may miss a window.
- Covered by a fullscreen window, the Island's window goes up to the Overlay layer and draws the cluster alone, at where the body would stand at Rest, with no input and no keyboard. This keeps ADR 0005's guarantee that nothing covers a capture indicator.
- The `privacy` module opens no window of its own.

## Consequences

- One window and one pipeline for the Island and its indicator; the cluster follows the Island's place, morph and autohide.
- A merged Keystone or Fold draws the pill over the Dock's icon on that side (the pill is the later layer, so a capture is always seen) of the body.
- The canvas keeps room for the longest pill (three glyphs, `geometry::CLUSTER_LONGEST`) on each side of the largest body, so the pill stands beside any Surface rather than over it.
