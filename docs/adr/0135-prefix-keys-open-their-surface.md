# 135. Prefix keys open their Surface

Status: accepted. Supersedes ADR 0134's claim that the Launcher's `:` provider is kept, ADR 0114's `>` ordering in the Launcher, and ADR 0130's `:`, `>` and `@` prefix exclusions. Builds on ADR 0112 (the closed command table), ADR 0131 (the Launcher in a window of its own) and the withheld-Surface invariant in `CONTEXT.md`.

## Context

The Launcher's prefixes were queries. `:` searched the emoji table, `>` searched the clipboard history, `@` answered one Wallpapers row, and each provider had to skip the others' prefixes so they never mixed. Three Surfaces now do those jobs properly: Emoji shows the whole table in a grid with recents (ADR 0134), Clipboard shows the history with pins, previews and its own ring (ADR 0114, 0115), Wallpapers shows the pictures. Typing a prefix to get a poorer copy of a Surface that already exists was an extra step for the same content.

The command table (ADR 0112) had no way in of its own: a command was found by typing its name and competing with the apps for the same letters, and there was no way to see every command at once.

## Decision

- **Four prefix keys.** `@` opens Wallpapers, `:` opens Emoji, `>` opens Clipboard, and `?` lists every command.
- **`@`, `:` and `>` hand off to the Surface of the same name at once.** The key is consumed: the Surface opens with an empty search and the next key reaches its field. The hand-off happens when a *typed* query is exactly the prefix, so a longer query like `:smile` is never built in the Launcher — the Surface opens on the first key. A prefix pasted with `Ctrl+V`, or one a deletion leaves as the whole query, stays an ordinary search: only the key that typed it hands off, so a lone prefix answers nothing and is never offered to the web (ADR 0130). The Surface opens only while its Module is on; with the Module off the character stays an ordinary search, following the withheld-Surface invariant — a Surface that is withheld never opens this way.
- **`?` is a mode, not a Surface.** It ranks the closed command table (`commands::TABLE`, ADR 0112) alone, filtered by what follows it, with no other provider and no web fallback; `?` alone lists every command. It is not a Surface because the table is a list of verbs, not a grid to browse, and the Launcher is already the place a verb is pressed.
- **The in-Launcher emoji provider and the clipboard-history provider are deleted**, since their Surfaces replace them. `Mark::Emoji`, `Action::Paste`, `emoji::PREFIX`, `history::PREFIX` and `destinations::WALLPAPER` go with them, and the providers no longer skip another's prefix.
- **A hand-off from the detached Launcher collapses that window first** (ADR 0131), so exactly one window holds the keyboard. In the island host the Surface is switched in place: the island expands on it, which starts a new visit and unpins it — exactly as pressing a destination row did before this change (ADR 0131). A hand-off from a pinned Launcher therefore unpins it; consistency with a pressed destination row is the point, not keeping the pin.

## Alternatives

- **A prefix kept as a query with a row to press.** This is what was there: the user typed `:smile`, moved to a row and pressed Enter, for a Surface that would have opened on `:` alone. It costs an extra Enter for the same content.
- **Opening the Surface and leaving the Launcher's query behind.** Two states to keep in step: the Launcher would reopen with a stale query, and the Surface's own field would disagree with it.
- **A commands Surface.** The table is a closed list of verbs, not a grid to browse; `?` in the Launcher reaches it in one key, and a Surface would add a Module, a verb and a window for a list the Launcher already draws.

## Consequences

- The Launcher no longer writes the clipboard itself: `Action::Paste` is gone, and the Clipboard Surface is what puts an entry back.
- A pasted lone prefix, or one a deletion leaves as the whole query, is a plain search with no answers: only a typed prefix hands off.
- `kanade ui describe` still names the Launcher's field, and the placeholder names the four keys, each gated on its Module.
- `CONTEXT.md`'s LauncherProvider section loses emoji and clipboard history from the provider list and gains the four keys as invariants.
