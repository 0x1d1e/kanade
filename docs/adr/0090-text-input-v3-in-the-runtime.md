# 90. Input methods through text-input-v3, in the runtime

Status: accepted. Fills a gap left by [ADR 0028](0028-the-runtime-is-ported-from-amane.md) (the runtime owns input); keys and compose stay as [ADR 0034](0034-key-repeat-and-toplevels-in-the-runtime.md) has them.

## Context

A text input took keys and, through xkb compose, dead keys, but a CJK or other input method (fcitx5, ibus) had nothing to talk to: no preedit, no candidate popup, no commit. niri serves `zwp_text_input_manager_v3`, and Kanade's text fields (Launcher, Clipboard, Calendar, Settings search, the Banner reply field) are where a user types.

## Decision

- The runtime binds `zwp_text_input_manager_v3` when the compositor has it (`wayland/ime.rs`, through the protocol crate SCTK already re-exports; no new dependency or crate) and makes one text input on the first seat with a keyboard. Without the global nothing changes.
- On the compositor's `enter` for a surface, and after each drawn frame of it, the runtime compares what the focused Kanade text input would tell the method with what it last told it, and sends `enable`, `set_surrounding_text`, `set_text_change_cause`, `set_content_type`, `set_cursor_rectangle` and `commit` only when something differs; it sends `disable` when no text input has the keyboard. Only a text input on the surface that has the keyboard is enabled.
- The caret rectangle is the focused input's caret mapped through its parents' transforms into the surface's logical pixels (`Target::caret`), one pixel wide.
- Surrounding text is at most 4000 bytes around the cursor, cut on letter edges. A password input (`TextInput::password`) is enabled with the purpose `password` and the hints `sensitive_data` and `hidden_text`, and is never given surrounding text. The lock screen is a separate crate and is not touched.
- `preedit_string`, `commit_string` and `delete_surrounding_text` are held and applied together on `done`, in the protocol's order: the old preedit goes, the deletion around the cursor, the commit at the cursor (over a selection), then the new preedit, shown inline at the cursor with an underline, the marked part tinted and the method's cursor as the caret. Offsets are bytes, clamped to the text and to whole letters; a commit or preedit over 4000 bytes is dropped, and both are made one line like a paste. A `done` that does not echo the commits sent is applied but nothing is sent until one does.
- While a preedit is shown the field takes no key and no paste: the method owns the keyboard then, and a key that reaches the field was not taken by it. The preedit ends when the method leaves, the input loses the keyboard, or another input is enabled.

## Consequences

- A key the method takes never reaches the runtime's key handler, so nothing is typed twice; xkb compose stays for apps and sessions without a method.
- A field that scrolls horizontally would need the caret offset scrolled with it; inputs are cut at their edge today, and the caret is clamped to it.
- Checked only against the protocol and unit tests of the pure part; the behavior against fcitx5 and ibus on niri is for a live session.
