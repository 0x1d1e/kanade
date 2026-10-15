# 81. Per-app output routing

Status: accepted. Builds on [ADR 0004](0004-wake-sources-on-announcements.md) (wake sources) and the Audio sub-surface (`src/sources/audio.rs`).

## Context

The Audio sub-surface lists apps with a level each, and the outputs with a default, but an app could not be sent to another output. WirePlumber honours the `target.object` key of the `default` metadata for a stream node: set, the session manager links the stream to that node and keeps it there; deleted, the stream follows the default again. Not tested against a live PipeWire in this change; `pw-metadata` is the tool the source already uses for the defaults.

## Decision

- `Stream.target` is the output the app was moved to, read from `target.object` in the `default` metadata (a number is a node serial or id, a string a node name; resolved against the outputs by serial, then name). Metadata for a node that is gone is dropped with the graph, and a removed key clears it.
- `Ask::Route(stream, Some(output))` runs `pw-metadata -n default <stream id> target.object <output serial> Spa:Id`; `Ask::Route(stream, None)` runs `pw-metadata -n default -d <stream id> target.object`. Like every Ask it runs serially off the draw thread, replaces an earlier route of the same stream, and is verified against the graph first: a stream or output that left is not acted on.
- The Audio sub-surface shows a Route row under each playing app, only while there are two outputs. Pressing it (Enter or Space, so the sub-surface stays keyboard-complete) moves the app to the next output; after the last, back to the default (the key deleted). The row names where it plays: the output it was moved to, or the default one. It is a separate row rather than a second target in the app's row because Left and Right already set that row's level.
- No new Module, setting or CLI verb: the Audio sub-surface belongs to `audio`, which is on or off as before. Nothing new runs while it is off.

## Consequences

- WirePlumber's own policy still decides what happens if the target cannot take the stream; Kanade only writes the key.
- Moving an app does not change the default output, and the key is not persisted by Kanade (WirePlumber may restore it per app, its choice).
- Files: `src/sources/audio.rs`, `src/surfaces/controls/audio.rs`, `src/surfaces/controls/focus.rs`, `src/surfaces/controls/mod.rs`.
