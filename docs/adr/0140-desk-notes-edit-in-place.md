# 140. Desk notes: the one Desk face typed into in place

Status: accepted. Builds on [ADR 0137](0137-the-desk.md) (the Desk) and amends its `Keyboard::None` always, for this face only.

## Context

ADR 0137 made every Desk face read-only and deferred a text field, and any focus on the Desk's window, to its own ADR with its own E2E. The notes were asked for next: a person's own few lines on the desktop, typed where they are read, without opening a window. That makes the Desk take text, which needs a keyboard, which needs this decision written down before the code.

The Desk's window is `Layer::Bottom` with `Zone::Ignore`, and ADR 0137 refused `Keyboard::OnDemand` there because it believed it would drop the user's toplevel focus. A layer surface does not take the toplevel focus that niri keeps for windows: it is focused by niri, and the window below it gets its focus back afterwards. The rest of ADR 0137's interaction rules stand: only drawn controls take the pointer, through an input region, and the text around them stays click-through.

## Decision

- **`desk.notes`, off by default**, per output, in the Desk's Shell settings section beside the other faces. Off, the card is not drawn, the file is not read and no input region is built for it: a click where it would be goes to the window below, and the Desk stays as it is.
- **The file.** `$XDG_STATE_HOME/kanade/desk-notes`, one note per line, written atomically (`atomic::write`) and readable by its owner only (`0600`). A note is one line: a newline, tab or return in one is a space, and it is held to 500 characters. The file holds at most 200 notes and 64 KiB; past that the oldest note goes. A file that cannot be read, or is larger than 64 KiB, is ignored and the card is empty rather than anything crashing — another program may have grown it. Note text is never logged, in a failure or anywhere else.
- **The card is a row of the Desk's own layout**, under the track line and above the media buttons, in the Desk's type, size and colour: the newest note last, up to `SHOWN` (5) lines, drawn in a box of fixed height, so it overlaps none of the faces above it at any size of the Desk. A note too long for its line is drawn with an ellipsis; the file keeps it whole. An empty card says what a click does.
- **Only the card's own rectangle takes the pointer.** It is the second input region the Desk builds, beside the media buttons' circles: `view::input_area` over the card's box, and nothing else in the window. The rest of the Desk stays click-through, as ADR 0137 says.
- **`Keyboard::OnDemand`, on the card alone.** The window takes the keyboard while the pointer is over the card, so the press that follows lands on a surface asking for it, and for as long as a card is being typed into; otherwise it is `Keyboard::None`, as before. It is never `Exclusive`: another window may always take the focus back. The card never asks for the keyboard on its own: no face, no hover and no clock tick focuses it.
- **A click starts a note; the keyboard leaves and it is kept.** A left click on the card starts a note of its own under the ones the file holds, shows a caret after it and takes what is typed. Enter starts another note, Backspace takes one letter and then the note's own line break, Escape leaves editing and saves, and any other key is left alone. A click anywhere else, another output's card, niri's overview opening, or the face or the Module turning off ends editing on the same path: what was typed is kept and saved. The event is the window's own `on_lost` (the keyboard went elsewhere) and the config and overview the window is already rebuilt on, not a new Service.
- **Writing happens off the draw thread.** The `desk` Module starts one worker thread, `desk-notes`, with the notes file as its job: the draw thread only posts a snapshot of the notes, while the worker reads the file when a card first draws and writes a snapshot when it is asked to. A snapshot is taken at most about once a second while typing and once more when the card is left, Escape included, so no keystroke waits on the disk and no view calls `fs::write`.
- **What the file holds is a Service.** `Notes` (`desk::notes`) carries the notes as the shell last read or wrote them, written by the worker only, so a second output's card and a redraw show the same notes without touching the file again. What a click started is the draw thread's own state, since a view writes no Service; the two meet at the worker.
- **No new window, layer or lock-screen presence.** The card is in the Desk's window, so it hides with the Desk under niri's overview and is under the lock screen as the Desk already is.

## Alternatives

**A Surface, or a window opened by a click.** A note is read while doing something else; a window in front of the work is the opposite of that. A Surface lives on the Island's body, and the Island is one core of several, so the notes would move with the core.

**A separate text-file editor in the launcher.** That is a different product. The point is that the text is on the desktop.

**Save on every key.** A write per keystroke on the draw thread is a pause no typing should feel, and a file rewritten 10 times a second wears the disk. One write a second and one at the end loses at most a second of typing, and only to a crash mid-sentence.

**`Keyboard::Exclusive`.** It would keep the focus for the Desk even with the pointer elsewhere, and ADR 0137's worry — the window below losing its focus — becomes real. `OnDemand` gives the keyboard to the Desk only while the user is working in it.

## Consequences

The Desk gains its first face that takes text, and the first focus it has ever asked niri for. Typing is per keystroke into the draw thread's own state, so a card never waits on the disk: what the file holds lags what the card shows by up to about a second, and catches up when the card is left. A file another program writes over is read once, when a card first draws; changing it on disk behind the Desk is not followed until the shell restarts.

Not verified live: that a click on a `Bottom` layer's input region focuses its `OnDemand` surface at all, and that the keystrokes then arrive. That is the E2E this ADR asks for: the pointer over the card, a click, typing, Enter, Escape, a click on the wallpaper, and the file on disk. Should niri not focus a Bottom surface, the card still draws and still saves, and the fallback is a focus by key from the Launcher rather than a new layer.
