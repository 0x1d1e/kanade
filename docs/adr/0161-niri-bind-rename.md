# 161. A key binding's chord can be renamed

Status: accepted. Builds on [ADR 0119](0119-kanade-edits-nieris-config-on-request.md) (the text
engine) and the niri page's staged Save.

## Context

niri 26.04 writes a bind as the chord itself: `binds { Mod+Return allow-when-locked=false {
spawn "kitty"; } }`. The node's name is the chord; its only child is the action; its properties
(`allow-when-locked`, `repeat`, `cooldown-ms`) sit on the node, before the block. Editing any of
that by regenerating the bind loses the properties and can move comments. The path engine cannot
address one either: `binds` is a block whose children are blocks, and the terms are single-valued
nodes with at most one value, so `binds.Mod+Return` is not a path a setting can take.

Changing a chord is the one bind edit users make routinely. Kanade already edits niri's file by
splicing bytes, so the rename belongs there rather than in a hand edit.

Key capture is deliberately not part of this. `Window::on_key` is `Fn(Key)`, and `Key` and
`Modifiers` collapse F-keys and non-standard modifiers, so a chord read from a press would be a
chord niri cannot be told about. Capturing would offer binds Kanade cannot write.

## Decision

- **Only rename.** `kanade niri-config rebind <old> <new>`, and a "Key bindings" section on the
  Settings niri page. Nothing adds or removes a bind, and no action or property is edited.
- **The rename re-finds the bind's leaf and splices its name token.** The bind is found by the
  path `binds.<chord>` like any setting; `Op::Rename` replaces only `[name_start, name_end)`, the
  span of the node's name. The action, its properties, a `/-` on the node, comments inside it,
  indentation, line endings and the one-line or multi-line form stay byte for byte. A new chord
  equal to the old one writes nothing.
- **A new chord must be a bare KDL v1 node name**: letters, digits, `-`, `_` and `+`, not starting
  with a digit, which `writable_name` already checks. `Mod+B` and `Mod+Shift+q` are accepted; a
  space, a quote or anything else KDL cannot write bare is refused before anything is written. A
  bind the file writes *quoted* is listed and renamed by its name's value, and the new chord is
  written bare, which KDL accepts for those characters.
- **The editable set.** `Editor::bindings()` lists the binds of the config itself, with the chord
  as the file writes it, the action text and the owner file. An include's bind and a `/-`-disabled
  bind are not in it: the engine does not edit them. The CLI prints one chord and its action a
  line, tab-separated.
- **Duplicates are compared after `Mod` is resolved.** `Mod` is the file's `input { mod-key }`
  (Super when it sets none), and two chords collide when they resolve to the same modifier set and
  keys, using `sources::binds::{keys, same, resolved}` — the parser the Keys Surface and the
  niri source already use, so there is no second chord parser. `Mod+Q` therefore collides with
  `Super+Q` and not with `Shift+Q`. Key names are not aliased beyond what that parser does:
  `Mod+Enter` and `Mod+Return` stay distinct spellings.
- **Refusals write nothing.** A chord only an include binds names the file; a `/-` bind says
  Kanade does not edit turned-off binds; a chord another bind already takes names that bind's
  spelling; a name KDL cannot write, and a file that changed since it was read, are refused too.
- **The write is the engine's.** Read, check, splice, `niri validate`, the backup ring, the lock,
  the symlink and hard-link refusal, and Undo are the same as for a setting.
- **Settings stages.** The page lists the editable binds with a text field per row; a chord and
  Enter stages the rename, Save writes, Undo restores the bytes, and a refusal shows in the footer
  with the file untouched.

## Consequences

- A chord can be renamed from the CLI or the niri page without losing the bind's action,
  properties or the comments around it.
- Moving a bind's action, adding a bind, renaming an include's or a `/-` one, and editing
  properties still mean editing the file by hand. That stays in [Limitations](../wiki/Limitations.md).
- A config that sets `input { mod-key "Alt" }` gets `Mod+Q` compared as `Alt+Q`, as niri reads
  it.
- A Save that stages a setting and a rename writes each through the engine in turn: one backup
  per write, and Undo is the last. A failure after an earlier write says what was written, keeps
  that write's Undo, and drops the written rename from the staging, so a retry renames only what
  is still there.
- **Not run live**: the page's rows and fields were not seen on an output (E2E NOT RUN, no
  session). The splice, the normalization, the refusals and the page's staged write are unit
  tested.
