# 87. The calculator converts units and bases from its own table

Status: accepted. Extends the Launcher's calculator provider (design.md, Launcher).

## Context

The calculator answered plain arithmetic only. Wanted: unit conversion (`10 cm to in`), number bases (`0xff`, `255 to hex`) and percentages (`20% of 50`). No currency and no network.

`fend-core` (MIT, no dependencies, 15k lines, 600 KB of source) was weighed. Size and licence are acceptable. It was not taken because:

- it answers almost any text (`m`, `c`, `a` are units or constants), so a Launcher that shows the calculator's answer first, on every keystroke, needs a gate that re-implements what the hand parser already knows, or it shows "answers" for app names;
- it replaces a small, bounded, tested parser whose results and twelve-digit rounding the user already knows with arbitrary precision and its own output format, and a power like `9^9^9` is its to bound, not ours;
- the three features are a table and about a hundred lines on the parser that is there.

## Decision

- `launcher/units.rs` holds the table: dimension, names (matched ignoring case, whole), and how many of the dimension's base unit make one. Temperatures go through Celsius with exact formulas, so 32 F is exactly 0 C. No currency, month or year. Gallons, pints and cups are US.
- Conversion is `<sum> <unit> to <unit>` (`in` and `as` read as `to`; the last such word splits, so `5 in in cm` works). The unit is the longest name the left side ends with at a word start (`fl oz`, not `oz`; `cm`, not `m`). Different dimensions, unknown units and a missing side show nothing. The row shows and copies the number and the unit (`3.93700787402 in`).
- Bases: `0x`, `0b`, `0o` literals up to 2^53 are numbers, and count as a sum on their own (`0xff` shows 255). `<sum> to hex|bin|oct|dec` writes a whole number from 0 to 2^53 as `0xff`, `0b..`, `0o..` or plain digits; anything else shows nothing.
- Percentages: `n%` is n/100; after `+` or `-` it is of the left value (`50 + 10%` is 55); `of` multiplies (`20% of 50`). A lone `50%` is no sum. `%` is never modulo.
- A whole number below 1e15 is shown whole, not rounded to twelve digits, so a base literal's value is not damaged by display.

## Consequences

- Wrong table entries are wrong answers, so the table has unit tests from definitions (`in` is exactly 0.0254 m; a gallon is 4 quarts) and a check that no name is two units.
- Adding a unit is one line. Currency stays out until the project decides to fetch rates.

Files: `src/surfaces/launcher/calculator.rs`, `src/surfaces/launcher/units.rs`.
