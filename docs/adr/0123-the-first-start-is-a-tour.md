# 123. The first start is a tour

Status: accepted. Not implemented in this change.

## Decision

- One pass, once, after the first start of an installed shell. It names the keybinds and how to open a Surface.
- A state file gates it, the same durability idea as `$XDG_STATE_HOME/kanade/health-notice` in ADR 0088. It does not choose settings, write niri config, or replace the health notice. Settings stays the place for choices.

Files: the tour is not in this commit.
