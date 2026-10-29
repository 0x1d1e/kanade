# 145. Usage rings on the Island

Status: accepted. Builds on the withheld-Surface rule and `Activity::new` policy in `CONTEXT.md`.

## Context

A user wants Claude's plan usage as two rings on the Island — five hours and seven days — like macOS shows subscription usage. The key is a secret: it must never appear in argv, the socket, a log or `kanade status`. The Module is off unless turned on: off, no thread, no key read, no fetch, nothing is posted.

## Decision

- **A `usage` Module, off unless turned on.** Off, this file is never read: no thread, no key read, no fetch, nothing posted. On, a worker fetches Claude's usage endpoint with a bearer token at most every five minutes, and at once on `kanade usage`.
- **The key comes from `$XDG_STATE_HOME/kanade/usage-key`, written by `kanade usage key` from stdin without echo and kept at 0600.** `kanade usage key` runs in the user's terminal: the key is read on stdin, never an argument, never through the shell's socket. The CLI answers `usage key` itself with `usage::key()`; the shell never sees it. `kanade usage` (refresh) goes over IPC to the shell's usage worker.
- **`Kind::Usage`** with `Detail::Usage(Usage)` for the rings and `Detail::UsageNotice(String)` for a once-per-window limit notice. Posted on the focused output, rings Persistent Ongoing, notices Transient Feedback.
- **Lenient parsing.** `read_usage` takes `utilization` or `used_percent`, a number or a string of one (percent over 1, fraction otherwise), clamps to 0..=1. A missing window leaves it `None`; a bogus cell is `None`, not fatal. Missing both windows is unrecognised. The body is read to 64 KiB and never kept: only the two numbers and when they were read, in memory.
- **One notice per window per reset.** At or over 90% of a window, one Transient for that window, not one per refresh. Dropping below clears the mark, so a later window may have its own one notice again.
- **Backoff.** Failures double from 60 s to 30 min, then one notice after three in a row and silence till `kanade usage` is asked again, never a tight loop.
- **CLI.** `kanade usage` (refresh) sends `Call::Usage(Request::Refresh)` over IPC; the shell's worker fetches at once and replies "refreshing". `kanade usage key` is answered in this process by `usage::key()`, which reads stdin hidden at a terminal and writes `$XDG_STATE_HOME/kanade/usage-key` at 0600, then exits. No response body is kept.

## Consequences

- Turning the Module on takes a restart, like every Module.
- `kanade usage` while off is refused with "module usage is off".
- `kanade usage key` needs no shell and no key file: it creates the file. A missing key file means the worker posts nothing and polls nothing.
- The bearer token is never logged: errors say `Fetch` or `Refused`, never the key or the URL with the key in it.
- `ureq` is already a dependency (Google Calendar uses it); no new crate.
