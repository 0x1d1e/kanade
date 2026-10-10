# 34. Key repeat and the toplevel list are the runtime's

Status: accepted. Supersedes ADR 0022. Amends ADR 0021, whose `crates/toplevels` and keyboard mask shrink, ADR 0025, whose lock screen stays apart, and the `wayland-client` rule in `AGENTS.md`, which loses two crates.

## Context

The runtime dropped `repeat_key` and `release_key`, so a held Backspace needed `/dev/input` read beside niri (ADR 0022): a masked evdev reader, a second Wayland connection in `crates/repeat` for the repeat rate, and a timer thread in `src/repeat.rs` that had to guess which press the Island's last key was. Which windows are fullscreen lived in `crates/toplevels` over a third connection (ADR 0021), feeding a Service by a thread of its own.

The runtime now owns the Wayland event loop (ADR 0028) and calloop.

## Decision

- The runtime's keyboard is created with SCTK's `get_keyboard_with_repeat`. The compositor's repeat info, the delay and the rate, drives a calloop timer; SCTK cancels it when the key is let go, when focus leaves, and when another repeating key goes down. A repeated key takes the path of a pressed one (`WaylandState::deliver`): the focused surface, the focused text field, the window's `on_key`.
- Only Backspace and the arrows repeat, as on macOS, where a held letter offers accents. Letters, Enter and the rest do not.
- `src/repeat.rs`, `crates/repeat` and the repeat half of `src/sources/keys.rs` are deleted. `keys` reads only the OSD's level keys and lock lights, so its kernel mask loses Backspace and the arrows, and `clock.rs` its repeat constants.
- The runtime binds `zwlr_foreign_toplevel_manager_v1` on its own connection and keeps a `Toplevels` Service: every fullscreen window with its output and app id, from the compositor's `done`-batched events, republished when an output gets its name. When the compositor has no such protocol, `Toplevels::unavailable` says so and `kanade status` prints "fullscreen windows are not followed: ...". `crates/toplevels` and the `fullscreen::follow` thread are deleted. `src/sources/fullscreen.rs` keeps niri's half (which window an output shows, and its tile) and the `covers` rule.
- The lock screen stays isolated (ADR 0025). It keeps its own connection, keyboard and key repeat: its recovery and security outweigh one fewer connection.

## Consequences

- Settings windows' text fields repeat too, which they did not.
- A held key repeats at the compositor's rate in every runtime window, with no evdev guess and no 50 ms matching of presses.
- Two crates and one thread less; the `wayland-client` rule names `crates/glass` (ADR 0020) and `crates/lock` (ADR 0025) beside the runtime.
- Modifiers and key-up are still not given to views.
