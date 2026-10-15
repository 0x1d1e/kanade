# 89. The lock screen shows the keyboard layout in use

Status: accepted. Extends the lock screen (ADR 0025, design.md Lock).

## Context

A password is typed on the layout in use, and the lock screen shows none, so a user who switched layouts types a wrong password with no hint why. The OSD shows a switch for a moment, but not on the lock screen and not the layout in use before any switch. macOS shows the input source on the lock screen.

## Decision

- niri's event stream lists the layouts on connecting and says each switch. `niri::on_keyboard` hears the layout in use whenever it changes (the list or a switch), as the name XKB gives it, and none with fewer than two layouts. `on_layout` (the OSD's) is unchanged: it hears switches only.
- The lock Module hands it to `kanade-lock` as `Scene::layout`, the same short label the OSD uses (`xkb::short`), empty when unknown. `kanade-lock` still depends on no Kanade crate: the shell pushes a string, the lock draws it.
- It is drawn at the field's end, before Caps Lock's arrow when that shows, and the dots give up the room. The `Keyboard` Service holds the label; the scene reads it, so a switch while locked redraws the field.
- Safety: the label is only a hint. The hook runs on the niri thread and writes one small Service; nothing it does is read by the lock, the PAM check or the unlock. No label (no niri layout events, one layout, no fonts) draws nothing.
- Only the layout: no notification, media, battery or other state is on the lock screen. The lock Module off registers nothing.

## Consequences

- A name that XKB's rules list does not hold shows its first two letters, as in the OSD.
- After niri restarts, the stream is lost, and the last label stays until the new stream lists the layouts.

Files: `src/sources/niri.rs`, `src/lock/keyboard.rs`, `src/lock/scene.rs`, `crates/lock/src/lib.rs`, `crates/lock/src/paint.rs`.
