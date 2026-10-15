# Runtime

Decisions: [ADR 0026](adr/0026-kanade-owned-runtime.md), step 1 of
[ADR 0027](adr/0027-the-runtime-takes-over-one-window-at-a-time.md), and
[ADR 0028](adr/0028-the-runtime-is-ported-from-amane.md). No niri fork,
patch, issue or PR.

## Where it stands

`crates/runtime` is Kanade's runtime:

- `service.rs`, `worker.rs`: the Service store and the control-call thread (ADR 0027 step 1).
- `queue.rs`, `unpoisoned.rs`: mailboxes a restarted thread reads again (#95), and the poisoned-lock take.
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
- [x] Text input: modifiers reach a focused text input (`Modifiers`, not a window's `on_key`),
      with Delete, PageUp and PageDown keys, word jumps, selection and paste from the Wayland
      selection (`text/plain;charset=utf-8`, 64 KiB, read on the event loop with a 2 s deadline).
      Delete repeats with Backspace and the arrows, which extends ADR 0034's list. SCTK's `utf8`
      already applies xkb compose (from `LC_ALL`, `LC_CTYPE` or `LANG`), so dead keys work.
      Not done: copy and cut (the runtime sets no selection), shortcuts on a non-Latin layout
      (Ctrl+V names the key by its keysym, so it needs a Latin letter on the layout).
- [x] Input methods: `zwp_text_input_v3` in `wayland/ime.rs` (ADR 0090). The focused text input is
      enabled with its surrounding text (bounded; none for a password) and caret rectangle, and a
      `done`'s preedit, commit and surrounding deletion are applied together, the preedit drawn
      inline. Not done: `input_method` (Kanade being an input method), a scrolling field's caret,
      and a check against a live fcitx5 or ibus.
- [x] Per-corner radius (`Rectangle::radius_top_left` and the other three, `Corners`), and the
      justify overflow spill. A drop shadow and the inner shadow of a rectangle with unequal
      corners still use the largest corner.
- [x] File drops, as a primitive: `Rectangle::on_drop` takes the paths of a `text/uri-list` let go
      over it. Only absolute `file://` links on this machine count, percent-decoded, no `..`,
      at most 256 files and a 64 KiB list, read on the event loop with the paste's 2 s deadline;
      a list with one bad link is refused whole. The drag is accepted only over a widget with
      `on_drop`. No consumer yet (the Dock's drop behavior is not built).
- [ ] Fold `crates/lock` (session lock in the runtime) into the runtime's
      connection. Drop its `src/boundary.rs` exception with it.
- [ ] The Island, its Satellites, Surfaces, Banners and OSD as one shape on one
      layer surface per output (ADR 0027 step 3.4); the Privacy cluster's dots
      are inside the Island's body (ADR 0033).
- [ ] The clipboard history through data-control in the runtime, in place of `wl-paste --watch`
      and its setpriv helper (`src/sources/clipboard.rs`, ADR 0019). Read, not built: it is
      L-sized and cannot be checked without a live session, so it waits. Findings:
      - Possible: niri serves `ext_data_control_manager_v1` and `zwlr_data_control_manager_v1`
        (smithay `selection`). `wayland-protocols` 0.32 (`client`, `staging`) has the ext one and
        is already locked through `crates/lock`; `wayland-protocols-wlr` (already a runtime
        dependency) has the wlr one. The two have the same shape and different types, so each
        needs its own `Dispatch` impls (a macro).
      - A data-control device sends `data_offer`, then one `offer` per type, then `selection` in
        one batch. Sensitivity (`x-kde-passwordManagerHint` among the types) is therefore read
        off the offer itself, never racing a newer selection, and a sensitive offer is simply not
        received. That is stronger than ADR 0019's version gate, which the change would replace
        (a new ADR amending 0019; `wl-paste --version`, `SENSITIVE_SINCE`, `Provider::Paste`,
        the doctor check, `--clipboard-selection`, `hand_over`, `watch` and their tests go).
      - Reading: `receive(mime, fd)` is sent on the event loop and flushed, and the pipe is read
        on a worker thread with a deadline and `ENTRY_BYTES`, as the paste's 2 s read is.
        The runtime's paste and drop read on the event loop; selections up to 16 MiB must not.
      - Gating: the runtime must bind the manager and create a device per seat only when the
        `clipboard` Module is on (an `App` builder switch), or a disabled Module would still
        see every copy. The restore path (`wl-copy --foreground`, announced by the selection
        event) can stay as it is; setting a selection from the runtime is a further step.
      - Not checkable here: the behavior against niri's two managers, a source that never
        writes, a selection replaced while it is read, and password-manager copies. Do it with
        the ADR 0019 validation (`wl-copy --sensitive` between two plain copies) and a source
        that stalls.
- [x] Liquid glass from GPU textures, without the capture to PNG to decode
      round trip (ADR 0037); `crates/glass` is gone. ADR 0049 then removed the capture.

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
layer surface is not a general Wayland right. The shell reads nothing behind its
panes (ADR 0049).
