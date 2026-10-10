# 26. Kanade owns its runtime

Status: accepted. Amended by ADR 0027, whose own steps 2 to 4 are superseded by ADR 0028: Amane's UI is ported into `crates/runtime`.

## Context

The shell currently uses Amane for the Wayland event loop, surfaces, rendering,
widget tree, text, input, focus, image lifetime, Services, and IPC. Kanade owns
its Activity/Arbiter domain and most OS adapters. Draft PR #208 exposes three
missing runtime capabilities: Amane does not pass a source texture into a
custom shader; it discards key releases and compositor repeat information; and
its fullscreen/hidden-window behavior requires external observation and careful
layer manipulation. Liquid glass then needs capture -> CPU refraction -> PNG ->
disk -> decode -> texture on every changed backdrop.

Treating these as isolated adapters would make the runtime boundary increasingly
expensive and leave the shell with multiple independent Wayland connections.

## Decision

Replace Amane completely with Kanade-owned, purpose-built infrastructure.
Do **not** fork, vendor, patch, or submit an issue/PR to Amane or niri. Do not
reproduce an entire general-purpose UI framework: implement the primitives
Kanade itself uses and maintain compatibility at its domain/UI contracts.

- `kanade-runtime` is the only normal shell owner of the Wayland connection,
  layer/xdg/lock surfaces, output/seat lifecycle, input, reactive invalidation
  and frame scheduling. Use smithay-client-toolkit, wayland-client and calloop,
  not custom Wayland wire-format code.
- A renderer owned by Kanade uses wgpu/WGSL and a vector/text stack chosen by
  benchmarks. It accepts captured textures directly. Static decoration,
  geometry and raster results are cached where that improves frame timing.
- Keyboard input comes **only** from the focused wl_keyboard and its keymap:
  press, release, modifiers, repeat_info, input method/text-input where
  available. No input-group membership or evdev for ordinary text editing.
- Wayland protocols determine capabilities; no global capture permission is
  assumed. Clear glass still requires compositor-supported background access
  or screencopy, even with our own renderer. DMA-BUF import is a researched
  optimization, not a prerequisite nor an invented guarantee.
- Each visible output/window maintains its own dirty state and refresh
  deadline. No work when nothing changes; unconfigured/hidden windows neither
  render nor retain a stream of pending redraws.
- Preserve Kanade's Island policies, module boundaries, UI behavior and CLI
  semantics. No scope expansion or feature redesign during migration.
- Build the replacement **beside** the operational Amane shell and switch in
  one gated cutover after parity. Do not ship an indefinite dual-backend layer.

## Migration gates

See `docs/runtime-migration.md` for the complete contracts and parity matrix.
The replacement cannot be the default until the following are verified on
niri 26.04 at 60 and 144 Hz and at fractional scale:

1. Surface geometry, input masks, hover, click, scroll, text input, clipboard,
   DnD, output hotplug and no focus theft.
2. All nine Surfaces, Dock, Banners, privacy, OSD, settings, and session lock,
   including failure and recovery paths.
3. Native shader/sample-texture effects, absence of the per-frame PNG pipeline,
   and correct fallback when capture is unavailable.
4. No leaked input/device privileges, broken session unlocks, or regression in
   notification delivery / service lifecycles.
5. Benchmarked idle wakeups/frames, morph frame pacing, CPU/PSS/GPU buffers,
   and no regression against the recorded Amane baseline.

## Consequences

An in-tree runtime is significantly more code and maintenance than Amane.
Rendering and Wayland failures become Kanade's responsibility. The gain is one
coherent event loop and GPU pipeline, direct control over surface/input
semantics, and removal of the adapters whose sole purpose was working around
Amane's private internals. Keep failures isolated and report them through
`kanade doctor`. Source-backed tests and real-session E2E are mandatory.

ADR 0011 continues to govern OS adapters; it no longer forbids the new runtime
owning Wayland after cutover. ADRs 0020-0022 in PR #208 are provisional
implementation decisions, not requirements to preserve their workarounds.
