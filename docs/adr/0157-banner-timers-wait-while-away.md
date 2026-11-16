# 157. Banner timers wait while the user is away

Status: accepted. Amends [ADR 0007](0007-banners-and-osd-leave-the-island.md) (a Banner's time)
and [ADR 0023](0023-banners-come-out-of-the-island.md) (the Banners column, which now counts the
ones waiting).

## Context

A Banner counted its time from arrival unless the pointer was on it, a fullscreen window held it
or DND dropped it. Under the lock, or while nobody was at the desk, a six-second Banner expired
unseen: the user came back to a history entry and no card, with no way to have seen it in time.
Isle, the reference, counts an alert only while someone is present: it stops on idle and shows its
full time again when the user returns.

The runtime already follows `ext-idle-notify-v1` and exposes `kanade_runtime::idle::Idle` with
`idled(timeout)`. The `idle` Module is off by default, so the signal cannot depend on it.

## Decision

- `kanade_runtime::idle::PRESENCE` is 60 s. `follow_idle` always arms it, deduplicated with any
  timeout the `idle` Module asked for, so one notification exists whether or not that Module is on.
  `Idle::idled(PRESENCE)` is the away signal; input resumes it. No new thread or service.
- `Stack` keeps `present`, separate from `hovered` and `held`. Going away turns every shown
  `Timer::Running` into `Timer::Paused` with what it had left; `expire` does nothing and `deadline`
  is none meanwhile. Coming back gives each Banner the absence paused its full length
  (`Timer::Running(now + length)`, or `Paused(length)` while the pointer is on the Banners), and a
  Banner the column had no room for holds that full time for when it is placed. Sticky Banners and
  one with a reply being typed stay as they are. The queue is untouched, so a Banner promoted
  behind them still gets its full time. Fullscreen silence keeps its own behavior.
- The Banners thread compares the idle state with the Stack's each loop and calls `presence` on a
  change; `service::watch::<Idle>` nudges it, so there is no poll.
- While Banners wait, the column draws a quiet `N more` line under the cards, on the same window
  and with no input area of its own: no synthetic notification or Banner, and no click action.

## Alternatives

**Expire unseen Banners as before and say so in the history.** The history already records them,
but the user still never saw the card; presence is the behavior the reference gets right.

**Follow presence only with the `idle` Module on.** The Module is off by default, so the default
setup would keep expiring Banners unseen.

## Consequences

- A Banner outlives the lock and the empty desk, and the user meets it with its full time.
- The queued count is visual only; it cannot be clicked in this slice.
