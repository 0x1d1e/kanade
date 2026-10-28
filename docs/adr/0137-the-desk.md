# 137. The Desk: read-only faces of Services on the desktop

Status: accepted. Reverses the non-goal "No desktop widgets" of `docs/archived/design.md` (§ Explicit non-goals), on purpose. Builds on [ADR 0006](0006-island-on-top-layer.md) (layers), [ADR 0004](0004-wake-sources-on-announcements.md) (idle cost) and [ADR 0028](0028-the-runtime-is-ported-from-amane.md) (the runtime owns windows). Numbers skip 0045.

## Context

The first design refused desktop widgets, to stay a shell around the Island and not a desktop environment. Kanade is a lab (AGENTS.md): the clock, the next event and the track playing belong on the desktop as much as on the Island, and a user with an empty desktop and a wallpaper they chose wants the time on it without a window open. The Island is the default core, not the only one; where the desktop shows these is not the core's business.

## Decision

- **The non-goal is reversed.** Kanade draws things on the desktop. It still is not a desktop environment: no icons, no file manager, no panels.
- **A Desk widget** is a read-only face of an existing Service, drawn as plain text on the desktop in a window of its own, one per monitor. It is not an Activity (no Arbiter, no Lifetime, nothing preempts it), not a Surface, not a Satellite and not a Pane. It reads Services and writes none. In code it is a *Desk* item, never "widget": `crates/runtime/src/widgets/` already means layout primitives.
- **A `desk` Module**, `requires: &[CORE]`, in `src/desk.rs`. It belongs to the shell whatever core is chosen, so it lives outside `island/` and the boundary rules of `src/boundary.rs` are unchanged. It opens one window per monitor (`app.window_per_monitor`), following monitors as they come and go; the runtime closes a gone monitor's window, and the Desk keeps no per-monitor state to forget.
- **Layer.** `Layer::Bottom`, `Zone::Ignore`, `Keyboard::None`, namespace `kanade-desk`, anchored by corner and margin (fixed size, `anchor_*` and `margin`); it reserves nothing. Bottom is under normal windows and above the wallpaper, so the Desk is the desktop a window sits on and a window hides it, as an icon would be.
- **Config.** `desk.clock` (bool, off by default) and `desk.clock_corner` (`top-left`, `top-right`, `bottom-left`, `bottom-right`; default `bottom-right`), both per output. Each later face takes its own `desk.<face>` switch, off by default.
- **The first face is the clock:** the time (`clock::now`, in `clock` 12 or 24 hours) large, and the date line (`clock::today`) under it, white plain text on the wallpaper. No tinted plate: the wallpaper is the Desk's ground, and a plate would make it a window. No new niri layer rule.
- **Private content is off by default.** A face that shows what is private (a calendar event's title, a track's title and artist) has its own switch, off, apart from the face's own. The clock shows none.
- **Idle cost.** With `desk` on and `desk.clock` off the window is `.visible(false)`: it attaches no buffer and reads no Service (CONTEXT.md: a Module that is off starts no thread, opens no window and reads no Service). With the clock on, the only frame is the clock's one a minute, armed by `clock::now` and nothing else (ADR 0004). With the Module off, nothing exists.
- **No lock-screen presence.** The Desk is not drawn on the lock screen (`crates/lock`, ADR 0025): the lock holds the session, and its own surface covers the outputs. The Desk is a layer under windows; the session lock covers it.
- **Hidden while niri's overview is open.** The window is `.visible(false)` then, built before any face, so a hidden Desk reads no clock and takes no tick. It follows the overview through a small `Overview` Service written from `Niri` only when the flag changes, so the Desk does not redraw at each of niri's events; it reads `Niri`, not `IslandService`, as it belongs to no core. The `desk` Module needs niri (`Provider::Niri`): without it the overview cannot be known and the Desk would be shown in it. Nothing is placed in the overview's backdrop.

## Why not another place

- **`Layer::Background`**: niri draws the wallpaper tool's surface (awww) there, and a Desk window on it would sit beside the wallpaper with no defined order against it. Bottom is above Background, so the wallpaper is always under the Desk.
- **A normal window**: niri would tile it, give it focus and a place in the Dock and the overview, and the user would have to float and pin it with window rules. A Desk is not a window the user manages.
- **A Surface**: a Surface is the Island's, opened by a user action and drawn on the Island's body (CONTEXT.md); the Desk is on while the user does nothing and stays put when the Island is another core.

## Interaction, a later slice

This slice is non-interactive: `.click_through()` gives the window an empty input region. It must not block interaction, which is intended (the user wants interactive widgets eventually). The constraints for it:

- **Bottom layer kept.** An interactive face takes the pointer only inside an input region of its drawn controls (`LayerWindow::input_region`, built from `view::input_area` as `dock.rs` and `view/mod.rs` do). The text around the controls stays click-through. With niri's overview open, or the Module off, the region is empty and the window hidden, as `view/mod.rs` empties the Island's region under the overview.
- **`Keyboard::None`, always.** No Desk control is focused by a key on the layer itself. `Keyboard::OnDemand` on a Bottom layer would drop the user's toplevel focus (`island/service.rs`, the focus handling), and niri's behaviour there is untested.
- **An effect is a Kanade verb** behind its Module gate, reached by key from the Launcher's commands ([ADR 0112](0112-launcher-runs-a-closed-set-of-kanade-verbs.md)), a niri keybind, and the matching Surface whose ring holds the same targets. This is a documented exception to CONTEXT.md's "every target in a Surface is a key away": that covers Surfaces, and Desk items are not Surfaces. A keyboard story for the Desk itself (a ring on the layer) is deferred to a follow-up ADR with its own E2E.
- **First interactive face (not this slice):** Media transport, previous / play-pause / next, through the existing `kanade media` verbs. Title and artist are private content and off by default; the buttons and the play state are not. `kanade ui` names them `desk/media/previous|play-pause|next` under the window `desk`; a player that cannot skip shows that button disabled.
- A later face with a text field or a ring needs its own ADR and an E2E of niri's focus on a Bottom layer.

## Consequences

- A desktop with the clock on shows it under every window, on every output, with no focus taken and no click taken.
- Text on a wallpaper is white and has no plate, so its legibility depends on the wallpaper. A light wallpaper may make it hard to read; the corner is the user's remedy. Whether a shadow is wanted is decided by looking at it running.
- The runtime needed nothing new: corner anchoring with margin and a fixed size, `Zone::Ignore`, `Layer::Bottom` and an empty input region all exist. Not verified live: that niri places a Bottom window with `Zone::Ignore` by corner and margin exactly so.
- Settings shows the two keys in the Shell page's Desk section; the wiki's Configuration page lists them.
