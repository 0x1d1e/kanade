# 134. The emoji grid is a Surface, sharing the Launcher's table

Status: accepted. Builds on ADR 0028 (runtime), ADR 0030 (canvas follows the largest body, list Surfaces fit content), ADR 0038 (Services), ADR 0114 (Clipboard Surface), ADR 0133 (Strip Surface).

## Context

`TODO.md` asks for an emoji grid picker with recents as a Surface. The Launcher already finds emoji after a `:`, but that is a query interface: it answers what is typed and shows nothing before a character, so browsing the table by group is not what it does. Browsing wants the whole set, a title over each of Unicode's groups, and the ones used lately close at hand.

Two things had to be decided: where the emoji table lives, and what the Surface is.

## Decision

- **One table, two interactions.** `src/surfaces/emoji/data.rs` holds the table: every emoji in Unicode's order with its name and shortcodes lowered once (`LazyLock`), and each group with the range its emoji take. The Emoji Surface lists it, and the Launcher's `:` provider reads the same table. It moved out of `launcher/emoji` so neither interaction owns it; the provider's matching, ranking and row shape are unchanged.
- **`Surface::Emoji`, one scrolling grid on the largest body** (`island.width` by `island.height`, 520×400): the body is capped by what the content asks, so the Surface is as tall as the grid's room, never taller (ADR 0030). Nine 48px tiles with their gaps fill the content's width, and the grid scrolls under a scrollbar. Not tabs: the whole table is one scroll away, and a group's title is a heading rather than a mode to leave.
- **The recents are a row above the first group**, headed `Recent`, holding what was copied here, newest first, up to 24, in `$XDG_STATE_HOME/kanade/emoji-recents`, owner-only. A file from a newer Kanade is left as it is; a damaged one is kept as a `.bak`.
- **Search as it is typed**, by name and shortcode, every term somewhere; a new query starts the list and the scroll at the top. A press copies the emoji through `wl-copy` off the draw thread and collapses the island once it is on the clipboard, recording it in the recents; a copy that fails leaves the Surface open saying `Not copied`. Space types, so only Enter presses, as in the Launcher and the Clipboard.
- **A tile draws the emoji's picture**, written once per tile from its color font, or the emoji as text where the font has no picture of it.

## Alternatives

- **Tabs per group.** Rejected: a group behind a tab is a mode to enter and leave, and the scroll reaches it in one gesture.
- **A separate emoji crate beside the Launcher's provider.** Rejected: two tables drift, and the two interactions would offer different emoji.
- **Recents in the clipboard history.** Rejected: the Clipboard keeps text and images with its own bounds and its own Surface; an emoji copied here would arrive as an unrelated entry.

## Consequences

- `CONTEXT.md`'s Surface list gains `Emoji`; the Module is `emoji-surface`; CLI gains `kanade emoji open|close|toggle`; the Launcher gains an Emoji destination gated on that Module.
- The Launcher's `:` provider is removed, not kept: `:` now opens the Emoji Surface itself (ADR 0135), which reads the same table.
- One unit test covers the table's shape and the matching rule, since a table of outside data and the rule that reads it are data integrity; the grid's layout and look are not tested.
