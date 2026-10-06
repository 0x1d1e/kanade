# Kanade

Niri Dynamic Island built on Amane. Design: `docs/plan.md`.

Domain terms and invariants: `CONTEXT.md`. Before naming a type or writing docs, use its terms, not its avoid-listed synonyms.

## Commands

Run from repo root.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                # unit tests, incl. the island/ purity check
cargo run                 # the shell against the pinned Amane
scripts/dev               # cargo build, restart the shell on each save; a failed build keeps the old one
```

- `Cargo.toml` and `Cargo.lock` are authoritative for every build. Add dependencies there as needed; none speculatively.
- Only one Amane shell per session. Stop any other (`amane: amane is already running`) first.
- Zero idle frames check: `AMANE_FRAMES=1 scripts/dev` prints frames only when something draws.
- Idle wakeups (#40): diff `voluntary_ctxt_switches` of each `/proc/$(pgrep -x kanade)/task/*/status` over 30 s at rest; expected numbers in `docs/adr/0004-wake-sources-on-announcements.md`.
- Morph smoothness (#36): with `AMANE_FRAMES=1 scripts/dev > LOG 2>&1` running, `scripts/frames LOG` prints frame gaps per Surface transition; mean gap should match the refresh interval.

## Amane pin

The exact `rev` of `amane` in `Cargo.toml`. Bump it deliberately; never follow Amane `main`. The installed `amane` CLI is only needed for `amane ipc call`, and its version does not matter.

## Rules

- `island/` is Amane-free except `island/service.rs`; enforced by `src/boundary.rs` (syn-based, covers aliases, nested modules, macros, `super`/`crate` escapes).
  `island/` may not depend on the rest of Kanade.
- Never `write()` a Service in a view. Sources post via `Island::write()`.
- Arbiter takes time as an input, never reads a clock.
- `src/clock.rs` is the platform boundary: the only `unsafe` and hand-kept libc ABI (`tm`, `timespec`, timerfd constants). Keep new FFI there.
