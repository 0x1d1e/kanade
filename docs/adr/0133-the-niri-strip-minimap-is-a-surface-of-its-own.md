# 133. The niri strip minimap is a Surface of its own

Status: accepted. Builds on ADR 0028 (runtime), ADR 0030 (canvas follows largest body, list surfaces fit content), ADR 0038 (Services), ADR 0132 (window and column controls).

## Context

niri is an infinite horizontal scrolling-tiling compositor where workspaces consist of strips of columns, with each column containing one or more vertically stacked window tiles. When scrolling across a wide strip, off-screen windows are out of sight. Users frequently lose spatial orientation regarding which windows exist off-screen, what columns are adjacent, and what is running across workspaces on the current output.

Existing Kanade tools provide piece-wise functionality:
- The Dock (`windows` service) groups windows by desktop app for launch/focus, but has no concept of spatial strip column ordering.
- `Surface::Keys` and `Surface::Controls` do not visualize the layout canvas.
- niri's native overview transitions away from the desktop context and operates modally.

The backlog (`TODO.md`) calls for:
"niri strip minimap Surface: the columns and windows of each workspace on niri's scrolling strip, click to focus. Nothing like it exists in Kanade."

Three structural constraints apply:
1. Under `CONTEXT.md`, the minimap is not an Activity (it is not an event and carries no lifetime/priority); it cannot be a Satellite or Peek. It is an Expanded Surface.
2. In niri 26.04, `WindowLayout::tile_pos_in_workspace_view` is `null` for tiled windows, and PR #4147 is unmerged. Per Kanade policy, no niri forks or patches are permitted; camera tracking must be resolved client-side.
3. Kanade invariants require zero polling, zero idle frame wakeups, and strict output scoping.

## Decision

- **`Surface::Strip`, drawn as a dynamically sized list Surface (`largest.width` by dynamic content height)**.
  Width is 520px (standard calendar/list width), height morphs smoothly via `Sizes::ask` (ADR 0030) from 154px (1 workspace) to 400px (clamped to `DEFAULT_LARGEST.height` with interior scroll for 4+ workspaces).
- **Stacked workspace strips for the focused monitor.**
  The Surface visualizes all workspaces belonging to the focused output. Each workspace strip displays its columns in horizontal scrolling sequence with proportional column widths, vertical tile splits, app icons, short titles, focus rings, and urgent badges.
- **Viewport camera frame overlay.**
  On the active workspace, an illuminated camera framing rectangle indicates the portion of the strip visible on the monitor ($M_w$), calculated deterministically from column geometries and active column focus. Off-screen columns are rendered at dimmed opacity (55%), highlighting on-screen windows while retaining full clickability for off-screen windows.
- **Click-to-focus with collapse.**
  Clicking any window tile focuses that window in niri (`FocusWindow { id }`) and collapses the Surface, returning the user directly to the focused target. If pinned via right-click, the Surface remains open and tracks focus changes live.
- **Full keyboard navigation via `surfaces::ring::Ring<At>`.**
  Arrows move through a 2D spatial grid (Left/Right across columns, Up/Down between stacked tiles and workspace rows), Tab cycles windows, Enter/Space focuses, and Escape collapses.
- **Purely event-driven from `sources::niri`.**
  Subscribes to existing `WindowsChanged`, `WindowOpenedOrChanged`, `WindowClosed`, `WindowFocusChanged`, `WindowLayoutsChanged`, and `WorkspacesChanged` events. Zero background polling.

## Alternatives

- **Single workspace view with switcher tabs:**
  Rejected. Tabbed switching conceals other workspaces behind clicks, failing the primary goal of cross-workspace spatial awareness.
- **Fixed `geometry::PAD` (440x236) body:**
  Rejected. 440px is overly constrained for multi-column horizontal strips, and a fixed height either wastes vertical space for 1 workspace or squeezes multi-workspace systems into illegible ribbons. Dynamic list fitting per ADR 0030 provides optimal scale.
- **Waiting for niri PR #4147 (`scrolling_view_pos`):**
  Rejected. Violates Kanade's independent runtime principle. Kanade computes the viewport camera bounds from existing IPC primitives, with forward compatibility when `scrolling_view_pos` is detected.

## Consequences

- `CONTEXT.md`'s Surface list gains `Strip`; the Module is `strip-surface`; CLI gains `kanade strip open|toggle` (with `minimap` alias).
- `niri.rs` event parsing extracts `pos_in_scrolling_layout` from `WindowLayoutsChanged` and window events.
- Floating windows (which have no column position) are tallied in a workspace header badge rather than mispositioned on the tiling strip.
- Zero idle frames at rest; redraws occur exclusively on layout/focus events while the Surface is Expanded.
