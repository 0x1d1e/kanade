# 77. The session countdown says what would be lost

Status: accepted. Builds on the Session Surface (ADR 0028 runtime, `docs/design.md` Session) and ADR 0047 (logind is asked over Kanade's own connection).

## Context

A restart, power off or log out counts down 60 s with Cancel and Now. It said only the time left, so a running recording or timer, or a program holding a shutdown inhibitor, was found out too late.

## Decision

- **The countdown view lists up to three lines** under the time: a running recording, a running Timer, the live Activities running, and each logind block inhibitor on `shutdown` with who and why. More than fit end in `and N more`. Read at each draw from the island's Activities and the Timer's standing, so a recording stopped meanwhile drops off.
- **A countdown says why it runs, before what it would lose.** `Cause` is `Asked`, for one the user asked from the Session Surface or `kanade session`, or `Battery(percent)`, for the sleep, hibernation or power off a critical battery asks for. A battery's adds `Battery at N%` as the first line, so an automatic ending is never taken for one the user asked for; an `Asked` one adds nothing and keeps `Unless cancelled`. The cause is the caller's line, not `warning::lines`': that bounds what would be lost, which the battery is not.
- **Warning only.** Nothing is refused, delayed or asked. The action runs as before.
- **Inhibitors by `ListInhibitors`**, asked once when a restart or power off countdown starts, on a thread of its own over a system bus connection, so the Surface never waits on the bus. logind blocks a reboot or power off by `shutdown` inhibitors with mode `block`; `sleep` ones block only sleep, so they are not listed. A log out is blocked by none. A logind that does not answer lists nothing.
- **Strings are untrusted.** Who and why are the inhibiting program's words: whitespace collapsed, control characters dropped, cut to 24 and 40 characters, drawn elided, never logged. The list is kept per countdown serial, so a replaced countdown shows none of the old one's.
- **Why not a confirmation step first.** `kanade session reboot` and the critical battery action would change meaning, and the existing countdown with Cancel is already the confirmation.
- `island/` gained `activities()`, a read of every Activity registered, as the one way to count live ones without the live source's lock in a view.

## Consequences

- The list shows from the moment the countdown starts; the inhibitors a moment later, when logind answered.
- A critical battery countdown (suspend, hibernate, power off) shows the same lines on its Surface, led by the battery's percent.
- A countdown with a battery cause and three warnings is four lines, which the reading's room does not hold; the warnings keep their own bound and the reading elides, so the cause is never the line dropped.
