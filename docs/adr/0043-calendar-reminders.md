# 43. Calendar reminders

Status: accepted. Builds on the local calendar (ADR 0015), which until now showed events but never said one was coming.

## Context

Calendar files already carry reminders: each `VEVENT` may hold `VALARM`s whose `TRIGGER` is a duration before or after the event's start or end, or a fixed moment. Kanade read the events and dropped the alarms. A user who keeps their calendar in Kanade gets no reminder unless another program watches the same files.

A reminder must go off once. The shell restarts (crash, `scripts/dev`, logout), the machine sleeps and wakes, and the clock is set; none of these may make a reminder go off again or be lost. The shell's idle cost must stay at zero frames and near zero wakeups. noctalia's reminder code is the reference for the shape: a pure planner beside a stateful monitor, a cap per event, a cap on catch-up, a lateness window, and a persisted set of what went off.

## Decision

- **Which alarms.** An event's `VALARM`s with `ACTION:DISPLAY` or `ACTION:AUDIO` both show as a reminder; Kanade plays no sound. `EMAIL` and `PROCEDURE` are for other programs, and Apple's `ACTION:NONE` is no alarm. A `TRIGGER` duration counts from the start, or from the end with `RELATED=END`. An absolute `TRIGGER` in UTC is that moment; one with no zone is when the local clock reads it. Recurring events expand through the calendar's existing recurrence code, each occurrence with its event's alarms.
- **Default lead.** `calendar.reminder` in minutes, `0` (off) by default, gives a reminder that many minutes before each timed event with no alarm of its own. All-day events get none, as their start is a midnight.
- **Pure planner, stateful monitor.** `plan(candidates, fired, lead, now)` in `src/sources/reminders.rs` is a pure function, unit tested. The monitor is a thread of the `calendar-reminders` Module that owns the state: the occurrences near now, the fired-set, and the Activities shown.
- **Caps.**
  - At most 5 reminders per occurrence (`PER_EVENT`), those nearest the event.
  - A reminder more than 30 days before its event (`FARTHEST`) is dropped, so the calendars expand over a bounded window, now - 1 day to now + 31 days.
  - At most 3 show at once (`AT_ONCE`). The rest collapse into one "N more reminders" Activity that names them.
  - Of one occurrence's reminders due at once, as after a restart, only the one nearest the event shows; the others count as gone off.
- **Lateness.** A reminder goes off from its due time until 5 minutes (`LATE`) after its event starts, or after it went due for one on an event's end. One missed while the shell was off or the machine slept still goes off within that window, and never after it.
- **Fired-set.** Each reminder that goes off is recorded in `$XDG_STATE_HOME/kanade/reminders`, one `<moment>\t<key>` line each, with the moment after which it could no longer go off, when it is pruned. The file is written whole to a `.part` file, synced, then renamed over the old one, so a crash leaves either file whole. It is mode 0600 in a 0700 directory. The key is `<event>|<start>|<lead>`: the event is the first 8 bytes of the SHA-256 of its UID, so the file holds no titles or UIDs; the start is the occurrence's start in seconds since the epoch; the lead is `s<seconds>`, `e<seconds>`, `a<moment>` for the event's own alarms, and `d` for the default lead. Because the default lead's key holds no minutes, changing `calendar.reminder` never fires again a reminder its old value fired. An unreadable or damaged line is skipped; an unwritable file is said in `kanade reminders status`.
- **Waking.** No polling. The monitor sleeps on a `CLOCK_REALTIME` timerfd (`clock::WallTimer`) armed at the earliest of: the next reminder due, the next minute a shown countdown reads otherwise, the moment a shown one goes, and the next daily expansion. `TFD_TIMER_CANCEL_ON_SET` wakes it when the clock is set, and a realtime timer fires on resume for a moment passed during sleep. A `service::watch::<Calendars>` wakes it when the calendars are read again, after a file change or a config reload. With no events near, it wakes once a day.
- **The Activity.**
  - Kind `Reminder`, a new Kind; its Detail is the title, the time with how long until (`14:05, in 15 min`, `Tomorrow 09:00, in 18 h`, `Ends 16:00, in 5 min`), and a short countdown for a Satellite (`15m`).
  - Id `Reminder` + the occurrence's key, so a repost of the countdown redraws in place, and one occurrence shows once whichever of its alarms went off.
  - Priority `Ongoing`, as the timer's: a thing on its way the user keeps in view. Above Media and Glance, so a meeting coming outranks the track playing. Below Actionable, Critical and Feedback, so a recording's error, low battery or an OSD still win the primary; there it shows as a Satellite, as only Ongoing and Critical can, rather than hidden as a non-primary Actionable would be.
  - Lifetime `Persistent`, withdrawn by the monitor at the event's start + `LATE`, but at most an hour after it went off and at least `LATE`, and by Open or Dismiss. A Transient would go before a user away from the screen saw it.
  - Scope `Global`: the user may be looking at any monitor.
  - Interrupt `None`: a reminder never collapses or replaces a Surface the user opened, nor opens one by itself.
  - Its own Surface is Calendar, so a click on it opens the Calendar Surface. Actions are Open (opens Calendar, takes the reminder away) and Dismiss. From the keyboard, `kanade reminders dismiss` takes away all shown and `kanade calendar open` opens Calendar.
- **Modules.** `calendar-reminders` requires `calendar`, optionally `calendar-surface` (without it Open only dismisses), owns `calendar.reminder` and `kanade reminders dismiss|status`. Off, it starts no thread and posts nothing.

## Alternatives

**Poll every minute.** Simplest, but it wakes the shell 1440 times a day for nothing, against the zero-idle design (ADR 0004).

**`CLOCK_MONOTONIC` or `CLOCK_BOOTTIME` sleep.** Neither follows a clock set, and monotonic stops during suspend, so a reminder due during sleep would wait for the time asleep after waking. The realtime timerfd with cancel-on-set follows the wall clock the calendar is written in.

**Keep the fired-set in memory.** A restart within the lateness window would fire every reminder again, which with `scripts/dev` restarting on each save is often.

**Key the default lead by its minutes.** Changing the lead from 10 to 5 minutes would fire again, 5 minutes later, a reminder already seen.

**Priority Actionable.** It asks for a choice, and it is: Open or Dismiss. But a non-primary Actionable is hidden, not a Satellite, so behind a low battery or a recording error the reminder would vanish until that went.

**AutoExpand to Calendar, or Preempt.** It would take the island from whatever the user is doing at every reminder; a reminder is not urgent enough to claim focus or close a Surface.

**Desktop notifications.** Posting to Kanade's own notification daemon would show a Banner, but a Banner times out in 6 s and has no countdown. The island is where something on its way lives.

## Consequences

- Reminders from any `.ics` file Kanade reads go off on the island. Google Calendar sync (ADR 0016) writes no `VALARM`s, so its events get only the default lead.
- The fired-set file grows with the reminders of the last day or so and is pruned on each write.
- A recurring event's alarms come from its master component; an alarm changed on one occurrence only follows that occurrence's override, as calcard gives it.
- No sound is played for `ACTION:AUDIO`; reminders are visual only.
