# 15. Local calendars as iCalendar files

Status: accepted (roadmap 10 Desktop/data, #150). Decides the local source format for the `calendar` Module in `docs/design.md`.

## Context

The Calendar Surface shows a month and the agenda of a day from local sources, with account sync optional and later. Local calendars already live on disk in one format: iCalendar (RFC 5545). vdirsyncer and khal keep a directory of `.ics` files per calendar (a vdir), and exported or subscribed calendars, like holidays, are single `.ics` files. Recurrence, exceptions and time zones are where a hand-rolled reader goes wrong.

## Decision

- **Format: iCalendar files.** `calendar.paths` lists `.ics` files and directories; a directory is read recursively (symlinks followed, each directory once, 8 levels deep), so a vdir root with one subdirectory per calendar works as is. Without the key, `$XDG_DATA_HOME/calendars`, which is vdirsyncer's usual place. Only `VEVENT`s show; cancelled ones do not.
- **Read-only, no network.** Kanade never writes a calendar, holds no account and fetches nothing. Sync is the job of vdirsyncer or a later account Module, whose files Kanade then reads.
- **Parsing and expansion through calcard** (Stalwart's, MIT/Apache): RRULE, RDATE, EXDATE, RECURRENCE-ID overrides and VTIMEZONEs. A moment with a zone becomes local time through `clock`; a floating one keeps its wall time; an all-day event covers its dates.
- **Bounded work.** A file over 32 MiB is skipped. Each recurring series is expanded on its own (the file split per UID, its VTIMEZONEs kept), with its UNTIL clamped to just after the days asked for and a cap of 100 000 instances, so a runaway rule cuts off only itself.
- **Followed by inotify**, on each place, its subdirectories, or the nearest existing parent of one not there yet; a change rereads everything after 150 ms of quiet. `kanade config reload` rereads too. No polling, no idle wakeups.
- **Weeks start on Monday**, ISO 8601. Names are English, as the rest of Kanade is.
- **Privacy.** Logs name a file and why it was skipped, never an event's content.

## Alternatives

**Parse iCalendar by hand.** Small for single events, wrong for the recurrence and time zone cases that make most real calendars.

**Read khal's or Evolution's databases.** Ties Kanade to one app's private store; both sit on `.ics` files anyway (khal) or export them.

**Fetch CalDAV or ICS URLs directly.** Needs credentials, retries and a cache, the account sync the design keeps optional; vdirsyncer already does it into a vdir.

## Consequences

- A calendar shows only once something has written it to disk.
- Every change rereads all files: fine for personal calendars, slower for very large archives.
- Locale-aware week starts and names wait for localisation, which Kanade does not have.
