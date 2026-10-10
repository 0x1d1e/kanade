# 35. Derived updates run in the runtime, in order

Status: accepted. Amends ADR 0030, whose `service::watch` ran on the writer's thread.

## Context

`service::watch::<S>(f)` ran `f` on whichever thread wrote `S`, once the write let go. What follows from a Service therefore ran on a Source's thread, a D-Bus callback or an input handler, each holding whatever it held, and the order of two derived updates depended on which writer got there first. A derive that wrote a Service another derive watched nested on the writer's stack. A panic in `f` unwound into an unrelated Source.

## Decision

The Service API stays: `read`, `write`, `quiet`, `watch`. What changes is who runs what is watched.

- A finished, non-quiet write marks its Service for deriving, then wakes the runtime, as it did for drawing.
- `service::derive()` runs what `watch`es each marked Service, on the runtime's thread, in the order it was watched. Writes made by a derived update mark again and run in the next round, up to 16 rounds, so a chain settles in one call and a cycle is reported instead of overflowing the stack.
- The runtime calls it when it wakes, before it takes what changed, and before a window's view runs, so every draw shows derived values and a derived write redraws the windows that read it.
- A derived update that panics is reported on stderr and the shell goes on.
- Several writes between two derives run a watcher once, which reads the latest.

## Consequences

- A derived update is no longer visible to its writer at once; it is visible to the next draw. Nothing in Kanade reads it between.
- Order is explicit and one thread's: to follow another derive, `watch` after it.
- Dirty tracking is by Service type, and by part for a Service that reads and writes one (`Service::read_part`, `Write::part`): `Backdrops`, by pane (ADR 0037), and `Pointers`, by output. Every other write draws every window that read the Service (`TODO.md`).
- A derived update must not block: it holds up drawing.
