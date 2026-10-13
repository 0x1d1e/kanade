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
  - Not run: losing the stream (restart niri). It should clear the dots and the shown windows, and they stay cleared: the stream is not reconnected.
- [x] Restart-surviving queues and poisoned locks: `kanade_runtime::queue::{Queue, Nudge}` and `MutexExt::locked`, `RwLockExt::{reading, writing}` (`unpoisoned.rs`)
  - Not folded in: `media.rs`'s `Feed`, which hands a clone of its sender to the bus watches while it is made; the two `Condvar` waits; `crates/lock`, which depends on no Kanade crate and has its own CPU renderer.
- [x] The Island's content turning inert or swapping (#210): runtime targets carry a key (`input::nest`, by place in their containers) and `Pointer` follows hover, press and drag by it, not by position
  - A hovered target that turned inert or went away still gets its exit (the pointer keeps the handler last seen), and every redraw path (frame, resize, scale) sets the cursor again (`refresh_cursor`).
  - A key is the place among siblings, so a container whose children come and go before another target must keep its own place (the Island's body takes a placeholder for each layer that is not there).
  - `Rectangle::identity` names a list item by what it is (notification cards and Banners by id, Dock icons by app or, with none, window, Controls rows by SSID, device path and node, Tray items and menu entries, Clipboard entries, Launcher results), so items that come and go above it do not hand it a neighbour's key. Two different widgets given one identity are one target.
  - Not taken: an old and a new form at the same place have the same key (a press begun on one and let go on the other is a click); hover is not re-evaluated after a redraw until the pointer moves.
- [ ] `dock.rs` (2524 lines) holds the Island-aware geometry (about lines 1084-1480: `covered`, `island_room`, `reaches`, ...); move it to `merge.rs` only if churn there shows the cost.

Not worth changing, by this review:

- `crates/lock/src/refract.rs` duplicates the glass math on the CPU by design (ADR 0025); folding it in is on `docs/runtime-migration.md`.
- A flat `Config` with per-module `Setting` consts: the registry already defines each key once.
- `island/service.rs` size: most of it is tests.
- On a side edge the Satellites travel with the body's height as a Compact peeks: they hang from the body's end.
- On a side edge, while a Peek holds the pill it grew from in the input region, a click beside the Peek on the pill's old place expands: the pill is still where the pointer rested.
- The lock thread's backdrop, wallpaper and faces kept while unlocked (ADR 0025): the first lock frame needs them.
- A PAM check that never returns blocks the next: PAM cannot be cancelled, and checks run one at a time by design (ADR 0025).

## Architecture (whole-codebase review, second)

- [x] One atomic file write: `atomic::write` with `Mode::{Shared, Private, Exactly}` (settings, reminders, recents, appearance adapters, calendar mirror)
  - Not folded in: `raster::store` (a cache of hundreds of pictures; the fsync would cost startup) and the glass shader (it locks the file before naming it).
- [x] One `Backoff` (`backoff.rs`) for logind, seat, sleep, tray, wake, night light, battery, weather, CalDAV and Google. Google now retries a failed sync after 1 min, doubling to 15 min, not a flat 15 min.
- [x] `sources/json.rs` is `serde_json::Value` with `parse`, `array` and `quote`; the hand-rolled parser is gone.
- [x] `sources/mirror.rs` is the calendar-sync file mirror for Google and CalDAV; `clock::unix_seconds` replaces four copies.
- [x] A Service's panic reaches `kanade status` (`service::on_panic`, set by `supervise::follow_services`); the restart delay is one constant.
- [ ] About 80 comment blocks run over four lines (worst: `shadow.rs:1`, `dock.rs:1344`, `view/mod.rs:729`, `calendar.rs:1087`). Each states real constraints, so cut them one by one, not in bulk.
- [ ] ADR 0003 still says "std-only JSON parser"; history, left as written.

## Architecture (#208 review)

- [x] Input and invalidation (ADR 0034, 0035; a copy redraws only its pane's readers)
- [x] The Island's scene (ADR 0036)
- [x] GPU glass (ADR 0037)
- [x] The layers in `docs/design.md`
- [x] `view.rs`, `modules.rs`, `lock.rs` and `glass.rs` split by responsibility, `view::island` into its layers
- [x] Scene: the largest body is per output (#212)
- [ ] Invalidation: `service.rs` and `changes.rs` still use process-global state.
  - `Backdrops` is scoped by pane and `Pointers` by output; no other Service is, so a write of any other draws every window that read it (see Glass).
  - `IslandService` is the one that matters, but scoping it is a resource saving (not a gate before 1.0) and not a safe one: 62 `Island::write()` sites would each have to prove they change only one output's state, as `Sizes`, the Arbiter and the overview are read by every window.
  - A write naming no part stays safe (it draws everyone), so sites can be moved over one at a time.
- [x] Baselines retaken 2026-10-11 (`docs/memory.md`, #198; idle wakeups 7.1/s against ADR 0004's 7.5/s, 0 idle frames, island morph gap 7.7 ms against a 6.94 ms refresh interval)
  - Memory grew with GPU glass: 495 MiB of GPU buffers and 312 MiB of RSS at Rest, against 225 and 150 before. The vello and wgpu upgrade adds about 26 MiB of PSS (heap, and `libnvidia-rtcore`).
  - 8% of the island window's frames came later than 1.5 refresh intervals while morphing, and the slowest took 48 ms of view, draw and gpu.
  - Not taken: more than one monitor. Not run (they unplug the output or restart niri): #214's unplug and replug with the Dock, a Banner and autohide active, and #216's lost niri stream.

## Glass

- [x] Settings windows are glass as niri's blur allows (ADR 0024): niri blurs only the wallpaper behind it, with no rim or frost.
- [x] Regular glass is frosted from the capture around the body (ADR 0024); faint wedge seams show in the fill where the nearest side changes, and a thing wholly inside the card is not seen.
- [x] The magnified Dock no longer reaches a Banner: `dock::strip` includes the grown icons (ADR 0030).
- [ ] Satellites shift; the first ~80 ms of the liquid reveal are plain (ADR 0037: a copy is asked for after the first draw and lands about 3 frames on; the shift is their pixels bent by the rim still lagging).
- [ ] A write of any Service but `Backdrops` and `Pointers` redraws every window that read it: see Invalidation under Architecture (only `IslandService` is per output, and it is not safe to scope in one pass).

## Island sizes (ADR 0030)

- [ ] A side-edge Banner's window is full height.
- [ ] An upright Compact drops titles to the app's tile; a rotated, book-spine title could keep them.
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
- Live Activities: no throttle on a client reposting many times a second.
- A three-letter layout label may be cramped in the OSD capsule; Controls grew 290 to 330 px, check Tray, Weather and Session by eye.
- The critical battery countdown names no battery reason line.
- The idle dim stays dark while a lock that failed is asked again; a click to wake reaches the window under the dim.
- A dmenu started in the background holds the keyboard until answered (as rofi does); Escape always reaches it.
- CalDAV: a keyring that answers with no item counts as logged out and drops the account's synced events, as does `forget` after a config read that fell back to defaults; they are a cache, synced again on login.

## Design audit (not done)

- No screen reader support (AT-SPI tree); an architecture change, see `docs/wiki/Limitations.md`.
- No UI text scale setting; type sizes are fixed tokens.
- The lock screen shows no Caps Lock hint.
- The Session Surface treats Lock and the counted-down Restart, Power off and Log out alike in weight; the countdown with Cancel is the guard.
- ADR numbers skip 0045.
