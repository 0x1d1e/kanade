# Kanade

niri shell built on Amane, centered on a Dynamic Island. Before architecture or scope work, read `docs/design.md` (target design; decisions in `docs/adr/`). v0.1 plan, history only: `docs/archived/plan.md`.

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
- Only one Amane shell per session. Stop any other (`failed to listen: amane is already running`) first.
- Zero idle frames check: `AMANE_FRAMES=1 scripts/dev` prints frames only when something draws.
- Idle wakeups (#40): diff `voluntary_ctxt_switches` of each `/proc/$(pgrep -x kanade)/task/*/status` over 30 s at rest; expected numbers in `docs/adr/0004-wake-sources-on-announcements.md`.
- Morph smoothness (#36): with `AMANE_FRAMES=1 scripts/dev > LOG 2>&1` running, `scripts/frames LOG` prints frame gaps per Surface transition; mean gap should match the refresh interval.
- Privacy E2E: `pw-record` for the mic, `wf-recorder` for a niri cast. Webcam: `gst-launch-1.0 pipewiresrc autoconnect=false client-name=camtest ! fakesink`, then `pw-link -L <v4l2 node>:capture_1 camtest:input_1` (plain `pipewiresrc` fails with `target not found`).
- Pointer E2E: `ydotool` needs `YDOTOOL_SOCKET=$XDG_RUNTIME_DIR/.ydotool_socket`, or clicks silently go nowhere. To check passthrough, log clicks in a fullscreen GTK window, not kitty mouse reporting, which drops clicks near the edges.

## Amane pin

The exact `rev` of `amane` in `Cargo.toml`. Bump it deliberately; never follow Amane `main`. The installed `amane` CLI is not needed: `kanade <verb>` talks to the shell (`src/cli.rs`).

## Rules

- `island/` is Amane-free except `island/service.rs`; enforced by `src/boundary.rs` (syn-based, covers aliases, nested modules, macros, `super`/`crate` escapes).
  `island/` may not depend on the rest of Kanade.
- Amane owns the Wayland runtime/UI seam. `wayland-client` only in `src/doctor/` (diagnostics, no shell behavior); enforced by `src/boundary.rs`.
- Never `write()` a Service in a view. Sources post via `Island::write()`.
- Arbiter takes time as an input, never reads a clock.
- `src/clock.rs` is the platform boundary: the only `unsafe` and hand-kept libc ABI (`tm`, `timespec`, timerfd constants). Keep new FFI there.
