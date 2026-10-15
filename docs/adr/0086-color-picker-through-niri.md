# 86. The color picker is niri's, as a Capture tile and a verb

Status: accepted. Builds on [ADR 0019](0019-clipboard-sensitive-content.md) (sensitive copies), [ADR 0071](0071-failed-niri-config-as-a-failed-job.md) (a Live Activity under an id no client has) and the Capture design (screenshots are niri's).

## Context

niri has a `PickColor` request: it shows its own picker over the screen and answers with the picked pixel's RGB (0 to 1), or null when the user leaves it. It is the same niri-native route as the screenshots, so no backend is added. The Capture Surface is a row of tiles whose width is divided by their count (`pad::tile_width`); the Timer Surface already fits five in the same shape.

## Decision

- `niri::pick()` sends `"PickColor"` and waits up to 60 s, as the answer comes only after the user has picked; the rest of niri's requests keep their 2 s. `capture::color()` turns the answer into `#rrggbb` (rounded, held to 0..1), puts it on the clipboard with `clipboard::put` and posts it. One pick runs at a time; a second is refused.
- Placement: a fifth tile, Color, between Screen and Record/Stop (Stop stays last), and the verb `kanade capture color` for binds and scripts. A Launcher destination was not needed: the tile fits the row, and a bound key is the quick route.
- The copy is plain text and sets no sensitive hint, so ADR 0019 does not apply: the clipboard history may keep it, as a hex the user asked to copy is not a secret.
- The hex shows as the done form of a Live Activity (title the hex, detail "copied"), a Transient on the focused output for `SHOWN`, under the id `picked color`, which has a space outside a client's alphabet (the ADR 0071 pattern). No new Kind and no view code; a swatch would need a Kind of its own and is left out.
- A cancel (null) copies and shows nothing. From the verb it is a refusal, "no color picked" (exit 1), so a script tells it from a color; from the tile it is silent.
- The tile closes the Surface and waits the same `CLEAR` as a screenshot, so niri's picker does not see the island. The verb waits for the user (`call_waiting`, as `dmenu`) and prints the hex.
- Part of the `capture` Module (needs niri and `wl-copy`); the tile follows `capture-surface`. Off, neither exists.

## Consequences

- A niri without `PickColor` answers an error, which the verb says and the tile logs.
- The Live Activity form shows "Done - copied" under the hex, with no swatch.
- With `live` off the Activity still shows: it is drawn by its Detail, not by the Module.

Files: `src/sources/niri.rs`, `src/sources/capture.rs`, `src/sources/live.rs`, `src/surfaces/capture.rs`, `src/modules/catalog.rs`, `src/cli.rs`, `src/ipc.rs`.
