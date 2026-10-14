# 59. A notification's progress is a Live Activity

Status: accepted. Builds on ADR 0042 (Live Activities from outside), ADR 0009 (policy fields) and ADR 0007 (Banners).

## Context

Senders of long operations (a file copy, a download, a volume script) repeat one notification with a `value` hint, a percent from 0 to 100, as `notify-send -h int:value:40` does. The freedesktop specification does not list the hint; it is a de facto one that other daemons read. Kanade showed each repeat as a Banner and a card, with no sense of "still running". A Live Activity already shows a title, a bar and a line, in the Island, over Media, as a Satellite beside a higher primary.

## Decision

- **The `value` hint is read** as a number of any kind, rounded and kept within 0 to 100; any other kind is none. `expire_timeout`, until now ignored, is kept when positive.
- **A notification with a value under 100 is a Live Activity of the `notifications` Module**, shown through `live::show` in this process, not through the verbs. Its policy fields are the `live` ones, not new ones: while running Ongoing, Persistent, Global, Interrupt None, so a Satellite and never an interrupt; ended, Actionable, Transient (`SHOWN`), on the focused output (ADR 0009: each field set by the source, none implied by another). It is one of the 8 live ones (ADR 0042), counted with those of clients and refused past them; the notification and its Banner stay as before.
- **The id is `notification.<app>.<id>`**, the app cut to lowercase letters and digits, at most 32, and the notification's id, which every replacement of it keeps. A title is the summary, else the app; the line is the body, else the app. Both are cut and made one line by `live`, and never logged.
- **It ends or goes with the notification:**
  - value 100 ends it as done (the Transient form above), only when it was shown;
  - closed by its sender or dismissed, expired by its rule, evicted by the history bounds, or replaced without the hint, it is withdrawn at once with no ended form;
  - a positive `expire_timeout` withdraws it after that long without a new version, else `live.stale` does, as for a client that stops talking. Each version is a word, so a copy that keeps reporting stays.
- **It follows the notification's rule and Do Not Disturb as its Banner does:** shown only when its rule lets a Banner show (`banner` not false) and, under Do Not Disturb, only when Critical or its rule says `bypass_dnd`. A version that is no longer allowed withdraws it. The `banners` Module may be off; the `live` Module must be on, and `notifications` names it as optional, so with it off nothing here runs and nothing is refused.
- **`live` has two more entry points beside `request`,** `show` and `withdraw`, and `Entry` has `lasts`, an own bound for staleness. They run under the same lock and post as `request` does; the `Board` stays pure.

## Alternatives

**Show it in the Notifications Surface or a Banner only.** A Banner leaves after seconds and reposts as a new one each time; no Activity says "still running".

**A new Kind, or a new policy for it.** A progress is what a Live Activity is. Another Kind would copy `live`'s bound, id and staleness rules, and the user would meet two kinds of progress.

**Go through the CLI verbs.** The daemon would call itself over the socket, and `Request` parses a client's text; a call in process needs none of it.

**Suppress the Banner while the Activity shows.** It would also hide a Critical or a last "done" card. Left to the rule: `banner = false` hides both.

## Consequences

- A sender that uses `value` for a level, not a progress (a volume or brightness script), shows an Activity until its `expire_timeout`, or `live.stale` (30 min) when it gives none. A rule with `banner = false` for that app shows none; this is documented in the wiki.
- A notification arriving while 8 are live shows no Activity, silently.
- A sender that sends 100 without ever sending less shows nothing; one that stops below 100 without closing shows the Activity until the bound above.
- `value` is not in the specification; a daemon of the same name elsewhere may read it differently, so the docs say it is de facto.
