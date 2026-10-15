# 95. The semantic tree is the input targets, said by the views

Status: accepted. Extends [ADR 0028](0028-the-runtime-is-ported-from-amane.md) (the runtime owns input and the widget tree). Item #21.

## Context

A script or an agent could drive the shell only by coordinates (`ydotool`) and keystrokes (`wtype`), which race the compositor and break when a layout moves, and nothing could read what was on screen. Kanade draws with the GPU, so no toolkit builds an accessibility tree for it. The runtime already builds, each frame, a list of interactive `Target`s with their areas and handlers, in one flat `Vec` per window.

## Decision

- No second tree. A `Rectangle` may carry `Semantics` (`kanade_runtime::ui`): a role, a name, and optional label, content, level, text and states (`disabled`, `selected`, `checked`, `focused`, `secret`). Views set it with `.semantics(|| ...)` where they already build the rectangle that takes the click; the closure runs only while `ui.describe` keeps a tree.
- Collecting targets copies the semantics onto the rectangle's `Target` and records where its subtree ends (`Target::end`), so a window's nodes are its targets with a parent, which the flat list and `end` give without a second walk. Runtime `TextInput`s describe themselves.
- A `Node` is `id`, `parent`, `role`, `label`, `value`, `bounds`, states: the shape an AccessKit or AT-SPI adapter needs. This crate has neither and adds neither.
- An id is the path of the names of the nodes it is in, then its own (`controls/wifi`), and a later sibling of the same name is numbered (`item#2`), so an id holds while earlier ones stay. Names are `&'static str`: never text a user or program wrote.
- Roles: region, button, toggle, slider, textinput, list, item, menu, menuitem, tab. Bounds are in the window's logical pixels before the compositor's scale (after [ADR 0091](0091-ui-scale-at-the-runtime-seam.md)'s division).
- Coverage is what views have said: since this was written, every Surface, Banners, the Controls sub-surfaces and the Settings window have said theirs (one `.semantics` call each); what has none is listed in the wiki's Limitations page. A hold to confirm cannot be completed by `press` or `key`.

## Consequences

- The tree cannot drift from what takes input, and costs nothing while off.
- A control whose click handler is on an ancestor of the annotated rectangle is not pressable by name; it is reported as covered.
- Ids of list items without a public label are ordinal.
