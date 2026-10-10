# 2. Defer fullscreen suppression until niri reports fullscreen state

Status: superseded by ADR 0006. Was accepted (Phase 1, #13), superseding the heuristic in plan §5.3.

## Context

Rule 6 (plan §5.1) hides the island over a fullscreen window on the focused output, except Critical and privacy Activities. The island is on `Layer::Overlay`, above fullscreen, so only Kanade can do this.

niri 26.04 IPC has no fullscreen flag. The plan proposed a heuristic: the active window's `window_size` equals the output's logical size. It shipped only if a check found no false positives in normal use.

Measured in #13 on niri 26.04 (`window_size` vs a 1920x1080 output, and vs 1433x1022 in a nested niri without a bar):

| Window state | With a bar reserving 50 px | Without a bar |
|---|---|---|
| Fullscreen (`fullscreen-window`, `mpv --fs`) | 1920x1080, match | match |
| `maximize-window-to-edges` | 1920x1030, no match | match |
| App maximize (titlebar double-click, `mpv --window-maximized`) | as above | match |
| Windowed fullscreen | tile size, no match | no match |
| Floating window resized to the output | match | - |

niri maps an app's own maximize request to maximize-to-edges. Without a bar reserving space, every maximized window looks fullscreen. That is a false positive in normal use, so the heuristic fails its own gate.

## Decision

Rule 6 does not ship in v0.1, and no geometry heuristic ships. Kanade waits for niri to report fullscreen state over IPC: niri PR [#2836](https://github.com/niri-wm/niri/pull/2836) adds `fullscreen_state` to the window JSON and a `WindowFullscreenStateChanged` event. Once that lands, rule 6 (#35) suppresses for real fullscreen only, not for maximized or windowed fullscreen.

## Alternatives

**Size heuristic alone.** It is correct only on setups where a bar reserves space. Without a bar, it hides the island whenever a window is maximized.

**wlr-foreign-toplevel state combined with size.** On niri, the toplevel state reports `maximized` for maximize-to-edges and `fullscreen` for real fullscreen. It also reports `fullscreen` for windowed fullscreen, so it needs the size check as well. The combination was correct in every measured case. But Amane does not expose the protocol and `src/` is std plus Amane only, so this means a hand-written Wayland wire client and a second compositor connection. That is too much to build and maintain for one rule that niri is adding to the IPC Kanade already reads.

**Ask Amane to expose foreign-toplevel.** It inherits the windowed fullscreen ambiguity, and Kanade is niri-first, so niri IPC is the natural source.

## Consequences

- In v0.1 the island stays visible over fullscreen video and games. The wiki says so.
- #35 waits on niri #2836 and a niri release that includes it. The source then joins `src/sources/niri.rs`, which already follows the EventStream.
- Rule 6's unit and E2E checks (plan §9) stay conditional until then.
