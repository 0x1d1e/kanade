# Kanade design

## Objective

Desktop shell for niri, Rust, on Kanade's own runtime (`crates/runtime`, ADR 0028) ([ADR 0008](adr/0008-kanade-is-a-shell.md)). No bar. One Island/output: status at rest, Activities live, Surfaces on demand. ~zero idle cost.

Sources of truth: tracker = work/status; `CONTEXT.md` = domain; ADRs = durable rationale; this doc = target architecture/constraints. Where this doc changes a `CONTEXT.md` term or invariant, `CONTEXT.md` changes in the PR that implements it; until then `CONTEXT.md` describes the code. History: `docs/archived/plan.md` (v0.1 plan).

## Product rules

- Island replaces bar/menu/taskbar.
- Niri owns layout, focus, Alt-Tab, Overview. No Mission Control clone.
- No desktop widgets/full DE.
- English-only UI.
- Surfaces keyboard-complete; reduced motion supported.
- Privacy default on; mic/cam/capture as dots inside the Island; not over fullscreen; `privacy.indicators` turns them off.
- Every feature module disableable. Disabled = no windows/subscriptions/IPC/side effects; unread Services stay cold.
- awww renders wallpaper. External polkit agent for now.

## Domain

- **Island**: shell surface/output, on the edge and side `island.edge`/`island.align` give it (top-center by default; also the middle of the left or right edge, where its small forms stand upright). The canvas follows the largest body (`island.width`/`island.height`) and the output; list Surfaces fit their content up to it (ADR 0030). Idle, it may slide past its edge until the pointer touches it or an Activity shows (`island.autohide`), or keep windows off its strip (`island.reserve`).
- **Rest**: idle Island; shows the clock.
- **Split**: Compact showing the primary and the top Satellite in one split body; each segment peeks and opens its own Activity ([ADR 0010](adr/0010-split-and-rest-right-click.md)).
- **Activity**: live item eligible for Island slots.
- **Satellite**: Persistent Ongoing/Critical Activity that is not the primary; cap `SATELLITES`, rest a count.
- **Arbiter**: Island selection policy.
- **Surface**: interactive content of an Expanded Island.
- **Privacy cluster**: dots at the Island body's trailing end; never Activity.
- **Module**: user-disableable feature/interface.
- **Service**: `kanade_runtime::service::Service`; lazy shared state/integration source.
- **LauncherProvider**: query → ranked launcher results/actions.

## Architecture

Runtime: [ADR 0026](adr/0026-kanade-owned-runtime.md), [ADR 0027](adr/0027-the-runtime-takes-over-one-window-at-a-time.md) step 1 and [ADR 0028](adr/0028-the-runtime-is-ported-from-amane.md); what is left in [runtime-migration.md](runtime-migration.md). Amane's UI is ported into `crates/runtime`; Amane is no dependency.


```text
OS / niri / D-Bus / PipeWire / files / network
                    ↓
             adapters + Services
                    ↓
                feature modules
              ↙              ↘
         Arbiter/Island    other windows
```

One Kanade process on its own runtime by default; security-sensitive code may split. Views do no I/O. External semantics stop at adapters/Services; policy stays in owning module. Services may be shared; avoid wrapper-per-dependency architecture.

### Layers

`src/boundary.rs` keeps the Domain pure and Wayland in the Runtime.

- **Domain**: `src/island/` (Arbiter, Activities, Presentation, policies, geometry). Pure: no runtime, no clock read, no I/O.
- **Application**: commands (`island/command.rs`, `cli.rs`), Sources (`sources/`) and Services (`island/service.rs`, ADR 0035's derived updates), and the Module lifecycle (`modules/`: `catalog.rs` lists them, `mod.rs` resolves and starts them).
- **Scene**: where things stand, once per output: `scene.rs` (the Island window's canvas, body, plate, input region; ADR 0036), `merge.rs` (Crown, Keystone, Fold), `cluster.rs`, and `glass/`, which says what to capture behind a body and hands its copy to the shader (ADR 0037). Views draw and hit-test from it.
- **Views**: widgets and windows over a Scene: `view/` (the Island's window; `forms`, `status`, `media`, `toast`, `rest`, `hud`, `satellites`, `input` and `shape` by what they draw), `dock.rs`, `banners/`, `surfaces/`, `settings/`. They never `write()` a Service they read for drawing.
- **Runtime**: `crates/runtime`: Wayland, input, key repeat, toplevels, backdrop capture, GPU (vello, shaders), text, scheduling and IPC. It knows regions and values, not Kanade's bodies.
- **Adapters**: OS and network services behind Sources (`sources/`, `bus.rs`), the platform boundary `clock.rs`, and the lock screen, which stays isolated in `crates/lock` with its own connection and recovery (ADR 0025).

### Modules

```text
Descriptor: config schema · CLI schema · requires · optional · runtime factory
Runtime:    policy/state · Service readers/subscriptions · windows
```

| Group | Modules |
| --- | --- |
| Core | `island` |
| Activities | `media`, `timer`, `battery`, `workspace` |
| Sources | `notifications`, `tray`, `audio`, `brightness`, `network`, `bluetooth`, `power`, `windows`, `clipboard`, `calendar`, `weather` |
| Overlays | `privacy`, `banners` |
| Island Transients | `osd` |
| Surfaces | `controls`, `launcher`, `notification-surface`, `clipboard-surface`, `calendar-surface`, `weather-surface` |
| Utilities | `capture`, `caffeine`, `doctor` |
| Desktop | `dock`, `wallpaper` |
| Session | `lock`, `session`, `settings` |

Rules:

- hard dep missing → disable + named error; soft capability missing → degrade.
- Island Surfaces require `island`.
- `notification-surface` → `island` + `notifications`; `banners` → `notifications`.
- `dock` → `windows`, never `workspace`.
- `osd` optionally reads `audio`/`brightness`, the keyboards' evdev devices (input group), `/dev/rfkill` and logind's `Active` (ADR 0021); typing and key repeat are the runtime's (ADR 0034).
- `privacy` reads capture state independently from `audio` and `capture`.
- `privacy` default on; disable warns indicators disappear.
- `calendar` network/account sync optional; local calendar still works without it.
- `weather` network-backed; no polling when disabled or without a location; bounded refresh when enabled ([ADR 0017](adr/0017-weather-from-open-meteo.md)). `weather-surface` → `weather`.
- module toggle requires restart while the runtime registers windows only at startup.

`notifications` owns bus/state/history; UI separate. `windows` owns normalized niri running-window/app state; `app_id` ↔ `.desktop` matching + overrides: [ADR 0014](adr/0014-running-apps-from-desktop-entries.md). Dock never consumes raw niri objects.

## Island + Arbiter

Fixed max window/output; input region follows pill. Presentations: Rest, Compact, Split, Peek, Expanded.

| Activity field | Meaning |
| --- | --- |
| `Priority` | slot order |
| `Lifetime` | Persistent / until-dismissed / Transient(ms) |
| `Interrupt` | none / preempt / auto-expand(ms) |
| `Scope` | Global / FocusedOutput |
| `Identity` | source + id |

Fields are independent; sources set each explicitly ([ADR 0009](adr/0009-orthogonal-activity-policy-fields.md)).

Rules:

- primary = highest priority; tie → newest. Satellites per Domain.
- primary dwell: a new primary stays ≥ ~1.5 s before a newer equal-priority Activity replaces it.
- higher priority, preempt, auto-expand, withdraw, expiry bypass dwell.
- auto-expand restores prior Surface/state.
- same identity updates in place; no entry replay.
- transient = lifetime/presentation, never priority.
- privacy never enters Arbiter.

**Fullscreen:** no suppression. Island = Top; at rest fullscreen covers it. Covered = the output's shown window is fullscreen per the runtime's wlr-foreign-toplevel list (ADR 0034) and its niri tile fills the output; there the Island waits hidden on Overlay and shows for the OSD or an open Surface. Geometry alone rejected. Privacy = the cluster, dots inside the body that a covered output does not show. [ADR 0033](adr/0033-the-privacy-cluster-is-dots-inside-the-island.md), [ADR 0006](adr/0006-island-on-top-layer.md), [ADR 0005](adr/0005-privacy-indicator-outside-arbiter.md), [ADR 0021](adr/0021-the-osd-returns-to-the-island.md).

**Rest:** minimal configured status. Tray appears on interaction: hover raises the Tray strip, the time over the date (and the weather and next event as set), then item icons; a slot is the item's (left/middle/scroll, right → its menu pinned) ([ADR 0012](adr/0012-tray-at-rest-and-item-menus.md)). Left click → Controls. Right click → Controls pinned; no context menu ([ADR 0010](adr/0010-split-and-rest-right-click.md)).

**Split:** follows the Frame like Compact: Split while a Satellite exists, Compact without. Hover/click/right click act on the segment under the pointer; Peek shows one Activity by identity. [ADR 0010](adr/0010-split-and-rest-right-click.md).

## UI

### Core Surfaces

- **Launcher**: providers = apps, calculator, emoji, wallpaper, destinations (Wi-Fi and Bluetooth open Controls on their sub-surface, Clipboard its history, each Settings page and setting opens Settings on its page; they only open, switching is done there). Focus only while open. Provider seam internal; no public plugin API yet.
- **Controls**: macOS pattern. Top-level quick tiles/sliders; sub-surfaces for Wi-Fi (done, #130), Bluetooth (done, #131), audio (done, #133).
- **Wi-Fi**: scan, connect/disconnect, network selection, state/error.
- **Bluetooth**: scan, pair, connect/disconnect, device selection, state/error.
- **Audio**: master volume/mute, input/output device selection, per-app mixer, mic state.
- **Notifications**: history, actions, dismiss, clear, DND, empty/error.
- **Clipboard**: text + image history, search, copy, delete, clear; bounded; memory-only default; optional persistence later. Sensitive copies (`x-kde-passwordManagerHint`) never read; needs wl-paste ≥2.3 ([ADR 0019](adr/0019-clipboard-sensitive-content.md), done, #162).
- **Calendar**: month/agenda; local iCalendar files, read-only (ADR 0015); optional Google Calendar account sync into files the calendar reads, OAuth with the user's own client, secrets in the Secret Service (ADR 0016).
- **Weather**: current + 5-day forecast from Open-Meteo, no account; explicit `weather.location`; 30 min refresh, failures back off 1 → 30 min; last good forecast shown stale with its time; error state without one (ADR 0017) (done, #152).

### Tray

StatusNotifierItem behavior:

- icon/status/title updates;
- primary activation;
- secondary/context activation;
- scroll events;
- menus/submenus/actions;
- malformed/dead item isolated, never shell-wide failure.

Shown as the Tray strip at Rest and the Tray Surface; menus are sub-surfaces (done, #135, [ADR 0012](adr/0012-tray-at-rest-and-item-menus.md)).

### Capture

`capture` initiates; `privacy` independently observes actual capture.

- screenshot: area crop, window, fullscreen/output;
- screenshots: niri-native IPC actions cover all three modes, so no external screenshot backend; `doctor` names a missing niri. External backend only for recording (#140);
- recording: `wf-recorder` through niri's wlr-screencopy, so `privacy` sees it as a cast; `h264_vaapi` to `~/Videos/Screencasts`, no fallback encoder; start/stop/status; a Persistent Ongoing Activity with Stop while it records, then the same Transient Activity as a screenshot (done, #140, [ADR 0013](adr/0013-screen-recording-with-wf-recorder.md));
- completion emits a Transient Activity + copy/open path actions, also for niri's own screenshot binds (done, #139);
- no OCR, reverse-image search, annotation editor, upload service.

### Caffeine

Idle inhibitor toggle/activity. Prevent idle/lock/suspend only while explicitly active; visible state; optional timeout.

A `systemd-inhibit --what=idle` holder (ADR 0011) while on, only under `setpriv --pdeathsig`; its command says when logind granted the inhibitor, then `sleep`s the timeout. The shell answers `on` at once as starting and a worker turns it on once that is said, letting go of the old holder only then, so it never lapses and the draw thread never waits; `kanade` waits on `caffeine status` within one 5 s patience. Off, toggle and a newer on supersede a start by serial. A refused or failed on shows a Transient Failed Activity while off. A Persistent Ongoing Activity with Turn off while it holds; ending on its own any way but the timeout posts a Transient Failed Activity with the reason (done, #141).

### Presentation migration

[ADR 0007](adr/0007-banners-and-osd-leave-the-island.md), its OSD part superseded by [ADR 0021](adr/0021-the-osd-returns-to-the-island.md).

- notification toast → **Banner**: focused output, hanging from the Island and coming out of it ([ADR 0023](adr/0023-banners-come-out-of-the-island.md)); 4 s low, 6 s normal, critical sticky; hover pause; max 3.
- volume/brightness/mic and Caps Lock/Num Lock/airplane mode → **OSD**, a `Feedback` Transient on the Island, from changes and from the level keys, masked evdev (ADR 0021).
- Island keeps durable Activities.

### Later

- **Dock**: pinned (`dock.pinned`) + running unpinned + indicator; click launch/focus/next window; edge and side, magnification under the pointer, autohide, reserve or float over windows; recents later; `.desktop` override map (`windows.apps`, ADR 0014) (done, #144); merges with the Island where they share an edge and a side, `dock.merge` (ADR 0031).
- **Wallpaper**: Kanade selects; awww renders. `wallpaper set <path>`, Launcher `@` over `wallpaper.directory`; Kanade holds `awww-daemon` unless one runs already; doctor checks awww (done, #145).
- **Lock**: `ext-session-lock` + PAM (`login` service); as macOS's, date, time, user and a glass password pill over the output's wallpaper as `lock.backdrop` says ([ADR 0024](adr/0024-glass-on-every-surface.md)), no notification content; drawn and held by `kanade-lock` over a Wayland connection of its own, a surface committed per output as niri sizes it, so niri shows no red ([ADR 0025](adr/0025-the-lock-screen-over-its-own-connection.md)); `kanade lock`, which exits 0 once niri holds the lock for that call and no password was accepted since, undoing an unlock niri has not ended yet, 1 when niri refuses it, and 3 when that is not known, as while a password is checked (#196); re-locks at start on a true logind `LockedHint`; Kanade runs as the `kanade.service` user unit tied to `graphical-session.target`, `Restart=always` with a start limit; doctor checks PAM, logind and the unit (done, #154, [ADR 0018](adr/0018-lock-stays-in-the-shell.md)).
- **Idle/suspend**: on logind `PrepareForSleep` the `lock` Module locks and confirms before letting go of its `systemd-inhibit --what=sleep --mode=delay` holder (ADR 0011, under `setpriv --pdeathsig`), retaken on resume; signals only from logind's unique name, over Kanade's own zbus system connection, connected again with a backoff when lost. From `PrepareForSleep(true)` until `(false)` no password goes to PAM: the gate shuts as the signal is heard, and on each connection from logind's `PreparingForSleep`; only logind saying `false` opens it, never a lost bus or no answer; a password typed while it is shut stays in the field, shows Locked for sleep and asks logind, so a `(false)` never heard does not keep it shut. Kanade checks a password with PAM once and asks `kanade-lock` to unlock under the same lock as the gate (ADR 0025); sleep drops a check, so its late result unlocks nothing. An unlock sent just before `PrepareForSleep(true)` is asked over, and sleep waits up to logind's delay for niri to hold the lock again, so the session unlocks only for that moment. A holder that ends or cannot start is retaken with a backoff (1 s doubling to 5 min), followed by its stdout closing, not polled; `kanade status` says how it stands. Caffeine is an idle inhibitor only: it stops hypridle's idle lock, not an explicit suspend's lock. Idle locking is hypridle's (`lock_cmd = kanade lock`), documented in README (done, #155).
- **Session**: the Session Surface and `kanade session`: lock by the `lock` Module, sleep/restart/poweroff/logout via logind over Kanade's own zbus system connection, not interactive, so polkit never asks for a password typed after a Cancel; restart, power off and log out first count down 60 s as a Critical Global Preempt Session Activity with Cancel and Now, a newer one replacing it by serial; one logind call at a time, never given up on: until it answers no other is asked and no countdown starts. A call never made, as with no system bus or session, is a refusal. An error logind sends is one too, unless it passes on an answer it never got (a timeout or lost connection); one the bus sends is a refusal only if it never delivered the call (no logind, its policy or limits denied it, activation failed). Anything else is no answer (the bus's NoReply, a timeout, a lost connection, a panic): a Critical Preempt Session Activity says it may still happen, Persistent on every island with Dismiss, a countdown running is refused at once, and none is asked until Kanade restarts. A refusal closes the Session Surface, which shows none, and shows a Failed Activity with the reason, logind's if it gave one, Critical while a countdown is, Transient on the focused output for a lock or sleep, and Persistent on every island with Dismiss for a restart, power off or log out, which may have counted down unattended (done, #156).
- **Settings**: floated normal window; schema-backed overrides; pages by task, with search, Inspect (the keys) and Undo ([ADR 0029](adr/0029-settings-pages-by-task.md)); per-output keys file-only (done, #148).

## Theme

No literal theme colors in components.

- `ThemeRoles`: changing M3-like roles.
- `SemanticColors`: `privacy`, `capture`, `warning`, `critical`, `island_surface`, `on_island_surface`.
- Island semantics = black or white by `appearance.tone`, independent of the wallpaper; the Dock and Banners take the same.
- radii/spacing/type/motion = tokens too; in `src/theme.rs`, motion in `island::service::Timings` + `island::motion` (`island/` stays pure).
- v0.1 source = the wallpaper `Palette` (`src/sources/palette.rs`).
- later = matugen template; cache key = wallpaper hash + scheme + contrast + mode + matugen version.
- matugen fail → last good → black/white.

## Config

Precedence: binary defaults → `~/.config/kanade/*.toml` alphabetical → `~/.local/state/kanade/settings.toml`.

- optional per-file `schema_version`; absent = documented baseline.
- migrate each file in memory before merge.
- never rewrite user/Nix config; only GUI state auto-migrates/writes.
- GUI value == lower layer → remove override.
- tables deep-merge; lists replace; output overrides global: `[output."<name>"]` sets per-output keys only, and wins over a global value from any layer.
- unknown key → warning with file/line.
- writes = temp + rename.
- export = merged current schema.

### Hot reload

```text
file change → debounce → parse → migrate → merge → validate
                                    ↓
                  live-safe fields → apply atomically
                  restart fields   → keep + mark pending restart
                  invalid          → retain last-good config
```

`status` reports config generation, last reload error, pending-restart keys.

Schema registry (`config::Setting`, per Module) defines each setting once: key/kind/default/validation/help/restart/per-output. Drives parsing, validation, reload restart, docs (`kanade config defaults`, README), Settings.

## CLI

Public: `kanade <verb> [args]`; the runtime's IPC socket (`$XDG_RUNTIME_DIR/kanade.sock`) internal.

```text
launcher open|close|toggle
controls open|close|toggle
media open|close|toggle
island collapse
notifications open|close|toggle|clear
notifications dnd on|off|toggle
clipboard open|close|toggle|clear
calendar open|close|toggle
weather open|close|toggle|refresh|status
timer start <dur>|pause|resume|cancel
capture screenshot area|window|output
capture record start|stop|status
caffeine on|off|toggle [duration]|status
wallpaper set <path>|status
osd volume|brightness
settings open [page]|close
config reload|validate|defaults
status
doctor
module list|enable <name>|disable <name>
lock
session menu|suspend|reboot|poweroff|logout
```

`status`: protocol/config versions, module/Service state, pending restart, last errors.

`doctor`: read-only diagnostics by default: Kanade/niri versions, required protocols, sockets/IPC, module deps, D-Bus services, PipeWire, NM/BlueZ, PAM/logind, awww/matugen, capture backends, config validity. Explicit fix mode only for safe Kanade-owned state; none in v0.2. Checks land with their Modules; matugen pending. Warns about a `kanade.service` unit that is not enabled or does not run the shell it talks to.

## Runtime + dependencies

Kanade owns its runtime (ADR 0026): Wayland, windows, input, layout, drawing,
Services and IPC, all Kanade's own code in `crates/runtime` (ADR 0028). Never
patch or fork niri.

- `Cargo.toml` authoritative.
- normal workflow: `cargo run`, `cargo test`, `cargo clippy`; repo watcher may restart on save.
- public CLI stays `kanade ...`; the IPC socket is internal.
- add normal Rust crates when they deepen Kanade.
- no second shell/UI framework: no Quickshell/Qt/GTK/Iced/Slint alongside the runtime.
- a missing renderer, input or Wayland capability is built in `crates/runtime`. The lock screen waits to be folded in, over its own connection in `crates/lock` ([ADR 0025](adr/0025-the-lock-screen-over-its-own-connection.md)).
- system capabilities (per-app audio, devices, tray, clipboard, idle inhibit) → Kanade adapters over `src/bus.rs`, external tools or a non-Wayland crate (zbus for the tray). External semantics stop at the adapter. [ADR 0011](adr/0011-kanade-adapters-where-amane-lacks-a-capability.md).
- `src/` uses no `wayland-client` but in `src/doctor/` (registry globals for `kanade doctor`), enforced by `src/boundary.rs`; the runtime and the crates above own Wayland.

References, not dependencies: Suzuha = panels as one liquid shape; Noctalia v5 = native-shell/config/plugin architecture; DMS = UI/backend service split; iNiR = Niri/Island UX + deferred surfaces; Caelestia = visual/motion/launcher reference.

## Constraints + SLOs

- niri ≥26.04; no geometry-only fullscreen heuristic (ADR 0021).
- niri is the supported compositor.
- views never block; no modifier/key-up from the runtime yet → no custom Alt-Tab; the runtime repeats Backspace and the arrows from the compositor's repeat info, other keys never (ADR 0034).
- source thread or Service listener panic → logged, restarted after 5 s (the runtime restarts Services, `src/supervise.rs` sources); view panic may kill shell.
- feature deps scoped: PipeWire audio/privacy; NM/BlueZ/PPD controls; logind/PAM session; awww/matugen/hypridle/wlsunset optional; network only for enabled remote-backed features.

Targets: idle CPU <0.1%; idle frames 0; RSS <80 MB; Island morph mean frame gap = refresh interval (`scripts/frames`); source→visible <100 ms; Launcher→first key <100 ms; cold start <500 ms.

Nested-niri E2E checks idle/morph each release.

## Security/privacy

- local-user IPC only; no network listener.
- notification body plain text; action only on click; history memory-only, 20/app, 100 total.
- clipboard history bounded; persistence off by default; a selection marked sensitive is never read or kept, unknown state fails closed ([ADR 0019](adr/0019-clipboard-sensitive-content.md)).
- calendar credentials/tokens never logged; weather sends only the configured coordinates, never logged; account secrets stored through system secret service, not TOML.
- never log passwords, notification/window/media/clipboard content.
- PAM password only enters unlock path; never store/log/echo.
- polkit credentials stay external.
- privacy dots inside the Island; not shown over fullscreen (ADR 0033).
- lock shows no notification content.

Lock ship gate: kill Kanade while locked → compositor remains locked → systemd restart → lock UI reacquired → auth succeeds. Any failure → split `kanade-lock`. Passed on niri 26.04 with a temporary test build (#153): lock stays in the shell. Passed again in #154 on the production binary and the installed `kanade.service` unit ([ADR 0018](adr/0018-lock-stays-in-the-shell.md)).

## Explicit non-goals

- system monitoring/dashboard;
- i18n; English-only UI;
- fingerprint auth until hardware/testing exists;
- on-screen keyboard;
- traditional bar;
- desktop widgets;
- Overview/Mission Control replacement;
- custom Alt-Tab;
- multi-compositor support;
- integrated file manager;
- compositor config editor;
- greeter/login manager;
- package-manager replacement/updater;
- OCR/reverse-image-search/capture editor.

## Deferred decisions

Need design before commitment:

- **Plugins**: likely after Module/Service/Activity/Surface/LauncherProvider contracts stabilize. No plugin ABI/runtime yet.
- **Hooks/event automation**: decide event model, permissions, execution isolation, failure/backpressure semantics; avoid shell-script event soup.
- **ASR dictation**: speech → text; modes: copy to clipboard, paste into focused input. Need engine boundary, microphone/privacy semantics, focus-safe paste path, cancellation, offline/remote policy.

## Roadmap

- **7A Modules**: registry/deps/capabilities; migrate v0.1 features.
- **7B Contracts**: config schema/hot reload, CLI/IPC, status, doctor.
- **7C Theme**: token refactor.
- **8 Presentation**: banners + OSD; delete Island toast/OSD paths (the OSD returned to the Island, ADR 0021).
- **9 Shell essentials**: detailed Controls, tray contract, clipboard, launcher providers, capture, caffeine.
- **10 Desktop/data**: Dock, wallpaper, Settings, calendar, weather, per-output config.
- **11 Session**: lock proof, idle/suspend, session menu, power/night light.
- **12 Polish**: Material You, blur/materials, accessibility, motion, perf/E2E.

## Open

- Track/test the privacy dots at each small form and beside a side edge.
- Blur waits for a viable niri background-effect path.
- Native polkit waits for safe helper stdin integration.
- Plugins/hooks/ASR remain deferred design decisions.

## Fixed

No bar. No Mission Control clone. No geometry-only fullscreen heuristic. Privacy outside Arbiter. Activity fields orthogonal. Notification state ≠ UI. Dock consumes `windows`, not `workspace`. `audio` ≠ privacy capture monitoring. Detailed controls use macOS-style sub-surfaces. Launcher ships apps/calculator/emoji/wallpaper providers. Clipboard/capture/caffeine/calendar/weather in scope. Per-app mixer + device selection in scope. Config hot reload + doctor in scope. English-only; no system monitor/fingerprint/OSK. Kanade Cargo owns builds; Kanade owns its runtime. No second UI framework. Real TOML/serde; user config never rewritten. v0.1 Palette; matugen later. Lock in the shell process; no `kanade-lock` (ADR 0018).
