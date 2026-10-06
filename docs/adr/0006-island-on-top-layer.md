# 6. Island on the Top layer, no fullscreen suppression

Status: accepted (design, roadmap 7A). Supersedes the `Layer::Overlay` choice in ADR 0001 and supersedes ADR 0002.

## Context

The Island is on `Layer::Overlay`, above fullscreen windows, so hiding it over fullscreen video or games was Kanade's job (plan rule 6). ADR 0002 found the geometry heuristic wrong in normal use and deferred rule 6 until niri reports fullscreen state. Until then the Island draws over every fullscreen window.

## Decision

The Island uses `Layer::Top`. niri draws a fullscreen window above the Top layer, so fullscreen covers the Island with no detection in Kanade. No fullscreen heuristic ships, and Kanade does not wait for niri's fullscreen state. Revisit only if niri reports it and a need appears that the layer cannot meet.

What must stay visible over fullscreen, the privacy cluster, lives on Overlay (ADR 0005).

## Alternatives

**Overlay plus niri fullscreen state (ADR 0002).** Correct once niri ships it, but it is Kanade code, an event to follow, and an Arbiter rule, for what the compositor's stacking already does.

**Overlay plus a geometry heuristic.** Rejected in ADR 0002.

## Consequences

- Under fullscreen nothing in the Island shows, Critical Activities included (critical battery). Only Overlay windows show.
- Rule 6 and its tests are dropped, not deferred.
- The fixed canvas, input region and keyboard handling of ADR 0001 are unchanged. Verify on niri that `OnDemand` and `Exclusive` behave the same on Top.
