# 36. The Island window's scene is resolved once

Status: accepted. Amends ADR 0031, whose shell `view::island` laid out and then re-derived piece by piece.

## Context

`view::island` computed the body, its input area, the Dock's plate, the body joined to the plate for the glass, the same at four instants ahead for the capture, the window's size and the input region, each by its own expression over the shape, `autohide`, the shell and the Dock's layout. Banners and the Dock repeated the shape-bounded, slid body. A change to where the body stands had to be made in each.

## Decision

`src/scene.rs` holds the placement as pure data, tested without a Wayland connection or a Service:

- `Stand`: the Island's canvas and the edge it hangs from. `Stand::pose(shape, out)` bounds a shape by the canvas; `body`, `area` and `liquid` place a `Pose` in the canvas.
- `Scene`: a `Stand`, the merged shell if any, and the Dock's strip (`scene::Dock`, from `dock::Laid::fit`) once the Dock is laid out around the body. It answers the window's size, where the canvas lies in it, the plate, the body joined to the plate for liquid glass (`glass`), and the input region (`reach`) for any pose.

`view::island` reads the Services and the instants, builds the poses, and draws and hit-tests from the `Scene`. Banners place their cards by `Stand::body`.

What stays outside it: `Pose` comes from the Island's spring and `autohide` at an instant, so those still read Services; `dock::between` and `dock::lay` still build widgets and request frames, as they hold the icons; space reservation (`view::reserved`) still adds `merge::apart` to the rest reach.

## Consequences

- A change to where the body stands is one change; the capture's pose at a later instant takes the same path as the one drawn.
- The Dock's own window and the Dock's reach test across edges (`dock::island_reach`) still place the Island's body themselves.
- The covered Banner and Dock policies stay their own, decided: under a fullscreen window the Top layer is the compositor's to cover, so the Dock and the Banners are covered with it (and capture nothing) and only the Island is raised to Overlay, for the OSD or an open Surface. Their windows are not combined with the Island's, which would raise the Dock and every Banner over a fullscreen window too.
- `view::reserved` and the Banners ask `merge::apart` for how far the merge pushes the body from its edge, one answer for both. `dock::island_reach` places the body at a shape with `geometry::body`, not the Island's pose, as it asks where the Island will reach.
