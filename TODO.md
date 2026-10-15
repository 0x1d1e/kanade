# TODO

Backlog left from the `liquid-glass` branch (#208), and the full architecture review after it.

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
- [ ] About 80 comment blocks run over four lines (worst: `shadow.rs:1`, `dock.rs:1344`, `view/mod.rs:729`, `calendar.rs:1087`). Each states real constraints, so cut them one by one, not in bulk.
- [ ] ADR 0003 still says "std-only JSON parser"; history, left as written.

## Architecture (whole-codebase review, third)

- [x] Slow CLI calls (lock, caffeine, battery limit, wallpaper, recording) are answered by the shell when done, or unknown after 5 s (`later.rs`); the five serial-numbered `settle_*` protocols and the CLI's 50 ms status polling are gone.
  - A shell that restarts mid-call now reads as "closed the connection without answering" (exit 3), not the old per-feature "lost" messages.
  - `capture record start` still polls for the first frame's file every 50 ms for up to 5 s; wf-recorder has no event for it.
- [x] One bounded-child runner for the appearance adapters and Matugen (`appearance/child.rs`); the adapters' output is now read as it is written and capped at 1 MiB.
- [x] Modules carry `hooks.status` and `hooks.reload`; `kanade status` and a config reload walk the Modules that are on. `modules::on("<name>")` stays string-keyed at its other 60 call sites; `boundary::module_names_are_in_the_catalog` fails on a name the catalog lacks.
  - Not done: replacing the central `Call` enum with a per-Verb `run`; `kanade status` lists the Modules' lines in catalog order, not the old hand-kept order.
- [x] "plan N" references in code comments now say the fact; `docs/archived/plan.md` stays history.
- Not taken: a shared remote-sync loop for Google and weather. Only the three-way `recv` / `recv_timeout` wait is common; what a timeout means, the due test and the message set differ, so a helper would need as many parameters as it saves lines.
- Not taken: a `Target` table for the appearance adapters' `apply` and `disable`; their sequencing differs per target (Zen's marker block, GTK's restore state).
- [x] `Image::blurred`, the loader's blur key and `graphics/image/soften.rs` are gone (no caller after ADR 0049).
- [ ] Dead after ADR 0049: `shadow.rs` still cuts each piece where the body covers it, which an opaque body no longer needs.
- [ ] A plain (unmerged) pane needs no shader: a `Rectangle` fill would drop the pane shader file's dependency on `XDG_RUNTIME_DIR` for the common case; only the merged outline needs it.
- [ ] Retake the memory and frame baselines (`docs/memory.md`) now the capture is gone.
- [ ] `sources/palette.rs` polls every 500 ms; a change event could replace it.

## Architecture (#208 review)

- [x] Input and invalidation (ADR 0034, 0035; a copy redraws only its pane's readers)
- [x] The Island's scene (ADR 0036)
- [x] GPU glass (ADR 0037; removed by ADR 0049)
- [x] The layers in `docs/design.md`
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

## Panes (Liquid Glass finish, ADR 0052)

- [ ] Retake the idle-frame, wakeup, frame-gap and memory baselines with `appearance.finish = "liquid-glass"`; no number is recorded for it yet.
  - First finding: with a page animating behind it (a spinner in a terminal) both the Island and the Dock drew at the refresh rate, as their copies kept changing. A copy now counts as new only outside the body and the panes the shader skips (ADR 0054); check by eye over a spinner, and over a truly static page (a plain window, cursor blink off), that 5 s at rest draws 0 frames.
- [x] Over a bright page the light text on Liquid Glass is hard to read: a scrim, then paper's ink (ADR 0054). Tune by eye; the ink follows the mean light of a ring 12 px outside the body, so a page half bright and half dark may keep the wrong ink on one side.

- [ ] The `unread` regions (`src/pane/capture.rs`) rest on the shader's read geometry (`glass.wgsl`), with no test tying the two; a merged outline's concave notch is the case to check by eye or by a test that samples the shader's offsets.

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
- [ ] A write of any Service but `Pointers` redraws every window that read it: see Invalidation under Architecture (only `IslandService` is per output, and it is not safe to scope in one pass).

## Island sizes (ADR 0030)

- [ ] A side-edge Banner's window is full height.
- [ ] An upright Compact drops titles to the app's tile; a rotated, book-spine title could keep them.
- [ ] An upright Rest and Compact have one length each (`geometry::rest`, `compact`), so a form that needs less (the timer) or more (a Rest with the battery) is padded or tight; the shape could take the primary Activity's Kind and the Rest's battery setting.
- [ ] A body morphing through about square, between a wide and an upright form, overlaps its shadow's corners for a frame or two.

## Island and Dock merge (ADR 0031)

- [ ] Merged Docks on the left or right of an edge are only roughly aligned with the Island's body.
- [ ] The Island's hover area and the Dock's overlap in a band; hover is notified for both.
- [ ] A Keystone's window is as wide as the icons at the largest body, and its row is laid out every frame the Island morphs.
- [ ] Many apps with a wide Surface open overflow a Keystone's window.
- [ ] `pointed()` writes `Pointers`, redrawing the Docks, and a merged Island window, of its output, on each pointer move that changes x, now in merged Crown and Keystone too.
- [ ] `Pointer.x` goes stale across a merge toggle, as its frame changes from the Dock's window to the shell's; it matters only while icons grow.
- [ ] Grown icons in a Keystone or Fold reach `Extent::rise` past the strip, which Banners and Surfaces below the Island do not clear (an unmerged Dock's strip does include its grown icons).
- [ ] A Dock that autohides on the Island's edge is only brought out by the Island, not by touching the edge along its width; it does not reserve space while autohiding.
- [ ] A non-liquid union has no border.
- [ ] Fold has no dots for running apps at rest and no keyboard focus hold; its Tray and hover Peek need a right click.
- [ ] Overview and fullscreen use the existing covered path only.

## Idle Island

- [x] The strip's agenda width (`view::resize`, `rest::Agenda`) arms the minute timer when Calendar is on and any connected output shows the agenda, even if no window draws the time; an ended event stops taking room on the next minute.
  - Not run: hidden-clock agenda expiry in the live session.
- [ ] The Tray Surface shrinks to its list as a menu goes back to it, under the pointer; Clipboard does the same on a query.

## Privacy dots (ADR 0033)

- [ ] A fullscreen window hides the dots by choice, and one `Fullscreen::covers` misses (no app id) hides them on the Top layer too; consider a niri-side hint if a capture under fullscreen proves a real gap.



## Acknowledgements features (review, low-level, unfixed)

- Keys and Wallpapers stat the files on each frame they draw (`binds()`, `wallpaper::images()`); cache by mtime if it shows.
- The image loader keeps queued thumbnails scrolled past; a thumbnail is keyed by path only, so a picture replaced in place shows the old one until restart.
- `night_light::turn` rereads the calendars it does not need.
- Reminders: MORE's `of` grows with each reminder joined; a deleted event's reminder stays until dismissed or due.
- Clipboard and Launcher search look duplicated with `src/surfaces/search.rs`.
- A dmenu reply to a client that stopped reading may block the UI thread up to 1 s (`IpcReply::send`).
- A three-letter layout label may be cramped in the OSD capsule; Controls grew 290 to 330 px, check Tray, Weather and Session by eye.
- The critical battery countdown names no battery reason line.
- The idle dim stays dark while a lock that failed is asked again; a click to wake reaches the window under the dim.
- A dmenu started in the background holds the keyboard until answered (as rofi does); Escape always reaches it.

- Calendar Surface: events added there are changed (`e`) and removed (`d` twice) by key only, with no click and no undo (ADR 0045); recurrence, place and a reminder cannot be typed.

## Design audit (not done)

- No screen reader support (AT-SPI tree); an architecture change, see `docs/wiki/Limitations.md`.
- No UI text scale setting; type sizes are fixed tokens.
- The Session Surface treats Lock and the counted-down Restart, Power off and Log out alike in weight; the countdown with Cancel is the guard.
- ADR numbers skip 0045.

- Capture Surface: a failed screenshot or recording start is only logged to stderr, as the island is already closed; Record starts as the island collapses.

- Settings: the header title sits at the window's left padding while the page column is centered; in a window maximized under the Island a long title runs under it.
- Settings: stepper step sizes are fixed per kind, not adaptive to the range; its `-`/`+` buttons are pointer-only (Up/Down cover the keyboard).

## External audit triage (a6f29f3)

Unfixed, no visual change wanted. Fixed in the same pass: notification `image-data` sizes checked against the bytes sent, font fallback skips unparseable files, dead backdrop mip pyramid removed.

- [ ] `Atlas::place` wraps and clears while earlier glyphs of the same batch still wait for `Quads::draw`; the write lands before the draw. Needs a flush before the wrap; only when one frame holds over a 1024x1024 atlas of distinct glyphs.
- [ ] `ipc` answers on the UI thread (1 s read timeout) and `request_frames` follows every call; handlers may change state outside a Service, so the redraw stays until each is checked.
- [ ] `update_surface` commits a layer change, then `present` commits the frame; the first commit is how a hidden window shows again. A property-only change could skip it.
- [ ] `open_normal` runs the view once only to read title and size.
- [ ] `Brightness` and `Palette` poll every 500 ms and the trimmer wakes every 10 s (ADR 0004 knows them); `osd` and `notifications` still spawn `pactl subscribe` and `dbus-monitor` and settle-poll.
- [ ] `elide` and the `tail` helpers re-measure from the start per removed letter (quadratic).
- [ ] `disc_angle`'s `DISC` is one process-wide state, so two monitors showing the disc advance it twice per refresh.
- [ ] `Reveal`'s `HONORED` is one cell for all scroll areas; a serial one surface used can hide the same serial in another.

## From the trend-improvements review (unfixed polish)

- [ ] Lock: a hung PAM module holds the one check at a time (no watchdog); `Dark` in `crates/lock/src/wake.rs` never expires; `kanade status` prints the grace with `idle.lock` off and the raw service name; `clean` keeps Unicode format characters; `Kind::Text` accepts a path as `lock.pam_service`; the interleaving model in `src/lock/mod.rs` lacks `grant` and `touch`.
- [ ] Lyrics: a paused track gets no local `.lrc`; `now + until.div_f64(rate)` in `media.rs` and `next_second` panic for a tiny MPRIS `Rate`; `mpris:url` may point at any readable `.lrc`; lyric text keeps control and bidi characters; stale `.part` files in the cache are never pruned.
- [ ] Calendar subscriptions: feeds are fetched one by one (16 dead hosts hold `Reread` for minutes), all are due at start with no jitter, each keeps up to 5 MiB resident; ureq honors proxy environment variables, undocumented.
- [ ] Wallpaper: `showing_on(Named)` falls back to the last image Kanade set when awww is not queried.
- [ ] Focus: `status()` lists caffeine after it ended late; `night_light::turn(true)` that saved but did not reload leaves the mode unrecorded in `Held`; a user's DND off-then-on during Focus looks the same as untouched.
- [ ] Notifications: four or more updating progress notifications rotate the three shown Activities (ADR 0059).
