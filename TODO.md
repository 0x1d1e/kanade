# TODO

Backlog left from the `liquid-glass` branch (#208), and the full architecture review after it.

## Architecture (full review)

- [ ] Surface visit and keyboard-ring state is copied per Surface (first move)
  - Seven Surfaces keep a visit-scoped Service reset by `of(visit)`: `Focus` in session, notifications, tray and controls, `Search` in clipboard and launcher, `Browse` in calendar.
  - The four `Focus` structs repeat `visit`/`at`/`shown`/`offset`, `place`/`found`/`lost`/`ring`/`row`, the `step` skeleton, and the free fns `focus(monitor)` and `set(focus)`.
  - The copies drifted:
    - "A hidden ring: the first key only reveals it" uses a fixed key list in tray and session, and "only a key that would act or move" in controls and notifications.
    - `lost()` exists only in tray and controls.
    - Tab navigates only in notifications and clipboard.
  - Move the visit-scoped ring into `surfaces/grid.rs` (or `surfaces/ring.rs`): a generic `Ring<A>` with `visit`, `at`, `shown`, `of`, `place`, `lost`, `ring`, `row`, the reveal rule, and the fresh read plus change-only write. Each Surface's `Focus` keeps its extras (`sub`, `item`/`entered`, `offset`) and its own `At` and grid. Notifications' ragged `Place::moved` stays local.
  - Migrate one Surface at a time (session, tray, controls, notifications; calendar, clipboard and launcher take only the visit helper), deleting each copy.
  - Pick the unified reveal rule first; prefer controls' (reveal only if the key would act or move). It changes tray and session slightly.
  - Verify: one unit test of the reveal rule at the shared seam; `wtype` E2E per Surface (ring appears without acting, second key acts, a removed target hides the ring).
- [ ] niri is a hub that knows its consumers; three modules re-implement output lifecycle
  - `sources/niri.rs` pushes into Fullscreen, Privacy, Banners focus, `autohide::outputs`, `banners::outputs`, `dock::outputs`, `IslandService`, `windows::hear` and `capture::captured`, gated by the six-flag `Posts` built in `modules/catalog.rs`. The source depends upward on `banners`, `dock` and `autohide`.
  - `autohide`, `dock` and `banners` each hand-roll "forget this output when it leaves"; `autohide::Outs` and `dock::Pointers` also keep a `left` list so a late draw does not re-add it.
  - The runtime already publishes `Monitors` and `service::watch` (ADR 0035). Make it the one presence authority, each module pruning its per-output maps from a watcher registered in its own start; drop `niri::outputs()` and the `banners`/`dock` flags of `Posts`.
  - Later: one niri-state Service that Privacy, Banners and Fullscreen derive from by watching, removing the rest of `Posts`.
  - Risks: niri's output list and the Wayland monitor list are different events (the `left` lists exist for a real race); keep writes narrow, since any Service write redraws every reader (see Glass).
  - Verify: unplug and replug an output with the Dock, a Banner and autohide active; nothing stale draws, a replugged output comes back as new, idle frames stay 0. It touches lock, privacy and fullscreen paths: run the AGENTS.md lock-safety checks.
- [ ] Restart-surviving worker queue is declared four times (low-level)
  - `google.rs`, `weather.rs` and `wallpaper.rs` declare an identical `static QUEUE: LazyLock<(Sender<T>, Mutex<Receiver<T>>)>`; `island/service.rs` and `banners/mod.rs` have the `SyncSender` NUDGE variant. A small `supervise::Queue<T>` would absorb them.
- [ ] `PoisonError::into_inner` is spelled out at 147 sites; a small lock helper would replace them (low-level).
- [ ] `dock.rs` (2363 lines) holds the Island-aware geometry (about lines 1084-1480: `covered`, `island_room`, `reaches`, ...); move it to `merge.rs` only if churn there shows the cost.

Not worth changing, by this review:

- `crates/lock/src/refract.rs` duplicates the glass math on the CPU by design (ADR 0025); folding it in is on `docs/runtime-migration.md`.
- A flat `Config` with per-module `Setting` consts: the registry already defines each key once.
- `island/service.rs` size: most of it is tests.

## Architecture (#208 review)

- [x] Input and invalidation (ADR 0034, 0035; a copy redraws only its pane's readers)
- [x] The Island's scene (ADR 0036)
- [x] GPU glass (ADR 0037)
- [x] The layers in `docs/design.md`
- [x] `view.rs`, `modules.rs`, `lock.rs` and `glass.rs` split by responsibility, `view::island` into its layers
- [ ] Scene: the largest body is still one for every output (#212)
- [ ] Invalidation: `service.rs` and `changes.rs` still use process-global state.
  - `Backdrops` is scoped by pane and `Pointers` by output; no other Service is, so a write of any other draws every window that read it (see Glass).
  - `IslandService` is the one that matters, but scoping it is a resource saving (not a gate before 1.0) and not a safe one: 62 `Island::write()` sites would each have to prove they change only one output's state, as `Sizes`, the Arbiter and the overview are read by every window.
  - A write naming no part stays safe (it draws everyone), so sites can be moved over one at a time.
- [ ] Baselines: `scripts/memory`, `scripts/frames` and ADR 0004's idle wakeups were not retaken after GPU glass, which removed the glass threads and the PNG files; they stop the user's shell, so run them when it can be spared. Measurements inform, not gate.

## Lock screen (ADR 0025)

- [ ] The lock thread wakes once a minute while unlocked, for the clock.
- [ ] Each caret blink repaints the whole surface, about 6 ms of CPU.
- [ ] About 8 MB of backdrop memory per 1080p output, plus the decoded wallpaper and faces, kept while unlocked too.
- [ ] A PAM check that never returns blocks the next one (documented).
- [ ] An svg wallpaper or face is drawn by `wallpaper::decoded` at 3840 px on its longest side, a near copy of the runtime's `svg::rasterize` (256 px); share one with a size argument, and draw a face smaller.

## Settings

- [ ] `control()` keeps a `Kind::Modules` arm the Modules page never reaches.
- [ ] `humanize` repeats the schema's units and clock choices; let the schema label its options.

## Glass

- [ ] Settings windows are not glass yet.
- [ ] Regular glass is not frosted.
- [ ] The magnified Dock can reach a Banner.
- [ ] Satellites shift; the first ~80 ms of the liquid reveal are plain.
- [ ] A write of any Service but `Backdrops` and `Pointers` redraws every window that read it, not only those showing the changed part.

## Island sizes (ADR 0030)

- [ ] The Launcher's `fit` runs `found()` twice on each Search write, wheel scrolls included.
- [ ] A side-edge Banner's window is full height.
- [ ] On a side edge shared with the Island, the Dock sits past the Island's reach and a Banner's width, far off the edge.
- [ ] An upright Compact drops titles to the app's tile; a rotated, book-spine title could keep them.
- [ ] A body morphing through about square, between a wide and an upright form, overlaps its shadow's corners for a frame or two.
- [ ] On a side edge the Satellites travel about 124 px along the edge as a Compact peeks.
- [ ] The upright Rest's Peek (`time_peek_upright`, 88 px) may overflow with a 12-hour clock and the weather; an upright timer's `h:mm:ss` may pass 38 px.
- [ ] On a side edge, while a Peek holds the pill it grew from in the input region, a click beside the Peek but on the pill's old place still expands.
- [ ] The largest body is one for every output: a small output shrinks it on a large one too (#212).
- [ ] `fit` writes `IslandService` on every Search write even when the ask is unchanged, redrawing its readers.

## Island and Dock merge (ADR 0031)

- [ ] Merged Docks on the left or right of an edge are only roughly aligned with the Island's body.
- [ ] The Island's hover area and the Dock's overlap in a band; hover is notified for both.
- [ ] A Keystone's window is as wide as the icons at the largest body, and its row is laid out every frame the Island morphs.
- [ ] Many apps with a wide Surface open overflow a Keystone's window.
- [ ] `pointed()` writes `Pointers`, redrawing the Docks, and a merged Island window, of its output, on each pointer move that changes x, now in merged Crown and Keystone too.
- [ ] `Pointer.x` goes stale across a merge toggle, as its frame changes from the Dock's window to the shell's; it matters only while icons grow.
- [ ] `dock::extent` runs `widest` for every `merge::apart`, `form` and `shell` call each frame.
- [ ] Grown icons in a Keystone or Fold reach `Extent::rise` past the strip, which Banners and Surfaces below the Island do not clear.
- [ ] A Dock that autohides on the Island's edge is only brought out by the Island, not by touching the edge along its width; with `dock.reserve` it now reserves as a merged Fold.
- [ ] A non-liquid union has no border.
- [ ] Fold has no dots for running apps at rest and no keyboard focus hold; its Tray and hover Peek need a right click.
- [ ] Overview and fullscreen use the existing covered path only.

## Idle Island

- [ ] Tray strip width is the union of every output's Peek settings (`view::peek`); an output with fewer extras keeps the widest strip. Per-output shapes would need the geometry per monitor.
- [ ] The Tray strip's agenda title gets a fixed 90 px budget (`geometry::AGENDA`), so it leaves room even with no event today.
- [ ] `view::reserved` and `hides` read `Config::rests()`, which includes the global Rest even when every connected output overrides it; the strip then stays reserved while the Island hides.
- [ ] A long event title elides hard in the Tray strip's 90 px.
- [ ] No charge indication at Rest since the bolt went; `CONTEXT.md` does not say a machine with no battery counts `rest.battery` as off for a bare Rest.
- [ ] The Tray Surface shrinks to its list as a menu goes back to it, under the pointer; Clipboard does the same on a query.

## Privacy dots (ADR 0033)

- [ ] The Tray strip and Surfaces show no dots; a capture there is named only in Controls. Find room in the strip's header or a Surface's corner.
- [ ] A fullscreen window hides the dots by choice, and one `Fullscreen::covers` misses (no app id) hides them on the Top layer too; consider a niri-side hint if a capture under fullscreen proves a real gap.
- [ ] Three captures at a bare Rest with a 12-hour clock and battery may overflow `shape`.
- [ ] Dots pop in and out; fade them with the Satellite opacity curve.
- [ ] `cluster::read()` rebuilds its marks on every Island redraw while capturing, morph frames included.
- [ ] `view::hides()` ignores privacy on purpose (merge and reserve must not flicker); document it in `reserved()`.

## Launcher destinations

- [ ] The Launcher view reads `Connectivity` and `Adapter`, so every Wi-Fi or Bluetooth state write redraws it while open.
- [ ] A 2-letter query shows a long tail of settings matched on help text after the apps.
- [ ] `Mark::Icon` has no tile behind it, unlike `Mark::Tile`; check it visually.
