# Kanade

niri shell on its own runtime (ADR 0028), centered on an adaptive activity island. Before architecture or scope work, read `docs/design.md` (target design; decisions in `docs/adr/`). v0.1 plan, history only: `docs/archived/plan.md`.

Domain terms and invariants: `CONTEXT.md`. Before naming a type or writing docs, use its terms, not its avoid-listed synonyms.

User docs: `README.md` is the short overview; install, CLI, config, keyboard and limitations live in `docs/wiki/`. The config page holds `kanade config defaults` verbatim (checked by a unit test).

## Commands

Run from repo root.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace    # the few unit tests, incl. the island/ purity check (see Tests and CI)
cargo run                 # the shell
scripts/dev               # cargo build, restart the shell on each save; a failed build keeps the old one
```

- `Cargo.toml` and `Cargo.lock` are authoritative for every build. Add dependencies there as needed; none speculatively.
- Only one Kanade per session. Stop any other (`failed to listen: kanade is already running`) first.
- Zero idle frames check: `KANADE_FRAMES=1 scripts/dev` prints frames only when something draws.
- Idle wakeups (#40): diff `voluntary_ctxt_switches` of each `/proc/$(pgrep -x kanade)/task/*/status` over 30 s at rest; expected numbers in `docs/adr/0004-wake-sources-on-announcements.md`.
- Morph smoothness (#36): with `KANADE_FRAMES=1 scripts/dev > LOG 2>&1` running, `scripts/frames LOG` prints frame gaps per Surface transition; mean gap should match the refresh interval.
  For the Calendar's worst case, give the shell a temporary `XDG_DATA_HOME` whose `calendars/` holds an `RRULE:FREQ=SECONDLY` event from years ago: no frame is slow; on opening or turning the month, the agenda says it is loading until the month is expanded.
- Memory (#198): before and after a memory change, take a baseline with `scripts/memory` as `docs/memory.md` says; RSS misses the GPU buffers.
- Privacy E2E: `pw-record` for the mic, `wf-recorder` for a niri cast. Webcam: `gst-launch-1.0 pipewiresrc autoconnect=false client-name=camtest ! fakesink`, then `pw-link -L <v4l2 node>:capture_1 camtest:input_1` (plain `pipewiresrc` fails with `target not found`).
- Pointer E2E: `ydotool` needs `YDOTOOL_SOCKET=$XDG_RUNTIME_DIR/.ydotool_socket`, or clicks silently go nowhere. To check passthrough, log clicks in a fullscreen GTK window, not kitty mouse reporting, which drops clicks near the edges.

## Tests and CI

Few tests, on purpose. Real behavior is checked by E2E on the user's session (below); a unit test is for what E2E cannot reach cheaply or what is costly to get wrong.

- Write a unit test only for: an enforced boundary or invariant (`src/boundary.rs`, `island/` purity, the Arbiter's rules); safety (session lock, privacy, input security); data integrity (config, settings and state files, recurrence and time maths, parsers of outside input); the runtime's core; a confirmed defect, once, at the seam it crossed.
- Do not write one for layout, geometry or motion numbers, view code, constants, or a rule it only restates; look at it running instead. A test that fails whenever the code is touched, or that you would delete rather than fix, should not exist. Delete such a test you meet, in the change that touches it.
- Never weaken a test to pass. Do not add tests merely because code changed.
- CI is one job: `cargo test --workspace --locked` (`.github/workflows/test.yml`). No fmt or clippy in CI; run them locally before every commit. No matrices, nightly runs or extra jobs without a stated need. E2E, baselines (idle frames, wakeups, frame gaps, memory) and the test network are manual, never CI.

## E2E session and test network

E2E runs on the user's own niri session; no nested niri. The user's session usually runs over its own Wi-Fi: never disconnect it, toggle its radio, or read its secrets.

- The test build replaces the user's shell (one Kanade per session): `scripts/dev` if running, else stop their shell (`systemctl --user stop kanade.service`, or `kanade` if they run it otherwise) and run `target/debug/kanade`; restart the unit after. Leave a working shell running afterwards.
- Config/state tests: run it with a temporary `XDG_CONFIG_HOME`/`XDG_STATE_HOME`, so the user's real config and `settings.toml` stay untouched; restart with their normal env after.
- Input lands in the user's session: note the focused window first (`niri msg focused-window`), refocus it after, and tell the user their pointer/focus was used.
- Keys: `wtype`. Send a whole key sequence in one call (`wtype -k Right -s 250 -k Return`). One call per key races niri's keymap update, so keys arrive decoded with the previous call's keymap. The first call after a while may race too; warm up with `wtype -k Shift_L`.
- Screenshots: `grim -o <output>` or `grim -g "x,y wxh"`; `niri msg windows`/`outputs` give positions. They lag about a frame.
- An island opened with `kanade controls open` is held for only 5 s after the last key (HOLD). Script each run in one shell command, and wait over 5 s between runs so it has collapsed.
- Controls act on the real NetworkManager, Bluetooth, audio and power profile.
  - Guard risky presses in the test build, e.g. replace `Network::set_wifi` with a log line, and revert before committing.
  - Restore anything a run changed and tell the user.

Test Wi-Fi network (`mac80211_hwsim`): `scripts/test-network up`, then `down` to clean up. The user runs it, since agents may not create access points. It finds the two simulated radios, hosts `kanade-test` (password `kanade-test-1`) on the first and prints the client.
- The AP is bound to its radio with `autoconnect no`. NM mirrors profiles into iwd known networks, and iwd may otherwise bring the hotspot up as a client on the other radio.
- `ipv4.method manual` is needed because NM's dnsmasq fails here (CAP_CHOWN). The client's IPv4 then times out, so joining takes about 45 s. That delay is from the test AP, not Kanade.
- iwd AP mode doesn't work: NM resets the device to station mode.
- `src/sources/wifi.rs` uses the first Wi-Fi device. For a run, temporarily pin `device()` to the client's `Interface`, and revert before committing.
- A wrong password shows up as device state FAILED with reason NO_SECRETS.

## Git

Commit locally during a phase; push nothing until it ends. Then squash the PR branch to one commit over `main` and force-push it (`--force-with-lease`), keeping a local backup branch of the old history.

## Runtime

`crates/runtime` is Kanade's own code (ADR 0028); change it freely. Remaining work: `docs/runtime-migration.md`. Fill a missing renderer/input piece in the runtime, not in a new crate beside it.

## Rules

- Kanade comes from 0x1d1e, an experimental lab: invent, don't just reuse. macOS and other shells, docks and tools are references to learn from and surpass, not ceilings. Build what no one has thought to build, where it makes the shell better. Do not rebuild generic infrastructure for its own sake.

- Until 1.0, ship the product fast; performance waits for 1.0. A look or motion that matches or surpasses macOS wins over its CPU, GPU or memory cost, and resource use is neither measured nor a gate. Reviews fix high-level issues: wrong behavior a user meets, architecture, session-lock safety, privacy, data integrity, input security, working-shell recovery. Low-level ones (resource use, polish, code-level cleanups) go to `TODO.md` unfixed. `cargo fmt`, `clippy` and `cargo test` still pass locally before every merge (CI runs only the tests).

- Prefer a coherent primitive in the runtime over another crate with its own Wayland connection. One proven pipeline beats parallel adapters; no per-frame screenshot-to-PNG transfer.

- Never open an issue or PR on niri, nor write a custom patch, fork or vendored change for it. Build everything in Kanade, working around its gaps from Kanade's side.

- `island/` names no `kanade_runtime` except `island/service.rs`; enforced by `src/boundary.rs` (syn-based, covers aliases, nested modules, macros, `super`/`crate` escapes).
  `island/` may not depend on the rest of Kanade.
- `crates/runtime` owns the Wayland and UI seam. `wayland-client` only in `src/doctor/` (diagnostics, no shell behavior), enforced by `src/boundary.rs`, `crates/lock` (the lock screen, ADR 0025) and `crates/runtime` (ADR 0028).
- Never `write()` a Service in a view. Sources post via `Island::write()`.
- Arbiter takes time as an input, never reads a clock.
- `src/clock.rs` is the platform boundary: the only `unsafe` and hand-kept libc and kernel ABI (`tm`, `timespec`, timerfd, evdev, rfkill). Keep new FFI there. Exception: `crates/runtime` (glibc allocator tuning, the GPU's Wayland surface; see its `lib.rs`).
