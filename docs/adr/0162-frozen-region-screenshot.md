# 162. Area screenshots freeze the screen in Kanade's own region picker

Status: accepted. The second slice of "Built-in screenshot, region picker and annotation editor"
(`TODO.md`), after the annotation editor (ADR 0142). It settles the region picker for an area
screenshot; window and output screenshots are still the compositor's.

## Context

`capture screenshot area` was the compositor's picker: niri's own screenshot UI under niri, and
`slurp` then `grim` under Umbriel. Both show the live screen while the user drags, so what is
saved can differ from what was under the pointer when the drag started: a video, a notification,
the picker's own scrim, or the drag itself moves first. The shell already draws its own region
picker for area recording (`src/sources/capture/region.rs`, the moved
`src/sources/recording/overlay.rs`, ADR 0116), on an Overlay layer window with a scrim, a
crosshair, a transparent cutout, a border and a pixel badge.

The runtime has owned the Wayland connection since ADR 0028 and already had a `wlr-screencopy`
client, removed with Liquid Glass (`fae1e04`). The compositor advertises the protocol under both
niri and Umbriel, and `grim` reads it under Umbriel, so a frozen picker needs no new dependency
and no second connection.

## Decision

- **One shot per output, from the runtime.** `kanade_runtime::capture` asks the compositor's
  `wlr-screencopy` for one full-output frame per monitor, copies it into shared memory and hands
  back RGBA with the top row first. The shell's thread asks, waits off the draw thread, and reads
  the pixels. There is no damage tracking, pacing, pause or dmabuf path: a frame is one copy.
  `capable()` says whether the compositor advertised the manager, and an output with no usable
  shared-memory format fails with a short reason.
- **The picker is Kanade's own, drawn over the frozen frames.** The recording overlay becomes the
  region picker (`src/sources/capture/region.rs`), open for either a recording or a screenshot.
  For a screenshot it holds one frame per monitor; with no drag it draws the frame under the scrim,
  and with a drag the scrim covers everything outside the cutout, so the cutout shows the frozen
  frame at full brightness. For a recording it keeps the live screen as before. One pick at a
  time; Escape, a right click and a lock cancel with a single `None`, and a close after an answer
  sends nothing more.
- **Only the crop is written.** The drag maps from the monitor's logical size to its frame's
  pixels per axis, rounded, clamped to the frame and at least 1 px. The shell cuts that rectangle
  out of the frame, encodes an 8-bit RGBA PNG and writes it under `~/Pictures/Screenshots`, named
  as niri names its own shots, making the name new so a file taken meanwhile is never replaced.
  Then the annotation editor hears of the path, as niri's own announcement would tell it.
- **Capability decides, not the compositor.** The frozen picker runs under niri and Umbriel when
  `kanade_runtime::capture::capable()`; niri's own picker and Umbriel's `slurp` and `grim` remain
  the fallback when capture cannot run or the focused output's frame does not arrive in time. A
  non-focused output without a frame still gets an overlay, with no frozen base, and a drag there
  is ignored. Window and output screenshots are unchanged.
- **No copy under niri, Umbriel's copy kept.** niri's own screenshot path never copied the file,
  so the frozen path does not either; Umbriel's area screenshots have always gone to the clipboard
  through `wl-copy`, so the frozen path keeps that under Umbriel.
- **Not run live.** The protocol round trip, the frozen look and the drag on a real session are
  E2E and were not run here; the pixel mapping and the picker's routing are unit-tested.

## Why not `grim`, and why not the compositors' pickers

`grim` is a program: it captures the screen after the user picks, so the file is not the frozen
frame the picker drew, and it would need a geometry handoff and another screenshot of the screen.
niri's picker shows its own overlay and saves on its own terms, and Umbriel's `slurp` compositor
is another program between the user and the file. The runtime already has the protocol and the
shell already has a region picker; using them is one path under both compositors.

## Consequences

- The overlay's drag coordinates are the monitor's virtual space (the compositor's logical pixels
  over the UI scale), while the frame is the output's pixels. A fractional scale therefore maps by
  the frame's ratio, not by an exact scale factor; the crop is rounded and clamped.
- The frozen capture is a one-shot cast of each output. It is not a screen cast the privacy cluster
  follows, and the dots do not show it; a cast that starts mid-capture is not seen by it either.
- Frames are held one per output only while the picker is open, and dropped on close, cancel, a
  lock and every exit of the screenshot, so no pixels outlive it.
- Only the two byte orders the protocol normally offers are read: ARGB8888 and XRGB8888 as BGRA,
  ABGR8888 and XBGR8888 as RGBA, with the copy's alpha forced opaque. Any other shared-memory
  format fails that output's capture with a short reason rather than guessing an order.
- Multi-monitor pickers are untested on a live two-output session, and a monitor hot-plugged while
  the picker is open has no frozen base.
- The 500 ms per-output wait is a bound, not a promise: a compositor that is slow to answer loses
  that monitor's frozen base, and a slow focused output falls back to the old picker.
