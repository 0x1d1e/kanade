# 82. DDC/CI brightness for external monitors

Status: accepted. Builds on [ADR 0004](0004-wake-sources-on-announcements.md) (wake sources).

## Context

`Brightness` set the first `/sys/class/backlight` device through logind. A desktop or a laptop on an external monitor has none, so the slider was hidden. Monitors take their luminance over DDC/CI on the I2C bus of the connector: VCP feature `0x10`. References read: DankMaterialShell's `brightness/ddc.go` (native i2c, `I2C_SLAVE` 0x37, 50 ms between request and reply) and Noctalia's `brightness` docs (`backlight|ddcutil` per monitor).

## Decision

- Native, not `ddcutil`: `sources/ddc.rs` writes the DDC/CI frames itself, to `/dev/i2c-N` opened and addressed (`I2C_SLAVE`) in `clock::open_i2c`, the one place of `unsafe` (rustix `IntegerSetter`). No helper process, no dependency.
- Frames: write `51, 0x80|len, payload, sum` with sum seeded `0x6e` (get: payload `01 10`; set: `03 10 hi lo`); reply `6e 88 02 result feature type max(2) current(2) sum` with sum seeded `0x50`. The reply is refused when short, not a feature reply, badly summed, unsupported or for another feature, or when max is 0. Encoding, parsing and the percent/value conversion are pure and unit tested; nothing in the tests touches a bus.
- `brightness.backend` = `auto` (backlight, else monitors), `backlight`, or `ddcci`. The value is `ddcci`, not `ddc`: `boundary::colors_stay_in_theme` reads a quoted `"ddc"` as a hex color. Restart to apply.
- Monitors are connected, non-internal DRM connectors (`/sys/class/drm/cardN-<connector>`, not `eDP`, `LVDS`, `DSI`) whose `ddc` link names an `i2c-N` adapter; sysfs only, looked for again every 10 s for a monitor plugged in later. The one shown and set is the monitor whose connector is the focused output's name (niri names outputs by connector); else the last one.
- All bus traffic is on a lazily started `ddc` thread that blocks on a channel, so it wakes nothing while unused or idle, and never runs under a Service lock or the draw thread. A level is read once per monitor (not polled: DDC traffic can upset monitors and takes ~50 ms); a set is remembered at once and written there, the newest per bus only, 50 ms after the last operation. Each transfer is bounded by the kernel adapter's timeout; after a failure the bus is left alone for 10 s. The bus is `flock`ed non-blocking, as other DDC tools do. Failures log the bus and reason, never text from a monitor.
- Never level 0: the value is at least 1.
- `kanade brightness set|up|down|status` (Module `brightness`) acts on the same target, so a keybind can drive a monitor; the Controls slider and `kanade osd brightness` read the kept level.

## Consequences

- A change made with the monitor's own buttons is not seen; the first level shown can be a few seconds late.
- Needs `i2c-dev` and access to the node; without either, the slider stays hidden, as before.
- Not tested against a live monitor or a real bus in this release: the framing follows the references and the VESA description.
- Files: `src/sources/ddc.rs`, `src/sources/brightness.rs`, `src/sources/osd.rs`, `src/clock.rs`, `src/config.rs`.
