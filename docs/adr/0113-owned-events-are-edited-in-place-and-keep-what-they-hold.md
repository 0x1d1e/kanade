# 113. Owned events are edited in place and keep what they hold

Status: accepted. Amends ADR 0045 (events added on the Calendar Surface) and builds on ADR 0085 (confirm by holding).

## Context

ADR 0045 let the Calendar Surface change and remove the events it added, but a change wrote the file again from the typed text, so a place, a reminder (`VALARM`) or a property another program added was lost, and neither a change nor a removal looked at the file first: one edited elsewhere since the Surface loaded it was overwritten or deleted without a word. The date could not be changed, and the rows took no click for Edit or Remove, so the pointer could not do what `e` and `d` do.

## Decision

Scope: events Kanade owns (ADR 0045: UID `<name>@kanade`, the file `<name>.ics` in `$XDG_DATA_HOME/kanade/events`), local only.

- **A change rewrites only SUMMARY, DTSTART and DTEND.** The file is read, and every other line stays as it was, in its place: the UID, DTSTAMP, LOCATION, DESCRIPTION, `X-` properties and `VALARM`s with their own SUMMARY and DESCRIPTION. Lines are unfolded and folded again, which changes no value. A missing DTEND is written next to DTSTART and a missing SUMMARY at the end of the event. The file stays whole and atomic, owner-only (`atomic::write`, `Mode::Private`), on the existing `calendar-write` thread.
- **A file that cannot be changed safely is refused**, not rewritten from scratch: more than one event, a UID that is not the file's own, a repeat (RRULE, RDATE, EXRULE, EXDATE, RECURRENCE-ID), a DURATION, a line that is no content line, components that do not nest, a file over 1 MiB, or a DTSTART or DTEND with a zone (a `Z`, a `TZID` or another form). Kanade writes none of these, so a file it wrote is never refused; the Surface says "Its file holds more than Kanade can change safely". The event can still be removed.
- **An event the field cannot say is refused too.** The field has one date and times to the minute. A change is refused, with the same message, when the event as it was loaded is not what its own typed form (`add::text`, parsed again) gives: an all-day event over several days, a timed one ending on a later day, a timed one with no DTEND, a time with seconds, or an all-day one whose title starts like a time (typed, it would be read as one). An all-day event with no DTEND is one day, can be said, and gets its DTEND written. The refusal says "The field cannot say this event; change its file", not that the file holds more. Saving it would cut it to fit, even for a title-only change. Every event Kanade writes can be said, so it stays editable. The title is not asked, so an event with none can be given one.
- **Owned events stay floating.** Times are wall times as ADR 0015 reads them, never written as UTC.
- **A change or a removal checks the file first.** It also applies the rule that read the file as Kanade's (`kept_here`, ADR 0045): every event in it must have this file's UID, so a file merged by hand with another UID's event is neither changed nor removed ("Its file holds more than Kanade can change safely"). The Surface keeps the event as it showed it. Before writing, the file is read the way the calendar reads it, and must still give an event with that title, start, end and UID; otherwise nothing is written, the field keeps its text and the Surface says "Changed elsewhere". A file gone says it too. A place, a note or a reminder changed elsewhere is not a change to what was shown. The check and the write are not one atomic step (another writer between them is lost), as no lock is shared with vdirsyncer or an editor; the window is the length of a file read.
- **The date can change.** An optional leading `2026-10-15` in the typed text (four, two and two digits) puts the event on that day; the field shows it first when editing. A first word that starts like a date (four digits and a dash, or only digits with two dashes or slashes) and is none, or is a day that does not exist, is an error that keeps the text, as a malformed time is. A title that starts like a date needs the date before it.
- **Edit and Remove on the row.** An owned event's row has `edit` and `remove` buttons (names for `kanade ui`). `edit` is `e`. `remove` is held 0.7 s with the pointer like `d` with the key (ADR 0085), the row filling meanwhile; a tap, and `kanade ui press`, which is a tap, remove nothing. A removal that is refused is said in the agenda until the next key. Read-only events show neither.
- **No log of content.** A refusal is logged as its kind only, never a title, time or file text.

## Deferred

- **Recurrence**: no way to type or change a repeat; a file with one is refused for change.
- **Google or vdir write-back**: Kanade writes only its own events; `calendar.paths`, Google's mirror and subscription files stay read-only (ADR 0015, ADR 0016). No Google write scope, no network write.
- **Time zones**: owned events are floating; a zoned file is refused for change.
- **Undo or a trash**: a removal is final.
- **CLI verbs** to add, change or remove an event.

## Alternatives

**Rewrite from the typed text, as before.** Simple, and it loses whatever was added by hand.

**Compare the file's modification time.** Any touch, even a save that changes nothing, would refuse; comparing what was shown is what the user means by "changed".

**Keep the raw text read when the Surface loaded.** The Surface holds expanded events, not files, and reading on a key press would stall the frame.

## Consequences

- A hand-edited event the field cannot say (above) can only be removed, or edited in its file.
- After "Changed elsewhere" the field retakes the event as its file now reads (`add::reread`), or drops what it cannot say, so the next Enter does not refuse on the old baseline again.
- The pointer hold on `remove` fills the same, and the footer wording names no key for one held with the pointer (`confirm::pointer`).
- DTSTAMP is not touched by a change.
- Not tested live here: the buttons on the row, the pointer hold on `remove` and the footer message need a run on the session.
