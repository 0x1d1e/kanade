# 99. The `ui` module and its verbs

Status: accepted. Item #21.

## Context

Every feature is a Module that can be turned off, with its settings in the registry and its verbs refused when off.

## Decision

- One Module, `ui` (requires `island`'s core, on by default, group Session in Settings), owning `ui.describe` and the verb `ui`. Off means the verbs answer `module ui is off` and `start` does nothing. `ui.describe = "off"` is the softer switch that leaves the module on and refuses.
- Verbs: `ui describe [--window W]` (JSON on stdout), `ui press|focus NAME`, `ui type NAME TEXT`, `ui key NAME KEY`, `ui scroll NAME LINES`, `ui wait NAME [--timeout S]`; `--window` takes `kanade-dock` or `dock`. Exit 1 with the reason on stderr for any refusal or a timeout.
- `describe` prints `{"windows":[{"name","size","scale","nodes":[{"id","parent","role","bounds","label"?,"value"?,"focused"?,"pressed"?,"disabled"?,"selected"?,"secret"?,"checked"?}]}]}`. `parent` is the parent's id or null. Keys are stable; absent means false or none.
- No new socket, protocol or crate: the verbs use the existing IPC handler and `Call` enum. `kanade help ui` and the shell completions come from the usage text.
- A reload applies a changed `ui.describe` at once (no restart).

## Consequences

- AGENTS.md prefers `kanade ui` over `wtype`/`ydotool` where a control is covered.
- Watching for changes (a stream) is not offered: `wait` covers the need without a long-held call.
