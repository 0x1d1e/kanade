# 152. A query's separators are spacing when the name runs its words together, and a typo answers last

Status: accepted. Amends ADR 0126 (the separator row and the threshold of its score, and the tier classification of a name with no separator) and adds a Fit tier after `Anywhere`. Builds on ADR 0127 (Fit stays outer), ADR 0130 (the web fallback) and ADR 0135 (the `?` mode).

## Context

ADR 0126's dynamic program required every query character to match a name character in order. A query separator, though, can be spacing a name never spells: `libre office` for `LibreOffice`, `my app` for `MyApp`. With no separator in the name, the separator row had no match and became all `None`, so every row after it failed too; the early return `query.len() > name.len()` rejected some of the same queries before the program ran. Apps then fell back to their metadata words or to `Anywhere`. An unmatched trailing alphanumeric character still ends a match on purpose: `f b` is not a name match of `Firefox`, and the app's own entry (`Web Browser`) is what answers it.

A typo found nothing at all, so the web fallback (ADR 0130) turned `fierfox` into a Google query. A misspelling one or two letters from a name is an answer, not a search, and it must never stand in front of a real match.

## Decision

- **A query separator the name does not spell is skipped.** When the query character is a separator and the name contains no separator, the score's row is skipped: the previous row's scores carry forward unchanged, and the threshold's query length is the query's own length minus the skipped separators. The `query.len() > name.len()` early return is removed; a query longer than the name can no longer pass without skipping something, since a row consumes a name character. A non-separator row that finds nothing still ends the match.
- **The tier tests read the same query.** For a name with no separator of its own, `fit` drops the query's separators before its Exact, Prefix, Word and Within tests (and the folded Within test). `libre office` is therefore `Exact` for `LibreOffice`; `my app` is `Exact` for `MyApp`; `l i b` is `Exact` for `lib`. The score itself is unchanged: the six golden scores of ADR 0126 still read `fir` 8, `fx` 5, `lw` 6, `lo` 6, `vsc` 8 and `f-b` 7, because none of them skips a separator.
- **`Fit::Typo`, after `Anywhere`.** The variants keep their order, so a typo loses to every earlier tier, including `Anywhere`; commands still cap at `Prefix`, so a typo command is last too. `fit` remains the strict tiers. `fit_or_typo` is `fit` then `typo` of the name, for a provider whose only haystack is the name: commands' ordinary search, destinations by their names, quicklinks by title, window titles and dmenu lines. The calculator, an empty query and `?`'s command listing (ADR 0135) never use it.
- **Typo admission**, on case- and diacritic-folded text with separators and punctuation ignored: the query needs at least four alphanumeric characters; its first alphanumeric character must be the name's; digits are literal, a digit aligning only with the same digit; and the optimal string alignment distance must be greater than zero (zero is a real fit) and within the budget, one edit for four to seven alphanumeric characters and two from eight. Anything else is `None`.
- **A typo is an answer.** Its row has `filler: false`, so a typo keeps the web's rows away (ADR 0130); a filler, like a passing quicklink that matches nothing, still does not.
- **Order where metadata is searched too.** Apps, window titles and Settings rows try their name, then their existing word-anywhere fallback, then a typo of the name: a real metadata match beats a typo. The ordering is what the tier order would give anyway, made explicit where the fallback is not a Fit.

## Alternatives

**Skip every query character that finds no match.** That would admit `f b` for `Firefox` and also let trailing garbage find a name, without a typo needing to be a typo. Separators are the only characters that are spacing rather than content.

**Count the separators in the threshold.** The skipped separators are not characters the name was asked to spell; counting them would scale the threshold with how the query was spaced and reject `l i b` for `lib`.

**Fold diacritics into the Exact and Prefix tiers.** ADR 0126 keeps `Café` from being a prefix of `cafe`; that stays `Within`, and only the typo comparison folds.

**A second provider or a web row for typos.** A typo is a name fit and belongs in the same ranking, where it can never outrank a real answer and keeps the web fallback away.

## Consequences

A query spaced the way the name is not now finds the name in its own tier instead of a metadata or `Anywhere` match. A one- or two-letter misspelling of a name answers it at the last Fit, suppresses the web's rows, and never beats a real match. Learned queries and frecency still order only answers in the same tier (ADR 0127). The six ADR 0126 golden scores and the Fit order are unchanged.
