# 105. Gestures are matched by a pure $1 recognizer

Status: accepted. Opens the gesture launcher (ADR 0105 to 0109).

## Context

`kanade gesture` needs to tell a hand-drawn stroke from a few recorded ones. It must run in the shell with no model files, no dependency and no thread of its own, and be testable without a window.

## Decision

- **The $1 unistroke recognizer** (Wobbrock, Wilson and Li, 2007) in `src/gesture/recognizer.rs`, pure Rust over `f32` points: resample to 64 points, turn the first point onto the x axis through the centroid, scale, move the centroid to the origin, and compare with each template by path distance at the best angle within 45 degrees (golden-section search to 2 degrees). Score is `1 - d / (0.5 * sqrt 2)`.
- **Uniform scale**, not the paper's non-uniform one: a straight line's sideways noise is not blown up to a square.
- **A match** needs a score of at least 0.85 and a lead of 0.04 over the best other gesture. Two gestures that close are ambiguous, and the stroke runs nothing.
- **A stroke under 60 px long or with under 24 px of reach is no stroke**: a click or a twitch never runs a verb.
- **Three strokes per gesture** at record time; a candidate's score is its best stroke's, so the user's variation counts.
- Recording does not compare with other gestures; two too close are told at draw time (ambiguous) and the user re-records one.

## Alternatives

**$P or $N (multi-stroke).** More code and more ways to match by accident; one stroke is the launcher's gesture.

**Hexecute's approach** (reference only): a stroke matched against shapes bound to commands. Kept the one stroke, dropped commands (ADR 0107).

**A crate.** None is small and maintained enough for 150 lines.

## Consequences

- Direction and start point matter; a line up and a line across are one gesture to the recognizer.
- Thresholds are tuned by reasoning and unit tests on generated shapes, not on real hands; they may need adjusting after use.
- Tested: circle match, rotation, scale and translation invariance, a different shape rejected, ambiguity rejected, accidental strokes.
