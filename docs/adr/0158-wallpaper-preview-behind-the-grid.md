# 158. A full-resolution wallpaper preview behind the grid, and page keys in both grids

Status: accepted. Builds on [ADR 0144](0144-moving-wallpaper.md) (a window per output, driven by
Services) and [ADR 0064](0064-per-monitor-wallpaper.md) (which output shows what).

## Context

The Wallpapers Surface lists the folder as 140 px tiles, decoded as small copies for the grid. The
picture is only set on Enter, so a user cannot judge its crop, mood or palette first: a tile says
which file it is, not what the output will look like. hyprquickpaper draws the focused picture
full-resolution behind its deck, which lets a picture be chosen on what it looks like; letting the
grid's ring drive the same preview gives that without setting anything.

The Surface already has one calculation of what the ring is on: the visit's `Pick`, bounded to the
query's finds and the wallpaper `awww` shows. A preview must read exactly that picture, through the
same bounded image path as the grid, or the two can disagree about what is focused.

Kanade also has the page keys unused in these grids: hyprquickpaper's PageUp/PageDown jump by a
screenful. The runtime already delivers `Key::PageUp` and `Key::PageDown`, and every Surface that
scrolls has a reveal rule that keeps its ring in view.

A full-resolution second window is the risky part: it must not become a second decoder, a second
wallpaper setter, a scrim over the desktop, or something that stays on screen over the lock, a
fullscreen window or the overview.

## Decision

- **The preview is a view of the same `Pick`.** `surfaces::wallpapers` has one module-private read
  that the Surface and the preview both use: the query's finds, `wallpaper::showing_on` for the
  output, `Pick::read().of(visit)` and `pick.bounded(...)`. A module-private derived `Service`
  (`Preview`) watches the relevant state and publishes each output's `(visit, focused path)`,
  filtering unchanged `IslandService` writes (hover, `attend()`, morph frames) with `quiet()`.
  `run()` immediately moves `Pick` on tile left-clicks (`selected`) and `Shuffle` presses
  (`shuffle`), keeping the grid, footer and preview in sync. No Service is written from the view.
- **One window per output, on the Top layer, created under the shell.** `wallpapers::preview` is
  registered by the `wallpaper-surface` Module with `app.window_per_monitor_under`, which inserts
  it ahead of every per-monitor window registered so far. niri (and any compositor that stacks one
  layer by mapping order) puts the oldest surface of a layer at the bottom, so the preview is under
  the Island, Dock and Banners and over normal windows without raising the Island to Overlay for
  every Surface. It is the full output on `Layer::Top` with `Zone::Ignore`, `Keyboard::None`,
  `click_through()` and the namespace `kanade-wallpaper-preview`, taking neither pointer nor
  keyboard. Its child is one square `Rectangle` filled with `Image::cover(path)`, with no scrim.
- **Shown only while this output's Island holds the Surface, and never unmapped.** `Preview` holds
  a picture while `IslandService::expanded_on` is this monitor, `surface()` is
  `Surface::Wallpapers`, the monitor's `presentation` is `Expanded(Surface::Wallpapers)`, a picture
  is focused, and `!lock::holds()`, `!overview`, and `!view::covered(...)`. While a newly focused
  target is `ImageState::Loading`, the preview keeps displaying the current visit's last
  `ImageState::Ready` picture instead of flashing a transparent `0x0` frame, clearing immediately
  when the target becomes `Failed` or focused becomes `None` (Shuffle, closed, locked, covered,
  overview, or a new visit). The window itself stays mapped from start to exit: mapping it again
  when it first has a picture would put it above the Island on a compositor that stacks by map
  order, so only its child changes. Because `wallpapers::preview` reads `Preview` and full-size
  `Decoded` state rather than `IslandService` or thumbnails, unrelated Surface activity never
  triggers spurious full-screen redraws.
- **No second decoder.** The picture is the runtime's `Image::cover` with no thumbnail, drawn from
  the same bounded image cache the rest of the shell uses: one decode of the full picture at the
  size the window asked, not one per preview. Full-resolution decodes (`Decoded`, `cover.is_none()`)
  wake independently of grid thumbnails (`DecodedThumbnail`, `cover.is_some()`), take worker
  priority over queued thumbnails, and stay bounded while waiting (`FULL_QUEUED = 4`) and when
  unheld (`FULL_UNHELD = 3`). `Gpu::forget_unshown()` drops removed bitmaps before trimming, and
  unheld full-size bitmaps are dropped when hiding a layer (`Gpu::release_images()`) or ending a
  Surface visit (`memory::trim_if_due()`). There is no scrim, no second image setting and no change
  to the awww path, the transition, Matugen or the moving-wallpaper window.
- **PageUp and PageDown are page jumps.** In Wallpapers, `Pick::step` moves the ring by one screenful
  of whole rows at the current offset (`((room() / step()).floor() as usize).max(1)`), clamped and
  without wrap. In Emoji, `Search::step` counts the target rows a screenful holds at the current
  offset, so a group's title is measured with its row, and moves by that many, clamped. Both use a
  new `grid::paged` helper beside `grid::moved`, take the target nearest across in the new row, and
  keep the ring visible through the existing reveal and scroll logic. PageUp at the top and PageDown
  at the bottom stay put rather than wrap.

## Consequences

- Choosing a wallpaper is judged at output resolution before it is set; moving the ring or clicking
  tiles/Shuffle changes the preview without calling `wallpaper::set` prematurely, holding the
  current visit's last ready picture across loading gaps without transparent flashes, and closing
  the Surface returns the window to an empty child so no picture and no scrim stay over the
  desktop. The mapped window draws only when `Preview` or full-resolution `Decoded` state changes,
  so unrelated Island morphs, hover, and grid thumbnail decodes cause zero redraws.
- The preview costs one full-resolution decode per focused picture, prioritized over grid thumbnails
  and bounded in the runtime's cache (`FULL_QUEUED = 4`, `FULL_UNHELD = 3`), releasing unheld
  full-size bitmaps after `Gpu::forget_unshown()`, layer hide (`Gpu::release_images()`), and visit
  end (`memory::trim_if_due()`); a folder browsed quickly drops skipped requests before decoding.
- The stacking rests on the runtime opening the preview's layer surface before the Island's and
  niri's map-order rule for one layer; a compositor that reorders layer surfaces on map or resize
  would need an explicit placement. `cargo test` passes, but the live stacking is NOT RUN here.
- Page jumps already clamp and keep the ring visible; a Surface that later adds a page jump should
  use `grid::paged` rather than its own arithmetic.
- **Not run live**: the preview behind the grid on a real output, its absence under the lock, over a
  fullscreen window and in the overview, flicker-free stepping/clicking, and the page keys on a
  session (E2E NOT RUN). The page arithmetic, direction, clamping and no-wrap are unit-tested in
  `grid`, `wallpapers` and `emoji`; preview hold-last-ready behavior and full-resolution decode
  priority/bounds are unit-tested in `wallpapers` and `image`.
