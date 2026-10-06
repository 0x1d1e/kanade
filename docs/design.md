# Kanade design

## Objective

macOS-style niri shell, Rust on Amane ([ADR 0008](adr/0008-kanade-is-a-shell.md)). No bar. One Island/output: status at rest, Activities live, Surfaces on demand. ~zero idle cost.

Sources of truth: tracker = work/status; `CONTEXT.md` = domain; ADRs = durable rationale; this doc = target architecture/constraints. Where this doc changes a `CONTEXT.md` term or invariant, `CONTEXT.md` changes in the PR that implements it; until then `CONTEXT.md` describes the code. History: `docs/archived/plan.md` (v0.1 plan).

## Product rules

- Island replaces bar/menu/taskbar.
- Niri owns layout, focus, Alt-Tab, Overview. No Mission Control clone.
- No desktop widgets/full DE.
- English-only UI.
- Surfaces keyboard-complete; reduced motion supported.
- Privacy default on; mic/cam/capture visible over fullscreen.
- Every feature module disableable. Disabled = no windows/subscriptions/IPC/side effects; unread Services stay cold.
- awww renders wallpaper. External polkit agent for now.

## Domain

- **Island**: top-center shell surface/output.
- **Rest**: idle Island; shows the clock.
- **Split**: Compact showing the primary and the top Satellite in one split body; each segment peeks and opens its own Activity ([ADR 0010](adr/0010-split-and-rest-right-click.md)).
- **Activity**: live item eligible for Island slots.
- **Satellite**: Persistent Ongoing/Critical Activity that is not the primary; cap `SATELLITES`, rest a count.
- **Arbiter**: Island selection policy.
- **Surface**: interactive content of an Expanded Island.
- **Privacy cluster**: separate Overlay indicator; never Activity.
- **Module**: user-disableable feature/interface.
- **Service**: Amane `Service`; lazy shared state/integration source.
- **LauncherProvider**: query → ranked launcher results/actions.

## Architecture

```text
OS / niri / D-Bus / PipeWire / files / network
                    ↓
             adapters + Services
                    ↓
                feature modules
              ↙              ↘
         Arbiter/Island    other windows
```

One Kanade process using Amane by default; security-sensitive code may split. Views do no I/O. External semantics stop at adapters/Services; policy stays in owning module. Services may be shared; avoid wrapper-per-dependency architecture.

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
| Overlays | `privacy`, `banners`, `osd` |
| Surfaces | `controls`, `launcher`, `notification-surface`, `clipboard-surface`, `calendar-surface` |
| Utilities | `capture`, `caffeine`, `doctor` |
| Desktop | `dock`, `wallpaper` |
| Session | `lock`, `session`, `settings` |

Rules:

- hard dep missing → disable + named error; soft capability missing → degrade.
- Island Surfaces require `island`.
- `notification-surface` → `island` + `notifications`; `banners` → `notifications`.
- `dock` → `windows`, never `workspace`.
- `osd` optionally reads `audio`/`brightness`.
- `privacy` reads capture state independently from `audio` and `capture`.
- `privacy` default on; disable warns indicators disappear.
- `calendar` network/account sync optional; local calendar still works without it.
- `weather` network-backed; no polling when disabled; bounded refresh when enabled.
- module toggle requires restart while Amane window registration is startup-only.

`notifications` owns bus/state/history; UI separate. `windows` owns normalized niri running-window/app state. Dock never consumes raw niri objects.

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

**Fullscreen:** no detection/suppression. Geometry heuristic rejected. Island = Top; fullscreen may cover it. Privacy = separate Overlay. Revisit only with reliable niri fullscreen state. [ADR 0006](adr/0006-island-on-top-layer.md), [ADR 0005](adr/0005-privacy-indicator-outside-arbiter.md).

**Rest:** minimal configured status. Tray appears on interaction. Left click → Controls. Right click → Controls pinned; no context menu ([ADR 0010](adr/0010-split-and-rest-right-click.md)).

**Split:** follows the Frame like Compact: Split while a Satellite exists, Compact without. Hover/click/right click act on the segment under the pointer; Peek shows one Activity by identity. [ADR 0010](adr/0010-split-and-rest-right-click.md).

## UI

### Core Surfaces

- **Launcher**: providers = apps, calculator, emoji, wallpaper. Focus only while open. Provider seam internal; no public plugin API yet.
- **Controls**: macOS pattern. Top-level quick tiles/sliders; detail sub-surfaces for Wi-Fi, Bluetooth, audio.
- **Wi-Fi**: scan, connect/disconnect, network selection, state/error.
- **Bluetooth**: scan, pair, connect/disconnect, device selection, state/error.
- **Audio**: master volume/mute, input/output device selection, per-app mixer, mic state.
- **Notifications**: history, actions, dismiss, clear, DND, empty/error.
- **Clipboard**: text + image history, search, copy, delete, clear; bounded; memory-only default; optional persistence later.
- **Calendar**: month/agenda; local sources; optional Google Calendar account sync.
- **Weather**: current + short forecast; explicit location/config; bounded refresh; stale/error state.

### Tray

StatusNotifierItem behavior:

- icon/status/title updates;
- primary activation;
- secondary/context activation;
- scroll events;
- menus/submenus/actions;
- malformed/dead item isolated, never shell-wide failure.

### Capture

`capture` initiates; `privacy` independently observes actual capture.

- screenshot: area crop, window, fullscreen/output;
- prefer niri-native capability where sufficient; otherwise external capture backend;
- recording: external GPU-accelerated recorder backend; start/stop/status;
- completion may emit Activity + open/copy path action;
- no OCR, reverse-image search, annotation editor, upload service.

### Caffeine

Idle inhibitor toggle/activity. Prevent idle/lock/suspend only while explicitly active; visible state; optional timeout.

### Presentation migration

[ADR 0007](adr/0007-banners-and-osd-leave-the-island.md).

- notification toast → **Banner**: focused output, top-right; 4 s low, 6 s normal, critical sticky; hover pause; max 3.
- volume/brightness/mic transient → **OSD** Overlay.
- Island keeps durable Activities.

### Later

- **Dock**: pinned + running unpinned + indicator; click launch/focus; recents later; `.desktop` override map.
- **Wallpaper**: Kanade selects; awww renders.
- **Lock**: `ext-session-lock` + PAM; no notification content.
- **Session**: lock/sleep/restart/poweroff/logout via logind; 60 s destructive countdown.
- **Settings**: floated normal window; schema-backed overrides.

## Theme

No literal theme colors in components.

- `ThemeRoles`: changing M3-like roles.
- `SemanticColors`: `privacy`, `capture`, `warning`, `critical`, `island_surface`, `on_island_surface`.
- Island semantics = black/white, independent of mode/wallpaper.
- radii/spacing/type/motion = tokens too; in `src/theme.rs`, motion in `island::service::Timings` + `island::motion` (`island/` stays pure).
- v0.1 source = Amane `Palette`.
- later = matugen template; cache key = wallpaper hash + scheme + contrast + mode + matugen version.
- matugen fail → last good → black/white.

## Config

Precedence: binary defaults → `~/.config/kanade/*.toml` alphabetical → `~/.local/state/kanade/settings.toml`.

- optional per-file `schema_version`; absent = documented baseline.
- migrate each file in memory before merge.
- never rewrite user/Nix config; only GUI state auto-migrates/writes.
- GUI value == lower layer → remove override.
- tables deep-merge; lists replace; output overrides global.
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

Post-v0.1 schema registry defines setting once: key/type/default/validation/UI metadata/restart/per-output. Drives parsing, Settings, docs, validation.

## CLI

Public: `kanade <verb> [args]`; Amane IPC internal.

```text
launcher open|close|toggle
controls open|close|toggle
media open|close|toggle
island collapse
notifications open|close|toggle|clear
notifications dnd on|off|toggle
clipboard open|clear
calendar open
timer start <dur>|pause|resume|cancel
capture screenshot area|window|output
capture record start|stop
caffeine on|off|toggle [duration]
config reload|validate
status
doctor

# later
module list|enable <name>|disable <name>
lock
session menu|suspend|reboot|poweroff|logout
settings open [page]|close
wallpaper set <path>
osd volume|brightness
```

`status`: protocol/config versions, module/Service state, pending restart, last errors.

`doctor`: read-only diagnostics by default: Amane/niri versions, required protocols, sockets/IPC, module deps, D-Bus services, PipeWire, NM/BlueZ, PAM/logind, awww/matugen, capture backends, config validity. Explicit fix mode only for safe Kanade-owned state; none in v0.2. PAM/logind, awww/matugen and capture checks land with their Modules.

## Runtime + dependencies

**Decision:** keep Amane. Kanade owns Cargo/build. Amane = UI/Wayland/rendering/input/window/IPC library, not build-system owner.

- `Cargo.toml` authoritative; pin Amane revision.
- normal workflow: `cargo run`, `cargo test`, `cargo clippy`; repo watcher may restart on save.
- `amane dev/compile` generated manifest not authoritative.
- `amane ipc call` may remain internal; public CLI stays `kanade ...`.
- add normal Rust crates when they deepen Kanade.
- no second shell/UI framework: no Quickshell/Qt/GTK/Iced/Slint alongside Amane.
- do not directly add low-level Wayland/render deps for behavior Amane should own. Extend Amane first; bypass only when seam belongs to Kanade.
- Amane owns Kanade's Wayland runtime/UI seam. Direct Wayland deps only for isolated diagnostics/preflight that implement no shell behavior: `wayland-client` lives only in `src/doctor/` (registry globals for `kanade doctor`), enforced by `src/boundary.rs`.
- switch from Amane only if it blocks core invariant: privacy Overlay, secure lock, focus/input, zero-idle rendering, required protocol access.

References, not dependencies: Suzuha = Amane full-shell proof; Noctalia v5 = native-shell/config/plugin architecture; DMS = UI/backend service split; iNiR = Niri/Island UX + deferred surfaces; Caelestia = visual/motion/launcher reference.

## Constraints + SLOs

- niri ≥26.04; no geometry fullscreen heuristic.
- Amane + niri = hard architecture deps.
- views never block; no modifier/key-up → no custom Alt-Tab; no text key-repeat yet.
- source thread or Service listener panic → logged, restarted after 5 s (Amane restarts Services, `src/supervise.rs` sources); view panic may kill shell.
- feature deps scoped: PipeWire audio/privacy; NM/BlueZ/PPD controls; logind/PAM session; awww/matugen/hypridle/wlsunset optional; network only for enabled remote-backed features.

Targets: idle CPU <0.1%; idle frames 0; RSS <80 MB; Island morph mean frame gap = refresh interval (`scripts/frames`); source→visible <100 ms; Launcher→first key <100 ms; cold start <500 ms.

Nested-niri E2E checks idle/morph each release.

## Security/privacy

- local-user IPC only; no network listener.
- notification body plain text; action only on click; history memory-only, 20/app, 100 total.
- clipboard history bounded; persistence off by default; sensitive-content policy required before persistence.
- calendar/weather credentials/tokens never logged; account secrets stored through system secret service, not TOML.
- never log passwords, notification/window/media/clipboard content.
- PAM password only enters unlock path; never store/log/echo.
- polkit credentials stay external.
- privacy Overlay fullscreen-visible while module enabled.
- lock shows no notification content.

Lock ship gate: kill Kanade while locked → compositor remains locked → systemd restart → lock UI reacquired → auth succeeds. Any failure → split `kanade-lock`.

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
- **8 Presentation**: banners + OSD; delete Island toast/OSD paths.
- **9 Shell essentials**: detailed Controls, tray contract, clipboard, launcher providers, capture, caffeine.
- **10 Desktop/data**: Dock, wallpaper, Settings, calendar, weather, per-output config.
- **11 Session**: lock proof, idle/suspend, session menu, power/night light.
- **12 Polish**: Material You, blur/materials, accessibility, motion, perf/E2E.

## Open

- Track/test privacy Overlay stacking + input passthrough.
- Define Dock `app_id` ↔ `.desktop` matching + override shape.
- Define capture backend selection + capability probing.
- Define Google Calendar account adapter/auth/storage.
- Define clipboard sensitive-content policy before persistence.
- Run lock crash proof; split on failure.
- Blur waits for viable Amane/niri background-effect path.
- Native polkit waits for safe helper stdin integration.
- Plugins/hooks/ASR remain deferred design decisions.

## Fixed

No bar. No Mission Control clone. No geometry fullscreen heuristic. Privacy outside Arbiter. Activity fields orthogonal. Notification state ≠ UI. Dock consumes `windows`, not `workspace`. `audio` ≠ privacy capture monitoring. Detailed controls use macOS-style sub-surfaces. Launcher ships apps/calculator/emoji/wallpaper providers. Clipboard/capture/caffeine/calendar/weather in scope. Per-app mixer + device selection in scope. Config hot reload + doctor in scope. English-only; no system monitor/fingerprint/OSK. Kanade Cargo owns builds; Amane stays library/runtime. No second UI framework. Real TOML/serde; user config never rewritten. v0.1 Palette; matugen later. Separate lock process only if crash proof requires it.
