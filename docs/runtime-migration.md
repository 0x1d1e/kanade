# Native runtime migration

Decision: [ADR 0023](adr/0023-kanade-owned-runtime.md). Base: `main` at
`60d1415`. PR #208 is a behavioral reference, not a runtime foundation.
No Amane or niri fork/patch/issues/PRs.

## Rule

**Replace Amane fully before expanding the product.** Work on a separate
`native-runtime` branch, preserve the working shell during development, and
flip to the native runtime only after an explicit parity gate. There is no
permanent runtime abstraction switching between Amane and native.

`crates/runtime` now contains a compiling native layer-shell backend,
input/keymap handling, output discovery, Service invalidation, a Wayland SHM
visibility probe, and a wgpu liquid-glass pass. **These components do not yet
constitute a native Kanade shell**: there is no GPU Wayland presentation or
live capture bridge, no complete UI, and no native main executable.
The existing binary still uses Amane. Only a full native migration with
verified parity can remove the dependency.

## Dependencies and ownership

| Boundary | Implementation | Constraint |
|---|---|---|
| Wayland loop | SCTK + wayland-client + calloop | one shell-owned connection; event-driven |
| Outputs | wl_output, fractional-scale, viewporter | hotplug; scale 1, 1.25, 1.5, 2 |
| Windows | wlr-layer-shell, xdg-shell, ext-session-lock | dynamic dimensions, anchors, exclusive zone, input regions |
| Input | wl_seat/pointer/keyboard + xkbcommon | correct key release/repeat/remaps; no evdev |
| Paint | wgpu + WGSL; vector, SVG, glyph and emoji support | GPU texture input, per-window invalidation, caching |
| Backdrop | niri-supported capture/effect protocol | sample real windows, optional GPU import; no assumed compositor hook |
| Reactive state | Kanade-owned subscription/dirty tracking | typed, lazily activated by module |
| Messaging | existing OS adapters + IPC | no view I/O; contract-preserving |
| Files/images | async decode/cache of stable content | no PNG intermediary in animation loop |
| Security | per-output session-lock, privacy overlay | fail safe; test lock crashes |

Do not implement a separate abstraction layer for each dependency. Keep
runtime mechanisms together, domain rules in existing modules.

## Complete Amane retirement checklist

- [ ] One native event loop, connection, output/seat registry and hotplug
- [ ] Layer-shell surfaces, dynamic anchoring, placement, input masks, exclusive zones
- [ ] XDG settings window: move, resize, keyboard focus, close and scale
- [ ] Session-lock protocol and crash/restart/re-lock safety equivalent to #154/#199/#200
- [ ] GPU device/surfaces, present scheduling, resize, device-lost recovery
- [ ] Quad/polygon/path, transforms, clipping, opacity, rounded borders and shadow
- [ ] Custom WGSL shaders with sampled GPU textures and parameters
- [ ] Text shaping, fallback fonts, Inter, Khmer/non-Latin rendering, emoji, caret, selection
- [ ] SVG icons, media artwork, async image decode/cache and release
- [ ] Layout primitives: row, column, stack, flexible measure, virtualization, scroll
- [ ] Pointer: hover/leave, click/release, drag, scroll, cursors, hit testing while geometry changes
- [ ] Keyboard: xkb keymap, press/release, repeat, shortcuts, text input, IME considerations
- [ ] Clipboard, drag-and-drop, MIME negotiation, sensitive-content protection
- [ ] Lazy Services, per-window invalidation, timers, scheduled animation, IPC/CLI
- [ ] Island Rest/Compact/Split/Peek/Expanded + satellites, all Activities and controls
- [ ] All Surfaces, including Calendar scrolling, Launcher, Tray and Clipboard
- [ ] Dock, banners, OSD, privacy cluster, lock screen and Settings
- [ ] Notification, session, Wi-Fi/Bluetooth, media, audio and battery state behavior
- [ ] Feature module gating and config hot reload; doctor detects missing backends
- [ ] Fractional scaling, output hotplug, fullscreen and niri overview behavior
- [ ] Glass over live windows without capture->CPU->PNG->decode; graceful capture fallback
- [ ] E2E parity on niri; accessibility, reduced motion, keyboard, multi-monitor
- [ ] Repeatable resource/performance measurements vs baseline
- [ ] Remove Amane from Cargo.toml/Cargo.lock and all source imports
- [ ] Remove bypass-only crates and the Amane-specific boundary assumptions

## Sequence

1. **Platform and input**: actual Wayland runtime with output/window lifecycle,
   keyboard/pointer, frame callbacks and IPC. First proof: native Island resting
   with accurate shape/input region; no competing Amane surface.
2. **Renderer**: direct GPU textures, draw list, text, icons and image cache;
   reproduce one Island morph, then liquid-glass refraction using captured
   buffers on the GPU. Verify background access and latency independently.
3. **UI mechanics**: exact widget/layout/input/scroll/animation requirements
   already exercised by the nine Surfaces. Reuse domain structures, not
   Amane's private widget/layout types.
4. **Window types**: all shell surfaces, settings, session lock, privacy,
   banners and Dock, including fullscreen.
5. **Service migration**: replace Amane Service, Bus, IPC and source adapters
   selectively where Kanade still uses them; preserve module contracts.
6. **Cutover**: parity tests, live niri session E2E, measurements, then remove
   Amane, repeat-only and PNG-rim workarounds. No dual runtime in released code.

## Acceptance checks

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`
and `cargo test --workspace` must pass. Build on the user's niri session and
compare to documented performance baselines with `scripts/frames` and
`scripts/memory`; do not assert any live test passed until actually run.

At idle, no animation or backdrop change means zero native redraws.
Preserve correct pointer passage through unused Island canvas area and
keyboard focus on fullscreen and locked sessions. Neither input-group nor
rfkill-group membership should be required just for ordinary keyboard input.

**Remaining upstream protocol limit:** sampling what the compositor draws behind
a layer surface is not a general Wayland right. A native renderer removes the
PNG round trip, not the requirement for niri-supported capture/effects. Any
capture is explicitly optional and must fail closed to transparent material
without turning off independent privacy indicators.
