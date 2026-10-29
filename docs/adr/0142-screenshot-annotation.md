# 142. Screenshots annotate in place, one slice of the editor

Status: accepted. The first slice of "Built-in screenshot, region picker and annotation editor" (`TODO.md`): the annotation editor. A built-in screenshot and region picker are not part of it; niri still takes those.

## Context

`capture screenshot` has niri save a shot under `~/Pictures/Screenshots` and the Island says the path. To mark a step on it, the user opens it in a drawing app, exports a copy and finds it again. The shell already has the picture's path the moment niri announces it, a runtime that decodes and draws images, and a Surface to draw on; the missing piece is the editor itself.

An editor is a product of its own if left open-ended: text, highlighter, crop, layers, selection, resizing and re-editing. This slice takes only what a screenshot mark needs, and says what it leaves out.

## Decision

- **Off by default, under its own key.** `capture.annotate` is a switch in the Settings' Screenshots section. Off, nothing changes: the saved Transient appears as before. On, a `capture screenshot` that saves a path opens the editor on the focused output with that file; the saved Transient waits until the editor closes, then names the original for Escape or the copy for Save. A capture with no path (clipboard only) opens nothing. A screenshot niri's own bind takes is not the editor's and shows as before.
- **No race with the file.** `capture screenshot` records the path it asked for before it asks; the editor opens on niri's announcement of that path, when niri has saved it, and never by polling. The record is bounded at 8 and a stale one is dropped; an unopened path (an area the user left) triggers nothing and is never shown.
- **The editor is its own window, not an Island Surface.** A layer-shell Overlay on the focused output, drawn and composited on the runtime's one pipeline, like the recording overlay (ADR 0028); it is no content of an Expanded island (`CONTEXT.md`, Surface). It takes the pointer in its own window only, with no global grab, and holds the keyboard (Escape closes, Ctrl+Z takes the last shape back); no second Wayland connection. It closes on Escape, Save or a lock.
- **The preview is vectors over the base texture.** The image is decoded once when the editor opens, scaled to fit with letterboxing over a dim backdrop. Rectangles and arrows are drawn from the edit list as the runtime's own shapes on a Canvas each frame; no frame composes pixels, and no frame re-encodes a picture.
- **Two tools, undo, save.** Rectangle and Arrow, drawn from the press of the left button to its release, previewed while dragging and fixed on release; a mark too small to see is dropped. Undo takes back the shape being drawn first, then the last fixed one. The tool and the marks use the Island's accent.
- **Save is the only write.** It rasterizes the base and the marks on a worker (`annotate-save`), encodes an 8-bit RGBA png, and writes it beside the original as `<stem>-annotated.png` (a free ` (2)` name if taken, never over the original) with `atomic::write` in the umask's mode, as wallpapers are written. It reports the copy in the saved Transient and closes the editor. Nothing else is ever written; Escape leaves no file.
- **Refusals are a line, never a crash.** A file over 64 MiB, an image past the runtime's `MOST_PIXELS`, or a picture `decode_image` cannot read is refused with a sentence on the Island and opens no editor; a Save that cannot write or encode keeps the editor open with the reason in its status line. No path and no pixel is logged or put in a notice (ADR 0019); the saved Transient names the path, as it already does for a screenshot.
- **Not in this slice.** No text, highlighter, crop, layers, selection, resizing, re-editing, or a re-open of a saved annotation. Each is a separate decision; the editor's edit list is where one would start.
- **Its own Module?** No. The editor rides the `capture` Module and its key, so with `capture` off nothing can ask for a screenshot and nothing opens.

## Consequences

- The editor needs the file to exist at niri's announcement, which is how niri saves; a niri that saved later would need the wait here. An open editor holds no lock on the file, so deleting it from a file manager while the editor is open makes Save say it could not be read again.
- Saving re-decodes the whole picture on a worker, so a very large image costs one read and one paint off the draw thread; the preview never re-encodes.
- The editor follows the theme's accent only; there is no colour choice, which the next slice would add with a palette row.
- The marks are pixels once saved: the copy is flat and cannot be edited again.
- **Not run live**: that a real `capture screenshot` with the key on opens the editor on the focused output after niri saved (unit-tested paths, frame mapping, arrow geometry, the pixel paint and the refusals; E2E NOT RUN), and how the pointer drag and the toolbar read on the session.
