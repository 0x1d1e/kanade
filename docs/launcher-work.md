# Launcher work: oma-cast features and a detached window

Source: user request, 2026-10-20. Reference implementation: `~/Projects/acknowledgements/oma-cast`.
Review with file and line citations: `~/Downloads/kanade-launcher-review-2026-10-20.md`.

## Order of work

Each step is one Felix task with its own verification. Step 1 is the only one that changes how results rank, so it goes first; the rest build on it.

1. **Fuzzy scoring.** Character-level dynamic-programming match behind the existing `Fit`/`ranked` seam in `src/surfaces/launcher/provider.rs`, so every provider improves at once. Scores: 4 at index 0, 3 at a word or camelCase start, 2 elsewhere, 1 for a separator-to-separator match, -1 per non-adjacent jump. Normalise: lowercase, camelCase boundaries, transliterate diacritics, separators `\t \n space - . / ( ) [ ]`. Sensitivity threshold with medium as the default, matching oma-cast's `Ranker.js:167-176`. Keep the existing `Fit` tiers as coarse sections so current behaviour does not regress, and use the score to order within them.
2. **Learned queries and decayed frecency.** Replace `count`/`last` in `src/sources/recents.rs` with a per-item anchor (`lambda = ln 2 / 10 days`, `+100` per launch, `score = max(1, exp(lambda*(anchor-t)/86400))`) and record the query that launched an item, valid 17 days, up to 20 terms per item. One state file, migrated from the current `launcher-usage` format, still `0600`. Track commands and destinations, not just apps.
3. **Empty-query sections.** With no query, list sections in this order: recents (by frecency), commands, windows, apps. `destinations.rs`, `commands.rs` and `windows.rs` currently return nothing for an empty query; section headers come from the existing list widget, not a new one.
4. **Action panel and quick picks.** `Ctrl+K` opens actions for the selected answer (run, its desktop actions, copy its path or id, pin). `Ctrl+1` through `Ctrl+9` press the nth answer. Enter keeps its current meaning.
5. **Web-search fallback.** With no match, offer "Search Google for <query>" and "Open <query> in the browser". Default browser only through `xdg-open`; Google is a fixed URL, never a configurable endpoint. Refused with a clear row when `xdg-open` is absent, following the existing `Need`/`Provider::Program` pattern. No query is logged, and the row states what it will send and to whom before the user presses it.
6. **Detached launcher.** `launcher.style = "island" | "detached"` in the existing `LAUNCHER` settings block as a `Kind::Choice`, default `island`, live-applied. Detached is an exclusive layer window centred on the focused output, reusing the existing providers and `Search::step`. Esc or focus loss closes it; it never holds the keyboard while closed, and it is mutually exclusive with the Island Surface.

## Constraints

- One writer at a time. Steps run in order, each verified before the next starts.
- `src/boundary.rs`: `island/` purity; no second Wayland connection; `wayland-client` only in `doctor/`, `crates/lock`, `crates/runtime`.
- ADR 0112 stands: the Launcher runs a closed set of Kanade verbs, never typed shell text.
- ADR 0019 stands: nothing marked sensitive enters the history.
- After each step: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- ADR for the ranking and frecency decision, and one for the detached window, since both change user-visible behaviour and touch state on disk.
