# 13. Screen recording with wf-recorder

Status: accepted (roadmap 9 Shell essentials, #140). Resolves the `docs/design.md` Open item "Define recording backend selection + capability probing".

## Context

`docs/design.md` Capture asks for an external GPU-accelerated recorder with start/stop/status, an Activity while it records, and `privacy` seeing the capture on its own. niri 26.04 has no recording action of its own. The recorder decides whether `privacy` can see it: `privacy` reads niri's casts (`niri msg casts`), and niri lists only capture that goes through it.

## Decision

- **`wf-recorder`, through niri's wlr-screencopy.** niri counts a screencopy with damage as a cast, so the privacy cluster shows the recording without asking `capture`, as the design wants.
- **`h264_vaapi`, mp4, `~/Videos/Screencasts`.** Encoded on the GPU. The file is `Screencast from <stamp>.mp4`, numbered past a taken name, as screenshots are.
- **No backend selection, no fallback encoder.** One recorder, required by the recording part of `capture` and listed by `kanade doctor`. A missing VA-API driver or a bad output fails the recording, and the island and `kanade` say why with the recorder's last stderr lines. A silent fall back to software encoding would hide a broken GPU setup and load the CPU; probing encoders ahead would duplicate what the recorder already checks in its first 100 ms.
- **A holder (ADR 0011), dying by SIGINT.** The recording lives exactly as long as `wf-recorder`; one that ends on its own ended the recording and is never restarted. It runs under `setpriv --pdeathsig INT`, not KILL: wf-recorder finishes the mp4 on SIGINT, so a recording outlives a Kanade crash as a playable file. `stop` sends SIGINT.
- **The IPC answers at once; `kanade` waits.** niri hands a recorder a frame only once the screen changes, and wf-recorder writes the file only at the first frame. On a still screen that change is often the island the draw thread would draw, and the draw thread is the one answering IPC. So `start` and `stop` return the path at once, and the `kanade` client polls `capture record status` until the file exists or the recorder exited, up to its usual 5 s patience. A timeout exits 3 (unknown) and names the path.
- **The Activity.** While recording: Kind Recording, Ongoing, Persistent, Global, with Stop, so it can be a Satellite. Once ended: Actionable, Transient for 10 s, on the focused output, with Copy path and Open as a screenshot's, or the failure and no action.

## Alternatives

**gpu-screen-recorder.** Faster and with more encoders, but it reads the planes through KMS, beside niri: niri lists no cast, so the privacy cluster would not show it. That breaks the design's "`privacy` independently observes actual capture".

**niri's xdg-desktop-portal ScreenCast plus a PipeWire encoder.** niri shows it, but the portal asks the user to pick a source each time, and needs a D-Bus portal client and a PipeWire pipeline in Kanade, against ADR 0011's preference for a tool.

**Software x264 fallback.** See above; a user who wants it can ask for it later as config.

## Consequences

- `wf-recorder` and a VA-API driver become runtime dependencies of recording.
- A still screen records few frames; the file is variable frame rate.
- A recording started by another tool shows in the privacy cluster but not as a Recording Activity: `capture` shows only its own.
