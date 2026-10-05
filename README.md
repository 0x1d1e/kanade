# Kanade

A top-center Dynamic Island for niri, built with [Amane](https://github.com/MystiaFin/amane).

https://github.com/user-attachments/assets/27e6837e-8cf1-4faa-8815-e5b14cdd631b

AI-generated concept, not a capture of the current build.

## Limitations

- The island stays visible over fullscreen windows. niri 26.04 does not report fullscreen state, and guessing it from window size also catches maximized windows, so suppression waits for [niri#2836](https://github.com/niri-wm/niri/pull/2836). See [ADR 0002](docs/adr/0002-defer-fullscreen-suppression.md).
- The Launcher cannot tell when an app fails to start. Amane's `DesktopApp::launch` runs the entry through `sh -c` and reports nothing back, so a broken `Exec` line just closes the island.
- The Launcher shows "Finding apps" forever on a system with no launchable `.desktop` entries. Amane's `Apps` is empty both while scanning and after finding nothing, and exposes no scan-complete state.
