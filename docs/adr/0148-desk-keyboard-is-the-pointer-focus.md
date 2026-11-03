# 148. The Desk's keyboard is the pointer's focus

Status: accepted. Builds on [ADR 0137](0137-the-desk.md) (the Desk), [ADR 0140](0140-desk-notes-edit-in-place.md) (the notes card) and [ADR 0143](0143-desk-faces-move-by-their-handle.md) (the handle). This is the follow-up ADR 0137 deferred. A keybind does not focus the Desk.

## Context

ADR 0137 deferred a keyboard story for the Desk itself: a ring on the layer, reached by a keybind, with its own E2E. That reach does not exist. wlr-layer-shell focuses `Keyboard::OnDemand` only on a pointer click (`crates/runtime/src/layer_window.rs`, the keyboard-mode comment; set in `crates/runtime/src/wayland/layer.rs`). A keybind cannot focus a Bottom `OnDemand` layer surface. Neither niri nor Umbriel has a layer-focus IPC; both focus toplevels only. `Keyboard::Exclusive` is the only mode that takes keys with no click, and ADR 0140 and ADR 0143 forbid it on the Desk. The ring reached by a keybind is not the first slice, and it is not implementable on the current compositor contract.

## Decision

- **The Desk stays `Layer::Bottom`, `Zone::Ignore`, and `Keyboard::None`**, except while the pointer is on a drawn control, a note is being typed, or a face is held by its handle. Then it is `Keyboard::OnDemand`, and never `Exclusive`.
- **The ring visits only the media buttons that can act** (`media/previous`, `media/play-pause`, `media/next`), on that same `OnDemand` focus the pointer already obtained. It does not take a key while a note is being typed. Escape during a drag still restores the place ([ADR 0143](0143-desk-faces-move-by-their-handle.md)) and is not the ring's.
- **A keybind or a Launcher verb does not focus the Desk.** Leaving the ring, or the keyboard leaving the window, returns the window to `Keyboard::None`. The compositor gives the window below its focus back. Kanade does not call `focus_window` for it. `focus_toplevel` in `island/service.rs` is niri-only and must not be copied here.
- **Read-only faces are not ring targets:** the clock, the weather, the agenda and the track title. Drag handles are holds, not ring targets; ADR 0143 keeps them out of `kanade ui`. Private titles stay behind `desk.agenda_titles` and `desk.media_titles`. Click-through outside drawn controls is unchanged.
- **No new idle frame.** The ring draws on the key that already wakes the window. The overview still hides the Desk on both compositors.
- **Not in this slice.** Focusing the Desk from a Launcher verb or a niri or Umbriel bind. A ring over notes, handles, or read-only faces. `Keyboard::Exclusive`.

## Implementation and acceptance

Implemented in `4532a93` (`src/desk/ring.rs`). With the pointer already over the media row, `desk.media` on and a player that can act: the first Left/Right only draws the ring on a button and presses nothing; later Left/Right moves it, skipping disabled buttons; Enter or Space runs that button's existing `media::request`; Escape clears the ring and drops the keyboard ask so the next character can type into the window below. A note being typed and a drag in progress take their keys first.

**NOT RUN (live):** niri and Umbriel Bottom-layer `OnDemand` focus, navigation and transport, disabled-button skipping, Escape returning typing to the window below, notes/drag precedence, click-through and no extra idle frames. These need E2E on the user's session, not a unit test of geometry.

## Consequences

It is still unverified live that a click on a Bottom input region focuses `OnDemand` at all ([ADR 0140](0140-desk-notes-edit-in-place.md)). If it does not, there is no keyboard slice. An output-sized window holding `OnDemand` must not widen the time it steals keys.
