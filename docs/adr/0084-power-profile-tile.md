# 84. A Power Profile tile, and the tiles left out

Status: accepted. Builds on [ADR 0067](0067-arrangeable-controls-tiles.md) (Controls tiles).

## Context

Controls has switches for Wi-Fi, Bluetooth, Caffeine, Do Not Disturb, Night Light, Focus and Limit Charge, and a profile bar for the power profile in the Battery area. The brief asked for more tiles: Power profile, Airplane mode, and Hotspot or VPN where already exposed.

## Decision

- A tile `power-profile`, off by default (add it to `controls.tiles`), icon Bolt, status the active profile's name, on while the profile is not Balanced. Pressing it asks for the next profile the machine has, wrapping, through `power::set`, as `kanade power-profile cycle` does. It shows only while the `power` Module is on, and is not pressable while the daemon is absent or has one profile. The profile bar keeps choosing a specific one.
- Airplane mode is not built. It would need rfkill writes for Wi-Fi, Bluetooth and WWAN together with the Network and Bluetooth Modules' own state, and a half-done switch (one radio left on) is worse than none. There is no single safe write path to reuse, and it cannot be tested without toggling the user's radios.
- Hotspot and VPN are not built. The network source exposes neither (it reads the first Wi-Fi device and its access points), so a tile would need a new NetworkManager connection model first.

## Consequences

- No new Service, source or wake source; the tile reads `Profiles`.
- Airplane mode, Hotspot and VPN remain open; each needs its source first.
