# 91. The UI scale is applied once, at the runtime's scale seam

Status: accepted. Builds on [ADR 0028](0028-the-runtime-is-ported-from-amane.md) (the runtime owns drawing and input) and [ADR 0030](0030-the-canvas-follows-the-largest-body.md) (the canvas never exceeds the output).

## Context

Kanade followed the output's scale only. Someone who wants everything larger or smaller (a small laptop panel, a distant TV) could only change the output's scale for every app. Every size in the shell is a constant in logical pixels, so scaling them one by one would touch hundreds of places and leave some behind.

## Decision

- `appearance.scale`, a whole percent from 80 to 200 (default 100), the same unit as `animation.speed`, is a multiplier on top of each output's own scale. The runtime holds it (`ui_scale`, `set_ui_scale`); the lock screen crate does not read it.
- Views, layouts, drawing and hit tests work in a virtual space: the compositor's logical pixels divided by it. The runtime draws with the output's scale times it, so text, icons, bodies, shaders, the canvas, input regions and pointer targets all scale together and no view knows. The buffer, the viewport and `set_buffer_scale` keep the output's own scale.
- The runtime multiplies back wherever it speaks to the compositor, and divides wherever the compositor speaks: a layer window's size, margins and exclusive zone, a normal window's size and smallest size, input and blur regions (grown to whole pixels), the backdrop copy region, the text input's caret rectangle, and pointer, drag and drop positions. The `Monitors` Service and the `Monitor` a per-monitor view gets are in the virtual space, rounded down, so a canvas that fits them times the scale still fits the output.
- niri's own sizes stay in the output's logical pixels. The fullscreen tile compare multiplies the monitor by `ui_scale()`.
- A change applies live: the config reload sets the scale, the event loop adopts it at its next wake before anything is drawn (so one frame never mixes two scales), republishes the monitors, copies the backdrops again and draws every window. Layer windows resend their size, margins and regions as the new settings they are. A normal window already open keeps its size, its content scales inside it.

## Consequences

- No geometry constant changed. A per-window or per-output scale would need a window to carry its own, which the seam allows but nothing asks for.
- Rasters prepared at a fixed density (the shadow and icon copies, 2 texels per pixel, `src/shadow.rs`) are stretched when the output scale times the UI scale passes 2, as they already are at an output scale above 2; they soften there. Text, paths and shaders are drawn at the effective scale.
- A stretched window (Full) at a fractional virtual size gets a fractional root; edges are not snapped to the real pixel grid.
- Checked by the build and unit tests only; the look, the pointer and the live resize are for a live session.
