# 28. The runtime is ported from Amane

Status: accepted. Supersedes steps 2 to 4 of ADR 0027; its step 1 (Kanade's Service store and adapters) stands.

## Context

After ADR 0027's step 1 Kanade read none of Amane's state. What it still took was Amane's UI: about 30 names (the widgets, `LayerWindow`, `Window`, `Monitor`, `Keyboard`, `Key`, `Color`, `App`, IPC) over some 12k lines of Amane: an SCTK event loop, a vello and quad renderer, text, images, layout and input. ADR 0027 had Kanade write a new renderer and widget set, then move each window group onto a second Wayland connection. That is several times the work, keeps two connections until the end, and every rewritten widget can draw differently from the one it replaces.

## Decision

- **Amane's UI is ported into `crates/runtime`**, with the same API, so `src/` only changes `amane::` to `kanade_runtime::`. Amane leaves `Cargo.toml` in the same change. The code stays under Amane's MIT notice (`THIRD_PARTY_NOTICES.md`).
- **The port is Kanade's code from then on, not a fork.** Nothing tracks Amane upstream, and nothing is sent back to it. It changes like any other Kanade code.
- **Dropped in the port:** Amane's Services, `Bus`, niri, Hyprland and Sway backends, and its session lock (Kanade locks through `crates/lock`, ADR 0025). Kanade's Service store calls the runtime's change tracking directly, so the bridge Service of ADR 0027 is gone.
- **Renamed:** the IPC socket is `$XDG_RUNTIME_DIR/kanade.sock`, `AMANE_FRAMES` is `KANADE_FRAMES`, and the default layer namespace and app id are `kanade`.
- **The crates with their own connections stay for now** (`crates/glass`, `crates/lock`; `crates/toplevels` and `crates/repeat` were folded in by ADR 0034). Folding them into the runtime's connection, and drawing the Island group as one shape on one surface (ADR 0027 step 3.4), are runtime work now, done when wanted, with no Amane step gating them.

## Alternatives

**ADR 0027's new renderer, one window group at a time.** Least inherited code, but the slowest, with two connections and two renderers side by side until the end.

**Keep Amane as a pinned dependency.** Every gap would still be worked around from outside, as the four crates above are.

## Consequences

- Behavior is the same as on Amane: the same code draws, lays out and takes input. Proven by the checks and a real-session E2E (the Island and Controls with liquid glass, banners, Settings, the Launcher's text input, pointer clicks, IPC).
- Kanade now maintains a renderer it did not write. Its gaps (shaders take no textures, no key release, no key repeat) can be fixed in the runtime instead of in a crate beside it.
- An installed Kanade from before this change listens on `amane.sock`, so the new `kanade` CLI cannot reach it. Restart the shell after updating: until then the CLI says no shell is running, and a second shell could start beside the old one.
- The Settings window's app id is `kanade`, not `amane`: a niri window rule matching `amane` needs updating.
