# 37. Liquid glass from GPU textures

Status: accepted. Supersedes ADR 0020's transport (its own Wayland connection, the CPU refraction, the PNG on disk and its decode); its material, light and tint stand. Amends ADR 0024 (panes are still named by a `Spot`, but a copy is a texture, not a rim picture) and ADR 0034 (the runtime now also captures). Amends ADR 0021 and ADR 0033: nothing raises the Island over fullscreen windows to draw rims.

## Context

ADR 0020 bent the rim on the CPU in `crates/glass`, wrote it as a PNG, and had the pane decode it, because a shader could not read a texture. The runtime is Kanade's own now (ADR 0028), so a shader can. The round trip cost a second Wayland connection, a worker and a poster thread per output, files on a tmpfs, a decode delay, and a hack to draw rims unseen over fullscreen windows (`Unseen`).

niri still advertises `zwlr_screencopy_manager_v1` v3, not `ext-image-copy-capture`.

## Decision

The runtime captures and the shader refracts.

- `kanade_runtime::backdrop` (`crates/runtime/src/backdrop.rs`, `wayland/backdrop.rs`) copies a `Region` of an output with wlr-screencopy over the runtime's connection into shared memory, converts it to RGBA, and keeps the newest copy of each `Spot`. A view `watch`es a `Request` (the region, a ring to read the light from, and rows of shader values) each time it draws; a request unchanged sends nothing. After a copy lands the next waits for the screen to change, unless the request moved. `release` drops a pane's copy, `pause` stops every capture (the lock), `capable` says whether the compositor can.
- `Rectangle::backdrop(Spot)` binds the newest copy to the rectangle's shader (`graphics/gpu/backdrops.rs` uploads it once per copy): texture at binding 4, sampler at 5. With no copy a 1x1 transparent texture is bound, so no capture means the transparent fallback, with no branch in the shader.
- The `Backdrops` Service holds, per `Spot`, the copy's light (mean luma on the ring) and the values it was asked with. A copy landing writes it for its `Spot` alone (`Write::part`), which redraws the windows that read that pane (`Service::read_part`), not every window showing glass.
- `src/glass/` decides what to ask (`capture.rs` the request, `body.rs` the outline and `ahead`, `pane.rs` the tint and the shader's values, `shader.rs` its file): the body (united with the Dock's plate when merged) out to the further of each edge of where it is and where it will be `LEAD` (33 ms) on (`ahead`), with a margin around it, so the copy never samples the Island as drawn now. `glass.wgsl` does the lens: it fills the band inside the edge with what is outside it, squeezed toward the edge, blue a little further than red, and fades into the backdrop. The shell echoes the body the copy was made for in the request's values, so the shader stretches the rim from the copied body to the one drawn now; a rim too far from its body is hidden (opacity 0), as before. The shader samples only outside the body, so its own pixels never feed back.
- The lock screen stays isolated (ADR 0025): `crates/lock` keeps its own connection and CPU refraction, now in `crates/lock/src/refract.rs`; `backdrop::pause` stops capturing while the session is locked.
- `crates/glass` is gone, and with it its `wayland-client` exception in `src/boundary.rs`.

## Consequences

- One Wayland connection, no PNG, no disk, no decode, no poster threads; a copy shows the frame it lands.
- A rim never draws unseen, so a covered Island is simply hidden and not raised to Overlay for rims; a Dock merged into the Island that is hidden is hidden too, and a Banner no longer lingers once its pane is idle.
- A copy is still a screen capture: the SHM copy and the upload cost CPU and bandwidth per changed frame. Resource use is for later (`AGENTS.md`).
- A pane's copy redraws only the windows that read that `Spot`, so another output's Island or the Dock does not draw for it; a write of the whole Service (`Write` without a part) still draws every reader.
- A window switching away from liquid glass releases its copies.
- niri lists the runtime's captures as casts of Kanade's own process, which the privacy dot ignores.
- A copy equal to the one held is dropped, so a still screen settles and the readers stop redrawing. Equal is within two levels a channel (`SAME`) and the light: the glass drawn at a copy's edge is in the next copy, off by a level or two each time, which kept a Crown's one pane capturing forever. The lock pauses capturing before it asks niri to lock and drops the held copies; a copy landing while paused is discarded.
- A failed capture is asked again by the next draw that watches the pane, not on a timer.
