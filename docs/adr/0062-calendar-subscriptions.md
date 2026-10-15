# 62. Calendar subscriptions

Status: accepted. Extends the local calendar (ADR 0015) and follows the Google mirror (ADR 0016, ADR 0044): a thread mirrors outside events into read-only files the calendar reads.

## Context

Holiday calendars, team calendars and a school's timetable are published as public iCalendar URLs, often as `webcal://`. Until now the calendar fetched nothing but a Google account's events, and a user had to cron `curl` into `calendar.paths` to see a feed.

A feed's URL is often its only secret: a private feed from a calendar service is "public" to whoever holds the address. So the address is data Kanade must not leak, and it arrives from the outside as text that can be huge, slow, hostile or not a calendar at all.

## Decision

- **Setting.** `calendar.subscriptions`, a list of up to 16 addresses (`Kind::Urls`), a later layer replacing the list. `https://` is taken; `webcal://` is read as `https://`; `http://` and anything else is refused with a message that never repeats the value. `calendar.subscription_interval` is 30 to 1440 minutes, 60 by default; the floor keeps a feed's host from being asked more often than a calendar is worth.
- **Module.** `calendar-subscriptions`, requiring `calendar`, optionally `calendar-surface`. Off, it starts no thread, makes no request and the calendar does not read its files. On with no feed, its thread waits on its queue and costs no wakeups.
- **Fetching.** One thread, one feed at a time, with the Google Worker's shape: a queue that outlives the thread, a per-feed due time, `Backoff` from 5 minutes up to the interval, a `Subscriptions` Service for the Surface and `status`. Each request carries the last answer's `ETag` and `Last-Modified` as `If-None-Match` and `If-Modified-Since`, only for a copy that is kept, so an unchanged feed costs a 304. The validators live in memory; after a restart the first request is whole.
- **Bounds.** ureq with `https_only` (checked on every request of a redirect chain, so a redirect to `http://` fails as `Insecure`), at most 3 redirects, a 30 s global timeout, the body read with a 5 MiB limit on what it decodes to (`sources::body`; ureq's own limit counts the wire, which a gzip answer outgrows, so a test serves one), the User-Agent `Kanade/<version>` and nothing else sent: no cookie, no credentials. An answer must be UTF-8 and start with `BEGIN:VCALENDAR` (after a byte order mark and blanks) to replace a copy. The same ureq the weather and Google use; no new dependency.
- **Mirror.** Through `mirror::sync`, as Google's: one `.ics` file per feed under `$XDG_CACHE_HOME/kanade/calendar-subscriptions` (directory 0700, files 0600, atomic writes), named by the first 8 bytes of the SHA-256 of the address, so the name holds no part of it. A file is written only when its text changed; a feed taken out of the config has its file deleted. The calendar reads and watches the directory like another place, only while the Module is on. Turning the Module off leaves the files, since Kanade does not clean what it does not run.
- **Failure.** A failed feed keeps its last good file (read back from disk at start, so a restart offline still shows it), backs off, and is a `Problem` of a few fixed kinds (`Offline`, `Insecure`, `Status(code)`, `TooLarge`, `NotCalendar`, `Disk`); none carries ureq's error text, which names the URL. The Calendar Surface's footer, where Google's problem shows, says "Subscription offline" for one failing feed and "2 subscriptions failing" for more. `kanade status` lists each feed by its place in the list, not by address or host; `kanade calendar-subscriptions sync|status` fetches now or says the same. A config refusal (an `http://` address) shows in `kanade status` and the doctor's config check without the address. The doctor does not see runtime state, so a failing feed appears in `status`, not the doctor.
- **Logging.** The address, host, path, headers and the feed's text are never logged. A write failure logs only the I/O error kind.
- **Recurrence.** A feed's rules run through the calendar's existing recurrence code, which already bounds a `FREQ=SECONDLY` rule (a day fills, six weeks report partial; the `a_rule_each_second_fills_a_day` test), so a hostile feed makes work bounded by the 5 MiB cap and the expansion limit, and never a stalled frame.

## Consequences

- A user with a secret feed address has it in their config file and the Settings window, as any value; Kanade's own outputs omit it. Config files holding tokens should stay private.
- A removed feed's file is deleted when the thread next starts or the config reloads; a feed removed while Kanade is off shows for the moments before that.
- Not done: authentication, `Retry-After`, CalDAV, writing back to a feed (they are read-only), and jitter between feeds.
