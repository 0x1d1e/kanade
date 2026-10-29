# 139. Quicklinks are your own rows in the Launcher

Status: accepted. Builds on ADR 0126 (fuzzy ranking), ADR 0127 (frecency), ADR 0128 (empty-query sections), ADR 0130 (the web fallback) and ADR 0019 (nothing marked sensitive enters history).

## Context

The Launcher finds apps, windows, actions, destinations and Kanade's own commands. Everything it lists is something the shell or the machine already has. A person's own handful of shortcuts — a page opened every morning, a script run from a key — had no row unless an app happened to register a Desktop Action for it. The plan (`#1`) asks for config-backed Quicklinks first and defers Raycast-style script commands, which are a different, plugin-adjacent thing.

## Decision

- **`launcher.quicklinks`, file only.** A list of tables, each a `title` and exactly one target: a `url`, or an `argv` list of strings. It follows the house pattern for a `Vec<...>` section, as `notifications.rules` does: `Kind::Quicklinks(Field<Vec<Quicklink>>)`, read by `quicklinks::read_all` and written by `quicklinks::value`, with each layer's list replacing the one below. It is not drawn in Settings (it joins `EXTERNAL_KEYS`), and `kanade config defaults` shows the commented example. `src/surfaces/launcher/quicklinks.rs` holds the type, the parser and the provider.
- **The empty list is the shipped default and does no work.** `Quicklinks::find` iterates the slice, so an empty list returns no rows and starts nothing; the section is left out because a heading over no answers says nothing.
- **A new provider, on the one Fit scale.** It joins the ranked providers for a typed query (after destinations, before commands) and the empty-query sections (after Recents, before commands). It never outranks a better Fit; within a Fit it keeps the config file's order.
- **No learned score.** A URL and a title are the content ADR 0019 keeps out of `launcher-usage`, and a quicklink has no id to key on, so it has no frecency: `kept` answers `None` for both of its actions and the state file never learns it. Frecency thus cannot reorder quicklinks, and cannot leak them.
- **A URL opens with `xdg-open` alone, an argv starts directly.** `Target::Url` becomes the existing `Action::Browse`, so the row opens through the same `web::browse` path as the web fallback: `xdg-open` with the address as one argument, by argv with no shell (ADR 0130). An address that begins with a dash is refused there rather than spawned, since `xdg-open` reads it as an option of its own and refuses `--` too. `Target::Argv` becomes `Action::Quicklink(Vec<String>)`, run by `Command` with argv off the view thread, stdin, stdout and stderr on `/dev/null`. A leading `~/` in an entry is the home directory, as it is for a `hooks` program, and no string ever reaches a shell.
- **`passes_query = true` turns a quicklink into a script command.** The typed query is appended as one final argv word, after the `~/` expansion, so no shell, no word splitting and no `$(...)` ever sees it; the stored list keeps only the configured words. A row that passes the query needs one to mean anything, so it is hidden while the query is empty rather than run without its argument; for any typed text it is offered, ranked by its title when the text fits and last (`Fit::Anywhere`) otherwise, so it never lifts over a better match. A row that waits at that last fit is marked `Answer::filler`: it is offered for any text but answers none of it, so it does not count as an answer when the web fallback is decided. A pass on a `url` target is refused, as is a non-boolean value. The failure wording is the argv path's unchanged: the program or the status, never the query or the argv (ADR 0019).
- **Never logged.** A quicklink's title and target are not written to stderr, a file or the state, and neither is a typed query a script command receives. A failure's words name the program (`<program>: <io error>`) or the status, never the argv, the URL or the query.
- **One kind of target, refused otherwise.** `url` and `argv` together, neither, an empty `argv` or an unknown key is a config error that keeps the list the lower file gave, as the parser's tests show.

## Alternatives

**Keep quicklinks in `settings.toml`.** The file only holds what the Settings window controls and a click can write; a list of tables has no control there, so it belongs in the config file alone, as `notifications.rules`, `hooks` and `gestures` do.

**Learn them like apps.** It would need a key naming the entry. Keying on the title or the URL would put either in the state file, which ADR 0019 forbids; keying on the index would relabel after a reorder. Config order within a Fit is both simpler and safe.

**A script-command runner as a separate surface.** A shell line or script file with a plugin API is a different, plugin-adjacent thing and stays deferred (see `TODO.md`). Passing the typed query to a configured `argv` needs none of it: it is one more word on a list the user already wrote, so it stays inside this ADR's no-shell Quicklink.

## Consequences

The Launcher gains its first rows that are entirely the user's, without a new Wayland connection, a plugin API or a shell. A URL row and a web-search row share `Action::Browse`, so both reach the browser by the same argv path and neither is logged. A `passes_query` row passes the typed text to the user's program as one argv word, and that program, not Kanade, decides what to do with it.

A filler row and the web fallback can now list together: a configured passing row no longer silences the web's rows for every typed text. `offered` shows the fallback when the ranking is empty or holds only fillers, after them, so a row the person configured stays first and the web fills the gap it does not. Every other `Fit::Anywhere` producer — an app's description or command, a window's app name, a Settings page's help — is a real match and is not marked a filler, so it still answers the query alone and keeps the fallback away.
