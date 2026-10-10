# 3. Mic and camera privacy from the PipeWire graph

Status: accepted (Phase 5, #34). Answers plan §12 Q2. Privacy as an Activity superseded by ADR 0005; detection stands.

## Context

Plan §5.1 and §7 reserve a Critical, green Privacy Activity for an app using the microphone or camera. niri reports screen casts but not mic or camera use. `src/` may use only std and Amane, and Amane's `Audio` reports volume and mute, not capture.

Measured in #34 on PipeWire 1.6.9 with WirePlumber:

| Capture | What PipeWire shows |
|---|---|
| `pw-record` on the mic | `Stream/Input/Audio` node, then links from the `Audio/Source` node to it, removed when it stops |
| `pipewiresrc` on the webcam | `Stream/Input/Video` node, links from the `Video/Source` node |
| pavucontrol level meters | `Stream/Input/Audio` linked from the mic, with `stream.monitor = true` |
| Desktop audio capture | stream linked from an `Audio/Sink`, not a source |
| `ffmpeg -f v4l2 -i /dev/video0` | nothing: the app opens the device directly |

`pw-dump --monitor` prints the whole graph once, then each object that changes, and `{"id": N, "info": null}` for each removed one. It blocks between changes, so following it costs no wakeups at idle.

## Decision

`src/sources/privacy.rs` runs `pw-dump --monitor --no-colors` and follows its output with the existing std-only JSON parser. A capture is an `active` link from an `Audio/Source*` or `Video/Source*` node to a `Stream/Input/*` node without `stream.monitor`. A link is `negotiating` before it carries media and `paused` while its nodes stop, so an app that holds a capture open but paused does not count. While any capture exists, one Persistent Critical Privacy Activity shows which sensors are in use and the apps using them. If pw-dump exits, the Activity is withdrawn and pw-dump restarts after 1s, doubling up to 60s while each run lasts under 60s. If pw-dump cannot be started, there is no indicator.

## Alternatives

**xdg-desktop-portal.** The Camera portal only grants access and reports whether a camera exists. It never reports use. There is no microphone portal, and apps outside a sandbox skip portals entirely. Not viable.

**libpipewire or a native PipeWire protocol client.** These give the same graph without a child process. But `amane dev` builds with Amane as the only dependency, so this means writing a PipeWire wire client by hand. pw-dump ships with PipeWire itself.

**Scanning `/proc/*/fd` for `/dev/video*` and `/dev/snd`.** This also catches direct v4l2 and ALSA users, but it needs polling, which breaks the zero idle cost goal.

## Consequences

- Privacy ships in v0.1. With nothing above it, it is the primary. A Satellite shows it only beside another Critical Activity.
- An app that opens `/dev/video*` or ALSA directly is not shown. The wiki says so.
- A filter that holds the mic open all the time, such as an always-on noise suppression chain, counts as capture and keeps the indicator on.
- pw-dump is a runtime dependency. It ships in the `pipewire` package.
