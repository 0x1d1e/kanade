# Kanade - design plan

Status: draft, pre-implementation. Archive as historical once v0.1 ships. Durable parts move to `CONTEXT.md` (glossary below) and ADRs.

## 1. Problem

Niri users who want a Dynamic Island style surface have two options: heavy whole-shell projects (iNiR, Ukishima/Ricelin) or small single-purpose ones (Tide). Kanade is a narrow third option: one top-center pill per monitor that shows the few live things worth attention and grows in place into four surfaces. Not a desktop shell.

Built on [Amane](https://github.com/MystiaFin/amane) (Rust, MIT, 0.1.0, experimental, Rust 2024): GPU-drawn Wayland layer shells, no QML/GTK/browser.

v0.1 target:

> A top-center Niri island built with Amane that presents media, notifications, volume/brightness, workspaces, battery, and screen capture state (mic/camera privacy if the Phase 5 spike lands), and grows into Media, Notifications, Controls, or Launcher.

## 2. Goals and non-goals

Goals
- One physical surface. Every state is that surface changing identity. No detached popovers.
- Arbitration is a pure, unit-tested core. UI only renders its output.
- Idle cost is zero: no frames, no polling redraws (`AMANE_FRAMES=1` shows nothing at rest).
- Niri-first behavior (focused output, fullscreen), not a generic adapter.

Non-goals
- Bar, dock, wallpaper, lock screen, settings app, clipboard/calendar/weather (later).
- Hyprland/Sway support. Amane supports them; Kanade does not promise them.
- Permanent Wi-Fi/CPU/RAM indicators. That turns the island into a status bar.

## 3. Facts about Amane that shape the design

From Amane docs and source (local clone `../amane`, `ARCHITECTURE.md`, `src/`), plus niri 26.04 on this machine. Items marked "unverified" are answered by the Phase 0 spike.

| Fact | Consequence |
|---|---|
| `LayerWindow`: `.width/.height`, `.anchor_*`, `.layer(Overlay)`, `.space(Zone::Ignore)`, `.keyboard(None/OnDemand/Exclusive)`, `.input_region(Vec<InputArea>)`, `.click_through()` | Fixed canvas + input region is supported directly |
| `Animation<T>` with `Easing::{Linear, Out, InOut}`, default 200 ms; costs frames only while moving | No spring. Kanade owns `motion.rs` |
| Views must not block, must not `write()` a Service they read (deadlock). Timers, I/O, D-Bus belong in Services (`new/interval/update/listen`) | Expiry, hover grace, and event edge detection live in services, never in views |
| `window_per_monitor(view)`, view gets `&Monitor { name, width, height }` | One island per monitor is free. Cross-monitor state is ours |
| `Workspaces` supports niri/Hyprland/Sway: id, index, name, output, active, focused, urgent, windows. No focused window, no fullscreen | Kanade needs its own `niri.rs` source |
| `Notifications`: daemon starts on first `read()`, only one daemon allowed (`running()`), timeouts ignored, items persist until dismissed. API: `click/invoke/dismiss/clear`. No DND, no history flag | Toast expiry, DND, and history semantics are Kanade policy |
| `Media`: MPRIS, polled 1/s, active player auto-chosen, `file://` art only (`https://` cannot be drawn) | Timeline is 1 Hz truth, interpolated locally. Art fallback needed |
| `Audio`: speaker + mic volume/mute, follows default sink, event-driven | Mic mute usable for Controls. No capture/privacy info |
| IPC: `App::ipc(name, handler(&[String]) -> String)`, socket `$XDG_RUNTIME_DIR/amane.sock`, handler runs on the draw thread, args cannot contain newlines | One handler `island` with verb args. Handlers only post, never work |
| Only one Amane shell per session | Kanade is the Amane shell. It cannot run beside another Amane bar. Document in README |
| `Workspaces` carries no window focus, fullscreen, or cast data | See Niri rows below |
| `Rectangle`: radius, border, shadow, blur, clip, rotate/scale/translate, shader | Morph visuals need no Amane changes |
| Verified in source (`src/wayland/update.rs`): `update_surface` runs every frame and re-sends changed input region and keyboard mode | Region tracks the spring each frame; keyboard mode can switch per Surface. Cost to measure: one `wl_region` per frame while moving |
| Verified (`src/services.rs`): `Service::listen()` is a free-form loop on the service's own thread; a poll that returns `false` is `quiet` and wakes nothing. `quiet()` is crate-private, so a custom `listen()` cannot make its own write quiet | Verified on niri 26.04 (#5): `IslandService::listen` blocks on `recv_timeout(next_deadline)`, nudged by a static `sync_channel(1)` whenever a deadline changes. It woke only on nudges and at deadlines (90-240 µs late), never at idle (0 wakeups in 60 s). It writes only when something is due, so a quiet write is not needed |
| Verified: no subscription between services. `read()` only subscribes windows | A source watching `Audio`/`Brightness` polls their `read()` every 50-100 ms and writes quietly unless changed. Revisit if profiling shows cost |
| Verified (`ARCHITECTURE.md`): `write()` is for input handlers and service threads, never views; IPC handlers run on the main thread | IPC verbs may call `Island::write()` |
| Verified: Amane's `compositor` module is private; its niri backend only tracks workspaces and window-to-workspace | Kanade opens its own `$NIRI_SOCKET` `"EventStream"` connection (JSON lines) |
| Verified on niri 26.04: event stream has `WindowFocusChanged`, `WorkspaceActivated`, `WindowLayoutsChanged`, `OverviewOpenedOrClosed`, `CastsChanged`/`CastStartedOrChanged`/`CastStopped`. Window JSON has no `is_fullscreen` | Focused output = output of the focused workspace. Screen capture comes from casts. Fullscreen only by heuristic (window size equals output logical size), see 5.3 |
| Verified (`cli/src/project.rs`): `amane dev` builds a generated crate around `~/.config/amane/src/main.rs` (multi-file `src/` is hashed and supported) against the unpacked library | Develop Kanade as a multi-file config `src/`. A standalone crate depending on `amane` by git rev is the fallback if the CLI constrains layout |
| Verified on niri 26.04 (#3): a still pointer that the input region shrinks away from gets `wl_pointer.leave`, 20-100 ms after Escape, while the morph is still running. Amane hit-tests hover only on pointer enter/motion/leave, so this leave is what fires `on_hover(false)` | Grace-period design holds. No fallback region needed |

## 4. Domain language

Proposed `CONTEXT.md` entries. Avoid the listed synonyms in code, tests, and docs.

### Island
The single physical surface on one monitor. One per monitor.
- **Avoid:** pill (as a type name; fine in prose for the compact shape), widget, popup, dropdown

### Activity
Something happening that may deserve attention. Has identity, Kind, Priority, Lifetime, Scope, Actions.
- **Invariant:** posting an Activity with an existing id replaces it and refreshes its lifetime (volume key repeat extends one OSD, not a queue of OSDs).
- **Avoid:** event, notification (a Notification is one Kind of Activity), OSD (use Transient)

### Lifetime
`Persistent` (lives until withdrawn: media playing, screen cast, timer, privacy, low battery) or `Transient(duration)` (volume, brightness, workspace switch, notification toast).

### Arbiter
Pure function of (registered Activities, now, focused output) to a Frame. Owns priority, preemption, expiry, satellite selection.
- **Invariant:** preemption never destroys. A Persistent Activity hidden by a Transient is shown again when the Transient expires, with no re-post.

### Frame
Arbiter output: `primary: Option<Activity>`, `satellites: Vec<Activity>` (bounded), `transient: Option<Activity>`. Global, then filtered per monitor by Scope.

### Scope
`Global` (shown on every island) or `FocusedOutput` (shown only on the island of the focused monitor). Transients are FocusedOutput.

### Presentation
Per-island visual level: `Rest | Compact | Peek | Expanded(Surface)`. Not called "state" because Amane already uses state for Services.
- **Invariant:** `Expanded` always carries a Surface. There is no surface-less expanded form.
- **Invariant:** which Surface opens is a pure function of (Presentation, primary Activity, request). No remembered last-used Surface.
- **Invariant:** at most one island is Expanded at a time.
- **Rule:** a Presentation change is geometry plus content crossfade in one motion, never collapse-then-grow.

### Surface
Full interactive content hosted by an Expanded island: `Media | Notifications | Controls | Launcher`.
- Media and Notifications are also Activity Kinds. Compact/Peek are the Activity's own small form, and Expanded is its Surface. That is the "predictable enlargement" rule.
- Controls and Launcher have no Activity. They open only by user action.

### Satellite
Small secondary indicator beside the primary island (screen cast, mic/camera, timer, VPN, critical battery). Persistent Activities only.

Presentation changes from the earlier draft: the old 5 states (`Rest, Compact, Peek, Expanded, Surface`) had two ways to be big. Collapsed into 4. Reason: with Expanded media at 440x150 and Media surface at 520x330 there were two competing "media, bigger" forms and no rule for which one a click opens.

## 5. Behavior

### 5.1 Activity model

```
Activity { id, kind, priority, scope, lifetime, interrupt, actions }
Kind     = Media | Notification | Volume | Brightness | Workspace
         | Battery | Network | Bluetooth | ScreenCast | Timer | Privacy
Priority = Critical     (low battery, privacy, call)
         > Actionable   (notification with actions)
         > Ongoing      (screen cast, timer)
         > Osd          (volume, brightness, workspace)
         > Media
         > Passive
Interrupt = Never | Transient | Preempt
```

Arbiter rules (each is a unit test):
1. Highest priority non-expired Activity is primary. Tie: newest.
2. Transient is shown over the primary for its lifetime, then the primary returns.
3. Persistent Ongoing/Critical Activities that are not primary become Satellites (cap 2; overflow collapses into a count).
4. Only `Preempt` (Critical) may displace a user-opened Expanded surface. Everything else queues as a satellite badge. A toast never steals a surface the user is using.
5. DND suppresses Notification toasts, not the Notification, and never Critical Activities.
6. If fullscreen suppression ships (Phase 5), fullscreen on the focused output: only Critical and privacy Activities render, as Satellite-sized.
7. Time is an input. The Arbiter never reads a clock.

### 5.2 Presentation transitions

```
Rest      --Activity posted-->        Compact
Rest      --click-->                  Expanded(Controls)
Compact   --hover 100-140ms-->        Peek
Compact/Peek --click / IPC-->         Expanded(primary's Surface)
Expanded  --Escape / pointer out 200-300ms grace / IPC collapse--> Compact or Rest
any       --Critical-->               Compact/Peek (Preempt policy, rule 4)
```

Which Surface opens (decided):

```
Rest click             -> Controls
Compact/Peek click     -> primary Activity's Surface
IPC / keybind          -> the requested Surface
```

Rest has no primary Activity, so "last used" would be hidden state: open Launcher once, click an idle island later, get Launcher. Controls is the neutral surface and fits "glance, act in one gesture, get out of the way". Not in v0.1: a `rest_click = controls | last_surface` config, only if asked for later.

Input:

| Input | Effect |
|---|---|
| hover | Peek |
| left click | Expand per the table above |
| right click | context / pin |
| wheel | contextual adjustment (volume on Media, etc.) |
| Escape | collapse |
| pointer out | collapse after grace |
| IPC | direct Surface or collapse |

Timings (starting values, tune in Phase 6): hover 100-140 ms, expand ~180 ms, surface change ~220 ms, collapse ~180 ms, leave grace 200-300 ms, OSD 1000-1400 ms, toast 4000-6000 ms. The hover hit area is the visible body plus a few px padding, so moving between the pill and its grown surface does not flicker.

### 5.3 Niri behavior

- One island per monitor (`window_per_monitor`).
- Transients and workspace OSD: focused output only.
- Focused output = output of the workspace with `is_focused` (from `WorkspaceActivated`/`WorkspacesChanged`).
- Fullscreen on an output: rule 6. The island is on `Layer::Overlay`, above fullscreen, so suppression is Kanade's job, not the compositor's. Deferred from v0.1: the size heuristic misfires on maximized windows, so it waits for niri to report fullscreen state over IPC. See [ADR 0002](adr/0002-defer-fullscreen-suppression.md).
- Overview open (`OverviewOpenedOrClosed`): collapse to Rest, hide transients.
- Screen capture: niri only says a cast exists, not why. `CastStartedOrChanged` posts a Persistent `ScreenCast` Activity (Satellite); `CastStopped` withdraws it. Label: "CAPTURE" compact, "Screen capture active" expanded. Never "Recording" or "Sharing" until attribution is reliable; those would be future specializations of `ScreenCast`. Concurrent casts stay one Activity (identity unchanged); a `count` field can be added later without touching identity. Mic/camera use is not covered by niri (Phase 5 spike).
- If the Niri socket is lost, degrade to "every monitor is focused", no error toast, log it.

## 6. Architecture

### 6.1 Window and input

Do not animate Wayland layer-window dimensions.

```
LayerWindow  top-center, Layer::Overlay, Zone::Ignore, 560x380 fixed, transparent
  └── island body (springs inside the canvas)
.input_region(...) = visible body rect + hover padding only
```

Why: no compositor configure round trip per frame, transparent area stays click-through, expanded room is pre-allocated, shadows have room.

Body sizes (starting values): rest ~150x32, compact ~220x38, peek ~300x52, expanded up to ~520x330. Rounded corners are approximated by the rectangular `InputArea`; accepted.

Keyboard: `Keyboard::None` normally. Escape and Launcher typing need focus, so the island uses `OnDemand` while the pointer is on the body or it is expanded (measured in #2 on niri: `OnDemand` focuses only on a press, so it must be on before the press that expands; `Exclusive` would keep the keyboard from overlays opened later, since niri gives it to the first mapped exclusive surface) and Launcher uses `Exclusive` or `OnDemand`. Amane re-sends keyboard mode per frame, so one window suffices (verified in #4: `None`, `OnDemand` and `Exclusive` switch on the same window from one frame to the next). Measured in #4 on niri: an IPC/keybind-opened Surface under `OnDemand` gets no Escape, since no press ever focuses it, so it holds `Exclusive`. The hold lasts until collapse, not the first pointer interaction: niri moves focus back to the window on any switch from `Exclusive` to `OnDemand`, even right after a press on the island, so releasing early loses Escape. While held, other windows get no keys until Escape, `collapse`, or the pointer leaving the body after entering it.

Recorded as [ADR 1](adr/0001-fixed-canvas-input-region.md).

### 6.2 Modules

Deep modules, small interfaces. The earlier tree split by Presentation (`compact.rs`, `expanded.rs`) and kept a `services/` mirror of Amane. Both removed: Presentation is data, not a file axis, and Amane services are read directly.

```
src/
  main.rs              App wiring: window_per_monitor, IPC handler, font
  island/              the core. Interface: post, withdraw, input, frame
    activity.rs        Activity, Kind, Priority, Lifetime, Scope
    arbiter.rs         pure: (activities, now, focus) -> Frame
    presentation.rs    pure: per-island state machine
    geometry.rs        Presentation + Frame -> target rect, input region
    motion.rs          critically damped spring
    service.rs         Amane Service: owns Arbiter + per-monitor Presentation,
                       deadlines via listen(), hover grace
  sources/             produce Activities from system state (policy lives here)
    media.rs notifications.rs osd.rs battery.rs workspace.rs
    niri.rs            own EventStream: focus, overview, fullscreen heuristic, casts
    timer.rs privacy.rs
  surfaces/            view code only: media, notifications, controls, launcher
  view.rs              body + satellites rendering from Frame and Geometry
  theme.rs
```

Why these seams:
- `island/` is deep: callers know four operations and get priority, preemption, expiry, return-after-transient, per-monitor routing. Deleting it spreads that into every source and surface.
- `sources/` earns its place by edge detection and mapping ("volume changed" becomes a Transient, "playing" becomes Persistent Media, "toast expired but notification remains"). It does not wrap Amane services one-for-one. Surfaces call `Audio::read()`, `Media::read()` directly.
- No trait for the Niri source: one implementation, no second in sight. The Arbiter takes a plain `focused_output` value, so tests need no Niri.
- `sources/` code never lives in a view. Sources post through `Island::write()`; views only read.

Dependency direction: `sources` and `surfaces` depend on `island`. `island` depends on nothing in Kanade.

### 6.3 Data flow

```
Amane services + Niri/timer/privacy
        │ (source threads: listen/interval)
        ▼
     sources  ── post/withdraw ──▶ IslandService { Arbiter, Presentation[monitor] }
                                          │ Frame + Presentation (read, per window)
IPC / hover / click / keys ── input ─────▶│
                                          ▼
                              geometry ─▶ motion ─▶ view (+ input_region)
```

### 6.4 Motion

Own critically damped spring, closed form (no integrator jitter): `x(t) = target - (A + B t) e^(-wt)`. Retargeting mid-flight keeps position and velocity, so a changed mind never jumps.

- One spring group animates width, height, radius together (same `w`), so geometry is one object. Content opacity and translation derive from progress, not separate timers.
- Frames are requested only until the spring settles (`request_frame()`), then the window rests.
- Upstream `Easing::Bezier` or spring to Amane later; not a v0.1 dependency.
- Reduced-motion config: snap geometry, 80 ms opacity only.
- Never: collapse old, fade out, resize, fade in new.

## 7. UX requirements

Task: glance at live state, act in one gesture, get out of the way.

- Attention budget: only Critical, privacy, and actionable notifications interrupt. Media never interrupts. Toasts never take keyboard focus.
- Color communicates state, not decoration: neutral, media accent from artwork, ScreenCast amber/orange, battery low amber (critical red). Green is reserved for mic/camera privacy if that Kind ships, since green reads as camera/permission. Red is reserved for critical. Never color alone: ScreenCast shows the ▣ glyph and "CAPTURE" text, battery shows a number. That also separates the two amber cases.
- Visual: near-black or palette-derived body, high-contrast foreground, one accent, subtle shadow, no heavy glass. Quiet at rest.
- Every Surface defines these states: empty (Media: "Nothing playing"), loading/partial (art not loaded: placeholder, no layout shift), error (notification daemon not running: surface says so and names the conflict), disabled (Controls item unavailable).
- Keyboard: every Surface reachable via IPC and a compositor keybind; Escape collapses; arrow keys and Enter work in Notifications and Launcher; focus visible; Launcher focus exists only while Launcher is open.
- Targets in Expanded >= 24 px, text >= 12 px.
- Screen reader support: unverified, likely none (GPU drawn, no accessibility tree). State as a known limitation in README until checked.
- Layout is fixed per Presentation: long titles elide, never resize the body.

Surfaces (v0.1):
- Media: art, title/artist, timeline, prev/play/next, volume, player selection. Timeline interpolates locally between 1 Hz polls, and only while visible.
- Notifications: newest as compact Transient, actions when expanded, dismiss, history, DND, Critical preempts.
- Controls: Wi-Fi, Bluetooth, volume, mic, brightness, DND, power profile. Not a settings app.
- Launcher: search + `Apps`. Keyboard focus only while open.

IPC (`amane ipc call island <verb> [args]`): `open <surface>`, `toggle <surface>`, `collapse`, `dnd toggle`, `timer start <duration>`, `timer stop`. Unknown verb returns usage text. Handlers only post to `IslandService`.

## 8. Alternatives considered

| Alternative | Why not |
|---|---|
| Resize the layer window per Presentation | Configure round trip per frame, jitter, input and shadow clipping |
| Separate popup windows per surface | Breaks the one-surface goal and spatial continuity |
| Port Ukishima/iNiR instead | Whole-shell scope. Wrong size for the problem |
| One central `Island` controller type that knows every Kind | The `DynamicIslandWindow.qml` monolith. Arbiter + sources keeps Kind knowledge at the edges |
| Trait for compositor backend now | One compositor, one implementation. Add when a second exists |
| Upstream a spring to Amane first | Blocks on a 0.1 library. Local `motion.rs` is replaceable |

## 9. Verification

Unit (pure, injected time):
- Arbiter: priority order, tie by newest, same-id replacement extends lifetime, transient then return, expiry boundaries, satellite cap and overflow, rule 4 (no displacement of Expanded except Preempt), DND, focused-output scope, and (if fullscreen suppression ships) fullscreen filter.
- Presentation machine: every transition in 5.2, Expanded always has a Surface, single Expanded island.
- Geometry: input region is always inside the canvas and equals body rect plus padding, for every Presentation and mid-spring value.
- Motion: no overshoot, settles, retarget keeps velocity continuous.

E2E (nested Niri session, `amane dev`):
- Idle: `AMANE_FRAMES=1` prints no frames at rest and after every transition settles.
- Click-through: pointer outside body reaches the window below.
- Volume key during Spotify: OSD ~1.2 s, media returns.
- Notification toast during Expanded Media: no displacement. Critical battery: displaces.
- If fullscreen suppression ships (see 5.3): fullscreen video hides the island except Critical. Also test the heuristic against a maximized borderless window for false positives; that check gates shipping.
- Satellite overflow: ScreenCast + mic + timer shows 2 satellites plus a "+1" count.
- Two monitors: transient on focused only; hot-unplug and replug.
- Screenshot every Presentation and Surface and inspect pixel by pixel (padding, radius, clipping, elision, fallback art). Fix visual defects found even if unrelated.
- Timing under 60 and 120 Hz: no stutter on expand/collapse.

Lint and tests stay green at every phase.

## 10. Phases

Each phase ends with its acceptance criteria met. No phase starts on unanswered unverified items it depends on.

**Phase 0 - spike (answers a question, then throwaway or hardened deliberately)**
Pill, hover, click, 32 px to 440x160 morph, input-region tracking, Escape.
Answers: pointer-leave delivery when the region shrinks; Escape focus for keybind-opened Surfaces; per-frame region cost during a spring; `listen()` with `recv_timeout` deadlines; layout under `amane dev`. (Per-frame region/keyboard updates and service semantics are already verified in Amane source, see section 3.)
Accept: smooth 60/120 Hz, no pointer blocking outside body, no resize jitter, zero idle frames.

**Phase 1 - shell**: `island/` presentation, geometry, motion; rest/compact/peek/expanded; multi-monitor; `niri.rs` focused-output routing; IPC.

**Phase 2 - activity engine**: Arbiter, expiry, preemption, return-after-transient, satellites. Test heavily before wiring any source.

**Phase 3 - system activities**: Media, Audio, Brightness, Battery, Workspace, Notifications, Network/Bluetooth sources.

**Phase 4 - surfaces**: Media, Notifications, Controls, Launcher with all states from section 7.

**Phase 5 - desktop-native**: ScreenCast, Timer, Mic/camera privacy, fullscreen suppression (waits on niri, ADR 0002).

**Phase 6 - polish**: tune motion and timings, album-art transition, satellite morph, keyboard navigation, themes, config, reduced motion, performance.

Later (after core is excellent): calendar, clipboard, weather, screen recording controls (starting captures; detection ships in v0.1), VPN, power menu, visualizer.

## 11. Risks

| Risk | Mitigation |
|---|---|
| Amane 0.1.0 breaks | Pin git rev; keep Amane calls in `main.rs`, `view.rs`, `sources/`, `surfaces/`; `island/` is Amane-free except `service.rs` |
| Pointer-leave lost when region shrinks | Answered in #3: niri delivers it. If another compositor does not, keep the region at the larger of current and target during collapse grace |
| Another notification daemon running | Notifications surface shows the error state; README says to stop mako/dunst |
| Screen capture is covered by niri casts; mic/camera has no source | Phase 5 spike on PipeWire streams. Mic/camera privacy may slip out of v0.1, ScreenCast does not |
| Niri exposes no fullscreen flag | Checked in #13: the heuristic misfires, so rule 6 is deferred until niri IPC reports fullscreen ([ADR 0002](adr/0002-defer-fullscreen-suppression.md)) |
| Source polling wakes the CPU at idle | Prefer `listen()`; any poll returns `false` unless changed |
| Island nags | Attention budget in section 7 is a review gate for every new Kind |

## 12. Open questions

1. Phase 0 items in section 3.
2. Mic/camera detection source (PipeWire streams vs portal)?
3. Can a cast be attributed (recorder vs share) reliably, e.g. via cast target or the requesting app? If yes, split `ScreenCast` into `Recording` and `ScreenSharing` later.

Decided: Rest click opens Controls (section 5.2). Satellite cap is 2 plus an overflow count (rule 3). Niri casts are `ScreenCast`, not "Recording" (section 5.3).

## 13. Documentation plan

- `README.md`: what it is, install (Amane deps, Niri, one-Amane-shell limit, single notification daemon), run, IPC verbs, known limitations.
- `CONTEXT.md`: glossary from section 4 once the first code lands.
- ADR: fixed canvas + input region. Add one for the spring only if it stops being replaceable.
- `AGENTS.md`: verification commands, "never `write()` in a view", "Arbiter takes time as input".
- This file: archived after v0.1. Do not keep editing it to match the code.

## 14. Sources

Borrowed ideas: iNiR (one body, many contexts, satellites, Niri-first), Ukishima/Ricelin (in-place morph, transient vs persistent, media UX, hover grace), Tide (lightweight, explicit IPC), Apple Live Activities (compact to minimal to expanded, coexisting activities, alert only on important change), lunanoir dynamic-island (privacy and timer staying compact). Avoided: whole-shell scope, giant central controller, phone gestures copied literally, constant attention requests.
