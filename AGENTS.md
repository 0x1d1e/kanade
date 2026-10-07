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

## Simulated session and test network

E2E without touching the user's own shell, input or network. The user's session usually runs over its own Wi-Fi: never disconnect it, toggle its radio, or read its secrets.

Nested niri session:
- `niri -c niri.kdl` from the user's session opens a winit window with a new socket (e.g. `wayland-2`). In `niri.kdl`: `output "winit" { mode "1280x800"; scale 1; }` and `hotkey-overlay { skip-at-startup; }`.
- Run a copy of `target/debug/kanade` (so `scripts/dev` doesn't kill it) with:
  - `WAYLAND_DISPLAY=$XDG_RUNTIME_DIR/wayland-2` as an absolute path.
  - Its own `XDG_RUNTIME_DIR` (e.g. `/tmp/e2e/run`), so its IPC socket doesn't collide with the user's shell.
  - `NIRI_SOCKET` set to the nested niri's socket.
  - The same env for `./kanade <verb>`.
- Input: `wtype`. Send a whole key sequence in one call (`wtype -k Right -s 250 -k Return`). One call per key races niri's keymap update, so keys arrive decoded with the previous call's keymap.
- Screenshots: `grim -g` on the nested window's region of the real output. They lag about a frame.
- An island opened with `kanade controls open` is held for only 5 s after the last key (HOLD). Script each run in one shell command, and wait over 5 s between runs so it has collapsed.
- Only the display is nested. Controls still act on the host's NetworkManager, Bluetooth, audio and power profile.
  - Guard risky presses in the test build, e.g. replace `Network::set_wifi` with a log line, and revert before committing.
  - Restore anything a run changed and tell the user.

Test Wi-Fi network (`mac80211_hwsim`). The user runs these, since agents may not create access points:
```sh
sudo modprobe mac80211_hwsim radios=2      # e.g. wlan1 = AP, wlan2 = client
nmcli dev wifi hotspot ifname wlan1 con-name kanade-ap ssid kanade-test password kanade-test-1
nmcli con modify kanade-ap connection.interface-name wlan1 connection.autoconnect no \
  ipv4.method manual ipv4.addresses 10.42.0.1/24 ipv6.method shared
nmcli con up kanade-ap
```
- Interface names vary per load (e.g. wlan3/wlan4); use the two new ones `nmcli dev` shows.
- `autoconnect no` and binding to wlan1 matter. NM mirrors profiles into iwd known networks, and iwd may otherwise bring the hotspot up as a client on wlan2.
- `ipv4.method manual` is needed because NM's dnsmasq fails here (CAP_CHOWN). The client's IPv4 then times out, so joining takes about 45 s. That delay is from the test AP, not Kanade.
- iwd AP mode doesn't work: NM resets the device to station mode.
- `src/sources/wifi.rs` uses the first Wi-Fi device. For a run, temporarily pin `device()` to the client's `Interface`, and revert before committing.
- A wrong password shows up as device state FAILED with reason NO_SECRETS.
- Cleanup (by the user): `nmcli con delete kanade-ap; nmcli con delete kanade-test; sudo modprobe -r mac80211_hwsim`.

## Amane pin

The exact `rev` of `amane` in `Cargo.toml`. Bump it deliberately; never follow Amane `main`. The installed `amane` CLI is not needed: `kanade <verb>` talks to the shell (`src/cli.rs`).

## Rules

- `island/` is Amane-free except `island/service.rs`; enforced by `src/boundary.rs` (syn-based, covers aliases, nested modules, macros, `super`/`crate` escapes).
  `island/` may not depend on the rest of Kanade.
- Amane owns the Wayland runtime/UI seam. `wayland-client` only in `src/doctor/` (diagnostics, no shell behavior); enforced by `src/boundary.rs`.
- Never `write()` a Service in a view. Sources post via `Island::write()`.
- Arbiter takes time as an input, never reads a clock.
- `src/clock.rs` is the platform boundary: the only `unsafe` and hand-kept libc ABI (`tm`, `timespec`, timerfd constants). Keep new FFI there.
