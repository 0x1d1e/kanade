# 64. Per-monitor wallpaper

Status: accepted. Extends the Wallpaper Module (ADR 0039, wallpaper palettes through Matugen); the Wallpapers Surface and `kanade wallpaper` now set one output.

## Context

`awww img` without `--outputs` sets every output, so a pick on one monitor replaced all of them. awww already keeps one image per output and remembers them across restarts in its cache. The shell has one palette (Matugen, ADR 0039) and several monitors, so a choice of which output's image it comes from is needed once outputs differ.

## Decision

- **Where a pick lands.** The Wallpapers Surface sets the monitor it is on, by the output name niri gives it, which is awww's (`awww img --outputs=NAME`). Other outputs are not touched: awww sets only the outputs listed. Its tiles mark the image showing on that monitor, and its Shuffle avoids that monitor's image only.
- **CLI.** `kanade wallpaper set <path> [--output NAME|all]` (default `all`, as before) and `shuffle [--output NAME|all]` (default the focused output, `all` if niri does not say: the client has no output of its own). `status` answers the last set and a line per output from `awww query`. A name is one word without comma, space or control character and not starting with `-`, so awww takes it as one output and never as a flag or a list. A name `awww query` does not list is refused (`awww shows nothing on an output named ...`) before `awww img` runs.
- **Persistence.** awww's cache, as for the whole wallpaper: Kanade stores no wallpaper state of its own. Per-output images survive a restart because awww restores each output's file.
- **The palette.** One palette follows one output, the driver: the output focused when Kanade last picked a wallpaper, remembered in memory until restart (none before the first pick, then the focused output as before). A pick on the driver, or on `all`, replaces the palette and fires the `wallpaper` hook as before. A pick on another output changes neither: the palette keeps the driver's image, and the look awww's cache triggers (ADR 0039) finds the driver's image unchanged. A set outside Kanade (`awww img`) replaces the palette with the driver's image if the driver is still listed, else the focused output's.
- **Why the focused output at the pick.** The user chose the wallpaper while looking at that monitor; focus is what niri tells, and no other signal says which screen is "the main one". The cost is that moving focus alone does not move the palette: it follows the next pick, so colors never change without the user asking.

## Consequences

- With one monitor nothing changes.
- With several, an external `awww img` for the non-driver output leaves the palette; before, the palette followed the focused output's image at the time.
- The `wallpaper` hook fires for the driver's wallpaper only, not for a pick on another output.
- A wallpaper awww shows that Kanade cannot decode (bmp) is missing from the per-output marks; the mark falls back to the last image Kanade set when no output is known.
