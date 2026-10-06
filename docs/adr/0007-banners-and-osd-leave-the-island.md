# 7. Banners and OSD leave the Island

Status: accepted (design, roadmap 8). Supersedes the "one physical surface, no detached popovers" goal of `docs/archived/plan.md` §2 for notification toasts and level changes.

## Context

The Island shows notification toasts and volume, brightness and mic changes as Transients over the primary. Each one hides the durable Activity for its lifetime, one toast at a time, and needs rules to keep it off an open Surface (`Surface::shows`, queued badges).

## Decision

- The `banners` module shows notification toasts as Banners: focused output, top-right, 4 s low, 6 s normal, Critical sticky, timer paused while hovered, at most 3.
- The `osd` module shows volume, brightness and mic changes in its own Overlay window.
- The Island keeps durable Activities and workspace switches. Its toast and OSD paths are deleted.
- Workspace switches stay an Island Activity: they are navigation, a change of context, not level feedback. Policy: `Transient`, `Osd` Priority, `FocusedOutput`, `Interrupt::None`. It may win the primary while it lives, then the previous primary returns; no auto-expand, no OSD.

## Alternatives

**Keep them in the Island.** One surface, but toasts and level changes keep displacing the Activity the user is following, and several notifications cannot show at once.

## Consequences

- More layer windows per output, each with its own input region.
- `notifications` keeps bus, state and history; `banners` and `notification-surface` are UI over it.
- "OSD" and "Banner" become domain terms, and the Transient examples in `CONTEXT.md` change, with that code.
