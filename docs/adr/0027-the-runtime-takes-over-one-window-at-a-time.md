# 27. The runtime takes over one window at a time

Status: accepted; steps 2 to 4 superseded by ADR 0028 (the runtime is ported from Amane). Amends ADR 0026, whose native shell was built beside the Amane one and switched to in a single cutover, and whose performance gate waits for 1.0.

## Context

ADR 0026 builds a whole native shell beside the working Amane one and switches to it once every window reaches parity. Until then no native code runs in the daily shell, the two shells drift as the Amane one keeps changing, and everything is switched at once.

Most of what `src/` takes from Amane is not drawing. 75 of its 110 files import Amane; 44 of those imports are `Service`, 12 `Bus`, and the shell also reads Amane's own Notifications, Audio, Battery, Brightness, Apps, Keyboard and Monitor. The views use a small set of widgets: text, row, column, stack, image, svg, button, scroll, canvas, shader and text input. `crates/lock` already draws the lock screen without Amane (ADR 0025); only the hidden window that builds its scene is an Amane view.

Suzuha, a full shell on Amane, draws its bar's panels as one liquid shape on one overlay surface per output. A signed-distance shader joins them with a smooth union, so a panel flows out of another with no window between. The surface covers only what the open panels can reach, since a full screen of shader is more than its GPU draws in a refresh, and it stays mapped at 1 px with every panel away, because mapping a window costs a compositor round trip.

## Decision

- **Amane leaves in steps, each merged to `main` with a working shell.** No long-lived branch; every step passes the checks and a real-session E2E before the next.
  1. **State and adapters.** Replace `Service`, `Bus` and Amane's own Notifications, Audio, Battery, Brightness, Apps and Palette with `crates/runtime`'s Service store and Kanade adapters (ADR 0011), inside the Amane shell. Amane views re-run on one bridge Service, bumped when Kanade state they read changes; the bridge goes with Amane. IPC, Keyboard and Monitor stay: they belong to the event loop, not to state (see step 3.4).
  2. **Renderer.** In `crates/runtime`: wgpu draw list, text (shaping, fallback, Inter, Khmer, emoji, caret, selection), SVG, image decode and cache, and Kanade's own versions of the widgets the views use. Proven by `crates/runtime` examples against the Amane shell's screenshots, not in the shell.
  3. **Windows**, one group per step, each drawn and fed by the runtime on its own connection while Amane still draws the rest:
     1. Settings: an xdg window with no hand-off to others, using nearly every widget, text input and scroll.
     2. The lock screen: its scene comes from Kanade state instead of a hidden Amane window, and `crates/lock` joins the runtime's connection.
     3. The Dock.
     4. The Island with its Satellites, Surfaces, Banners, OSD and Privacy cluster, together and last, as they hand off to each other. The Island, its Satellites, Surfaces, Banners and OSD become one shape on one layer surface per output, as Suzuha's overlay: covering only their reach, staying mapped when at rest, moving between Top and Overlay as the Island's window does now (ADR 0021). The Privacy cluster keeps a surface of its own on the Overlay layer, so it shows over fullscreen windows whatever the Island's surface does. IPC moves to the runtime's socket with this group, as does Amane's Keyboard and Monitor: a verb is answered on the thread that owns the windows it opens, since Settings' text inputs and the lock screen live there and the children some verbs start (the recorder, `systemd-inhibit`) die with the thread that started them. Amane gives no other code a way onto its loop, so before this group IPC stays Amane's.
  4. **Amane removed** from `Cargo.toml`, `Cargo.lock` and `src/`, with `crates/glass`, `crates/repeat` and `crates/toplevels` once the runtime does their work, and the `src/boundary.rs` rules that only kept Amane's seam.
- **The glue between Amane and the runtime stays thin and listed** in `docs/runtime-migration.md`, each item with the step that deletes it. Nothing of it outlives step 4: ADR 0026's no permanent dual runtime holds.
- **ADR 0026's gates apply to each group as it moves** (geometry, input, focus, failure paths), and all of them again before step 4. Its fifth gate, performance against the Amane baseline, waits for 1.0, as does all optimizing (`AGENTS.md`): until then nothing is measured and low-level issues go to `TODO.md`.
- **No new features during steps 2 and 3**, only fixes, so nothing is built twice.

## Alternatives

**ADR 0026's single cutover.** Native code is used only at the end, the shells drift until then, and every window's failures arrive at once.

**Kanade widgets drawn through Amane first.** Moves the views early, but the layer under them is thrown away with Amane.

**The Island group first.** It is the most used and the most tangled; moving it before the renderer has run Settings and the Dock puts the riskiest window on the least-tested code.

## Consequences

- Between steps 3.1 and 4 the shell holds two Wayland connections, Amane's and the runtime's. Focus and stacking between their windows are niri's, not Kanade's; the Island group moves together so no hand-off crosses them.
- The bridge Service of step 1 wakes every Amane view reading it, more wakeups than now, left for 1.0.
- Text input, the hardest of the widgets, is proven early, by Settings.
- One surface for the Island group changes how Banners and the OSD come out of the Island (ADR 0023): they are drawn joined, not placed as windows beside it. Banners then share the Island's layer, so on an output a fullscreen window covers they hide with it, as they do now on Top. Its shader cost is bounded by the reach; how much it is waits for 1.0.
