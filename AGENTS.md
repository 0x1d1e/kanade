# Kanade

Niri Dynamic Island built on Amane. Design: `docs/plan.md`.

## Commands

Run from repo root.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                # unit tests, incl. the island/ purity check
scripts/check-amane       # installed `amane` CLI must embed the pinned Amane
scripts/dev               # check-amane, then `AMANE_CONFIG=$PWD amane dev` (rebuilds on save in src/)
```

- `cargo` commands use the root `Cargo.toml` (`amane` by git rev, plus test-only `syn`).
- `amane dev` ignores `Cargo.toml` and `Cargo.lock`. It builds `$AMANE_CONFIG/src/main.rs` against the library embedded in the installed CLI, with `amane` as the only dependency, and never runs tests.
- So: `src/` uses only `amane` and std outside `#[cfg(test)]` (`src/boundary.rs` is test-only), and `island/` stays std-only.
- Only one Amane shell per session. Stop any other (`amane: amane is already running`) first.
- Zero idle frames check: `AMANE_FRAMES=1 scripts/dev` prints frames only when something draws.

## Amane pin

Single source of truth: the `rev` of `amane` in `Cargo.toml`. `amane dev` runs the library embedded in the CLI, not that rev, so `scripts/check-amane` compares the CLI binary byte for byte with the pinned checkout and prints the exact `cargo install --rev ... amane-cli` command on mismatch. Run it after bumping the rev or reinstalling the CLI.

## Rules

- `island/` is Amane-free except `island/service.rs`; enforced by `src/boundary.rs` (syn-based, covers aliases, nested modules, macros, `super`/`crate` escapes).
  `island/` may not depend on the rest of Kanade.
- Never `write()` a Service in a view. Sources post via `Island::write()`.
- Arbiter takes time as an input, never reads a clock.
