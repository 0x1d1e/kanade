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
- [ ] 501 comment blocks run over four lines (worst: `sources/recording/encode.rs:244`, `sources/recording/plan.rs:302`, `surfaces/launcher/calculator.rs:1`, `shadow.rs:1`, `dock.rs:1342`). Each states real constraints, so cut them one by one, not in bulk. The earlier citations of `view/mod.rs:729` and `calendar.rs:1087` were wrong: the first is inside `#[cfg(test)]`, the second is a `.padding(..)` call.
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
- [ ] A plain (unmerged) pane needs no shader: a `Rectangle` fill would drop the pane shader file's dependency on `XDG_RUNTIME_DIR` for the common case; only the merged outline needs it.
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

## Island and Dock merge (ADR 0031)

- [ ] Merged Docks on the left or right of an edge are only roughly aligned with the Island's body.
- [ ] The Island's hover area and the Dock's overlap in a band; hover is notified for both.
- [ ] A Keystone's window is as wide as the icons at the largest body, and its row is laid out every frame the Island morphs.
- [ ] Many apps with a wide Surface open overflow a Keystone's window: `src/merge.rs:285` takes `let width = left + canvas.width + right;` with no clamp to the output, while the height at `:287` is bounded by the strip. It needs `.min(monitor.width)`.
- [ ] `pointed()` writes `Pointers`, redrawing the Docks, and a merged Island window, of its output, on each pointer move that changes x, now in merged Crown and Keystone too.
- [ ] `Pointer.x` goes stale across a merge toggle, as its frame changes from the Dock's window to the shell's; it matters only while icons grow.
- [ ] Grown icons in a Keystone or Fold reach `Extent::rise` past the strip, which Banners and Surfaces below the Island do not clear (an unmerged Dock's strip does include its grown icons). `clearance_of` at `src/merge.rs:93` omits `extent.rise` while the merged window's own height at `:287` and the Dock's plate at `src/dock.rs:1306` both include it, so the gap is `size.icon * (most - 1.0)` — 56 px at `size = "large"`, `magnification = "large"`.
- [ ] A Dock that autohides on the Island's edge is only brought out by the Island, not by touching the edge along its width; it does not reserve space while autohiding.
- [x] A non-liquid union has no border.
  - Describes a removed finish: `look::Finish` has no liquid variant, and the union's rim comes from `pane.wgsl:69-71`.
- [ ] Fold has no dots for running apps at rest and no keyboard focus hold; its Tray and hover Peek need a right click.
- [x] Overview and fullscreen use the existing covered path only.
  - A statement of an accepted limitation (`view/covered.rs:17-21`), not open work.

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

- Calendar Surface: no undo for a removed event, and recurrence, place and a reminder cannot be typed (a file that holds them keeps them, ADR 0113); a change is read, checked and written in separate steps, so another writer between them wins.
- Calendar Surface: after "Changed elsewhere" the open field keeps its old baseline until Escape; the pointer hold on an event's Remove shows the key wording "Keep holding d to remove".
- Text fields: Ctrl+A does not select all (pre-existing, not from ADR 0113). An open Calendar edit field survives the Surface collapsing and reopening (pre-existing). A `kanade ui press` is followed by the Surface collapsing after about 0.5 s (pre-existing).

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
- [ ] `ipc` reads and runs its handlers inline (1 s read timeout) and `request_frames` follows every call; the reply write is off the UI thread with a 1 s timeout, but handlers may still change state outside a Service, so the redraw stays until each is checked.
- [ ] `update_surface` commits a layer change, then `present` commits the frame; the first commit is how a hidden window shows again. A property-only change could skip it.
- [ ] `open_normal` runs the view once only to read title and size.
- [ ] `Brightness` and `Palette` poll every 500 ms and the trimmer wakes every 10 s (ADR 0004 knows them); `osd` and `notifications` still spawn `pactl subscribe` and `dbus-monitor` and settle-poll.
- [ ] `elide` and the `tail` helpers re-measure from the start per removed letter (quadratic).
- [ ] `disc_angle`'s `DISC` is one process-wide state, so two monitors showing the disc advance it twice per refresh.
- [x] `Reveal`'s `HONORED` is a `HashMap` keyed by each scroll area's own name, so a serial one area used does not hide the same serial in another (`crates/runtime/src/widgets/reveal.rs`).

## From the trend-improvements review (unfixed polish)

- [ ] Lock: a hung PAM module holds the one check at a time (no watchdog) — a decision, not a defect; `Dark` in `crates/lock/src/wake.rs` never expires; `kanade status` prints the grace with `idle.lock` off and the raw service name; `clean` keeps Unicode format characters; `Kind::Text` accepts a path as `lock.pam_service`; the interleaving model in `src/lock/mod.rs` lacks `grant` and `touch`.
- [ ] Lyrics: a paused track gets no local `.lrc`; `mpris:url` may point at any readable `.lrc`; lyric text keeps control and bidi characters.
  - Premise no longer holds for the other two: the rate maths is total now (`until()`/`moved()` with a checked `next_second`) instead of dividing by `div_f64`, and the cache's `.part` files are swept.
- [ ] Calendar subscriptions: feeds are fetched one by one (16 dead hosts hold `Reread` for minutes), each keeps up to 5 MiB resident; ureq honors proxy environment variables, undocumented.
  - Premise no longer holds for the start jitter: a feed's first read is staggered.
- [x] Wallpaper: `showing_on(Output::Named)` no longer falls back to the last image Kanade set; `known()` answers from awww alone, and only `Output::All` still falls back when awww reports nothing (`src/sources/wallpaper.rs:472`).
- [x] Focus: `status()` no longer lists expired caffeine, `night_light::turn(true)` that saved but did not reload is recorded in `Held`, and a user's DND off-then-on during Focus no longer looks untouched (`turns_off` compares the exact revision; `src/sources/focus.rs`).
- [ ] Notifications: four or more updating progress notifications rotate the three shown Activities (ADR 0059).

## Semantic tree (#21, ADR 0095)

- [ ] Name the rest of the controls with `.semantics(...)`: Banners, Notifications, Clipboard (its rows, `pin`, `delete`, `clear` and the search are named), Calendar, Weather, Wallpaper, Timer, Capture, Tray, Session, Media, Settings buttons, and the Controls sub-surfaces (Wi-Fi, Bluetooth, Sound, Arrange). Content they show (titles, bodies, SSIDs, tracks) must be `.content(...)`, never `.label(...)`.
  - A control whose click handler is on a parent of the annotated rectangle is not pressable by name.
  - Not run: a live session pass of `kanade ui` (see the report for the steps).

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

## Live actions (ADR 0110, review, low-level, unfixed)

- [ ] `activity wait` and `run --action cancel` poll every 250 ms: each ask is a connect and an IPC on the draw thread, about 4 shell wakeups a second while anyone waits (for `run`, the command's lifetime). A subscription or a longer-held call would cost less.

## Notification sounds (ADR 0118, review, low-level, unfixed)

- [ ] The stderr note for a layer that cannot be used prints for every notification, even one dropped because a sound already plays (`cue()` emits the notes before the `PLAYING` check): dedupe or rate-limit it.

## Clipboard pins and previews (ADR 0114, 0115, low-level, unfixed)

- [ ] Pins persist (ADR 0115) as one file rewritten whole on each change, up to 16 MiB; one save thread per change, each taking the latest set, so a burst of pins writes more than needed.
- [ ] A newer Kanade's pins file, or one that could not be read at start, turns saving off for the run; a damaged one is kept as `clipboard-pins.bak`, which Kanade never removes.
- [ ] A pin change right before exit may not be saved: the save thread is detached.
- [ ] Not run: a restart with pinned text and pictures on the session (pins back in order, thumbnails regenerated, Launcher `>`, unpin removes them from the file).

- [ ] A preview is written while the delete pass is held off (`preview::TURN`), so a 16 MiB picture holds a copy's `record` for the write; and the Surface copies the whole history (`to_vec`) on each draw and key, as before.
- [ ] A Kanade that exits (cleanly, SIGTERM or killed) leaves its previews until the next start, which sweeps them (tmpfs clears at logout); a second Kanade's start sweeps the running one's too, which the running one rewrites on the next draw.
- [ ] The runtime's thumbnail cache keeps a decoded thumbnail of a deleted preview until it evicts it (512 copies, 128 MiB).
- [x] A preview's copied image bytes are bounded before they are decoded: `decode_png` calls `count(width, height)` and `bytes_fit(size)` before allocating, against `MOST_PIXELS` (`crates/runtime/src/graphics/image/decode.rs:94`), so a decompression bomb is refused rather than decoded.
- [ ] `kanade ui press` closing the Surface is older than this change.
- [ ] An unpin puts the entry at the top of the rest, not where its age would; entries keep no time.
- [ ] Not run: the Surface (thumbnails, the pin stop and button, the footer copy) and the idle numbers with a history of pictures.

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
  - [ ] **`videorate` does not belong in the window chain, and is the likely cause of the broken recordings (found 2026-10-20).** It was inserted at `src/sources/recording/plan.rs:277` by `fae1e04` ("Fix UI/UX audit findings", 2026-10-19) with the stated purpose of pacing a still window to a steady 30 fps. Two things are wrong with that. **First, it contradicts the design ADR 0117 records and never updated:** that ADR's verified pipeline is `pipewiresrc path=<node> ! vapostproc …` with no `videorate`, and it deliberately records **variable frame rate** ("vfr FILE", "a still window records few frames (variable frame rate, as with `wf-recorder`)"), stamping by wall clock. `fae1e04` did not touch ADR 0117, so the code and its record now disagree. **Second, it fights the wallclock stamping:** `videorate` rewrites frames to a synthetic 30/1 timeline, while `ffmpeg -use_wallclock_as_timestamps 1` then **overwrites those timestamps on arrival** — so the pacing either does nothing or, on a still window, makes `pipewiresrc`'s 1 s `keepalive-time`/`resend-last` re-emissions into a burst whose arrival times collapse. The measured live numbers fit this exactly: 27 s wall gave **730 frames in a 2.08 s file** (≈27 captured fps, so **no frames were lost**) whose file rate is **351 fps** — the frames arrived almost together at `stop`.
    - Decide it by measurement, not by argument: run the chain on a real niri cast with `videorate` and without (`video/x-raw(ANY),framerate=30/1` with nothing pacing it), and compare the file's duration against wall time. A still window is the worst case and the one `videorate` was added for.
    - Removing `videorate` also removes the reason `created`/`videorate` had to sit between `pipewiresrc` and the encoder, which is what emits the `handle_format_change: assertion 'gst_caps_is_fixed (pwsrc->caps)' failed` CRITICALs seen live.
    - Whatever is decided, ADR 0117 must record the pacing decision and the VFR change.
  - [ ] The window path has no watchdog for a chain that hangs without failing: `falls_back` fires only on `Segment::Lost` (`sources/recording.rs:1504`), so a pipeline that emits nothing but never exits is never retried with the software/EGL chain and never reported. A still window's `start` hits the 5 s `LATER` and exits 3, but nothing abandons or restarts the segment. Wanted: treat "no byte written within `start`'s budget" as a fault worth the same fallback `Segment::Lost` gets.
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
- [ ] Settings window: change it to GPUI (now the runtime's own UI).
  - Needs an ADR, and a decision on how it coexists with the runtime of ADR 0028.
- [ ] niri config editor: edit niri's `config.kdl` from Kanade's settings (ADR 0119).
  - Slice 1 done: the text engine and `kanade niri-config validate|get|set|remove|undo|backups|restore` (byte-span splices, `niri validate` before the replace, backup ring, undo; symlinks, include-owned nodes and repeated nodes refused).
  - Slice 2 done: the Settings `niri` page (Layout, Focus ring and border, Input, Keyboard, Touchpad, Mouse, Cursor, Other) over the engine's new batch `apply`, with a worker thread, Reset, include-owned rows read-only and the footer's Undo. Not run on the session: the page itself (needs a live niri; the paths and extremes were checked with `niri validate` 26.04 in tests). Slice 3: symlinked configs, repeated nodes (`spawn-at-startup`, `window-rule`), offering the rules in Installation. Slice 4 done: `ConfigLoaded` confirms the niri page's own reload, and a restart note covers `spawn-at-startup`/`environment`.
  - Outputs slice 2 done (code, unit-tested; not run on the session, since an agent may not change the live outputs): the Settings niri page's Outputs section with live try, a Keep/Revert bar with 15 s, the pending-revert file and its recovery at start (`src/settings/outputs*`). Slice 3: the layout canvas, `output` blocks of disconnected monitors. To check live on eDP-1 and HDMI-A-5: each control tried, Keep, Revert by button/Esc/timeout, closing Settings and killing Kanade (-9) mid-trial then restarting it, a rejected change, the last-output switch, VRR on-demand, a hot-plug.
    - Outputs follow-up (Vera), done: `Real::run` (`niri msg output`) now runs through `appearance::child::run` with a 2 s timeout, returning `RunError::Refused` or `RunError::Unanswered`; an unanswered first edit keeps the trial (the deadline reverts it). `ConfigLoaded` is now the confirmation that niri loaded a Keep: the worker snapshots the counter before writing, waits up to 2 s for it to advance, and handles failed loads (revert), disagreements (keep the pending file) and timeouts (do not revert; compare). A Keep no longer deletes the pending file before niri confirms the reload; `confirm_keep` and `abandon_keep` split the step. The rest of Vera's follow-up is fixed, each with a regression test at the seam it crossed: the trial's own clock thread (`kanade-outputs-clock`) waits on `Controller::wait` (`retry_at`, the deadline while no revert has failed) and ends the trial through `controller`/`revert_when_due` as the worker does, so a worker that panicked or is blocked in `niri msg output` cannot leave a trial with no timer, and a trial the config holds (`Session::kept`) is never reverted by it; `recover` leaves the file alone while the shell holds a trial and `try_edit` refuses a fresh unsaved file another Kanade left (`is_foreign`), so recovery and a new trial never share it; `keep_request` refuses a request made after the deadline whether or not a tick has run, and a failed revert moves `retry_at` rather than `deadline`, so it does not reopen the Keep window; a recovery sends `vrr --on-demand` back only for a trial that changed the variable refresh rate, since it is invisible in IPC.
    - `clock::on_terminate` also replaces an inherited ignored SIGINT (a background job of a script) with its handler; `main.rs` words a failure to start it as "SIGTERM will not put outputs back", which also covers SIGINT.
    - Not run live (an agent may not change outputs): `niri msg output <name> position set -- -1920 0` (the `--` form is only from `--help` and clap's rules), SIGTERM mid-trial (`systemctl --user restart kanade` should put the monitor back before the restart), typing in a position field during the 15 s.
  - Outputs slice 1 done (engine only, no UI): `output "<name>"` blocks addressed by ordinal, resolved as niri does (first match, includes in order, include-owned refused), properties (`position x= y=`, `variable-refresh-rate on-demand=true`), reset, and `kanade niri-config get|set|remove 'output["name"].key'` (ADR 0119 amendment). Slices 2-3 pending: the Settings Outputs section (live try over `niri msg output`, a confirm bar, a pending-revert file), then the canvas and the rest. Not run on the session: no `niri msg output` was run; the engine was tested on temp configs and `niri validate` only. `max-bpc` is not in niri 26.04's parser, so it is left out.
  - Unfixed, low-level (review of outputs slice 1): `Props` set replaces the entry list and collapses its spacing (`x=0   y=0` becomes `x=1 y=2` with single spaces); comments between the entries of a `Props` node are dropped on Set; bidirectional control characters in an output name are written as is; the message of a remove of a never-set `variable-refresh-rate` reads `variable-refresh-rate =  (was ...)` with an empty value in one path.
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
- [ ] niri strip minimap Surface: the columns and windows of each workspace on niri's scrolling strip, click to focus. Nothing like it exists in Kanade.
- [ ] Column and window controls from the Island, tied to the focused window: consume or expel, column width presets, move to workspace.
- [x] Per-app volume mixer in Controls: each playing app gets its own level (`Line::Stream`/`Line::Route` in `src/surfaces/controls/audio.rs`, from `Stream` in `src/sources/audio.rs:101`), and a stream can be moved to another output.
  - Premise no longer holds: the mixer exists.
- [ ] Emoji grid picker with recents, as a Surface (optional; `:emoji` search in the Launcher already exists).
- [ ] Desktop widgets on the desktop layer (clock, weather, calendar, media); scope to be decided.

## SDDM theme and `kanade sddm` (feat/sddm-theme, low-level, unfixed)

- [x] Done with the CLI and docs: ADR 0120 and the wiki page; `theme list|info|use`, `enable|disable`; `sddm.sync` follows the wallpaper (coalesced, flock, no write through a link or a hard link, BOM-tolerant, a clear missing-overlay message); `kanade doctor` and Settings' Login screen page.
- [ ] Control characters in a theme's file names are removed in `theme list|info` but not in every message that names a path: `status` prints `closed[0].display()` raw in its two permission lines (`sddm.rs:1459`, `:1681`), `class::read`'s error text carries `Path::display()` and `gather` echoes it as `modes: {error}`, `disable_text` prints `shown.display()` raw (`sddm/commands.rs:494`), and a video found in a theme directory is named through `name(rel)` without `plain()`.
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
- [ ] The "a third of the screen" text of the over-size error is approximate (each step shrinks each side by a quarter, up to four times): the message prints 32% but the smallest size tried is about 42% (`sddm.rs:537`).
- [x] `PREFIX` (Rust) and the overlay path in `Sanitize.js` are the same literal twice, with no cross-check between them.
  - Premise no longer holds: `Sanitize.js` names the data file only (`dist/sddm/kanade/Sanitize.js:65`), and test `the_theme_names_the_data_path_the_rust_side_writes` holds every theme file to the Rust `DATA` path (`sddm.rs:2760`).
- [x] Fixed: the overlay's `metadata.desktop` gets the `QtVersion` of the greeter found — `target_qt_version` reads the installed greeter (or `PATH`) and `set_qt_version` writes it on setup and on a theme's state, so a Qt 5 SDDM needs no hand edit (`sddm.rs:943`, `:976`, `:1155`; tests `setup_writes_the_qt_version_of_the_greeter_it_finds` and `target_qt_version_recovers_to_qt6_when_greeter_upgraded`).
- [ ] The conf-file sort is case-insensitive as sddm.conf(5) says, but not checked against SDDM's source; a wrong guess gives a benign false warning from `status`. SDDM sorts a drop-in directory with Qt's `QDir::LocaleAware` (upstream `src/common/ConfigReader.cpp`, cited at `sddm.rs:167`) while `configs` sorts by ASCII lowercase (`:183`), so a mixed-case or non-ASCII name can be read in the wrong order.
