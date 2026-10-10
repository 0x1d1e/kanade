# 38. niri is a Service its readers watch

Status: accepted. Amends ADR 0014, whose `windows` was handed niri's window events by the niri source.

## Context

`sources/niri.rs` followed niri's EventStream and wrote straight into what depended on it: `Fullscreen`, `Privacy`, the Banners' focus, `IslandService`, `windows::hear` and `capture::captured`. A five-flag `Posts`, built in `modules/catalog.rs` from which Modules were on, said which to write. The source therefore knew its consumers, depended upward on `banners`, and a new reader of niri meant editing the source.

## Decision

- niri's state is the `Niri` Service: the focused output, the overview, the focused workspace, whether anything casts, and the window each output shows. The niri thread writes it only when it changes, and writes the default again when the stream is lost.
- Each reader `watch`es `Niri` in its own start (ADR 0035) and derives a narrow value of its own: `Fullscreen`, `Privacy::casting`, the Banners' focus, the workspace switch. A watcher that finds nothing changed calls `quiet`, so a write of `Niri` redraws only what changed.
- A screenshot niri saves and a window it opens, closes or focuses are events, not state. A Module takes them with `niri::on_screenshot` and `niri::on_windows` in its start. `modules::start` runs the stream after every start, so no first event is missed.
- The source names no reader, and `Posts` is gone.

## Consequences

- A new reader of niri is one `watch` in its Module's start; the source does not change.
- Watchers see only the latest state, not each event: a workspace switched twice between two derives posts one switch, and a switch that comes with the overview closing is told from one made outside it by the overview's state alone (`workspace::change`).
- A write of `Niri` runs every watcher. Each must compare before it writes, or it redraws its readers for nothing.
