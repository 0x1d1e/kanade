# 21. The OSD returns to the Island

Status: accepted. Supersedes the OSD part of ADR 0007; Banners stay out of the Island. Amends ADR 0020 and the `wayland-client` rule in `AGENTS.md` for one more crate, `crates/toplevels` (since folded into the runtime, ADR 0034). Amended by ADR 0022, whose key repeat the keyboard mask let through; ADR 0034 then moved both key repeat and the fullscreen toplevel list into the runtime, leaving the mask the level keys and lights only.

Validated on niri 26.04: a brightness key over a fullscreen window shows the OSD within 100 ms, also at the limit, where nothing changes; Controls opened over it shows at once and moves no focus; at rest the Island stays hidden there, but shows over a windowed-fullscreen window and in the overview. Airplane mode is covered by unit tests only, as checking it end to end turns the session's radios off.

## Context

ADR 0007 moved volume, brightness and mic changes into an Overlay pill of their own, bottom centre of the focused output, so they would not displace the Activity being followed. A second floating thing on screen is not how the Dynamic Island shows a level: the Island itself grows into it. The pill also sat where the Dock is, so a large or magnified Dock had to stay clear of it.

## Decision

- An OSD is an Activity: Kind `Volume`, `Brightness` or `Mode` (Caps Lock, Num Lock, airplane mode), `Transient(timings.osd)`, `FocusedOutput`, `Interrupt::None`, Detail the level or mode. The Island shows it in its small form, as it shows a workspace switch.
- Its Priority is `Feedback`, above `Critical`: it answers a key just pressed, so it shows over anything for the moment it lives, alone, with no Satellites, then the primary and its Satellites return. The workspace switch's Priority, once named `Osd`, is now `Glance`. Below a timer or a call it would show nothing, and at the workspace switch's Priority it would wait out its dwell.
- One OSD at a time: one id per Kind, and an OSD posted withdraws the other Kinds' under the same Island write, so a held key extends one Transient.
- Over an open Surface or a Peek, which draw themselves and not the primary, the OSD floats as a capsule of its Compact form over the near edge, popping in and fading out as it expires. What is under it stays as it was: a pinned Peek stays pinned. A Peek of the OSD itself grows from it, as any Peek does.
- The keys: a keybind changes a level outside Kanade, so at a limit nothing changes for Kanade to hear, and a change is heard only once it lands. So `keys` reads the keyboards' evdev devices beside niri, ungrabbed, and shows the level as it is the moment a level key that steps it goes down or repeats; the change follows as it lands. The mute keys are not read: they toggle, never at a limit, and their press would show the state before the toggle, so their change shows as it lands. Each device is masked in the kernel (`EVIOCSMASK`) before it is read, to the level keys of the levels read and the Caps Lock and Num Lock lights, so no other key ever reaches Kanade: what a device queued before its mask is flushed unread, as its clock is set (`EVIOCSCLOCKID`), and one that cannot be masked is not read. niri lights a keyboard's lock lights as it toggles them, so the lights are the modes; every keyboard says each change, which shows once. Reading needs the input group.
- Brightness is read from sysfs when it shows, not from Amane's polled Service, which lagged a key by up to its poll.
- Airplane mode is radios of more than one kind, all blocked, as `/dev/rfkill` says each change, whoever made it. Only a radio's own change turns it: a radio coming or going, a dongle unplugged or a driver reloaded, changes the radios silently. A machine with one kind of radio has no airplane mode to show, as blocking it is that radio's own switch. Reading needs the rfkill group.
- Keyboards and radios are heard whichever session has the seat, so the OSD shows nothing while logind says another session is active, as after a switch to another VT. Kanade follows its session's `Active` over a zbus connection of its own, with a match rule naming the session, so nothing else on the system bus wakes it.
- A volume read for `kanade osd volume` gives way to an OSD shown since it was asked, as before.
- Over fullscreen windows, as the pill was. A fullscreen window covers the Island's Top layer, and niri sends a covered surface frame callbacks only about once a second, so an OSD drawn there and raised to Overlay showed up to a second late, often after half its time. niri's IPC says a window's size, not that it is fullscreen, and a window maximized to the edges is as big but leaves the Island visible. So a workspace crate, `kanade-toplevels` (`crates/toplevels`), opens its own Wayland connection, with no `unsafe`, and follows `wlr-foreign-toplevel-management` for the fullscreen windows, their outputs (wl_output v4 names) and app ids. An output is covered while niri's IPC says the window it shows, its active workspace's active window, is of a fullscreen app id and in a tile as big as the output; not in the overview, which draws the Top layer above every window. Keyboard focus counts for nothing, so the Island taking the keyboard over a fullscreen window does not give the focus back and forth, and a windowed-fullscreen window, fullscreen to its app but in a column, covers nothing. On a covered output the Island's window is on the Overlay layer and hidden, as Amane draws a hidden window at once, and shows from an OSD or an open Surface, as from a keybind, until its morph back has come to rest; it stays hidden otherwise (no rims linger, ADR 0037). Without the toplevel list or niri nothing counts as covered, and an OSD over a fullscreen window shows late.
- The `osd` Module, its `kanade osd` verb and `timings.osd` keep their names. The pill window and its Service are deleted.

## Alternatives

**Keep the pill.** Nothing displaced, but two surfaces for one shell, and in the Dock's way.

**Raise the Island to Overlay while a level shows.** No new connection, but the raise waits for the throttled frame callback.

**A full-output tile in niri's window layouts.** No new connection, but it cannot tell fullscreen from maximized to the edges, so the Island would hide over the latter.

**The Island always on Overlay.** Shows at once, but covers every fullscreen video and game at rest.

**The focused window's fullscreen state alone.** No niri windows to match, but a layer surface taking the keyboard takes the window's focus, and an empty workspace has none either: the Island opened over a fullscreen window would drop under it and hand the focus back, again and again.

**Levels only from their change.** No input device read, but a key at a limit shows nothing, and the lock keys and airplane mode have no change Kanade hears otherwise.

## Consequences

- An OSD hides the primary and the Satellites for its lifetime, Critical ones included, then they return (preemption never destroys).
- The Island shows over a fullscreen window for an OSD or an open Surface, and only then; at rest it stays out of the way as before.
- One more Wayland connection, as the glass has; `wayland-client` stays out of `src/` except `src/doctor/`.
- Kanade reads input devices and rfkill, through the kernel ABI kept in `src/clock.rs`. Without the input group no key at a limit and no lock light shows; without the rfkill group no airplane mode. `kanade doctor` says which.
- A fullscreen app with two windows, one fullscreen elsewhere and one maximized to the edges here, counts this output as covered: the match is by app id.
- The Dock no longer has to stay clear of a pill at the bottom edge.
