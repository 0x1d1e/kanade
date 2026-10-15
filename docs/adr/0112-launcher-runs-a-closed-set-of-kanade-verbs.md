# 112. The Launcher runs a closed set of Kanade verbs

Status: accepted. Builds on ADR 0107 (a verb runs through the shell's own socket) and ADR 0055 (closed, bounded; no shell). Supersedes nothing.

## Context

The Launcher finds apps, windows, values and where Kanade goes, but not what Kanade does: turning caffeine on, taking a screenshot or locking the session means a keybind, the CLI or Controls. A person who types "caffeine" into the Launcher expects to get it. The verbs already exist, with their Module gates and replies (ADR 0107); the Launcher only needs a way to name some of them.

## Decision

- **A `Commands` provider, `src/surfaces/launcher/commands.rs`**, in the provider order after destinations (a tie with a destination of the same name, like `wifi`, shows the destination first). Its table is static: each row has a title, a detail, an Icon, aliases for `provider::fit` and a static argv. Rows: `caffeine toggle`, `night-light toggle`, `focus toggle`, `notifications dnd toggle`, `wifi toggle`, `bluetooth toggle`, `power-profile cycle`, `capture screenshot area|window|output`, `capture record start|stop`, `lock`, `wallpaper shuffle` and `dictate toggle`.
- **A row exists only while its Module is on** (and, for Wi-Fi and Bluetooth, while the machine has the radio), the flags set from `modules::on(..)` as `Destinations` does. The Module gate is also applied again by the verb itself.
- **Found by name only**, from two letters, never for a query starting `:`, `>` or `@`, never for an empty query. A row takes no typed text: pressing it runs exactly its argv, never a word the user typed. There is no free text and no user-supplied argv.
- **`Action::Run(&'static [&'static str])`.** Pressing a row runs the verb through `cli::call_within`, the client the CLI uses, on a worker thread, so the Launcher never calls a source function itself and takes the path every verb takes: the Module gate (`ipc::answer`), the parse and the same replies (ADR 0107). It is not run while the session is locked (as a gesture's is not). The row waits (`Pressing::Waiting`) as a copy does; on `Done` the island closes. Otherwise the row says why, in a line of at most 60 characters with its prefix intact: `Not run: <why>` for a refusal or no shell, which did nothing, and `Not confirmed, may have run: <why>` for an `Unknown` reply or a call that did not answer within 6 s (past `later::LATER`), as the verb may have run, so the user is not invited to press it again blindly.
- **A capture waits for the island to clear.** `capture screenshot area|window|output` and `capture record start` would catch the island in what they take, so, as the Capture Surface does (`surfaces::capture::CLEAR`), the island collapses first and the verb runs 400 ms later, on a thread. No row is left then, so a failure shows as a failed Live Activity (`live::gesture`, the one a refused gesture uses), titled with the row's title and worded as above. Every other verb runs with the island open and closes it on `Done`.
- **Commands never outrank an app.** A command fits no better than `Prefix` (`commands::BEST`), so an app or destination whose name is the query leads, whatever generic alias a command has (`record`, `lock`); a tie keeps the provider order, apps first.
- **A list of its own, not the gesture allow-list (ADR 0107).** A gesture is data in a config file and needs a deliberately narrow allow-list; the Launcher's table is code, reviewed, and shorter. `commands::denied` is the pure deny set the table is tested against, and `press` checks it again: `ui`, `activity run|hook`, `session logout|reboot|poweroff`, `dmenu`, `gesture`, `capture color`, and anything that does not parse. A verb the CLI gains later is not a row until it is added to the table.
- **`dictate toggle` is a row** because the user types it, and the user's own keystroke is as deliberate as a keybind. Nothing about dictation's gate changes (ADR 0111): without a key and the consent, which only `kanade dictate key|consent` in a terminal give, the verb is refused and the row says so. No audio is recorded or sent by the row beyond what the verb already does when the gate allows it.
- **No plugin API and no config written.** The table is in the binary. `launcher.commands` (default on) switches the rows off, as `launcher.actions` does for apps' actions.

## Alternatives

**Reuse the gesture allow-list.** It is a list for data the user wrote into a config; coupling the two would make a change to one widen the other.

**Parse the typed query as a verb** (`> caffeine toggle`). Runs user-typed argv, which breaks ADR 0055's closed set and the "no typed text" rule.

## Later slices

Web search and recent files are not rows of this provider. A web search sends what the user typed off the machine; recent files are a parser of outside input (`recently-used.xbel` and the like). Each wants its own ADR, with its privacy and parsing bounds.

## Consequences

- Other verbs keep the island open while they answer (a lock waits for niri to hold it, up to `later::LATER`), unlike a gesture, whose overlay closes first (ADR 0107).
- A refusal shows on the row until the next key, like any failed press.
- `dictate toggle` from the Launcher may only copy the text: the focused window is unknown while the Launcher holds the keyboard, so `dictate.insert = "paste"` or `"type"` falls back to a clipboard-only copy (ADR 0111, `FocusUnknown`) when the recording starts from it. Not verified on a session.
- The capture path and the clear are not verified on a session.
