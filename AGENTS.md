# Kanade

Niri Dynamic Island built on Amane. Design: `docs/plan.md`.

## Commands

Run from repo root.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                              # unit tests, pure modules in src/island/
AMANE_CONFIG=$PWD amane dev             # run on the live session, rebuilds on save in src/
```

- `cargo` commands use the root `Cargo.toml` (`amane` by git rev, for fmt/clippy/test only).
- `amane dev` ignores `Cargo.toml` and `Cargo.lock`. It builds `$AMANE_CONFIG/src/main.rs` against the library embedded in the installed CLI, with `amane` as the only dependency, and never runs tests.
- So: no other crates in `src/`, and `island/` stays std-only.
- Only one Amane shell per session. Stop any other (`amane: amane is already running`) first.
- Zero idle frames check: `AMANE_FRAMES=1 AMANE_CONFIG=$PWD amane dev` prints frames only when something draws.

## Amane pin

Rev `30800248fc51a663ec1f852047818641d6218786`, set in `Cargo.toml`. Install the matching CLI:

```sh
cargo install --git https://github.com/MystiaFin/amane --rev 30800248fc51a663ec1f852047818641d6218786 --locked amane-cli
```

Bump rev and CLI together, then rerun all commands above.

## Rules

- `island/` is Amane-free except `island/service.rs`; enforced by a test in `island/mod.rs`.
- Never `write()` a Service in a view. Sources post via `Island::write()`.
- Arbiter takes time as an input, never reads a clock.
