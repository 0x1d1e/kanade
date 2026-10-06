# 10. Split Presentation and Rest right click

Status: accepted (design, roadmap 9 Shell essentials). Changes the `CONTEXT.md` Presentation, Satellite and Pin entries when implemented.

## Context

`docs/design.md` names a Split Presentation, "Compact showing the primary and the top Satellite in one split body", but never says when it starts or ends, how it morphs, or what the pointer does on it. Today Satellites are 28 px dots beside the body that take no pointer, so a secondary Activity (a timer beside media) can be seen but not peeked or opened, and a dot is too small for more than a short label.

Right click at Rest does nothing (`CONTEXT.md` Pin), while `docs/design.md` says "Right click → context/pin" without defining "context".

## Decision

### Split

- **Entry and exit follow the Frame, never input.** An island that would be Compact is Split while its Frame has a Satellite, and Compact again when it has none, just as Rest and Compact follow the primary. The user never raises or lowers an island to Split.
- **Body.** One wider pill in two segments: the primary on the leading one, the top Satellite on the trailing one. The other Satellites and the overflow count stay dots beside it, so the `SATELLITES` cap counts the segment.
- **Morph.** Compact → Split: the body widens and the top Satellite's dot slides into the trailing segment as it fades into it; Split → Compact runs the same motion backwards. A primary change that swaps the two segments slides each to its new place. A Presentation change still moves geometry and crossfades content in one motion.
- **Hover.** The segment under the pointer when the hover delay ends is peeked. The Peek shows that Activity alone. The other segment tucks under the body, the way Satellites do when the body grows, and comes back when the Peek ends.
- **Peek identity.** A Peek shows one Activity, named by identity, not by slot. It lasts while that Activity still shows on the island as primary or top Satellite. A Satellite that becomes the primary keeps its Peek, and one that leaves both ends it, pinned or not.
- **Click.** A click on a segment opens that Activity's Surface (`Surface::of` its Kind). On a Peek it opens the peeked Activity's Surface.
- **Right click.** A right click on a segment peeks that Activity pinned. On a Peek or a Surface it toggles the pin as it does today.
- **Collapse.** Collapsing a Peek or Surface returns to Split while the Frame still has a Satellite, otherwise to Compact or Rest.

Which Surface opens is still a pure function, now of (Presentation, the Activities shown, the segment under the pointer, request). Nothing is remembered.

### Rest right click

A right click at Rest opens Controls pinned. Everywhere else right click means the same thing: raise the island to what the pointer would raise it to, and pin it. Compact raises to its Peek. Rest has no Peek, so right click goes to what a click opens. A Pin on Controls at Rest lasts as any Surface pin does, and Escape, a second right click, collapse or another Surface ends it.

There is no Rest context menu. Its candidates (Launcher, Notifications, session, settings) already have Surfaces, the CLI, or Controls tiles. A menu would be a second, hidden route to them and a new UI component with its own focus and dismissal rules.

## Alternatives

**Drop Split; keep Satellites as dots.** Simplest, but a secondary Activity stays out of pointer reach and squeezed into a 28 px label.

**Make Satellite dots pointer targets instead.** The pointer could reach a secondary Activity, but the targets would be small, the input region would grow beyond the body (ADR 0001), and nothing would gain room to draw.

**Split as a user-raised level.** It would add state to remember and clear, which Compact, derived from the Frame, does not need.

**Leave right click at Rest a no-op.** Consistent but wasted; the "raise and pin" reading costs nothing and needs no new UI.

**A Rest context menu.** Rejected above.

## Consequences

- `Presentation` gains `Split`, and `Peek` carries the identity of the Activity it shows. `Presentations` learns the top Satellite's identity and Surface alongside the primary's.
- Geometry gains a `SPLIT` shape, and the body gets one hover and click target per segment. The input region still follows the body.
- `satellites.rs` stops drawing the top Satellite as a dot while the island is Split, and hands it to the segment and back.
- `CONTEXT.md` Presentation, Satellite and Pin change with the implementing PR: the "Rest does nothing" Pin rule becomes Controls pinned, and Peek is no longer only the primary's.
