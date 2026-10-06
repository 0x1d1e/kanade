# 8. Kanade is a shell, not only an island

Status: accepted (design). Supersedes the scope and non-goals of `docs/archived/plan.md` §1-2.

## Context

v0.1 is one Island per output, explicitly not a bar, dock, wallpaper, lock screen or settings app. Only one Amane shell runs per session, so anything else the desktop needs must come from another framework, which `docs/design.md` rules out, or from Kanade.

## Decision

Kanade becomes a macOS-style niri shell on Amane, scoped by `docs/design.md`: no bar, the Island replaces bar, menu and taskbar, plus the Modules listed there; dock, wallpaper, lock, session and settings come in later roadmap phases (10-11). Every feature is a Module the user can disable; a disabled Module opens no window, reads no Service, registers no IPC and has no side effects. One process, except where security needs a split (lock, per its crash proof).

## Alternatives

**Stay an island beside another shell.** Another Amane shell cannot run beside it, and a second UI framework duplicates theme, config and IPC.

## Consequences

- A module registry with dependencies and capabilities comes first (roadmap 7A).
- README and the v0.1 non-goals change as Modules ship.
- Non-goals that remain are listed in `docs/design.md`.
