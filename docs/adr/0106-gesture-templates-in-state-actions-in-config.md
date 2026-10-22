# 106. Gesture templates live in state, their verbs in config

Status: accepted. Part of the gesture launcher (ADR 0105).

## Context

A gesture is a recorded shape and what it does. The shape is machine-made; what it does is the user's. The config is never rewritten by Kanade (design.md), and `kanade config set` does not edit tables: it refuses `gestures` with a reason, as it does `hooks` (`src/cli/keys.rs:208`).

## Decision

- **Templates** are state: `$XDG_STATE_HOME/kanade/gestures.toml`, `version = 1`, an array of `[[gesture]]` with a `name` and up to three `strokes`, each a flat list of 64 normalized points (`x, y, x, y, ...`, four decimals). Written whole with `atomic::write(.., Mode::Private)` by `gesture record` and `forget`.
- **Verbs** are config: `[gestures]`, `"name" = "<kanade verb>"`. It is data only, set in files like `hooks` (not by `config set` or Settings, both of which refuse it). Each value is checked when the config is read (ADR 0107).
- A name is 1 to 32 letters, digits, `-` or `_`; at most 64 gestures.
- The state file is outside input. It is read whole or refused with a reason (version, names, stroke length, finite and bounded numbers, duplicates), and **a refused file is never written over**: `record` and `forget` answer with the reason.
- A name in `gestures` with no template does nothing; a template with no verb is recorded and, when drawn, shows a Transient Failed Activity titled by its name saying there is no action. `record` tells the user to set one.

## Alternatives

**Both in config.** 192 floats per gesture in a file the user edits, and Kanade would have to rewrite it.

**Both in state.** The user's mapping would not be in their dotfiles.

## Consequences

- Moving to a new machine takes the config and a copy of `gestures.toml`.
- Tested: the file round trip, replace and forget, name and stroke checks, malformed files refused.
