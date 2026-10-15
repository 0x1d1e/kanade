# 71. A failed niri config load shows as a failed Live Activity

Status: accepted. Builds on [ADR 0038](0038-niri-is-a-service-its-readers-watch.md) (niri events a Module takes by a hook) and [ADR 0042](0042-live-activities-from-outside.md) (the failed form of a Live Activity).

## Context

niri's EventStream sends `ConfigLoaded { failed }` on connecting, for the last load, and after each reload. A config with a parse error leaves niri on its previous config, and niri's own notice is a small red banner from the compositor that is easy to miss, and gone when Kanade starts after it. Kanade shows nothing, so a bind or rule that did not apply looks like a bug in Kanade.

## Decision

- `sources/niri.rs` offers `on_config_failed(then)`, a hook like `on_layout`: `then` runs on the niri thread for each `ConfigLoaded` with `failed: true`, the one on connecting included. A load that succeeded runs nothing, so a shell started on a good config shows nothing.
- The `live` Module registers `live::niri_config_failed` in its start. It posts the failed form of a Job (`Ended::Failed`, title "niri config", detail "run niri validate") under id `niri config`, a Transient Actionable on the focused output for `SHOWN`, the same as a command that failed under `kanade activity run`. A space is outside a client's id alphabet, so no client can start, end or replace it; the live board never holds it.
- No Arbiter change, no new Kind, no Surface. Privacy never enters it.
- The text is fixed: niri's error message, which may quote the user's config, is not read or shown.

## Consequences

- With `live` off, a failed reload shows nothing.
- The stream is not reconnected after a loss (a restart of niri), so a niri restarted onto a broken config shows nothing until Kanade restarts; with it, the failure on connecting shows once.
- Each failed reload shows again; a repeat within the lifetime refreshes the same Activity.

Files: `src/sources/niri.rs`, `src/sources/live.rs`, `src/modules/catalog.rs`.
