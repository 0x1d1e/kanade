# 22. Key repeat on the Island

Status: superseded by ADR 0034, which moves key repeat into the runtime. Amends ADR 0021, whose keyboard mask now lets Backspace and the arrows through too, and the `wayland-client` rule in `AGENTS.md` for one more crate, `crates/repeat`.

Validated on niri 26.04 (repeat 200 ms, 50/s): in the Launcher, Backspace held for 0.35 s deletes 10 characters, held for 1.5 s clears the field, and nothing more is deleted once it is let go.

## Context

In Wayland a client repeats a held key itself: the compositor says only how soon and how often (`wl_keyboard.repeat_info`). The pinned Amane creates its keyboard without repeat and keeps both the repeat info and the focused text field to itself. So a held Backspace deletes one character in the Launcher, and holding an arrow moves the selection once. Amane and niri are not to be patched, forked or asked.

Amane does give Kanade each key as it goes down, but not as it is let go. Kanade alone cannot tell a held key from one tapped.

## Decision

- The keyboards' evdev devices, which `keys` already reads beside niri (ADR 0021), say when Backspace, Up, Down, Left or Right goes down and up. Their kernel mask now lets those five through too. The kernel's own repeat events are ignored.
- A workspace crate, `kanade-repeat` (`crates/repeat`), opens its own Wayland connection with no `unsafe`, and its own keyboard, which no surface ever focuses. It hears only the repeat info, at first and whenever niri's config changes it. Until that arrives, niri's defaults (600 ms, 25/s) stand.
- `repeat` (`src/repeat.rs`) gives the key to the Island's key handling again: after the delay, then at the rate. It does this while the key is held and the Island's last key from Amane was this press of it, no more than 50 ms before evdev saw it go down. It stops once the Island no longer asks for the keyboard, as when it collapses. A key held in another window never reaches the Island, and any other key the Island gets stops the repeat, as in any client. A modifier, or a key Amane has no name for, stops nothing, as no client repeats Shift or Ctrl.
- Keys reach the Island's handlers one at a time, whether from Amane or repeated, and a repeat decided before a key is typed or let go is dropped, so a repeat never undoes or follows a letter typed meanwhile. A dropped evdev buffer, which may have lost a key let go, stops the repeat.
- Letters and other keys are not read and do not repeat, as on macOS, where a held letter offers accents rather than repeating. Only the keys that edit and move repeat.

## Alternatives

**Repeat every key Amane gives, on a timer, until another arrives.** No input device read, but nothing says when the key was let go, so a tap would repeat.

**Read every key from evdev.** Letters would repeat, but Kanade would see everything typed in every window. Masking to five keys keeps that cadence to Backspace and the arrows.

**A second keyboard focus through Amane's Wayland connection.** Amane does not expose its connection or seat.

## Consequences

- Without the input group nothing repeats; a key still types once. `kanade doctor` says the group is missing.
- Kanade learns when Backspace and the arrows go down and up in any window or session, though not where. This is the same kind of reading as the level keys, and goes nowhere but the repeater. Held in any window, they wake the keys thread at the kernel's own repeat rate, whose events it throws away.
- Amane never says when the Island loses the keyboard focus, only whether it asks for it. Backspace held in an Island opened by a click, while the focus moves to another window, keeps repeating on the Island until it is let go or the Island collapses.
- A key is matched by its evdev code, Amane's by its keysym. A remap that makes another key Backspace, like `caps:backspace` or Colemak's Caps Lock, types once and never repeats.
- Delete has no name in Amane, so the Island cannot act on it, held or not.
- The Settings window's text fields are Amane's own, and their focus is private to it, so they still do not repeat. The lock screen's field repeats, as `kanade-lock` has its own keyboard (ADR 0025).
- One more Wayland connection, as the glass and toplevels have; `wayland-client` stays out of `src/` except `src/doctor/`.
