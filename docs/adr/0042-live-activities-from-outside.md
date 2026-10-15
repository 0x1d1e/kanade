# 42. Live Activities from outside

Status: accepted.

## Context

Every Activity came from a Kanade source. A build, a download, a backup or any script had no way to say on the Island that it runs, how far along it is and how it ended; the user watched a terminal or waited for a notification that says only the end. iNiR's `LiveActivities` service shows that a shell can take this from outside, but trusts whatever it is given.

## Decision

- `kanade activity` (the `live` Module, on by default) posts Activities of a new Kind, `Live`, whose Detail is a `Job`: a title, a detail line, how far along in thousandths or nothing, and how it ended.
  - `start <id> <title>` posts it, or starts a live one afresh. `progress <id> <amount>` takes a percent (`40`, `40%`) or a fraction (`0.4`). `detail <id> [text]` sets or clears the line under the title. `end|fail <id> [text]` ends it. `touch <id>` says it still runs. `list` names the live ones.
  - `run <id> <title> -- <command>...` runs in the client: it starts the Activity, runs the command with Ctrl+C reaching only the command, touches the Activity every 20 s while it runs, ends it as done on exit status 0 and failed otherwise, and exits as the command did (127 when it cannot start, 128 + the signal when one ended it). Without a shell, or with `live` off, the command still runs and stderr says why nothing shows: the command is what the user asked for.
- Live, it is Persistent, Ongoing, Global, Interrupt None: over Media and a Satellite beside a higher primary, as a timer. Ended, it is the same Transient Actionable on the focused output for 10 s as a saved recording, so the end is seen where the user looks and goes on its own.
- It has no Surface; a click opens Controls, as for any Kind without one.
- What comes in is another program's, so it is bounded and cleaned in the parse both sides share:
  - at most 8 live at once, at most 3 of them a notification's (ADR 0059); a ninth `start` is refused until one ends;
  - an id is 1 to 64 of `[A-Za-z0-9._-]`, so it never carries markup, space or control characters, and never begins with `notification.`, which is a notification's progress (ADR 0059);
  - a title and a detail are one line: control and bidirectional override characters become spaces, runs of space one, cut to 80 and 160 characters with an ellipsis;
  - a verb for an id that is not live is refused by name;
  - titles and details are never logged.
- A live one its program stops talking to is withdrawn after `live.stale` (30 min by default, 1 to 1440). A thread waits on a condition variable until the earliest one is due and not at all while none is live, so it costs no idle wakeup. The board and its rules are pure, time given, and unit tested.

## Consequences

- A script that dies before `end` leaves its Activity up to `live.stale`; `activity run` avoids that for a command it wraps.
- A program can repost as fast as it likes, but a change within 250 ms of that Activity's last post is held and posted as it falls due, and one that changes nothing is not posted (ADR 0058), so the island redraws at most four times a second per Activity.
- A changed `live.stale` applies from the next `kanade activity` call or withdrawal.
- The Kind has a short Satellite form (a percent, else the pulse) and no own Surface; a Surface listing them is left until one is asked for.
- Any local process of the user can post one, as it can call any other verb; the bounds keep it from flooding the Island or drawing misleading text over other lines.
