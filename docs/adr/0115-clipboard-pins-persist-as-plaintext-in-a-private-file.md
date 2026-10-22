# 115. Clipboard pins persist as plaintext in a private file

Status: accepted. Supersedes only the memory-only decision of [ADR 0114](0114-clipboard-pins-and-previews-live-in-memory-and-tmpfs.md) (pins, and so pinned pictures, are kept across restarts); the rest of 0114 stands. Builds on ADR 0019 (sensitive content).

## Context

ADR 0114 left persistence of pins undecided because an entry that passed ADR 0019 is still plaintext, and a pin is what a user chooses to keep, so the likeliest to be a token or an address. The user has decided: pins may be stored as plaintext on disk, and pinning a secret is the user's own doing.

## Decision

- **Pins, text and pictures, are stored; nothing else.** The history, the previews on tmpfs and the unpinned entries stay as ADR 0114 has them. Only entries that already passed `record()` and so ADR 0019 can be pinned, so only they are written. The sensitive gate is unchanged, and a pin adds no filter.
- **Plaintext at rest, owner-only.** One file, `$XDG_STATE_HOME/kanade/clipboard-pins` (else `~/.local/state/kanade/clipboard-pins`), written through `atomic::write` with `Mode::Private` (the file 0600, the directories it creates 0700). No encryption: the user's decision, and a key Kanade could read unattended would sit in the same account. The wiki says plainly that a pinned password is on disk until unpinned or removed.
- **State, not data.** Pins are Kanade's own record of what the user kept, read by no other program, like the recorded gestures (`gestures.toml`) and the launcher's usage; `$XDG_DATA_HOME` is for files other tools read, like the Calendar's events. A backup or sync of the data directory then does not carry a secret by accident.
- **Format.** One file, versioned, read without a parser dependency: a first line `kanade-clipboard-pins 1`, then for each pin in order a line `text <length>` or `image <type> <length>`, that many bytes and a newline. Lengths frame the bytes, so nothing is escaped or encoded. The file is replaced whole, so a crash leaves the old store or the new one. Pictures are in it too, at most 16 MiB with the other pins.
- **When it is written.** After any change of which entries are pinned or in what order: a pin, an unpin, the removal of a pinned entry, a copy of a pinned entry's content (which moves it first of the pins). Clear all keeps the pins and writes nothing; neither does a new copy or an eviction. With no pin left the file is deleted. A removal or an unpin therefore leaves no stale plaintext in the file. The write happens on a thread of its own, one at a time, which reads the pins as they are when it gets its turn (so a late one never writes an older set) and holds no lock across the disk; it never writes the Service.
- **When it is read.** At the `clipboard` Module's start, before the watcher records anything. A stored pin gets a fresh id (ids are never reused within a run, and a pin's id is not stored), keeps its place in the order, and is an ordinary pinned entry from then on: sorted, found by the Launcher `>`, removed, unpinned, previewed (a picture's preview is written to tmpfs when a row asks, as for any entry).
- **A copy of a stored pin is that pin.** The stored content is in the history as a pinned entry, so the existing dedup in `record()` finds it by content: it keeps its pin and id and moves first among the pins, never a duplicate.
- **Failure handling: the shell never stops, nothing garbage loads, content never logs.**
  - No file is no pins, and the first pin change creates it. Only the kind of an error is logged, never a path or content.
  - **What was read decides what may be written.** A store read whole, or absent, is written as the pins change. A file that could not be read for an I/O reason (any error but not-found: EIO, EACCES, EMFILE), or that is not a regular file, is *held*: loaded as no pins, left untouched, and nothing is written or deleted for the run, as for a newer Kanade's, so a transient error cannot cost the user's pins. A file that was read but is *damaged* (no first line of ours, larger than a store within the caps can be, an entry skipped, or the framing lost partway) loads what it validly held, and before the first save of the run is moved to `clipboard-pins.bak` (mode 0600, a rename), so the next pin change cannot destroy the remains; the save then writes normally. If the move fails, nothing is written and the next change tries again.
  - The file is opened first, without blocking and without following a link (`O_NONBLOCK|O_NOFOLLOW`), and is judged a regular file by the open handle, so a pipe put in its place cannot block the start thread. A symlinked pins file is therefore held, not followed.
  - Each entry is checked alone and skipped when it is empty, over the 16 MiB entry limit, text that is not UTF-8, or an image whose type is not an `image/` type of the shape a copy's is (within 255 bytes, no space or control character). A length that runs past the file or is not followed by the newline loses the framing, so the entries before it are kept and the rest is dropped (a truncated file loses only its tail).
  - The pins' caps hold on load: at most 20 pins and 16 MiB, in file order; an entry whose content is already kept is left out.
  - A file of a version this Kanade does not know is left as it is: nothing is loaded and nothing is written over it for the run, so an older Kanade does not destroy a newer one's pins.
  - A newer Kanade's file is held like an unreadable one; a damaged file is kept as `.bak` (above), not rewritten in place. A later damage replaces an older `.bak` (the latest remains are what the user last had, and one bounded file stays at rest, not one per damage).
  - A save that fails (disk full, no permission) logs its error kind; the pins stay in memory and the next change tries again.
- **`kanade clipboard clear` and Clear all keep the pins** and the file, and the reply `kept N pinned` stays right.
- **Snippets and placeholders are declined.** The user decided against them (ADR 0114 listed them as deferred); a pin is kept text, not a template, and they are not planned.

## Alternatives

**A file per pin, pictures as their own files.** Easier to bound one entry, but a set of files cannot be replaced atomically, and a half-removed set is stale plaintext. One file is all or nothing.

**TOML or JSON with base64.** Pictures grow by a third and are parsed whole into strings before they can be checked; length framing reads each entry with its bound known first.

**`$XDG_DATA_HOME`.** Pins are user-made, but no other program reads them, and the data directory is what people back up and sync.

**Encrypt the file.** A key Kanade can read unattended is a key in the same account, so the file would protect against nothing the mode does not; refused.

**Persist the whole history.** Not decided; this change stores what the user chose to keep.

## Consequences

- Anything pinned, a password the app did not mark sensitive included, is on disk unencrypted, readable by the user's processes and by anyone with the disk or a backup of the state directory, until it is unpinned or removed. The wiki says so; the decision is the user's.
- A deletion is a file replacement, not a secure erase: the old bytes may remain in free blocks or a snapshot.
- A pin or an unpin with a large picture pinned rewrites up to 16 MiB, off the Surface's thread.
- A damaged file loses its bad entries from the store with one line on stderr; the file itself is kept as `clipboard-pins.bak`.
- The `.bak` is plaintext at rest too, 0600, and Kanade never deletes it: it stays until the user removes it, and the wiki says so.
- A pin change right before exit is saved: the clean exit, a SIGTERM or SIGINT, and a restart, power off or log out from the Island each wait for the save to reach the file, bounded by its own 5 s limit, and only where a pin changed this run. A SIGKILL still loses a pin made a moment before.
- Not tested live here: the restart with pins, which needs a run on the session.
