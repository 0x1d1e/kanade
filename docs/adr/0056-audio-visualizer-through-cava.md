# 56. The audio visualizer through cava

Status: accepted. Extends `media.visualizer`, which stays drawn, not heard, by default.

## Context

The Media visualizer is synthetic: bars that swing at fixed paces while a track plays. Some users want it to follow the music. Hearing the audio means capturing it, which risks the microphone privacy indicator (ADR 0003) and a second audio pipeline in the shell. `cava` already turns a sink's monitor into bar heights.

## Decision

- `media.visualizer = "audio"`, a fifth choice beside `bars`, `wave`, `dots` and `off`; the default stays `bars`.
- Kanade runs `cava -p <its own config>` (raw output, ASCII, four bars, 30 frames a second, `method = pulse`, `source = auto`: the default sink's monitor) and reads its stdout. Its config lives in `$XDG_RUNTIME_DIR/kanade/cava.conf`, so the user's terminal cava config never applies.
- **Privacy.** Checked against `src/sources/privacy.rs`: a sensor needs a link from an `Audio/Source` or `Video/Source` node into a `Stream/Input/*` that is not `stream.monitor`. A sink monitor's link leaves an `Audio/Sink` node, and PipeWire marks such a stream `stream.monitor`, so both tests keep the indicator dark. Test `level_meters_and_desktop_audio_capture_no_sensor` holds the second.
- **On demand.** The visualizer's draw asks for cava (`cava::want`); the first ask starts the thread, the next starts cava through `wake::run` (setpriv `--pdeathsig KILL`, a bounded, supervised child). A cava frame is a Service write, which draws the windows showing it, and each draw asks again. A frame that finds the last ask older than 1.5 s kills cava, so a paused track, a collapsed island, a covered one or another setting stops it, and nothing draws frames while it is not playing. Without a first ask there is no thread.
- **Fallback.** Until the first frame, or when cava is missing or fails, the view draws the synthetic bars (four, the same width), and a failed start is not tried again for 30 s. `kanade doctor` warns about a missing cava while the setting is `audio`.

## Consequences

- One more helper process while music plays on screen, about 30 wakeups a second for it and Kanade; none otherwise.
- Cava must be built with PulseAudio support (Arch, Nix and Debian are), served by `pipewire-pulse` on PipeWire systems.
- Heights are cava's, smoothed by its own defaults; a silent but "playing" track draws flat bars at 30 frames a second.
- Cava reads whatever the default sink plays, not the one player's stream, so other apps' sound moves the bars too.
