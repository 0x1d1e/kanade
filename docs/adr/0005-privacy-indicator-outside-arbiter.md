# 5. Privacy indicator outside the Arbiter

Status: accepted (design, roadmap 7A-8); the window is amended by ADRs 0032 and 0033 (no Overlay guarantee over a fullscreen window). Supersedes the Activity half of ADR 0003; its PipeWire detection stays.

## Context

ADR 0003 made mic and camera use one Persistent Critical Privacy Activity, and niri casts a Persistent ScreenCast Activity. Both compete with everything else for the primary and Satellite slots, and both show only where the Island shows.

ADR 0006 moves the Island to `Layer::Top`, so a fullscreen window covers it. A capture indicator must never be covered: an app that records the mic under a fullscreen game has to stay visible.

## Decision

The `privacy` module owns a separate `Layer::Overlay` window per output, the privacy cluster. It shows mic, camera and screen capture. It never posts an Activity and never enters the Arbiter. It reads capture state itself: PipeWire per ADR 0003, niri casts per `docs/archived/plan.md` §5.3. It does not read the `audio` or `capture` modules, so disabling either never hides an indicator.

`privacy` is on by default. Disabling it warns that the indicators disappear.

## Alternatives

**Keep privacy as a Critical Activity.** Hidden under fullscreen once the Island is on Top, and it takes the primary from Media whenever any capture runs.

**Keep the Island on Overlay.** Rejected in ADR 0006.

## Consequences

- A second layer window per output, as ADR 0001 foresaw for a surface with different needs. Its input passthrough and stacking against the Island need a test.
- The Privacy and ScreenCast Kinds leave the Arbiter. `CONTEXT.md` (Satellite examples, Lifetime list) changes with that code.
- What is and is not detected is unchanged from ADR 0003.
