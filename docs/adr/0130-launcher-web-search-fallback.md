# 130. A query nothing answers is offered to the web

Status: accepted. Builds on ADR 0112 (a closed set of Kanade verbs), ADR 0129 (the actions panel) and ADR 0019 (nothing marked sensitive enters history).

## Context

A typed query that matched nothing said `No matching apps` and stopped. What a person does next is what the search was for: put it in a browser. The plan (`docs/launcher-work.md`, step 5) asks the Launcher to offer that, through the default browser only, with the search's address fixed rather than a configurable endpoint. It is the one answer that sends what was typed off the machine, so it needs its own bounds.

## Decision

- **The fallback is not a provider.** `found` keeps the providers' list as it is; when the ranking comes back empty or holds only filler rows (answers marked as offered for any text but fitting none of it, ADR 0139), `offered` (`src/surfaces/launcher/mod.rs`) answers with `web::offers` after them instead. Nothing a provider returns can produce it and it can never outrank an answer, so no provider gains a row and the Fit scale is untouched. It waits for the desktop scan, because until the apps are read one of them may still be the answer, and the state under the field says `Finding apps` meanwhile.
- **`src/surfaces/launcher/web.rs`.** Two rows for a query nothing answers: `Search Google for "<query>"`, then `Open "<query>"` in the browser, each with a second line saying where the text goes (`Sends it to Google in the default browser`, `The default browser opens it as an address`).
- **`Action::Browse(String)`.** The address the row opens, carried by the answer, pressed as every other action is (`Pressing::Waiting`, then `Failed` said on the row as `Not opened: <why>`). `actions::of` gives it a `Copy id` row that says why it copies nothing, as a `Paste` and a `Choose` already do.
- **`xdg-open` alone, by argv.** The default browser is whatever `xdg-open` starts; Kanade names no browser and reads no `$BROWSER`. The address is one argument of `Command::new("xdg-open")`, never a shell line, so no word of the query is ever a command (ADR 0055, 0112). It is asked, not waited on: an opener may live as long as the browser it started, so only the spawn decides whether the row opened, and the child is reaped on a thread of its own.
- **The Google address is fixed in the binary.** `https://www.google.com/search?q=` with the query percent-encoded for a whole URL, so an `&` or a `#` in the query cannot add a parameter of its own. There is no setting for it, and none is planned.
- **The browser row opens the query as an address.** One that names its scheme (`scheme://`) goes as it is; anything else gets `https://` in front, so the argument never begins with a dash that `xdg-open` would read as an option of its own.
- **`xdg-open` is a `Need` of the `launcher` Module** (`Provider::Program(web::OPEN)`), as `wl-paste` is of its own, so `kanade doctor` and Settings' Health page say when it is missing. Without it neither row is offered: `web::refused` answers instead, one row titled `Needs xdg-open` that says nothing was found for the query and that the browser cannot open it. A query the web is not offered for at all — one letter, or one starting `?` — gets neither the rows nor the refusal, and keeps the state it had.
- **Nothing is logged or kept.** Neither the query nor the address is written to stderr, to a file or to `launcher-usage` (ADR 0019, ADR 0127); a failure's words are `xdg-open failed (<status>)` or `xdg-open: <io error>`, never the address. The recents keep no key for a web row, since it has none to name.

## Alternatives

**A `Web` provider in the provider list.** It would have to answer every query to stay last, and Fit would then decide whether it outranks an answer; the ranking already knows when it found nothing, so the fallback belongs there.

**A configurable search endpoint.** A setting is a second place the query goes, chosen outside the code that says where it goes, and the row could then say one thing and send another. One fixed address is the whole point of the row saying what it sends.

## Consequences

The Launcher now has a path that reaches outside the machine, and it is the only one: an answer's press either acts on the session, runs a Kanade verb, or opens this address. Typing a query nothing answers no longer shows a state row once the apps are read, so the web is the last thing the Launcher tries, and the first thing it sends off the machine. A row that is offered for any text but fits none of it is a filler, not an answer (ADR 0139), so it does not keep these rows away; they list after it.
