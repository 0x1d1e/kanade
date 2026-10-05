# Kanade

A top-center Dynamic Island for niri, built with [Amane](https://github.com/MystiaFin/amane).

<video src="assets/preview.mp4" controls muted width="640"></video>

[Preview video](assets/preview.mp4) (AI-generated concept, not a capture of the current build)

## Limitations

- The island stays visible over fullscreen windows. niri 26.04 does not report fullscreen state, and guessing it from window size also catches maximized windows, so suppression waits for [niri#2836](https://github.com/niri-wm/niri/pull/2836). See [ADR 0002](docs/adr/0002-defer-fullscreen-suppression.md).
