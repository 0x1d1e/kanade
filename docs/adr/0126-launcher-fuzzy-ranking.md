# 126. Launcher matches characters within its existing fit tiers

Status: accepted.

## Context

The Launcher's providers share `provider::fit`, which classified names only by exact, prefix, word, substring and terms matches. Abbreviations such as `vsc` could not find Visual Studio Code, and small jumps between letters were not ranked.

## Decision

Match names with a character-level dynamic program. A matched character scores 4 at the start of the name, 3 at a word start (a separator or recorded camelCase boundary), 2 elsewhere, and 1 when both query and name characters are separators. A jump between two non-adjacent matched characters costs 1 point, however many name characters it skips: `fx` in `Firefox` scores 4 - 1 + 2 = 5, not 4 - 5 + 2. Names and queries are lowercased and common Latin diacritics transliterated to ASCII; separators are tab, newline, space, hyphen, period, slash, parentheses and square brackets.

Use Raycast's medium sensitivity by default: a non-exact match must score at least `1.5 * (query length - 2) + 4`. Exact names retain the `Exact` fit. Existing `Fit` tiers remain the outer ordering, with the match score ordering answers only within a tier; stable sorting preserves provider order on ties.

## Consequences

Abbreviations and well-scoring character sequences can match across every provider that searches display names. The scoring is deterministic and depends only on the query and name. Providers that search additional metadata keep their existing behavior.
