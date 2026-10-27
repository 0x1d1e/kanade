# 114. Clipboard pins and previews live in memory and tmpfs

Status: accepted; its memory-only decision for pins ("it is memory only", and the deferral of persisting pins and pictures) is superseded by [ADR 0115](0115-clipboard-pins-persist-as-plaintext-in-a-private-file.md). The text pane of the Decision ("Text previews") was removed: the user did not want the box, so the Surface shows rows only, which take its room; image thumbnails stay in the rows. The preview folder is now the Kanade's own, `kanade-clipboard/<pid>/` (see the Consequences): a shared one let a start sweep a running Kanade's previews, and the turn `TURN` keeps now covers only publishing a finished file. Builds on ADR 0019 (sensitive content) and ADR 0085 (confirm by holding); the first slice of a longer-lived Clipboard.

## Context

The Clipboard history (ADR 0011) is bounded (50 entries, 64 MiB) and only in memory, so what the user copies most is the first to be pushed out, and an image shows only as its kind and size because the runtime draws images from files. Keeping something past the bounds, and seeing what an entry holds before copying it, are the two gaps users meet first. Persisting anything is a different matter: ADR 0019 let nothing sensitive in, but an entry that passed it is still plaintext, and writing the history or its pins to disk is a decision about plaintext at rest that this change does not take.

## Decision

- **A pin marks an entry; it is memory only.** `Entry` has `pinned`. A pin only marks an entry that is in the history, so one that already passed ADR 0019: no second filter, no heuristic, and a sensitive selection is still never read. Nothing about a pin or the history reaches disk. (Superseded for pins by ADR 0115, which stores them.)
- **Pinned entries sort first**, then the rest, each newest first. A copy of a pinned entry's content keeps its pin and id and moves it to the top of the pins; an unpinned one moves to the top of the rest. An unpin puts the entry at the top of the rest.
- **The bounds never evict a pin.** A pin counts in `ENTRIES` and `TOTAL_BYTES`, which drop the oldest unpinned entry. Pins have their own caps: 20 pins and 16 MiB together. Pinning past either is refused, changes nothing, and the Surface says so until the next key.
- **Clear all keeps the pins** (the Surface's held button and `kanade clipboard clear`); the footer says how many will be kept ("Hold Enter to clear 12, keeping 3 pinned, no undo") and the CLI replies `kept 3 pinned`. Removing an entry removes it whether pinned or not, without a hold. Ids are still never reused.
- **The Surface gets a ring stop** between the entry and its delete (Right, Tab), and Enter on it pins or unpins, and the ring follows the entry to where it moves. The pin is also a button, named `pin` for `kanade ui`, with a checked state; it draws filled while pinned. `entry#N` names are unchanged. A press from the pointer goes through the same press as the key and calls `clipboard::pin` or `unpin` from its callback, never writing the Service from a view.
- **Previews of images are files on tmpfs, nowhere else.** The runtime draws images from files, so the bytes of a picture are written, only for a row the Surface builds, off the draw thread, to `$XDG_RUNTIME_DIR/kanade-clipboard/<pid>/<id>.<ext>` through `atomic::write` with `Mode::Private` (the directories 0700, the file 0600). The name is the entry's id and an extension from a fixed table (`image/png` png, `image/jpeg` jpg, `image/webp` webp, `image/gif` gif); nothing the copy says is part of a path, and a type outside the table has none. The folder is named by the Kanade's pid (see below). With no usable `$XDG_RUNTIME_DIR` there is no preview: no fallback to another directory. The row draws a thumbnail as the Wallpapers' tiles do; while it decodes a quiet tile, and if it cannot be drawn the picture icon.
- **A preview goes with its entry.** After every change of the history (an entry removed, evicted by the bounds, or cleared) the files of entries no longer kept are deleted, and the folders of Kanades that ended are deleted at start, so what a Kanade that ended without cleaning up left does not stay. Writing and deleting take turns, so a preview is not deleted as it is written or registered after. The turn is the publish alone: a picture's bytes are written and flushed beside its name before it, so a copy's `record`, and the sweep, never wait behind the decode and write of the bytes.
- **Text previews. (Removed: the pane below is no longer built, and the rows take the room it had; the user did not want it.)** Rows stay one line. Where the body is as tall as the default (`island.height` 400 or more) a pane under the rows shows the selected text entry's first 6 lines, within 400 characters, from the same memo as the row; a smaller body keeps all its rows and no pane. An image's pane says its kind and size.
- **The Launcher `>` changes in order only:** pinned entries come first (the history's own order), and their detail starts with "Pinned". No new prefix, verb or action. (`>` later became the Clipboard Surface's own key, ADR 0135.)
- **No content in logs.** A preview that cannot be written logs its error kind only; a pin and an entry's `Debug` print neither the content nor a path. `kanade status` adds a pinned count, nothing of what was copied.

## Deferred

These need the user's decision on plaintext at rest, so none is built here:

- **Persistence of pins** across restarts (done, ADR 0115) or **of the history** (still not built).
- **Snippets and placeholders**: text kept on purpose and filled in when pasted, and any CLI verb that takes text. Declined (user decision after this ADR), not deferred: they are not planned, see ADR 0115.
- **Image persistence**: a pinned picture surviving a restart (done, ADR 0115), or a preview anywhere but tmpfs (not built).
- **Sync** of the history or pins between machines.

## Alternatives

**Persist pins now, owner-only.** Pins are what a user chooses to keep, so they are the likeliest to be a token or an address; a file of them is plaintext at rest that the privacy policy (ADR 0019) did not yet decide. Deferred, not refused.

**Hold pins outside the bounds' count.** A pin would still use memory; its own caps keep the history's 64 MiB meaningful and a pinned 16 MiB image from starving the rest.

**A pin that also filters.** A second sensitivity check would repeat ADR 0019 with a weaker signal; the hint is the only one.

**Draw previews from memory.** The runtime decodes images from files; a memory path would be a second image pipeline.

**Previews in `$XDG_CACHE_HOME` or `/tmp`.** Both are disk, or world-visible directory names; the runtime directory is the user's, 0700 and tmpfs.

**A pane that always shows.** It would take rows from the smallest body, which keeps four; the pane was shown only where the body is as tall as the default, and was removed altogether afterwards.

## Consequences

- A pin lasted as long as the shell ran; restarting forgot it with the history (ADR 0115 now keeps pins across restarts).
- A picture's preview is its full bytes on tmpfs (RAM) until its entry goes, up to the same 64 MiB as the history, and a decoded thumbnail lives in the runtime's cache until evicted there.
- Previews are swept only at the next start: a Kanade that exits, cleanly or by SIGTERM, or is killed leaves them until then (or the session ends, tmpfs); the folders are 0700, the files 0600, the same user's.
- **The preview folder is the Kanade's own.** `kanade-clipboard/<pid>/`, so a sweep removes only the folders of the pids that are gone, and this one's own (a crash under a reused pid leaves it stale, and the ids start again at 1). A second Kanade started beside the running one, or a restart racing the process it replaces, keeps its previews. A second Kanade still sweeps at module start, before it finds the socket taken and stops, but nothing it can see is a running Kanade's. A bare preview in `kanade-clipboard` itself, which only a Kanade before the folder-by-pid wrote, is swept too; `atomic::alive` is the test, the same one `atomic::sweep` uses for a dead write's part.
- **A preview that cannot be drawn is written again.** The preview layer treats a written preview whose file is gone as missing, and writes it on the next draw of its row.
- Only the rows the Surface builds ask for a preview, so a long history of pictures writes only what was scrolled to.
- Not tested live here: the thumbnails, the pin ring stop and button, the footer copy needed a run on the session.
