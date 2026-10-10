# 33. The privacy cluster is dots inside the Island

Status: accepted. Supersedes ADR 0032; amends ADR 0005: the cluster stays outside the Arbiter and takes no pointer, but it no longer shows over a fullscreen window.

## Context

ADR 0032 drew the cluster as a pill beside the Island's body, with a glyph style and a dot style, and raised the Island's window to the Overlay layer to draw it alone over a fullscreen window. The pill needed its own room on the canvas, shifted Satellites, and overlapped the Dock's icon on a merged Keystone or Fold. The Overlay raise made the covered state a second drawing path.

## Decision

- The cluster is a dot of the capture's color each (microphone, camera, screen cast), at the trailing end of the body, inside it: the right end of a horizontal Island, the bottom end of one on a side edge. The primary leads, so the dots trail it, as on macOS, iOS and Android.
- Beside a Rest, Compact, Split or Peek form the dots take room from the form: `view::shape` gives the form less of its trailing axis (`Room`, set while the Island's own forms are built; the HUD capsule is not), so anything centered, the clock at Rest included, lays out in what is left and so stands toward the start. Forms lay out from their `shape`, so none knows of the dots.
- A Tray strip or an expanded Surface has no dots: its slots and header fill the body. The Controls Surface header names the apps that capture.
- Covered by a fullscreen window, the cluster does not show. The Overlay raise for it is gone; the Overlay layer serves the OSD and an open Surface only (ADR 0037).
- `privacy.indicators` (Settings, Privacy) turns the dots off. The `privacy.style` choice is gone. The Controls header follows `privacy.indicators` too. Disabling the `privacy` Module still warns at start.
- Capturing, the Island does not autohide and a bare Rest shows (unchanged), so the dots are not off screen, unless a fullscreen window covers the output.
- Still never an Activity, never in the Arbiter, and no pointer.

## Alternatives

**Keep the pill and the Overlay raise (ADR 0032).** The only way to guarantee a capture indicator under a fullscreen window. Rejected for the user's call: a fullscreen window is the user's own choice of what to see, and the pill's room, Satellite shift and Dock overlap were costs on every other screen.

**Leading end.** Takes the primary's place, and shifts what a user reads first.

**Change every form to leave room.** Reflowing at `view::shape` does it at one place.

## Consequences

- A capture under a fullscreen window is not shown on that output. ADR 0005's guarantee that nothing covers an indicator no longer holds; the Controls Surface and the compositor's own indicator are the only other signs.
- `geometry::cluster`, `cluster_room` and `CLUSTER_LONGEST` are gone. The canvas keeps its `ROOM` so nothing moves.
- Tray and Surfaces show no capture on the Island itself.
- Three captures at a bare Rest with a 12-hour clock and a battery can overflow the form's `shape`.
- `privacy.style` in an older config is dropped by the schema 3 migration.
