# 129. The Launcher's actions are a panel, and Ctrl+1..9 press the nth answer

Status: accepted. Builds on ADR 0112 (a closed set of Kanade verbs) and ADR 0128 (sections).

## Context

The Launcher answered a query and pressed one answer. What else an answer could do was reachable only elsewhere: a browser's Desktop Actions only by typing their name, an app's path or id only by reading it, the Dock's pin only from the Dock. The plan (`docs/launcher-work.md`, step 4) asks for a panel of actions over the selected answer, and for `Ctrl+1` through `Ctrl+9` to press the nth answer without moving the selection.

The keyboard was the obstacle. The Launcher's field is drawn by the Surface itself, not registered as a runtime text input, so `on_key` receives a ctrl chord as the letter it types: `Ctrl+K` arrived as `k` and was typed. Registering the field with `focus::set` would have made every Launcher keystroke go through the runtime's text-input widget, which also backs Settings' real `text_field`s; the Launcher's field is not one.

## Decision

- **The runtime names a ctrl chord.** `Key::Shortcut(char)` (`crates/runtime/src/input.rs`) is a key whose own letter, with ctrl held, names a shortcut. `deliver_to()` in `crates/runtime/src/wayland/keyboard.rs` maps `ctrl+letter` to `Shortcut(letter)`, `ctrl+a` keeping `SelectAll` as it was. `focus::send()` forwards `Shortcut(_)` to the window's `on_key` rather than the focused text input, so a real input never swallows it. Nothing else about a text input changes.
- **`Search::step` answers with an `Intent`, not a `bool`.** `Intent::{Idle, Selected, At(usize), Offer, Panel}` says what the key asked for: nothing, the answer the search selects (Enter), the nth answer (`Ctrl+1`..`Ctrl+9`, none past the list's end), the row of the open panel its ring is on, or the panel of the selected answer (`Ctrl+K`). `Search` gained `panel`, `chosen`, `panel_offset` and `note` beside the query, selection and scroll it already held.
- **The actions are their own module, `src/surfaces/launcher/actions.rs`.** `Offer { label, detail, act: Option<Act> }` is one row, `Panel { offers }` the rows of one answer, and `Act::{Press(Action), Copy(String), Pin { id, pinned }}` what pressing a row asks for. `actions::of` builds them from the answer, the apps as they are now and the Dock's pins as they are now: pressing the answer, each of its app's own Desktop Actions, copying the path or id that names it, and the Dock's pin or unpin. A row that cannot act carries `act: None` and says why in its `detail`; it is dimmed and takes no press.
- **The panel is a level of the Surface, not a Surface.** It draws in the list's place, with its own ring, scroll, wheel and clicks, and a note under its rows for a pin the settings file refused. Enter or Space presses the ringed row and `Ctrl+1`..`Ctrl+9` press the nth; Escape and `Ctrl+K` close it for the list, as does a key no row takes, so the list is never typed under a panel.
- **Escape reaches the Launcher.** `src/view/input.rs` had the island collapse on Escape before the Launcher's `key` was offered it, so the panel could never be left for the list. The Launcher's `key` now runs before that collapse, as Controls, the Tray and the Calendar already do, and only Escape with no panel open falls through to collapse the island.

## Consequences

`Ctrl+K` and `Ctrl+1`..`Ctrl+9` are the Launcher's alone; a Surface that wants a ctrl chord of its own reads `Shortcut(letter)` and names it, and no runtime text input sees one. The panel reads the apps and the pins when it opens, so it says the pin the Dock holds rather than the one it held when the Launcher opened. A pin is written through the settings file off the view thread, as the Dock's own menu writes it, and a refusal is said under the rows.
