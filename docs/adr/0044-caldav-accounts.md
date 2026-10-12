# 44. CalDAV accounts

Status: accepted. Builds on the local calendar (ADR 0015) and follows Google Calendar's account sync (ADR 0016).

## Context

Most calendars outside Google are reachable over CalDAV: Nextcloud, Fastmail, iCloud, mailbox.org, Radicale, Baikal. Until now a user of one needed vdirsyncer to see it in Kanade. ADR 0015 left fetching to "a later account Module, whose files Kanade then reads", and ADR 0016 did that for Google. The design's rules hold here too: account secrets never reach a log or the TOML config and are kept by the Secret Service, the local calendars keep working whatever happens to an account, and nothing touches the network unless the user asked for it.

CalDAV differs from Google in two ways that shape the design. It has no OAuth: servers take a username and a password, and those with two-factor sign-in (iCloud, Fastmail) take an app password instead. And it returns real iCalendar with recurrence rules, time zones and alarms, which Kanade's calendar already reads.

## Decision

- **A Module of its own, `caldav`, requiring `calendar`.** It owns `calendar.caldav` and `kanade caldav login|logout|sync|status`. It is on by default, but with no account configured its thread waits on its queue and asks nothing of the network or the keyring.
- **Accounts are config; passwords are not.** Each account is an entry in `calendar.caldav` (`[[calendar.caldav]]` tables, or an inline array), with `name`, `url` and `username`, and no other key. The address must be `https://`, or `http://` to `localhost`, `127.0.0.1` or `::1`, as a Radicale run locally is; any other is refused when the config is read, so no password is ever sent unencrypted off the machine.
- **The password lives in the Secret Service only, bound to its server.** `kanade caldav login <name>` runs in the `kanade` process the user started, not in the shell. It reads the password from the terminal with echo off (raw kernel termios in `clock.rs`, the FFI boundary), or one line from a pipe. It checks it with the server by discovering the calendars, and only then stores it as one item (`application=kanade`, `account=caldav`, `server=<url>`, `username=<username>`). Then it tells the shell to sync. The password never crosses Kanade's socket and is never printed, logged or written to a file; in memory it is held in zeroizing buffers. Because the item is found by the address and username, editing an account's `url` to another server finds no password, so a changed config cannot send a password to a server it was not given for.
- **Discovery is the standard one.** A `PROPFIND` for `current-user-principal` on the configured address, else on `/.well-known/caldav` of its server; then the principal's `calendar-home-set`; then the home's children (`Depth: 1`) that are calendars and hold `VEVENT`s, at most 100. A redirect is followed only on the same server, at most 5 times; one to another server is reported, with the advice to configure that address instead. An `href` a server names for its own resources may be on another host, as iCloud's are. The password goes there only if it passes the same address check and is the configured origin (scheme, host and port). iCloud, the one provider known to name another host, is trusted by name: `caldav.icloud.com` and its `pNN-caldav.icloud.com` partitions over HTTPS. Any other host fails the sync, which says which server was named, so it can be configured instead. A name or password in the `url` itself is refused.
- **Events come unexpanded.** Each calendar is one `calendar-query` `REPORT` for `VEVENT`s overlapping a year back to two years ahead, without `<expand>`. Recurring events keep their rule, exceptions, time zones and `VALARM`s, so the calendar's own recurrence code expands them (ADR 0015) and reminders work (ADR 0043). Each calendar is written as one `.ics` file of the `VCALENDAR`s returned, under `$XDG_CACHE_HOME/kanade/caldav/<account>/`. The account's directory is the first 8 bytes of the SHA-256 of its address and username, so another server's events are never shown under an account's name. Directories are 0700 and files 0600, written through a `.part` file and a rename, so the calendar never reads one half written. A file whose content did not change is not rewritten.
- **The answers are parsed as untrusted input.** roxmltree, which the runtime already builds through usvg, refuses documents with a DTD, so an answer cannot expand entities. Properties are matched by namespace and local name, never by prefix. A `propstat` with a status other than 2xx contributes nothing. Each answer is at most 32 MiB and each request at most 60 s.
- **The calendar reads only configured accounts.** `caldav::places()` gives the calendar the directories of accounts in the config, and the sync thread deletes the directory of an account removed from it. `kanade caldav logout <name>` deletes the password and the account's events.
- **Bounded refresh with backoff.** Each account syncs at start, every 15 minutes, on a config reload that adds or changes it, and on `kanade caldav sync [<name>]`. When the server cannot be reached or answers wrongly, it tries again after 1 minute, doubling up to 15. A refused password (401 or 403), a missing password, or a keyring prompt the user dismissed waits for a login or an explicit `sync` instead: retrying would not help and could ask the user to unlock the keyring again and again. A keyring that cannot be reached, as one not started yet, backs off as a server does.
- **Failures are a state, never a crash.** `kanade caldav status` and `kanade status` give a line per account (synced, syncing, or the problem and when it last synced), `kanade doctor` warns on each that is not syncing, and the Calendar Surface's footer (or its empty state) names the account and the problem in a few words. The events synced before stay, and the local calendars are untouched.
- **HTTP Basic over ureq with rustls,** the client Google's sync already uses.

## Alternatives

**Ask the server to expand recurrences (`<C:expand>`).** This would match Google's sync, but expansion is optional for servers and done unevenly. It also drops `VALARM`s and time zones on some servers, and it duplicates what the calendar already does.

**The password in the config, or in a file beside it.** This is simplest, but the design keeps account secrets out of TOML, and configs are often published in dotfiles.

**Send the password over the shell's socket and let the shell store it.** The socket is the user's own, but this puts the secret in a second process and in an IPC path that logs its errors. Storing it from the `kanade` process that read it keeps it in one place.

**Follow redirects to any server.** A server could move an account, but a redirect to another host should be visible to the user and set in the config, rather than followed with the password silently.

**An adapter through the shared Outside trait and thread.** CalDAV's discovery, per-account backoff and keyring rules do not fit a single fetch, so it has a worker of its own, as Google's sync does.

**vdirsyncer.** It still works with `calendar.paths` and is the choice for write access or servers Kanade cannot reach. But it is another program to install, configure and schedule, and it keeps its passwords in its own config or a fetch command.

## Consequences

- An account appears in the Calendar Surface within one sync of `kanade caldav login`, and a change on the server within 15 minutes, or at once with `kanade caldav sync`.
- Only Basic authentication is supported. Servers with Digest only, or with OAuth only (Google's CalDAV), do not work; Google has its own Module.
- A server answering 403 for a reason other than the password is shown as a wrong password.
- Exact origins over a parent-domain rule: a guess at a parent domain without the Public Suffix List trusted `attacker.co.uk` for `example.co.uk`. A provider that names another host fails until it gets a rule of its own, as iCloud has.
- Events are read-only, and only those within a year back and two years ahead are synced, although a recurring event within the window shows beyond it.
- Without a Secret Service (`kanade doctor` says so), logging in fails with its reason.
- The synced events are on disk in the user's cache, readable only by the user, until logout or until the account is removed from the config.
