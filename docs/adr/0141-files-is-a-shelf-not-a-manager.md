# 141. Files is a shelf, not a manager

Status: accepted. Keeps the non-goal [ADR 0137](0137-the-desk.md) restates ("no file manager") and gives the recent-files item its read-only form. Index: `files-surface`.

## Context

`TODO.md` held "Files tray with drag-out on the Island", parked as *not a file manager* since the Desk's ADR. The wish behind it is real: a file just saved in a browser, or a document a GTK app wrote and closed, is hard to find again, and the desktop already keeps that list. The desktop's own `recently-used.xbel` records every document an application registered as recently used, newest entries first.

A file manager is another product: it owns a tree, a selection, a clipboard, moves, deletes, thumbnails and drag targets, and it must be right about all of them. A shell that reads one list is a different, much smaller thing, and it does not need any of that.

## Decision

- **A shelf.** `files` is a Surface of the Island like the Clipboard or Emoji, opened by the Launcher's Files destination, `kanade files open|close|toggle`, a niri keybind, or a gesture. It lists the desktop's recent files, newest first. It shows each file's name and the folder it is in, and it filters them by both as the query is typed.
- **Read-only, and nothing else.** No delete, rename, move, copy, cut, drag-out, sort order or folder browsing. There is no file tree and no file is ever read: only its path, its folder and its modification time. Pressing a row opens the file with `xdg-open`; the folder target at the row's end, and Right with the ring on the row, opens the folder it is in. Both go through the Launcher's own opener (`surfaces::launcher::web::browse`), so an address that begins with a dash is refused there rather than run, and no shell sees a word of it (ADR 0130, 0055).
- **The data is the desktop's.** `$XDG_DATA_HOME/recently-used.xbel`, else `~/.local/share/recently-used.xbel`. Its `file://` addresses are read out of the file's text without an XML dependency, percent-decoded, and kept only where they name a regular file this user can open. Anything under Kanade's own config, data and state directories is left out: what the shell writes for itself is no file the user opened. Duplicates collapse, the list is ordered by the file's own mtime, newest first, and capped at 64 rows.
- **A list that cannot be read is an empty shelf**, never a failure: missing, not a file, larger than 2 MiB, unreadable, or naming nothing that still exists. No path or file name is logged anywhere (ADR 0019).
- **Read off the draw thread, on request.** The shelf is read when the Surface opens (once per visit) and again on the footer's Refresh or `Ctrl+R`, on a thread of its own; a frame of a morph never reads the file. Typing `r` still types, since the Surface is searched as it is typed.
- **Its own Module.** `files-surface`, `requires: &[CORE]`, on by default, with the verb `files`. With `xdg-open` missing it runs without opening anything, which its `Need` says. Off, the Surface never opens and nothing reads the list.
- **Empty states say what is missing.** "No recent files" with nothing on the desktop's list, "No matching files" for a query that finds none.

## Why not the tray, and why not drag-out

- **A tray item** would put the list under the pointer along the Island's edge, where a shelf of files is neither a status nor a menu, and it would compete with the apps' own tray items for the same slots. A Surface is what every other list in Kanade is.
- **Drag-out** needs a drag source, which needs a data device and a drag icon, and a drag destination in another app that this Surface cannot test or see. It is a manager's affordance; a shelf's is "open it".

## Consequences

- The list is only as good as the desktop's; an app that registers nothing is not on it, and nothing sweeps it.
- Opening a file is `xdg-open`'s business, so the app that opens a `.txt` is the session's default one, not Kanade's.
- The folder the shelf shows is the file's own, written as `~` under the home directory; a folder that moved after the entry was written is not followed.
- **Not run live**: that a real `recently-used.xbel` from a GTK app lists as expected, that `xdg-open` opens a row, and how the list reads on a session with hundreds of entries.
