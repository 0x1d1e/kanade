# TODO

Backlog from the full architecture review.

## Architecture (full review)

- [x] Keyboard ring shared by Session, Tray, Controls and Notifications (`surfaces/ring.rs`, `surfaces::store`, controls' reveal rule)
  - Notifications embeds `Ring` for its state and keeps a local, equivalent reveal because its ring re-seats by `At::place`.
- [x] Surface visit state shared by the visit-scoped Searches and Browse (`surfaces/visit.rs`)
  - `grid::moved` takes Tab, so Session, Tray and Controls read on row by row and wrap. Notifications, Clipboard and Calendar keep their own Tab.
  - `Focus::of` in the Ring-based Surfaces still repeats the reset; the trait does not fit its shape.
- [x] Output presence: `Monitors` is the one authority; `autohide`, `banners` and `dock` prune from a watcher in their own start (#214)
  - Not run: the unplug and replug with the Dock, a Banner and autohide active (one output on the dev machine). Nothing stale should draw, a replugged output should come back as new, idle frames stay 0.
  - Watchers see only the latest list, so an output destroyed and created again between two derives keeps its state, a stale hover included.
- [x] niri is a Service its readers watch (`sources/niri.rs`, ADR 0038)
  - Fullscreen, Privacy, Banners, the core and the workspace switch watch `Niri` in their own start; windows and screenshots take niri's events by `niri::on_windows` and `niri::on_screenshot`. `Posts` is gone.
  - `niri.rs` still imports `island::activity::Workspace` (the DTO a `Focused` holds), the one upward import left; move it into the source if full independence is the goal.
  - Not run: losing the stream (restart niri). It should clear the dots and the shown windows, and they stay cleared: the stream is not reconnected (`supervise::run` only restarts a panic; a lost stream ends the source for good, which `losing_the_stream_degrades_to_every_monitor_focused` pins). Restarting Kanade brings it back.
- [x] Restart-surviving queues and poisoned locks: `kanade_runtime::queue::{Queue, Nudge}` and `MutexExt::locked`, `RwLockExt::{reading, writing}` (`unpoisoned.rs`)
  - Not folded in: `media.rs`'s `Feed`, which hands a clone of its sender to the bus watches while it is made; the two `Condvar` waits; `crates/lock`, which depends on no Kanade crate and has its own CPU renderer.
- [x] The Island's content turning inert or swapping (#210): runtime targets carry a key (`input::nest`, by place in their containers) and `Pointer` follows hover, press and drag by it, not by position
  - A hovered target that turned inert or went away still gets its exit (the pointer keeps the handler last seen), and every redraw path (frame, resize, scale) sets the cursor again (`refresh_cursor`).
  - A key is the place among siblings, so a container whose children come and go before another target must keep its own place (the Island's body takes a placeholder for each layer that is not there).
  - `Rectangle::identity` names a list item by what it is (notification cards and Banners by id, Dock icons by app or, with none, window, Controls rows by SSID, device path and node, Tray items and menu entries, Clipboard entries, Launcher results), so items that come and go above it do not hand it a neighbour's key. Two different widgets given one identity are one target.
  - Not taken: an old and a new form at the same place have the same key (a press begun on one and let go on the other is a click); hover is not re-evaluated after a redraw until the pointer moves.
- [ ] `dock.rs` (2524 lines) holds the Island-aware geometry (about lines 1084-1480: `covered`, `island_room`, `reaches`, ...); move it to `merge.rs` only if churn there shows the cost.

Not worth changing, by this review:

- A flat `Config` with per-module `Setting` consts: the registry already defines each key once.
- `island/service.rs` size: most of it is tests.
- On a side edge the Satellites travel with the body's height as a Compact peeks: they hang from the body's end.
- On a side edge, while a Peek holds the pill it grew from in the input region, a click beside the Peek on the pill's old place expands: the pill is still where the pointer rested.
- The lock thread's backdrops and faces kept while unlocked (ADR 0025): the first lock frame needs them. The decoded wallpaper is not kept.
- A PAM check that never returns blocks the next: PAM cannot be cancelled, and checks run one at a time by design (ADR 0025).

## Architecture (whole-codebase review, second)

- [x] One atomic file write: `atomic::write` with `Mode::{Shared, Private, Exactly}` (settings, reminders, recents, appearance adapters, calendar mirror)
  - Not folded in: `raster::store` (a cache of hundreds of pictures; the fsync would cost startup) and the pane shader (it locks the file before naming it).
- [x] One `Backoff` (`backoff.rs`) for logind, seat, sleep, tray, wake, night light, battery, weather and Google. Google now retries a failed sync after 1 min, doubling to 15 min, not a flat 15 min.
- [x] `sources/json.rs` is `serde_json::Value` with `parse`, `array` and `quote`; the hand-rolled parser is gone.
- [x] `sources/mirror.rs` is the calendar-sync file mirror for Google; `clock::unix_seconds` replaces four copies.
- [x] A Service's panic reaches `kanade status` (`service::on_panic`, set by `supervise::follow_services`); the restart delay is one constant.
- [ ] 191 `/* */` blocks run over four lines and 137 runs of `//` lines do (counted over `src/` and `crates/`, interior lines). Each states real constraints, so cut them one by one, not in bulk. The ten once-longest are cut: `crates/runtime/src/graphics/gpu/drain.rs:21` (now 3), `src/sources/wifi.rs:685` (3), `crates/runtime/src/graphics/gpu/flush.rs:9` (3), `src/sources/calendar.rs:1148` (3), `src/sources/recording/plan.rs:249` (3), `src/sources/recording/encode.rs:266` and `:253` and `src/sources/recording/plan.rs:288` (3 each), `src/surfaces/launcher/calculator.rs:1` (2), `src/shadow.rs:1` (3) and `src/dock.rs:1456` (2). Earlier counts and citations were wrong: `encode.rs:244` is a `match` arm and `dock.rs:1342` a `dock_strip(` call, not comment blocks; `view/mod.rs:729` is inside `#[cfg(test)]` and `calendar.rs:1087` a `.padding(..)` call.
- [x] ADR 0003 still says "std-only JSON parser"; history, left as written.

## Architecture (whole-codebase review, third)

- [x] Slow CLI calls (lock, caffeine, battery limit, wallpaper, recording) are answered by the shell when done, or unknown after 5 s (`later.rs`); the five serial-numbered `settle_*` protocols and the CLI's 50 ms status polling are gone.
  - A shell that restarts mid-call now reads as "closed the connection without answering" (exit 3), not the old per-feature "lost" messages.
  - `capture record start` and `resume` still poll for the first frame's file every 50 ms, only while the request is open (at most 5 s); wf-recorder has no event for it (ADR 0116).
- [x] One bounded-child runner for the appearance adapters and Matugen (`appearance/child.rs`); the adapters' output is now read as it is written and capped at 1 MiB.
- [x] Modules carry `hooks.status` and `hooks.reload`; `kanade status` and a config reload walk the Modules that are on. `modules::on("<name>")` stays string-keyed at its other 60 call sites; `boundary::module_names_are_in_the_catalog` fails on a name the catalog lacks.
  - Not done: replacing the central `Call` enum with a per-Verb `run`; `kanade status` lists the Modules' lines in catalog order, not the old hand-kept order.
- [x] "plan N" references in code comments now say the fact; `docs/archived/plan.md` stays history.
- Not taken: a shared remote-sync loop for Google and weather. Only the three-way `recv` / `recv_timeout` wait is common; what a timeout means, the due test and the message set differ, so a helper would need as many parameters as it saves lines.
- Not taken: a `Target` table for the appearance adapters' `apply` and `disable`; their sequencing differs per target (Zen's marker block, GTK's restore state).
- [x] `Image::blurred`, the loader's blur key and `graphics/image/soften.rs` are gone (no caller after ADR 0049).
- [x] Dead after ADR 0049: `shadow.rs` still cuts each piece where the body covers it, which an opaque body no longer needs.
  - Premise no longer holds: ADR 0057's Blur made the pane translucent again (`pane::BLUR_FILL`), so the cut is live. Keep it.
- [x] A plain (unmerged) pane needs no shader: a `Rectangle` fill drops the pane shader file's dependency on `XDG_RUNTIME_DIR` for the common case; only the merged outline needs it.
## Architecture (#208 review)

- [x] Input and invalidation (ADR 0034, 0035; a copy redraws only its pane's readers)
- [x] The Island's scene (ADR 0036)
- [x] GPU glass (ADR 0037; removed by ADR 0049)
- [x] The layers in `docs/archived/design.md`
- [x] `view.rs`, `modules.rs`, `lock.rs` and `glass.rs` (now `pane/`) split by responsibility, `view::island` into its layers
- [x] Scene: the largest body is per output (#212)
- [ ] Invalidation: `service.rs` and `changes.rs` still use process-global state.
  - `Pointers` is scoped by output; no other Service is, so a write of any other draws every window that read it.
  - `IslandService` is the one that matters, but scoping it is a resource saving (not a gate before 1.0) and not a safe one: 62 `Island::write()` sites would each have to prove they change only one output's state, as `Sizes`, the Arbiter and the overview are read by every window.
  - A write naming no part stays safe (it draws everyone), so sites can be moved over one at a time.
- [x] Baselines retaken 2026-10-11 (`docs/memory.md`, #198; idle wakeups 7.1/s against ADR 0004's 7.5/s, 0 idle frames, island morph gap 7.7 ms against a 6.94 ms refresh interval)
  - Memory grew with GPU glass (removed by ADR 0049; retake the baseline): 495 MiB of GPU buffers and 312 MiB of RSS at Rest, against 225 and 150 before. The vello and wgpu upgrade adds about 26 MiB of PSS (heap, and `libnvidia-rtcore`).
  - 8% of the island window's frames came later than 1.5 refresh intervals while morphing, and the slowest took 48 ms of view, draw and gpu.
  - Not taken: more than one monitor. Not run (they unplug the output or restart niri): #214's unplug and replug with the Dock, a Banner and autohide active, and #216's lost niri stream.

## Panes (Satellites as one liquid shape, ADR 0101)

- [ ] Not seen: the neck as a Satellite comes out and tucks back (tinted, paper, blur; a dot bringing in a second one; a dot sliding to another slot). Tune `pane::blob::REACH` by eye: the neck lasts the last quarter of a dot's travel.
- [ ] Banners do not join the Island's body: they are a window of their own (ADR 0023).

## Panes (Blur finish, ADR 0057)

- [ ] Not verified: windows behind a pane blurred (non-xray). It needs the niri layer rule `xray false` for `kanade`, `kanade-dock` and `kanade-banners` (`dist/niri/config.kdl`), which the E2E session's niri config lacks and an agent may not add. Without the rule, E2E on niri 26.04 showed the wallpaper blurred and a window's text under the Island hidden (xray), as niri's docs say. Check with the rule: text behind the Island, Dock and a Banner blurred, not hidden, and the fill's 78% (`pane::BLUR_FILL`) readable over a white window and a dark one.
- [ ] Tune by eye once windows blur: `pane::BLUR_FILL`, and whether a flip to dark ink over a bright window (ADR 0054's) is wanted; none reads the page under Blur.
- [ ] Not taken: the wakeup, frame-gap and memory baselines with Blur; the cost of non-xray blur in niri while a body morphs and a page animates behind it. Idle frames were 0 over 20 s at rest on the Island, with Blur and no rule, but not rechecked with it.
- [ ] Not run: the fallback with a compositor that lacks the global or its blur capability (the opaque tinted solid, the doctor warning); and niri's `blur { off }`, where the capability may still say blur.
- [ ] A Banner stops blurring the moment its fade out begins and starts when its fade in ends; the compositor's blur does not fade. A Dock fading aside for an open Surface (`aside`) likewise.
- [ ] The shadow's corner pieces reach under a body's corners, so a translucent pane shows them faintly; cut them to the outline for Blur if seen.
- [ ] Every size change of a body sends a new region (rows of rects, about the body's height in the worst case); coalescing or a cheaper cut is possible if a morph shows slow.

## Panes (glass removed, ADR 0049)

- [x] The magnified Dock no longer reaches a Banner: `dock::strip` includes the grown icons (ADR 0030).
- [x] A write of any Service but `Pointers` redraws every window that read it: see Invalidation under Architecture (only `IslandService` is per output, and it is not safe to scope in one pass).
  - Duplicate of the Invalidation item above; this one is the cross-reference.

## Island sizes (ADR 0030)

- [x] A side-edge Banner's window is full height.
  - `banners/mod.rs:315` already sets `monitor.height as f32` when `hang.sideways()`.
- [ ] An upright Compact drops titles to the app's tile; a rotated, book-spine title could keep them.
- [ ] An upright Rest and Compact have one length each (`geometry::rest`, `compact`), so a form that needs less (the timer) or more (a Rest with the battery) is padded or tight; the shape could take the primary Activity's Kind and the Rest's battery setting.
- [ ] A body morphing through about square, between a wide and an upright form, overlaps its shadow's corners for a frame or two.

## Idle Island

- [x] The strip's agenda width (`view::resize`, `rest::Agenda`) arms the minute timer when Calendar is on and any connected output shows the agenda, even if no window draws the time; an ended event stops taking room on the next minute.
  - Not run: hidden-clock agenda expiry in the live session.
- [x] The Tray Surface shrinks to its list as a menu goes back to it, under the pointer; Clipboard does the same on a query.

## Privacy dots (ADR 0033)

- [ ] A fullscreen window hides the dots by choice, and one `Fullscreen::covers` misses (no app id) hides them on the Top layer too; consider a niri-side hint if a capture under fullscreen proves a real gap.



## Acknowledgements features (review, low-level, unfixed)

- [x] Keys and Wallpapers stat the files on each frame they draw (`binds()`, `wallpaper::images()`); cache by mtime if it shows: resolved (premise no longer holds) — both sites already cache scan results against filesystem metadata modification timestamps (`src/sources/binds.rs:94-118` via `READ`, `src/sources/wallpaper.rs:768-790` via `LISTED`); each frame checks mtimes with a stat to detect changes, while file reads, KDL parsing, and directory re-scans are fully short-circuited.
- [x] The image loader keeps queued thumbnails scrolled past; a thumbnail is keyed by path only, so a picture replaced in place shows the old one until restart: resolved — path-only keying already resolved in commit ff56942 (`Stamp` in `Key`), so replaced files reload; queued thumbnails waiting for a worker are now bounded (`QUEUED = 64`) with longest-unused eviction from both `waiting` and `entries`, and `trim()` counts finished copies rather than loading ones so fast scrolling does not grow unbounded or flush the ready cache.
- [x] `night_light::turn` rereads the calendars it does not need: resolved in commit 2c50f08 by running `modules::reread()` only when module enablement changes (`next.off != running.off`).
- [x] Reminders: MORE's `of` grows with each reminder joined; a deleted event's reminder stays until dismissed or due: resolved in commit c58bf5a via `fn stands()` and candidate reconciliation (`src/sources/reminders.rs:612`, `:1128`).
- [x] Clipboard and Launcher search look duplicated with `src/surfaces/search.rs`: Clipboard's search field now uses `search::field(Icon::Search, ...)`, while Launcher's field retains its selection highlight and in-query cursor placement over shared `search::tail`.
- [x] A dmenu reply to a client that stopped reading may block the UI thread up to 1 s (`IpcReply::send`): resolved in commit ff56942 by dispatching replies on a dedicated thread in `IpcReply::send` (`crates/runtime/src/ipc.rs:63`).
- A three-letter layout label may be cramped in the OSD capsule; Controls grew 290 to 330 px, check Tray, Weather and Session by eye.
- [x] The critical battery countdown names no battery reason line: resolved in commit a2c9d5a via `Cause::Battery(percent)` which renders "Battery at {percent}%" in both `forms.rs` and `surfaces/session.rs`.
- [x] The idle dim stays dark while a lock that failed is asked again; a click to wake reaches the window under the dim: resolved via Action::Undim in Schedule::lock_failed when dimmed and overlay::undim in idle::act.
- A dmenu started in the background holds the keyboard until answered (as rofi does); Escape always reaches it.

- Calendar Surface: no undo for a removed event, and recurrence, place and a reminder cannot be typed (a file that holds them keeps them, ADR 0113); a change is read, checked and written in separate steps, so another writer between them wins.
- [x] Calendar Surface: after "Changed elsewhere" the open field keeps its old baseline until Escape; the pointer hold on an event's Remove shows the key wording "Keep holding d to remove": resolved via `add::reread`, which the save's end calls to retake the event as its file now reads or drop what it cannot say (`src/surfaces/calendar.rs` `settled`), and `confirm::pointer`, which the footer asks so a pointer hold names no key (`src/surfaces/confirm.rs`).
- Text fields: [x] an open Calendar edit field survives the Surface collapsing and reopening (pre-existing): resolved via `surfaces::calendar::start` watching `IslandService`, which drops the open field the moment the Surface is not showing (`src/surfaces/calendar.rs` `follow`). A `kanade ui press` is followed by the Surface collapsing after about 0.5 s (pre-existing).

## Design audit (not done)

- No screen reader support (AT-SPI tree); an architecture change, see `docs/wiki/Limitations.md`.
- No UI text scale setting; type sizes are fixed tokens.
- The Session Surface treats Lock and the counted-down Restart, Power off and Log out alike in weight; the countdown with Cancel is the guard.
- ADR numbers skip 0045.

- Capture Surface: a failed screenshot is only logged to stderr, as the island is already closed; a failed recording start now shows a Failed Activity (ADR 0116).

- Settings: the header title sits at the window's left padding while the page column is centered; in a window maximized under the Island a long title runs under it.
- Settings: stepper step sizes are fixed per kind, not adaptive to the range; its `-`/`+` buttons are pointer-only (Up/Down cover the keyboard).

## External audit triage (a6f29f3)

Unfixed, no visual change wanted. Fixed in the same pass: notification `image-data` sizes checked against the bytes sent, font fallback skips unparseable files, dead backdrop mip pyramid removed.

- [x] `Atlas::place` defers a glyph that would wrap rather than overwriting it: `Placement::{Letter, Nothing, Overflow}` returns `Overflow` where the row would wrap, `batch.rs` pushes a `Deferred` entry, and `Quads::draw` re-places it once the batch has drawn, so the order is safe and terminates (a letter larger than the atlas is `Nothing`, not deferred).
- [x] `ipc` reads and runs its handlers inline (1 s read timeout) and `request_frames` followed every call. The read stays off the draw thread and the handlers stay on it (moving them is a separate, riskier item), but the redraw is now the call's own answer: `ipc::draws` classifies the verb from its words before the handler runs, `Draws::Yes` only for `settings`/`close-settings`, `gesture draw|record`, `osd` and `brightness set|up|down`, and a `ui` act needs no frame of its own, since it draws the one window it reached through that window's `request_frame_on` and a `ui describe` or `ui wait` draws nothing. A verb whose change rides a Service write or `open_window`/`close_window`'s `mark_all` draws nothing extra, so `kanade status`, `module list`, `config validate`, `lock status`, `idle status`, `ui describe`, `ui wait`, `activity list`, `activity pressed` and `brightness status` draw no frame at all (measured: 15 calls, 0 frames).
- [x] `update_surface` no longer commits before `present` does: it applies the layer settings and reports that a commit is owed, and `redraw` commits them itself only when it presents no frame (unconfigured or hidden, a view with no root, or a skipped `Gpu::draw`), since a presented frame's own commit carries them. A hidden window showing again still gets its buffer-less commit, which is what makes the compositor configure it (a Banner appearing was checked on the session).
- [x] A property-only change still resends every setting through `layer::apply`; `Settings` compares whole and could send only the fields that differ.
- [x] `open_normal` runs the view once only to read title and size: `View::normal` evaluates the view once to read title and sizing properties and stores the resulting `Window` and `Reads` in `View::Normal`'s initial state; the first frame run reuses this retained `Window` and restores its service reads, while subsequent redraws re-evaluate `view()`.
- [ ] `Brightness` polls every 500 ms and the trimmer wakes every 10 s (ADR 0004 knows them); `osd` and `notifications` still spawn `pactl subscribe` and `dbus-monitor` and settle-poll.
- [x] `elide` and the `tail` helpers re-measured from the start per removed letter (quadratic).
  - Premise no longer holds: there is one `fn tail` (`src/surfaces/search.rs:83`) and it halves into the cut that fits, one `Text` measured per probe, so it shapes the text O(log n) times rather than once per removed letter; `clipboard.rs` and `launcher/mod.rs` both call `super::search::tail` (`src/surfaces/clipboard.rs:653`, `src/surfaces/launcher/mod.rs:1364`).
  - `fn elided` accumulates the advances once over the line (`crates/runtime/src/widgets/text/lines.rs:130`), pinned letter for letter against eliding one at a time by `elided_one_letter_at_a_time` (`:182`).
- [x] `disc_angle`'s `DISC` was one process-wide state, so two monitors showing the disc advanced it twice per refresh.
  - Premise no longer holds: the angle is keyed by output name, so each output turns only its own, pinned by `a_disc_on_one_output_turns_only_its_own_angle` (`src/view/media.rs:491`).
- [x] `Reveal`'s `HONORED` is a `HashMap` keyed by each scroll area's own name, so a serial one area used does not hide the same serial in another (`crates/runtime/src/widgets/reveal.rs`).

## From the trend-improvements review (unfixed polish)

- [x] Lock: a hung PAM module holds the one check at a time (no watchdog) by design (one at a time, ADR 0025); `Dark` in `crates/lock/src/wake.rs` never expiring is intentional per ADR 0065 (reset on lock begin/end, first wake input swallowed to light screens); resolved concrete defects and test gaps — `kanade status` prints the grace only when `modules::on("idle")` and `idle.lock > 0` (`status_lines`), and verifies PAM file presence (`pam().is_some()`); `clean` strips Unicode format characters (`is_format`); `Kind::PamService` validates `lock.pam_service` as a service file name and rejects paths; the interleaving model in `src/lock/mod.rs` covers `grant` and `touch`.
- [x] Lyrics: a paused track gets no local `.lrc`; a `.lrc` is read only beside a `file://` track, so one named like a network stream's URL is never found; lyric text keeps control and bidi characters: resolved — a paused track reads its local beside `.lrc` or cache answer without network fetch (`src/sources/lyrics.rs` `resolve`); lyric text strips control, format and bidi-override characters while preserving `\t` (`words`, `is_format`). Network streams have no beside-path by design (ADR 0063 Consequences); in `lrclib` mode they resolve from cache or LRCLIB, while `local` mode deliberately confines lookups to files on this machine.
  - Premise no longer holds for the other two: the rate maths is total now (`until()`/`moved()` with a checked `next_second`) instead of dividing by `div_f64`, and the cache's `.part` files are swept.
- [x] Calendar subscriptions: feeds are fetched one by one (16 dead hosts hold `Reread` for minutes): resolved — fetches already ran on bounded worker threads (`IN_FLIGHT = 4`), but a result was applied by vector index and a dropped entry leaked its in-flight slot; feeds now carry an id that survives a reread, results apply by it (`apply_fetched`), and a finished feed mirrors and publishes at once (`src/sources/subscriptions.rs`). Each keeps up to 5 MiB resident (still open); ureq honors proxy environment variables, undocumented (still open).
  - Premise no longer holds for the start jitter: a feed's first read is staggered across the interval for previously kept feeds, while concurrent in-flight dispatch handles due feeds without serial pile-up.
- [x] Wallpaper: `showing_on(Output::Named)` no longer falls back to the last image Kanade set; `known()` answers from awww alone, and only `Output::All` still falls back when awww reports nothing (`src/sources/wallpaper.rs:472`).
- [x] Focus: `status()` no longer lists expired caffeine, `night_light::turn(true)` that saved but did not reload is recorded in `Held`, and a user's DND off-then-on during Focus no longer looks untouched (`turns_off` compares the exact revision; `src/sources/focus.rs`).
- [x] Notifications: four or more updating progress notifications rotate the three shown Activities: intentional design trade-off per ADR 0059 ("Past 3 notification Activities the oldest is withdrawn for it, so a flood of ids shows only the newest 3 and a copy that is not among them loses its Activity until its next version"); `NOTIFIED = 3` deliberately caps notifications to 3 of 8 total Live Activity slots (`src/sources/live.rs`) so external clients and hooks always retain at least 5 slots.

## Semantic tree (#21, ADR 0095)

- [x] The surfaces this item named are already annotated, `.on_click`/`.on_hold` sites against `.semantics(` sites: banners/mod.rs 4/3, surfaces/clipboard.rs 3/7, calendar.rs 4/7, wallpapers.rs 2/4, tray/mod.rs 6/7, media.rs 4/6, pad.rs 2/3, controls/list.rs 4/5, controls/arrange.rs 3/4, controls/audio.rs 1/3, settings/view.rs 9/16. Timer, Capture and Session draw through `pill` (`src/surfaces/pad.rs:297`) and the Controls switches and profile segments carry their own (`src/surfaces/controls/mod.rs:845`, `:1122`), so annotating those files again would regress them. A live `kanade ui describe` names the Island, the Dock's items and the nine Controls nodes, and `wait 'app#2' --window kanade-dock` resolves; a node answers by its id, not a `name` field (`src/ui.rs:54`, `crates/runtime/src/ui.rs:257`), and `TextInput` targets annotate themselves (`crates/runtime/src/widgets/text_input/targets.rs:58`).
  - The one pressable left unnamed is the Dock context menu's action rows, which were never on this list (`src/dock/context_menu.rs:639`); the other `on_click` rectangles without semantics of their own wrap the annotated control or dismiss (`src/dock/context_menu.rs:349`, `src/gesture/mod.rs:663`, `src/settings/view.rs:3084`).

## From the research-improvements review (unfixed)

- [x] Session hooks: the wait on a `logout`, `reboot` or `poweroff` hook is capped at `hooks.timeout` plus slack, and an ending session takes the lower `ENDING` ceiling (`src/hooks.rs:594-599`), so a hung hook no longer delays the action.
- [x] D-Bus signals are checked for their sender: the portal's `SettingChanged` (accessibility) goes through `trusts()` and the Unity `LauncherEntry` signal (badges) through a `Senders` owner cache, fail-open on an unowned or invalid name and fail-closed only on a differing owner (a strict equality would break Chromium's progress).
- [x] DDC: `monitors()` re-reads sysfs on each call and resolves each connector's `ddc` link through `canonicalize` to its `i2c-dev` node, so a replugged monitor's new bus number is found (`src/sources/ddc.rs:169`).
- [x] Banners: `reply_field(monitor, pane)` interns one name per output and pane, so two outputs showing a reply-capable Banner at once keep separate drafts (`src/banners/mod.rs:570`).
  - The residual is small and different: each `(monitor, pane)` pair leaks a `&'static str` into a never-evicted map, bounded by outputs times panes.
- [ ] E2E not reached: IME against a live fcitx5/ibus (none installed), the lock-screen layout label (needs two layouts), hold-to-confirm for Session, portal reduced-motion, `focus.auto` with a real cast. DDC/CI works on a Dell P1917S when the bus is given, but an NVIDIA connector has no `ddc` link, so no monitor is found: a fallback matching the connector's EDID to the `/dev/i2c-*` buses is the fix.

## appearance.sync and the GTK file chooser

- [ ] Not run on a live session: change the wallpaper or `appearance.tone` with `appearance.sync` on and a GTK file dialog reopened takes the new theme; `systemctl --user show -p ActiveEnterTimestamp xdg-desktop-portal-gtk` and `systemctl --user show -p ActiveEnterTimestamp xdg-desktop-portal-gnome` move, once per burst and at most every 30 s; with a unit stopped, it stays stopped. Only those two units are restarted, never `xdg-desktop-portal`. No `kanade doctor` note.

## Launcher commands and dictation (ADR 0111, 0112, low-level, unfixed)

- [x] Dictation's `paste` copies the raw `text`, so a multi-line dictation keeps its line breaks on the fallback path; only `Insert::Type` is flattened through `insert::typed` (`src/sources/dictation/run.rs:623`).
- [ ] A recording stopped just before the session locks can capture audio until the recorder exits (at most the 3 s kill timeout); documented in ADR 0111.
- [ ] Launcher decayed frecency (ADR 0127) keeps up to 300 items and 20 learned queries each; not verified live on an all-day session.

## Live actions (ADR 0110, review, low-level)

- [x] `activity wait` and `run --action cancel` polled every 250 ms: each ask was a connect and an IPC on the draw thread, about 4 shell wakeups a second while anyone waited. Fixed for `wait` (`pressed-wait` holds one call the shell answers when a press arrives, `src/sources/live.rs`, `src/cli.rs`); `run --action cancel` still polls every `POLL` for the command's lifetime, as it must also watch the child itself.

## Notification sounds (ADR 0118, review, low-level, unfixed)

- [x] The stderr note for a layer that cannot be used prints for every notification, even one dropped because a sound already plays (`cue()` emits the notes before the `PLAYING` check): dedupe or rate-limit it.
  - Premise no longer holds: `says_once` (`src/sources/sound.rs:71`) says each note once, up to `SAID_MOST` (64) different ones, and `play` passes every note through it (`:229`).

## Recording targets, audio and pause (ADR 0116, unfixed)
- [ ] Recording quality and encoder choice (ADR 0116, Update): quality verified live: pending Iris. Not verified through `wf-recorder`: `-F`, `-p rc_mode=CQP -p qp=N`, the color `-p` tags (read back as BT.709 limited from libavcodec given the same options, not from a `wf-recorder` file), `-d`, NVENC and software; the runtime software fallback; the window path's probe and elements, and `va<node>postproc` with `va<node>h264enc` on a second VA-API node. A live run only: it needs the session, the GPUs and real footage, and nothing here is decidable or fixable from the code.
  - [x] Fixed: a start's 5 s wait no longer contains the probe — the trial is 1.5 s inside a 3 s budget (`sources/recording/encode.rs:12-13`), and a finished probe re-arms each waiting call's deadline (`sources/recording.rs:367`, `:1324`).
  - [x] Fixed: a vanished output is not retried at all — `again` refuses `Fault::Vanished`, which `vanished` decides from the reason, so neither the EGL step nor software is started on it (`sources/recording.rs:1433`, `:2134`).
  - [x] Fixed: the trial encodes with the recorder's own rate control — `rate_control` feeds the trial and the recorder alike, so it proves CQP (`sources/recording/encode.rs:200`, test `a_trial_encodes_with_the_recorders_codec_and_filter`).
- [x] Window recording (ADR 0117).
  - [x] Fixed: a window's own cast, which carries no pid, is left out of the privacy dots when the window-cast ledger holds its session, so Kanade's recording no longer shows beside the Recording Clip (`sources/niri.rs:383`, `sources/recording/window.rs:472`).
  - [ ] Not verified live: niri's Stop sharing on a window cast (is the recording saved?), a lock during a window cast, `mic`, a HiDPI window, a window that resizes, A/V sync, the Capture Surface's Rec Window pill, the Launcher's Record Window row, the "Saved, window closed" Clip, the watchdog killing a stuck encoder.
  - [x] Live: a stop with no frame written. Screen path passes — a static screen held 12 s stopped in 0.025 s, the file surviving at 11.97 s / 165 frames, so the 5 s `LATER` limit is not hit. The window path fails: `start` returned exit 3 "no frame was written within 5s" yet a 433 KB / 130-frame file still appeared, so `Segment::Empty` and `discard_empty` never fired — see the broken-window-recordings item above.
  - [x] Fixed: a graceful exit ends a window cast it holds — `main.rs` calls `recording::shutdown`, which abandons the segment and stops the cast (`main.rs:84`, `sources/recording.rs:1693`).
  - [ ] Iris saw ffmpeg warn of a non-monotonic dts at the join of a paused window recording (two packets with one pts). Not reproduced with synthetic fragmented segments joined by `-f concat -c copy` (clean), so the last fragment's final packet probably has no duration in a real one, and the join offsets the next segment by it. Predates the quality branch (the join is ADR 0116's, the fragmented segments ADR 0117's). A live run only: read a real pair with `ffprobe -show_packets`; no code change can settle it.
  - [ ] **Window recordings on this machine are effectively broken (Iris live, 2026-10-20, commit 8a6213c).** `capture record start window <id>` yields a file whose duration is a small fraction of wall time, and nothing is on disk until `stop`: 27 s wall → 2.08 s / 730 frames, 23 s wall → 1.72 s / 610 frames. During the run `ffmpeg` holds no output file (`/proc/<pid>/fd` shows no mp4) and both `gst-launch-1.0` and `ffmpeg` keep `wchar`/`rchar` frozen. `start` always hits the 5 s `LATER` budget ("no frame was written within 5s", exit 3) even on a window animating at 30 fps, because `written` is a file-size check (`sources/recording.rs:1585`) and the window path writes nothing until the muxer finishes.
    - The screen path is unaffected: it goes through `wf-recorder`, and its `kill -9` leaves a playable 8.04 s h264 1920x1080 file.
    - A crashed window recording leaves **nothing** — no file, no `.parts` folder — so there is no footage to recover.
    - Attribution is not settled. Measured: `pw-cat --record --target <cast node>` gets 1.1 MB from the same node, so niri does deliver frames; the identical gst chain by hand (`pipewiresrc path=<live node 89> … ! fdsink`) yields 0 bytes for 12 s; the same chain on `videotestsrc` streams 23 MB, so `vah264enc` is not the blocker. Two things to check first: `path=` is **deprecated** on `pipewiresrc` in favour of `target-object=` (`gst-inspect-1.0 pipewiresrc`), and the shell log shows only `capture: encoding with h264_vaapi` with no software-fallback line, so `falls_back` never fired — it only fires on `Segment::Lost` (`sources/recording.rs:1504`), and a chain that hangs without failing is never `Lost`.
  - [ ] A window's frames reach `ffmpeg` in bursts (4 frames, then ~134 ms) with VA-API, an average of 30.8 fps: it stamps by arrival (`-use_wallclock_as_timestamps`), so the encoder's batching shows in the file's times. Predates the quality branch (ADR 0117). Stamp from `pipewiresrc`'s own times if the cast gives usable ones, or smooth the arrival.
  - [x] **Fixed: `videorate` was what broke window recordings — removed from the chain, verified live.** `fae1e04` put `videorate ! video/x-raw(ANY),framerate=30/1` at `src/sources/recording/plan.rs:277` to pace a still window, and never amended ADR 0117. `pipewiresrc` offers no **fixed** input framerate (niri's cast carries none, and nothing fixated one before the element), and fed caps that fix none, `videorate` passes a single frame and then bursts the rest at `stop`. Lab, 10 s each: `videotestsrc is-live=true ! video/x-raw,framerate=0/1 ! videorate ! framerate=30/1 ! fdsink` gave 614 KB / **one frame**; the same source without `videorate`, 221 MB; a fixed `30/1` input into `videorate`, 184 MB. It was not the element but the caps it was given.
    - Live before: 729/1489/1460 frames in 2.45/5.02/5.02 s files, a file rate of **291-297 fps** against ~27 fps of real content, nothing on disk until `stop`, and `start` always hit its 5 s budget (exit 3).
    - Live after (Felix, `03ad9a2`, niri's own cast, VA-API): an animated window 20.01 s wall → **20.17 s / 609 frames / 30.2 fps**, playable, the file growing throughout (299 KB at 5 s, 1.2 MB at 20 s) and `start` answering in 0.96 s; a 25.18 s take → 25.10 s / 2916 frames; a still window 24.01 s wall → **24.28 s / 30 frames** and 23.7 s / 44 frames, i.e. few frames at a correct duration, which is VFR working as ADR 0117 decided. `kill -9` mid-recording now leaves 8.13 s of playable footage (before: nothing), and the next start reclaimed the leftover cast.
    - The chain is `pipewiresrc path=<node> keepalive-time=1000 resend-last=true ! video/x-raw(ANY),pixel-aspect-ratio=1/1 ! <encoding chain> ! h264parse ! fdsink`. `PACED` is gone; `SQUARE_PIXELS` stays (it fixes the resize border bug below). A test on `plan::encoder` asserts the square-pixel caps are there and that no `videorate` or `framerate=` caps are, so the element cannot come back unnoticed.
    - ADR 0117 now records the decision: **no pacing element, and a source whose framerate is not fixed MUST NOT go through `videorate`**.
  - [x] Fixed: the window path has no watchdog for a chain that hangs without failing: `falls_back` fires only on `Segment::Lost` (`sources/recording.rs:1504`), so a pipeline that emits nothing but never exits is never retried with the software/EGL chain and never reported. A still window's `start` hits the 5 s `LATER` and exits 3, but nothing abandons or restarts the segment. Wanted: treat "no byte written within `start`'s budget" as a fault worth the same fallback `Segment::Lost` gets.
    - The defect was real and the prescribed fix was **wrong**: a still window's file legitimately holds zero bytes for many seconds. Fed one buffer a second through `x264enc` into `plan::muxer`'s exact arguments, the mp4's first byte appeared at **12.9 s**, and `-probesize` at 65536, 4096, 64 and 0 changed neither that nor the final size, so a byte-count rule inside `start`'s 5 s budget would abandon healthy takes. `pipewiresrc` also cannot emit before a first buffer arrives (`keepalive` copies `last_buffer`, and there is none), so pre-first-byte silence cannot tell a cast delivering nothing from a chain waiting. What was done instead: GStreamer's `watchdog` element watches **buffer arrival** and is placed before the encoding chain, with `QUIET = 3 s` (`encode::QUIET`, `Encoding::guard`, `plan::encoder`), so a trip is a `Fault::Stalled` that reaches ffmpeg as no mp4 and walks `falls_back` as any lost segment does. A window that closed trips it too and is read first, as a recording saved. Already in `docs/wiki/Limitations.md`: a window niri never damages now ends its segment after `QUIET` instead of waiting at `start`'s exit 3. Landed as `22b9319` and `09649b4`, merged at `6acad11`. **NOT RUN live**: a wedged `vah264enc`, a cast that stalls mid-take, an off-screen window; lab only, synthetic producers. The why is ADR 0117's "Update: what a silent window chain means, and what ends it".
  - [x] Fixed: a stop's own SIGKILL of an encoder that wrote frames is `Written`, not `Lost` — `escalated` with `written` passes `classify_window`, and only the encoder's own failure is blamed (`sources/recording.rs:2188`, `:2107`).
  - [x] Fixed: the stderr join is bounded — `join_within` waits no more than `SAID_JOINED` (1 s) for a descendant that inherited the pipe (`sources/recording.rs:1768`, `:2081`).
  - [x] Fixed: the element names are asked of GStreamer — `probe::inspect_va` runs `gst-inspect-1.0 va` (cached, 2 s) and `Va::is_first` reads its answer; the node-name guess is the fallback only when the inspection is empty (`sources/recording/probe.rs:261`, `sources/recording/encode.rs:484`).
  - [x] Fixed: `downloading` has no `Probed::Unknown` arm that passes a trial as working — an unanswered `gst-launch-1.0` trial is a failure with its reason (`sources/recording/probe.rs:444`).
  - [x] Fixed: the walk is a pure `step(given, ran, fault)`, which `falls_back` calls and the test walks, so a dropped `no_va`/`software` transition fails it (`sources/recording.rs:1473`, test `a_failed_start_gives_way_once_per_step_and_never_loops`).
  - [x] Fixed: the software arm's log names the muxer when the muxer was what failed (`sources/recording.rs:1539`). The muxer's reason is still kept in `take.software`, whose only reader renders the words "software encoding" (`:964`), so it is not shown either way.
  - [x] Fixed: `follow_window` blames the encoder only when the writer succeeded, so a muxer that died first costs no EGL restart (`sources/recording.rs:1980`).
  - [x] Fixed: `Empty` needs the writer to have succeeded and printed nothing or said the stream had nothing, so it no longer hides a real writer error (`sources/recording.rs:2203`).
  - [x] Fixed: a reused pid's ledger is taken over on start — `window::inherit` reads `window-casts.<pid>`, reports it and clears it, and `stale` collects only other-pid ledgers whose owner is gone, so a live Kanade's ledger is left alone (`sources/recording/window.rs:337`, `:385`). The ledger was always `window.rs`'s, not `recording.rs`'s.
  - [x] Fixed: `THIRD_PARTY_NOTICES.md` names `vendor/vello_encoding` in the "vendors no other third-party code" sentence.
  - [x] The size is niri's logical size, fixed at start; a resize letterboxes.
    - Intended, and said in the code: every frame is fitted to the size the window had at the start and `add-borders` keeps the aspect, so a window resized mid-recording stays whole in the frame and the pause segments join (`sources/recording/plan.rs:179`, `sources/recording/encode.rs:246`).
  - [x] Fixed: "Saved, window closed" needs the window really gone — `take.closed` is set only where niri says the window is absent (`sources/recording.rs:1993`, `:2262`), so a cast that ends with the window still there cannot say it closed (`view/forms.rs:324`). No neutral wording was needed, since that path no longer ends as a saved clip; what it does end as needs a live run.
  - [x] Fixed: `default_sigint` unblocks SIGINT in its `pre_exec` as well as resetting its disposition, so an encoder started while Kanade's mask blocks SIGINT still takes the interrupt (`clock.rs:584`).
  - [ ] The clip's duration includes the first-frame latency (0.2-2 s).
  - [ ] The same-second NOREPLACE collision for window recordings (`-n`) is not verified live.
  - [ ] A `kill -9` of Kanade during a real window recording leaves a playable fragmented mp4: verified on a synthetic pipeline only, not live.
- [ ] A system and microphone mix (one source at a time now).
- [ ] Kanade's own region overlay in place of `slurp`.
- [x] Live: `wf-recorder` takes `--audio=@DEFAULT_MONITOR@` and `--audio=@DEFAULT_SOURCE@` — both produced a real `aac 48000 2ch` track (7.79 s and 5.98 s). Their **content** is NOT RUN: this machine's system-audio and mic paths carry no signal (`-91.0 dB`, and a direct `pw-record` from the same source is equally silent), so nothing about audio levels is proven. Area geometry passes: `area "100,50 640x480"` gave exactly 640x480 / 5.01 s, and a screen recording on HDMI-A-5 gave 1280x1024.
- [ ] Each resumed segment pays the recorder's first-frame delay (the file appears only at the first frame).
- [x] Fixed: the start's reclaim sweeps leftover parts folders — `sweep` moves a lone segment back to the `.mp4` (or joins them), and a failed join puts the first segment back (`sources/recording.rs:2337`, `:2484`). The folder is `.<stem>.parts` beside the file now, not `.Screencast from <stamp>.parts` (`:866`); no test walks the leftover path.
- [x] Fixed: a `slurp` that ignores SIGTERM is escalated — `escalate_picker` kills it after `STOPPING` (5 s) if it is still the one running (`sources/recording.rs:1640`).
- [ ] The Launcher's recording rows read the recording's standing (Rec) once per frame: they are recomputed on each fit, key, wheel and render, so a recording that ends while it is open shows at most one frame late.
- [ ] Not verified live: that the Failed Activity (preempting, for a start that fails) is seen over the Capture Surface when `slurp` is missing; a failure from a join or a recorder that ends does not preempt.
- [x] Live: `capture record stop` on a static screen does **not** wait past the CLI's 5 s reply — a still screen held 12 s stopped in 0.025 s, the file surviving at 11.97 s / 165 frames, so the damage-driven next frame arrives in time.
- [x] Live: a paused window recording's join excludes the pause instead of splicing it in — 4 s record, 5 s pause, 4 s resume gave two segments in `.parts` joined to one 8.02 s file, 346 frames, clean decode.
- [x] Fixed: a join no longer outlasts a waiting call — `Effect::Join` answers with the file before the join runs when there is more than one segment, and a failed join refuses only whoever waits after it (`sources/recording.rs:280`; the join itself is still bounded by `JOINING`, `:78`).
- [ ] Not run: the Surface, the Launcher, the lock rule and the area picker on a session (no E2E in the change).

## Backlog (user, 2026-10)

- [x] Liquid glass: update it to match Vitreus (reference checkout at `/home/k/Projects/acknowledgements/Vitreus/`: `liquid.wgsl`, `cardrip.wgsl` and its README). A reference to learn from, not to copy; if code is ported, the license and acknowledgements rules apply (THIRD_PARTY_NOTICES.md).
  - Abandoned by the user; not done. The branch `feat/glass-frost-light` is kept.
- [x] Settings window: change it to GPUI (now the runtime's own UI).
  - Done before it was written: `src/settings/view.rs:10-21` draws with `kanade_runtime`'s own widgets (`Button`, `TextInput`, `ScrollArea`, `Row`, `Column`, `Window`), and the window opens through `kanade_runtime::open_window` (`src/settings/mod.rs:320`) on the runtime's one Wayland connection (`crates/runtime/src/wayland.rs:270`), beside the lock's (`crates/lock/src/client.rs:225`) and no other. Zed's `gpui` would be a second toolkit, a second event loop and a second Vulkan device in the one process, which ADR 0026 and 0028 exist to prevent; the item was written six days after the port (`e8d49d5`), and its "(now the runtime's own UI)" is the port itself. What the runtime genuinely lacks for Settings is nothing: every `config::Kind` has a runtime control. Reopen only if the motive is accessibility, and the answer then is an AccessKit or AT-SPI adapter over `crates/runtime/src/ui.rs`, not a toolkit swap.
  - Needs an ADR, and a decision on how it coexists with the runtime of ADR 0028.
- [x] niri config editor: edit niri's `config.kdl` from Kanade's settings (ADR 0119).
  - Slice 1 done: the text engine and `kanade niri-config validate|get|set|remove|undo|backups|restore` (byte-span splices, `niri validate` before the replace, backup ring, undo; symlinks, include-owned nodes and repeated nodes refused).
  - Slice 2 done: the Settings `niri` page (Layout, Focus ring and border, Input, Keyboard, Touchpad, Mouse, Cursor, Other) over the engine's new batch `apply`, with a worker thread, Reset, include-owned rows read-only and the footer's Undo. Not run on the session: the page itself (needs a live niri; the paths and extremes were checked with `niri validate` 26.04 in tests).
  - Slice 3 done: symlinked configs, repeated nodes (`spawn-at-startup`, `window-rule`, `layer-rule`), offering and applying recommended rules (`blur-xray`, `screencast-banners`, `overview-backdrop`) in engine (`Editor::rules|apply_rule|remove_rule`), CLI (`kanade niri-config rules [apply|remove]`), doctor advice, and Settings niri page with toggle switches, undo, and search.
  - Slice 4 done: `ConfigLoaded` confirms the niri page's own reload, and a restart note covers `spawn-at-startup`/`environment`.
  - Outputs slice 2 done (code, unit-tested; not run on the session, since an agent may not change the live outputs): the Settings niri page's Outputs section with live try, a Keep/Revert bar with 15 s, the pending-revert file and its recovery at start (`src/settings/outputs*`). Slice 3: the layout canvas, `output` blocks of disconnected monitors. To check live on eDP-1 and HDMI-A-5: each control tried, Keep, Revert by button/Esc/timeout, closing Settings and killing Kanade (-9) mid-trial then restarting it, a rejected change, the last-output switch, VRR on-demand, a hot-plug.
    - Outputs follow-up (Vera), done: `Real::run` (`niri msg output`) now runs through `appearance::child::run` with a 2 s timeout, returning `RunError::Refused` or `RunError::Unanswered`; an unanswered first edit keeps the trial (the deadline reverts it). `ConfigLoaded` is now the confirmation that niri loaded a Keep: the worker snapshots the counter before writing, waits up to 2 s for it to advance, and handles failed loads (revert), disagreements (keep the pending file) and timeouts (do not revert; compare). A Keep no longer deletes the pending file before niri confirms the reload; `confirm_keep` and `abandon_keep` split the step. The rest of Vera's follow-up is fixed, each with a regression test at the seam it crossed: the trial's own clock thread (`kanade-outputs-clock`) waits on `Controller::wait` (`retry_at`, the deadline while no revert has failed) and ends the trial through `controller`/`revert_when_due` as the worker does, so a worker that panicked or is blocked in `niri msg output` cannot leave a trial with no timer, and a trial the config holds (`Session::kept`) is never reverted by it; `recover` leaves the file alone while the shell holds a trial and `try_edit` refuses a fresh unsaved file another Kanade left (`is_foreign`), so recovery and a new trial never share it; `keep_request` refuses a request made after the deadline whether or not a tick has run, and a failed revert moves `retry_at` rather than `deadline`, so it does not reopen the Keep window; a recovery sends `vrr --on-demand` back only for a trial that changed the variable refresh rate, since it is invisible in IPC.
    - `clock::on_terminate` also replaces an inherited ignored SIGINT (a background job of a script) with its handler; `main.rs` words a failure to start it as "SIGTERM will not put outputs back", which also covers SIGINT.
    - Not run live (an agent may not change outputs): `niri msg output <name> position set -- -1920 0` (the `--` form is only from `--help` and clap's rules), SIGTERM mid-trial (`systemctl --user restart kanade` should put the monitor back before the restart), typing in a position field during the 15 s.
  - Outputs slice 1 done (engine only, no UI): `output "<name>"` blocks addressed by ordinal, resolved as niri does (first match, includes in order, include-owned refused), properties (`position x= y=`, `variable-refresh-rate on-demand=true`), reset, and `kanade niri-config get|set|remove 'output["name"].key'` (ADR 0119 amendment). Slices 2-3 pending: the Settings Outputs section (live try over `niri msg output`, a confirm bar, a pending-revert file), then the canvas and the rest. Not run on the session: no `niri msg output` was run; the engine was tested on temp configs and `niri validate` only. `max-bpc` is not in niri 26.04's parser, so it is left out.
  - [x] Fixed (review of outputs slice 1): `Props` set splices each property where it stands, so untouched spacing and the comments between the entries stay (`position   x=0   y=0` keeps its blanks); a bare node in a message says `true (was unset)` instead of an empty value (`niri_config/tests.rs:376`); an `output` block's new name holding a bidirectional control character is refused (`edit.rs:1312`).
  - Checked for the class of the border bug (a node niri reads from a block's presence), over the page's rows and `hot-corners`, `animations`, `tab-indicator` and `insert-hint`: only `layout.border` reads a bare block as on (niri-config/src/lib.rs, `recursion == 0`, top-level config only). `layout.focus-ring`/`shadow` blocks with neither flag keep their default; `hot-corners` is value-read (a bare block turns no corner on, and niri's default top-left corner is not the block's doing); `animations`, `tab-indicator` and `insert-hint` are presence-flag reads whose `off`/`on` a bare block leaves alone. No mismatch, so no code change.
  - Fixed: a search of one or two letters ("n", "ni") no longer lists every row; `niri_page::mentions` matches from the start of a word, and the page's own name only as the whole word "niri".
  - Low-level, unfixed: a panicked worker leaves its rows busy (`Saving…`) until the window reopens.
  - Low-level, unfixed: Undo can undo a CLI change made after the page's own change; the engine's hash check still gates it, but the change it reverts is not the page's.
  - An include that newly starts setting a watched path is stamped only from the next read on; a change to it before that shows when the page is next read.
  - Not run on the session: the CLI was run on a copy of the config only, never the live file.
  - Fixed (review of slice 1): `atomic::sweep` removes the `.config.kdl.<pid>.<n>.part` files a killed write left, when that process is gone, on every staged write; a new top-level node goes at the start of an indented include's line (no whitespace-only line, and the include keeps its indent); a new nested node takes its own block's indent step, so a file mixing tabs and spaces gets no odd indent.
  - Fixed: a property Set splices each property where it stands, so untouched spacing and the comments between the entries stay (`position   x=0   y=0` keeps its blanks); a block name holding a bidirectional control character is refused; a bare node in a message says `true` instead of nothing.
  - Fixed: a panicked niri-page worker releases its row instead of leaving it saying Saving for good.
  - Fixed: Undo on the page is only this window's own change (`Editor::undo_only` with the hash its write left), so a later CLI change in the shared backup ring is refused instead of silently reverted.
  - Fixed: the page's include stamps are taken before the options are read, so an include that changes while it is being read is read again instead of keeping its stale values under a fresh stamp.
  - Done: `ConfigLoaded` confirms the niri page's reload in its footer (reloaded, rejected, or not yet), and a change to `spawn-at-startup`/`environment` says it applies after a restart, with a standing note on the page.
- [x] SDDM: our own theme, `kanade sddm` and the overlay (ADR 0120, `docs/wiki/SDDM.md`); qylock was not used.
- [x] Notification sound customization: per app and urgency, a chosen sound file, volume, or off; in the notifications Module and its config.
  - Done (ADR 0118): `notifications.sound_file`, `sound_low`/`_normal`/`_critical`, `sound_volume`, and rule `sound = "<file or name>"` with `volume`; Settings shows the switch, file and volume. Not run on the session: live playback through `pw-play` and `canberra-gtk-play` (volume, the missing-file note).
- [x] Dock click behaviour: left click focuses the app, middle click opens a new instance, right click opens the application's actions (Desktop Entry actions).
  - Done: middle runs `Launch::run` through `dock::new_instance` (nothing for an app with no `.desktop` entry); left keeps cycling; right was already the menu with the actions. Not run on the session: the middle click.
- [x] Clipboard Surface: remove the preview box (the large box that shows the full text of the selected entry, ADR 0114).
  - Done: the box is gone for text and images; rows keep their one-line preview and thumbnails and take its room; ADR 0114, the wiki and the design are updated. Not run on the session: the Surface's layout.
- [x] niri strip minimap Surface: the columns and windows of each workspace on niri's scrolling strip, click to focus. Nothing like it exists in Kanade.
  - Done (ADR 0133): `Surface::Strip` with dynamic body height per workspace count, tracking `StripWindow` column/row layout from niri IPC, horizontal strips with column tiles and camera viewport framing, app icons, 2D keyboard ring navigation, module `strip-surface`, verbs `kanade strip` and `kanade minimap`, and Launcher destination. Not run on the session: live visual observation across multi-workspace scrolling.
- [x] Column and window controls from the Island, tied to the focused window: consume or expel, column width presets, move to workspace (ADR 0132).
  - Done: `Surface::Window` on `geometry::PAD` (440×236), tracking focused window layout and workspaces from niri IPC, column actions (consume right, expel bottom, ⅓, ½, ⅔, preset width, maximize, float) and workspace transfer pills, keyboard ring navigation, module `window-surface`, verb `kanade window`, Launcher destination (commits `0662f77`, `34bb2d1`).
- [x] Per-app volume mixer in Controls: each playing app gets its own level (`Line::Stream`/`Line::Route` in `src/surfaces/controls/audio.rs`, from `Stream` in `src/sources/audio.rs:101`), and a stream can be moved to another output.
  - Premise no longer holds: the mixer exists.
- [ ] Emoji grid picker with recents, as a Surface (optional; `:emoji` search in the Launcher already exists).
- [ ] Desktop widgets on the desktop layer (clock, weather, calendar, media); scope to be decided.

## SDDM theme and `kanade sddm` (feat/sddm-theme, low-level, unfixed)

- [x] Done with the CLI and docs: ADR 0120 and the wiki page; `theme list|info|use`, `enable|disable`; `sddm.sync` follows the wallpaper (coalesced, flock, no write through a link or a hard link, BOM-tolerant, a clear missing-overlay message); `kanade doctor` and Settings' Login screen page.
- [x] Control characters in a theme's file names are removed in `theme list|info` and in every message that names a path: all human-facing SDDM path messages route through `class::plain()` (`sddm.rs`, `sddm/class.rs`, `sddm/commands.rs`, `sddm/follow.rs`).
- [x] Fixed: `theme info|use <name>` refuses a name before it is echoed or used — `theme_ok` runs first and a refused argument is said so, not printed, and later errors pass the name through `plain()` (`sddm/commands.rs:191`, `:394`).
- [x] `installed_user`'s error "not a file of a reasonable size" is wrong: `class::read` truncates at 256 KiB, hides the open error, and silently truncates an oversized seed.
  - Premise no longer holds: `installed_user` and that message were deleted (commit 4a5c24a). `class::read` now returns the text with a `whole` flag and errors naming the path and the real io error (`sddm/class.rs:119`), and the seed reader keeps only a whole file, so a seed cut at the limit is not written back (`sddm.rs:1086`).
- [x] Fixed: a panicking follow worker clears only its own pending path and running flag, so a path submitted during the failed refresh does not stay pending (`sddm/follow.rs:110`, test `a_worker_that_panics_clears_its_path_and_lets_the_next_change_start_another`).
- [x] Fixed: the doctor skips the mode check for a removed installed theme, so its modes are not warned about a third time (`sddm.rs:1657`, test `the_doctor_does_not_check_the_modes_of_a_removed_installed_theme`).
- [x] Fixed: `follow::refresh` asks niri for the output size before it takes the flock, and asks again under it only if the config changed (`sddm/follow.rs:180`, test `the_size_is_asked_of_niri_before_the_lock_is_taken`).
- [x] Fixed: a `theme use` that fails after the picture step puts the picture back as well as the setting — the picture bytes are snapshotted before and restored on failure (`sddm/commands.rs:362`, `:417`).
- [x] Fixed: the Login screen page's header says a row below is not current — `is_status_warning` over the status lines decides both the wording and its ink (`settings/view.rs:1121`).
- [x] Fixed: the Login screen status is re-read after a change to one of its rows, not only on open and Refresh — `change`/`change_all` call `login::load()` when a changed key is under `sddm` or the page is shown (`settings/view.rs:42`, `:51`).
- [ ] Iris: the avatar overlaps the clock at 520x330 (a small login surface). A live or rendered greeter only: the layout is QML (`dist/sddm/kanade/Greeter.qml:120`) and `scripts/sddm-preview` has no target at that size.
- [ ] Iris: a Qt 5 greeter decodes a huge hand-set backdrop in `theme.conf.user` with no size cap; the theme cannot check a file's size from QML, so the wiki says to keep it small (Kanade's own picture stays under 16 MiB). A live greeter only, and an accepted limitation (`docs/wiki/SDDM.md:51`).
- [ ] Not run live: the follow path in the running shell (the hook, the worker thread, the niri size query), the Settings page, a farmed third-party theme on a live greeter, and the class of every installed theme. A live greeter and session only; nothing here is decidable from the code.
- [x] Fixed: `plan`'s `walk` counts entries as it reads them and refuses the theme as soon as the cap is passed, so one huge directory is not read whole first (`sddm.rs:699`, test `a_directory_over_the_entry_cap_is_refused_as_it_is_read`).
- [x] Fixed: `status` says `theme.conf.user: stale: does not name /usr/local/share/kanade-sddm-data/backdrop.png` when the overlay's file no longer names the data backdrop, and that line gates `current` (`sddm.rs:1485`, `:1492`).
- [x] The over-size error text's "about a third" matches the math (`0.75^4 ≈ 32%`, each step shrinks each side by a quarter, four tries) (`sddm.rs:537`).
- [x] `PREFIX` (Rust) and the overlay path in `Sanitize.js` are the same literal twice, with no cross-check between them.
  - Premise no longer holds: `Sanitize.js` names the data file only (`dist/sddm/kanade/Sanitize.js:65`), and test `the_theme_names_the_data_path_the_rust_side_writes` holds every theme file to the Rust `DATA` path (`sddm.rs:2760`).
- [x] Fixed: the overlay's `metadata.desktop` gets the `QtVersion` of the greeter found — `target_qt_version` reads the installed greeter (or `PATH`) and `set_qt_version` writes it on setup and on a theme's state, so a Qt 5 SDDM needs no hand edit (`sddm.rs:943`, `:976`, `:1155`; tests `setup_writes_the_qt_version_of_the_greeter_it_finds` and `target_qt_version_recovers_to_qt6_when_greeter_upgraded`).
- [x] The conf-file sort is case-insensitive as sddm.conf(5) says; `to_ascii_lowercase` approximates Qt's `QDir::LocaleAware`. Mixed-case or non-ASCII drop-in names can only produce a benign false status warning when ordered differently than SDDM would — no code change warranted (`sddm.rs:188`, `:203-205`).
