# 45. Events added on the Calendar Surface, and CalDAV removed

Status: accepted. Amends ADR 0015 (Kanade never writes a calendar) and removes ADR 0044 (CalDAV accounts).

## Context

The Calendar Surface could only show events. Someone with no calendar app, no vdirsyncer and no Google account had no way to put one in, and the first thing the Surface said was "No calendars". CalDAV (ADR 0044) was the other way in for people with a server, but it brought an HTTP and WebDAV client, a password prompt on the terminal, a sync thread per account and a place in the Secret Service, for a feature vdirsyncer already covers into a vdir that ADR 0015 reads as is.

## Decision

- **Add from the Surface.** `a`, or the Add pill, opens a field in the agenda for the chosen day. `[start[-end] ]title`: a time as `14:30`; with none the event lasts the day, with a start alone an hour. Enter saves, Escape drops it, and what cannot be an event (a time that is none, an end before the start, no title) says what to correct and keeps the text.
- **One file per event** in `$XDG_DATA_HOME/kanade/events`, each an iCalendar file written whole and atomically, readable only by the owner. Times are floating, so they keep their wall time, as ADR 0015 reads them. The directory is read in addition to `calendar.paths` or its default, so no setting is needed and a vdir tool never sees Kanade's files. The calendar's watch reads a new file as it reads any change, so the event shows with no second path to the Surface.
- **Kanade writes only these.** Files under `calendar.paths` and what Google syncs stay read-only.
- **CalDAV is removed**: the Module, its verbs, `calendar.caldav`, its Settings row and doctor check, its Secret Service items' use and the XML dependency, with the terminal password prompt only it used. Its cache under `~/.cache/kanade/caldav` is no longer read and may be deleted; passwords it kept in the Secret Service stay there until deleted by hand. vdirsyncer, pointed at a CalDAV server and at a directory in `calendar.paths`, does the same sync.

## Alternatives

**Write into the first of `calendar.paths`, or the default directory.** Mixes Kanade's files into a vdir another tool syncs, and with several paths needs a rule for which.

**Keep CalDAV and add events to it.** Writing to a server needs conflict handling and error states a local file does not.

## Consequences

- Events can be added but not yet edited or deleted from the Surface; deleting one means deleting its file.
- No recurrence, location or reminder can be typed; `calendar.reminder` still reminds of a timed event.
