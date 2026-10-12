# 46. Night light through wlsunset

Status: accepted.

## Context

A warm screen at night needs gamma tables set on each output through `wlr-gamma-control`. niri hands gamma control to one client at a time and restores the output's gamma when that client goes away. `wlsunset` is that client, small and widely packaged. Setting gamma from the runtime would mean a second Wayland protocol client in the shell process for what one helper already does well, and its sun maths and transitions.

wlsunset 0.4 has no "always on" switch: it fades between a low and a higher temperature on a schedule, from `-l`/`-L` (the sun at a place) or `-S`/`-s` (fixed times), and refuses a high temperature not above the low one.

## Decision

- The `night-light` Module, on by default, holds one `wlsunset` while `night-light.mode` is `on` or `schedule`, started from its own long-lived thread through `setpriv --pdeathsig KILL`, so it dies with Kanade and never leaves the screen warm. Off kills it, and niri restores the gamma.
- `on` runs `-t <temperature> -T <temperature + 1>` with fixed times, so day and night differ by 1 K. `schedule` runs `-t <temperature> -T 6500` with `-S night-light.sunrise -s night-light.sunset` when both are set, else with `-l`/`-L` from `weather.location`; with neither, it says what is missing and runs nothing.
- Any config change that alters these arguments starts wlsunset again. One that ends on its own runs again after a backoff, 1 s doubling to 60 s, reset after it ran 60 s. Its first failure in such an episode shows once as a Transient Failed `night-light` Activity with what it said on stderr; a missing program or a config it cannot run is not retried until the config changes.
- The Controls Night Light switch, `kanade night-light on|off|toggle` and Settings all write `night-light.mode` to the settings file and reload at once, so the choice survives a restart. Turning it on restores the mode the config files below the settings file give, else `schedule` when both times are set, else `on`.
- `kanade doctor` names a missing `wlsunset` or `setpriv`; the switch then says it needs wlsunset and does nothing.

## Consequences

- One more helper process while on, idle between transitions.
- Another gamma client (gammastep, a wlsunset run by hand) conflicts: the compositor gives an output's gamma to one client at a time, so whichever starts second fails; if that is Kanade's, it shows so once and keeps retrying with the backoff.
- With `on`, wlsunset still fades by 1 K at its fixed times, which is invisible.
- A location change in `weather.location` moves the schedule without the `weather` Module being on.
