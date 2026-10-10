# Runtime

Decisions: [ADR 0026](adr/0026-kanade-owned-runtime.md), step 1 of
[ADR 0027](adr/0027-the-runtime-takes-over-one-window-at-a-time.md), and
[ADR 0028](adr/0028-the-runtime-is-ported-from-amane.md). No niri fork,
patch, issue or PR.

## Where it stands

`crates/runtime` is Kanade's runtime:

- `service.rs`, `worker.rs`: the Service store and the control-call thread (ADR 0027 step 1).
- Since ADR 0028: the SCTK event loop
  and windows (`wayland/`, `layer_window.rs`, `window.rs`), drawing
  (`graphics/`: vello plus a quad pipeline, text, images, SVG, shaders),
  widgets and layout (`widgets/`, `placement.rs`, `style/`), input
  (`input/`), animation, IPC (`ipc.rs`, `$XDG_RUNTIME_DIR/kanade.sock`) and
  frame timing (`KANADE_FRAMES=1`).

## What is left

Runtime work, done when wanted.

- [x] Key repeat and the fullscreen toplevel list are the runtime's (ADR 0034);
      `crates/repeat` and `crates/toplevels` are gone.
- [ ] Fold `crates/lock` (session lock in the runtime) into the runtime's
      connection. Drop its `src/boundary.rs` exception with it.
- [ ] The Island, its Satellites, Surfaces, Banners and OSD as one shape on one
      layer surface per output (ADR 0027 step 3.4); the Privacy cluster's dots
      are inside the Island's body (ADR 0033).
- [x] Liquid glass from GPU textures, without the capture to PNG to decode
      round trip (ADR 0037); `crates/glass` is gone.

## Glue to delete

None.

## Acceptance checks

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`
and `cargo test --workspace` must pass. Build and run on the user's niri
session; do not assert any live test passed until actually run. Performance
(`scripts/frames`, `scripts/memory`, idle redraws and wakeups) is not checked
until 1.0.

Preserve correct pointer passage through unused Island canvas area and
keyboard focus on fullscreen and locked sessions. Neither input-group nor
rfkill-group membership should be required just for ordinary keyboard input.

**Upstream protocol limit:** sampling what the compositor draws behind a
layer surface is not a general Wayland right. Any capture is optional and must
fail closed to transparent material without turning off independent privacy
indicators.
