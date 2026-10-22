# 127. The Launcher ranks on decayed frecency and remembers the queries that launched an item

Status: accepted.

## Context

`launcher-usage` kept a `count` and a `last` per app. A count never fades, so a habit from a year ago outranked a new one, and the count only ever ordered apps; nothing was learned from the query that found an item, and Kanade's own commands and destinations were not tracked at all. The Launcher's empty query listed the last six starts, newest first, which is not the same question as what is used most.

## Decision

- Each item — an app (its `.desktop` id), a Kanade command (its verb) or a destination (`Destination::name`) — keeps one `anchor`, in Unix seconds. Its score at time `t` is `max(1, exp(λ · (anchor − t) / 86400))` with `λ = ln 2 / 10`, so it halves every ten days and is 1 once the anchor is ten days past.
- A launch at `t` moves the anchor to `t + 86400 · ln(score + 100) / λ`, `score` being the one at `t` before the launch, so the score rises by exactly 100 and never falls. The anchor first moves forward to at least `t`, so a clock that went back takes no score down, and the anchor is capped where `exp` would overflow. The anchor is stored to the millisecond, which is all the file holds and a billionth of a score.
- The query that launched an item is remembered too, lowercased with its whitespace collapsed, at most 20 per item, each valid 17 days and expired on read and on save. A remembered query lifts an item within its Fit tier: the query itself first, then a query the term starts, then a query that adds at most three letters to it. Nothing is learned from an empty query, and nothing is learned for an item whose score is back at 1.
- Within a Fit the order is the learned query's class, then the score, then the provider's own order. That score is ADR 0126's match score, used only as the tiebreak after the learned class, since it reads the title where a Fit may read an alias. Fit remains the outer ordering, so an exact name always leads. With no query the six highest scores are the Recents section, which ADR 0128 places first, before the Commands, Windows and Apps sections, so the section order rather than Fit orders them.
- One file, `$XDG_STATE_HOME/kanade/launcher-usage`, written whole through `atomic::write` with `Mode::Private` (`0600`). Its first line is `kanade-launcher-usage\t2`, and each item is its key, its anchor and its terms, tab-separated. A file without that head line is the older `count`, `last`, id form, or, with no `launcher-usage` at all, the older `launcher-recent` list of ids; either is migrated and written back in this format at the first read, so the old form is never read twice. A count becomes the score it stood for, and a later `last` only breaks a tie.
- At most 300 items are kept; the lowest scores go first, and the item just started always stays.

## Consequences

A habit fades on its own, so the empty query follows what is used now rather than what was used most. A query that led to an item finds it again for seventeen days, which is what makes abbreviations and odd spellings work without a second scoring pass. The file changes shape, so an older Kanade reads this file as a list of unreadable lines and keeps nothing until the next launch; a newer Kanade migrates an old file once. The query text is kept on disk (mode `0600`) and nowhere else: it is never logged, copied or shown, and ADR 0019 is untouched because the Launcher writes no clipboard or notification from it.
