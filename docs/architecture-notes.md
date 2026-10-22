# Architecture notes

Facts no other doc holds. Domain terms and invariants are in `CONTEXT.md`; decisions are in `docs/adr/`.

## What `src/boundary.rs` enforces

It is `#[cfg(test)]` (`src/main.rs:6-7`), so it checks code and never ships. Four rules, one test each:

- `island_is_pure`: no file under `src/island/` except `service.rs` names `kanade_runtime`, `service`, or anything of Kanade outside `island/`. Parsed with syn, so aliases (`use kanade_runtime as ui`), nesting, `super` climbing past the module, macros and `#[path]` modules are covered.
- `wayland_stays_in_doctor`: `wayland_client` appears in no file under `src/` outside `src/doctor/`. A raw token scan, not syn: `mentions` reads identifiers out of the token stream, so `extern crate wayland_client as w` counts too. `crates/lock` and `crates/runtime` are outside `src/` and outside this check.
- `colors_stay_in_theme`: no `Color` constant, `Color::rgb/rgba/from` with a literal argument, or hex string literal in non-test code outside `src/theme.rs`. syn-based, and it follows `use .. Color as X`.
- `module_names_are_in_the_catalog`: every string passed to `modules::on("…")` outside `src/modules/` names a Module in `modules::ALL`.

## What it cannot see

- Renaming the dependency in `Cargo.toml` (`rt = { package = "kanade-runtime" }`) makes `use rt::…` invisible to the syn checker: it looks for the ident `kanade_runtime`, not for the crate.
- `module_names_are_in_the_catalog` matches only the literal token sequence `modules . on ( "…" )`. `use crate::modules::on;` followed by `on("x")`, or a name held in a variable or a const, is never collected. Five such call sites already pass a name they hold rather than a literal (`src/ipc.rs:46`, `src/settings/view.rs:1300`, `src/surfaces/slider.rs:84`, `:116`, `src/hooks.rs:573`), and the test's `names.len() > 50` floor only guards the literal ones.
- `wayland_stays_in_doctor` is a token scan, so it sees the ident wherever it appears, but only under `src/`; a second connection opened in a new crate would not be caught.

## The lock a sleep began without

`src/sources/sleep.rs` asks again for a lock a sleep began without, once logind says the machine woke. `hold_again` returns early when `lock::holds()` is already true, so a lock niri was still confirming as the machine slept is not asked for twice: asking would drop a password typed on the lock screen meanwhile.

Seen on a real suspend and wake (KVM, `s2idle`, niri 26.04): `lock::hold` failed before the sleep (`kanade: did not lock before sleep: niri did not lock within 5s`), and after the wake the run logged `kanade: locked on waking, as sleep began without it` with `LockedHint=yes`. That cycle does not show niri asked a second time: the failure was the 5 s patience timeout while niri was already locking, so `lock::holds()` held at wake and `hold_again` returned without an ask. A cycle whose pre-sleep ask niri refuses, another locker holding the session, would show the ask itself, and has not been run.
