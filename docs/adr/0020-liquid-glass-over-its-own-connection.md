# 20. Liquid glass over its own Wayland connection

Status: transport superseded by ADR 0037, which captures in the runtime and bends the rim in a shader; the material stands. Amends ADR 0011 ("Never a Wayland or render crate") and the `wayland-client` rule in `AGENTS.md` for one crate, `crates/glass`. Amended by ADR 0024, which puts the material on every surface.

Validated on niri 26.04: the Island's rim bends a striped window behind it, the backdrop stays sharp through the tint, and over a still picture with a still Island the glass captures nothing.

## Context

`appearance.material = "liquid-glass"` is Apple's clear liquid glass: what is behind seen sharp through a tint, bent at the rim, specular highlights, a tint that follows the backdrop, a lit edge. The pinned Amane cannot bend what is behind: a shader gets `size`, `time` and 16 values, no texture, and Kanade has no access to what the compositor drew under the Island.

niri advertises `zwlr_screencopy_manager_v1` v3, not `ext-image-copy-capture`. Amane and niri are not to be patched, forked or asked.

## Decision

A workspace crate, `kanade-glass` (`crates/glass`), opens its own Wayland connection beside Amane's, with no `unsafe`, and only captures:

- **Refraction.** `wlr-screencopy` captures the body and 13 px around it; once a capture fits the pane's placement, the next waits for the screen to change. `refract.rs` fills a band of up to 12 px inside the edge with what is outside it, squeezed toward the edge as a lens rim does and the right way round, blue a little further than red, fading into the backdrop seen through the middle; near the screen's edge it squeezes into the room left. It samples only outside the body, so the body's own pixels never feed back; what the Island draws beside the body, like Satellites, is bent in as backdrop. The rim reaches the pane as a PNG; a rim that looks the same as the last is not posted, so an unchanged rim draws nothing. Each file is named anew, as Amane never decodes a path again once it failed; a newest rim that does not decode within 2 s, or whose file is gone, is captured again, once. A rim that cannot be written, as on a full disk, is not posted. The connection's thread only copies; a worker bends the newest capture of each pane, skipping older ones, so copying never waits on bending. In `glass.rs`, each pane's poster thread writes the rim, decodes it through `Image::loaded` (polled for up to 50 ms, as Amane only says when it landed), and posts it through the `Backdrops` Service once the pane shows the one before (or 50 ms passed); meanwhile the worker bends the next, and the poster then takes only the newest. Decoded on the pane's thread a rim would wait for a frame to start and another to show, and trail a scrolling backdrop by several frames; posted faster than the pane draws it would only be drawn unseen. One thread per pane, so one pane not drawing never holds up another's rims; while a pane did not show the rim before in time, as when hidden, its rims are posted without decoding ahead, as only its drawing frees what was decoded, and a poster that ended, as by a panic, is started again. The queues to the worker and posters are unbounded, as each drains to its newest at once. The pane draws it under the tint, decoding there only what the poster did not, one at a time, and keeping the last decoded one until the next is ready. Each pane's rims sit in a folder of their own, and a picture a pane still reads or decodes stays on disk. Amane frees a picture only once a window drew it and stopped, and may skip drawing a frame, so a decoded rim no longer shown, or decoded ahead and never shown, is drawn unseen for at least 3 frames and 100 ms, not much longer while its window draws at the refresh rate, as each costs the pane draw time and a scrolling backdrop replaces dozens a second; under a fullscreen window niri sends the Dock and Banners a frame only about once a second, so theirs linger a few seconds; a frame Amane skips past both may leave one held, so this is best effort. A pane is woken while one lingers, so it lets them go with nothing else redrawing it, and its window, even one with nothing else to show, stays up for one frame drawn without the last, as a hidden window draws none; over a fullscreen window the Island stays on the Overlay layer for it, drawing nothing else and taking no input, as niri all but stops the frames of a surface under the window. An unplugged output's decoded rims stay held, as nothing draws them again; at most a few per output. The PNG is stored, not compressed: compressing costs more to encode and decode than the smaller file saves on a tmpfs.
- **No blur.** Clear liquid glass shows the backdrop sharp, so nothing asks the compositor to blur. A blur through `ext-background-effect-v1` was tried: it needs a surface of its own under the Island, a restack to keep the Island above it, and niri's default xray blurs only the wallpaper, so the middle and the rim showed different backdrops.
- **No shadow.** The Island casts no drop shadow while liquid glass is drawn: it would gray the backdrop just outside the body, which the rim samples.
- **Dynamic tint.** The capture also gives the mean luma just around the body; the tint grows with it so light text stays readable.
- **Directional highlight.** `glass.wgsl` lights the rim from the top left: a glint on the corner facing the light, fainter on the far one, almost none along the sides, so the edge reads as glass catching light rather than a drawn outline. A small body, as the Rest, catches less, so the highlight never outshines the time. `appearance.highlight` scales the light on any material, the pointer's included: "subtle" (the default), "standard", "bright" or "off", which leaves only the faint dark outline light glass keeps on a bright backdrop to stay apart from it.

The glass starts only once a pane draws liquid glass. Where the screen cannot be captured or the connection is lost, `liquid-glass` looks `transparent`. The material was the Island's only; ADR 0024 puts it on the Dock, Banners and lock screen. Turned off and on again, liquid glass ignores the rims seen before.

`wayland-client` stays out of `src/` except `src/doctor/`; `src/boundary.rs` still enforces it there. `crates/glass` is the one other place it is used.

## Alternatives

**Compositor blur under the body.** See above; dropped for clear glass.

**Refraction in WGSL.** Amane's shaders cannot read a texture.

**An external tool, like `grim -g`.** ADR 0011's first resort, but it would start a process and write a file per frame, and cannot wait for the screen to change.

**Frosting an image of the wallpaper.** Shows the wallpaper, not the windows under the Island.

## Consequences

- Kanade continuously captures the screen around each Island showing liquid glass, and writes each changed rim as an uncompressed PNG to `$XDG_RUNTIME_DIR/kanade/rim`, at most 4 per output beside those the Island still reads; an unplugged output's are removed. Over video that is a capture, a CPU pass, a write and a decode per frame. Resource use is for later (`AGENTS.md`).
- ADR 0004's idle wakeups are for the other materials: with liquid glass, a backdrop or Island that changes wakes the glass's threads (connection, worker, and each output's poster) for each frame captured.
- The Island is inside its own capture, so while its content moves, as the media bars do, the glass captures and bends every frame, even over a still backdrop.
- A failed capture, as while the screen is locked, is retried only when the Island next draws; until then the last rim stays.
- A growing body is captured where it will be once its rim shows, ahead by how long rims have taken on that output, so the rim follows it through the morph; a shrinking one is captured out where it was a quarter of that before, as the screen captured lags the Island's frame, never sampling the Island inside it. A rim further from the body than a few pixels and about a tenth of its size is hidden, as while a body shrinks fast, and comes back as the spring slows.
- Liquid glass is the default material.
- A second Wayland connection is a second client to the compositor. niri lists its capture as a screen cast; Kanade ignores casts of its own process, so the privacy dot shows only others.
- Any output's new rim redraws every Island, as `Backdrops` is one Service. Resource use is for later.
