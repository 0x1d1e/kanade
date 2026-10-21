# 131. The Launcher opens in the Island, or in a window of its own

Status: accepted. Builds on ADR 0028 (Kanade's own runtime), ADR 0030 (the largest body), ADR 0126–0130 (the Launcher's ranking, frecency, sections, actions and web fallback).

## Context

The Launcher has always been one of the Island's Surfaces: `Input::Open(Launcher)` expands the Island and the Launcher fills its body with the largest size `island.width` and `island.height` allow (ADR 0030). That ties it to the Island's position, its morph, its autohide and its merging with the Dock, and it caps how much of the screen a long list may use.

Step 6 of `docs/launcher-work.md`, the last planned step, asks for a second place to open it: an exclusive layer window centred on the focused output, reusing the existing providers and `Search::step`, closing on Escape or on the keyboard leaving it, never holding the keyboard while closed, and mutually exclusive with the Island Surface.

## Decision

- **`launcher.style`, a `Kind::Choice`** in the existing `LAUNCHER` block (`src/config.rs`), `island` by default, `restart: false` so the shell follows it at once. The choice is `LauncherStyle { Island, Detached }` with `NAMES`/`name()`/`named()`.
- **`island/` still reads no config.** The style is a field of `Presentations` (`src/island/presentation.rs`), set by `IslandService::set_launcher_style` from a watcher outside the module (`follow_style` in `src/surfaces/launcher/mod.rs`, on `Reloads`), because `src/boundary.rs` keeps `config::get()` out of `island/`. It is one value for every output, so it sits beside `withheld` and `tray`.
- **The detached Launcher is its own visit, not an island's Surface.** With `Detached`, `Input::Open(Surface::Launcher)` adds a visit and records the output in `Presentations`, leaving every island at Rest and no `Raised::Expanded` anywhere: `expand` never runs, so the Island cannot expand to the Launcher and the two are mutually exclusive by construction rather than by two paths agreeing. Escape, a Preempt and an IPC close or toggle end that visit; a press that ends a visit closes it through `IslandService::finish`.
- **`src/surfaces/launcher/detached.rs`, on the Dock's context menu's model** (`src/dock/context_menu.rs`). One Overlay window per output, `Zone::Ignore`, anchored `Vertical::Top` and `Horizontal::Middle` with a top margin that centres the body, `Keyboard::Exclusive` only while it is open, and an invisible 1×1 `click_through` window when it is not — so it is never focused while closed. `hold` is the pure function of the open state that decides the keyboard, so no key arrives at a closed window.
- **Focus loss, in the runtime.** A new `LostHandler` beside `KeyHandler` (`crates/runtime/src/input/target.rs`), `on_lost` on `LayerWindow` and `Window`, carried through `OpenWindow` and `into_parts` as a `Parts` struct, and dispatched from the keyboard's own `leave` (`crates/runtime/src/wayland/keyboard.rs`), which already names the surface and so finds its window. Additive: a window that never asks for it behaves exactly as before. The Launcher's detached window closes on it.
- **One key path, not two.** `key`'s post-guard body is `typed(monitor, visit, rec, key)`: the search's `step`, its sections, its actions and the press of an answer. The Island host guards on `Presentation::Expanded(Launcher)` and calls it; the detached host guards on its own open visit, closes on Escape with no actions open, and otherwise calls the same function. Neither host owns a provider, a `Search::step` or a list.
- **No migration.** A change of `launcher.style` while the Launcher is open closes it where it opened (`follow_style`), rather than carrying an open visit from one host to the other. The place it opens is not the place it stands.
- **The body is the Island's Launcher surface**, `super::surface`, at the size of the largest body `island.width` and `island.height` allow, so both hosts draw one list and a change to the Launcher reaches both. `view::largest` follows the detached Launcher's output as it follows an open Surface's, so the body is sized for the output it stands on.

## Alternatives

**A second Surface, `Surface::LauncherDetached`.** It would add a variant to every `match` on `Surface` — the forms, the sizes, the withheld Surfaces, the frame — for something that is not an Island body at all, and the Island would still have to refuse to expand to it. Recording the visit in `Presentations` instead keeps the Island's own Surface list unchanged.

**Duplicating the Launcher's keys in the detached window.** Two copies of the `Search::step` call, the panel handling and the presses would drift: the first change to an intent would have to be made twice. Extracting `typed` is the seam the second host needs; the file holds no list, no provider and no search of its own.

**Closing the detached window by polling the Island's state.** The window's `on_lost` is the compositor saying the keyboard left, which is exactly the event; a poll would sample it late and would need a wake with nothing to draw.

## Consequences

The Launcher has two hosts and one search. `launcher.style = "detached"` gives it the whole screen height on the output it opens on and lets it stand over a fullscreen window, at the cost of a window of its own to keep invisible, unfocused and closeable: a window that is shown but not focused, or hidden but still exclusive, would take the keyboard from whatever the user is typing into, so `hold` is a function of the open state alone and the closed window is `click_through`.

Detached has no morph, no hover strip and no merge with the Dock — it appears and disappears. Where the Island's Launcher fades in as part of the Island's motion, the detached one is a plain window with a shadow.
