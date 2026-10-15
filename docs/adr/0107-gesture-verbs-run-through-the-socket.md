# 107. A gesture runs a Kanade verb through the shell's own socket

Status: accepted. Part of the gesture launcher (ADR 0105). Keeps the boundary of ADR 0055.

## Context

A recognized gesture must do something. Running a user command would make the pointer overlay a second Hook with none of ADR 0055's bounds (closed events, no shell, bounded child). An `IpcReply` also cannot be built outside the runtime, so the shell cannot call its own `ipc::answer` directly.

## Decision

- **Only Kanade verbs.** `gestures.<name>` is a verb string, split on spaces (no quoting), parsed by `cli::parse` when the config is read and again when the gesture runs. `gesture::check` refuses what does not parse; `dmenu`, `capture color` and `gesture` itself (they wait on the user); `activity run` and `activity hook` (they run a program); and `session logout|reboot|poweroff` (a pointer slip must not end the session). Docs say user commands go through Hooks or a verb.
- **Through the socket.** On a match the verb runs on a worker thread by `cli::call`, the client the CLI uses, so it takes the path every verb takes: the Module gate (`ipc::answer` refuses a verb of an off Module), the parse, the same replies. No second dispatcher.
- **Failure shows.** A refused verb posts a Transient Failed Live Activity (`live::gesture`) with its first line (cut to 60 characters); never the verb's output otherwise.
- `gesture record` is answered when the third stroke is stored or the overlay closes, so its client waits (`call_waiting`), as `capture color` does.

## Alternatives

**Call `run` directly.** Needs a reply the runtime does not let the shell make; would also skip the Module gate.

**Allow shell commands.** Breaks ADR 0055's closed set and review surface.

## Consequences

- A verb that wants keyboard focus (`launcher open`) runs just after the overlay lets go of it. Not verified on a session.
- A gesture cannot run a program other than through a Hook.
