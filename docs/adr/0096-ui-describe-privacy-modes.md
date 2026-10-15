# 96. `ui.describe`: structure by default, content on request, off to refuse

Status: accepted. Item #21.

## Context

A tree of the screen is a way to read it. Notifications, clipboard entries, lyrics, calendar events, window titles, network and device names and passwords must not reach a script that did not need them.

## Decision

- `ui.describe` is `structure` (default), `content` or `off`.
- Structure: roles, names, states, levels and bounds. Every `content` string and every text input's text is `<redacted>`.
- Content: as structure, with the text as it is, for debugging on this machine. A password input's text is never stored on a target in any mode.
- Off: the module keeps no tree, every `kanade ui` call is refused (`ui.describe is off`), and `.semantics` closures are not run, so nothing is built or read.
- Typed by construction: `name` is a `&'static str`; `label` is for what the shell or installed software calls a thing (an app's name from its `.desktop` entry, "Wi-Fi"), said in every mode; `content` is for what the user or the world wrote. A view picks `content` for anything it did not write in source. When unsure, `content`.
- Scrubbing happens when the target is stored (`Semantics::scrubbed`), not when it is read out, so a redacted string does not sit in memory behind a flag. A mode change marks every window to draw again.
- A name is matched against ids and public labels only. A redacted label matches nothing, so naming cannot probe for redacted text.
- Nothing from the tree is logged.

## Consequences

- Dock icons are addressable by app name in structure mode; Launcher results are `result`, `result#2`, in order.
- Content mode shows secrets of the session to whoever can reach the socket: it is the user's own choice, and the socket is user-only ([ADR 0098](0098-ui-is-refused-while-locked-and-the-socket-is-user-only.md)).
