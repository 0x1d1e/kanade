# 58. Agent hooks, and reposts of a Live Activity coalesced

Status: accepted. Builds on ADR 0042, whose "nothing throttles it yet" it answers.

## Context

A coding agent runs for minutes at a time and asks for the user at points the user cannot see from another window. Claude Code fires hooks on its events, each a command given a JSON document on stdin, and a hook that fails or hangs sits in the agent's own loop. `kanade activity` already shows what another program does, so the only missing piece was a reader of that document. A hook fires once per tool call, so the first real client reposts many times a second, which ADR 0042 left unthrottled.

## Decision

- **`activity hook claude`** runs in the client, as `run` does. It reads one hook document from stdin (at most 8 MiB), takes only `hook_event_name`, `session_id`, `cwd`, `tool_name`, `notification_type` and `error`, and says the same verbs to the shell. Only Claude Code's events, as its documentation gives them, are read; no other agent is claimed.
  - `UserPromptSubmit` starts the Activity, titled `Claude Code: <cwd's folder>`. `PreToolUse` sets the detail to the tool's name. `PermissionRequest` and the `Notification` types that wait for the user (`permission_prompt`, `elicitation_dialog`, `elicitation_url_dialog`, `agent_needs_input`, or none given) set it to `Waiting for you`; other types need nothing. `Stop` and `SessionEnd` end it; `StopFailure` fails it with the error type, starting it first when it was not live, as a failure is worth seeing.
  - Only `UserPromptSubmit` and `StopFailure` start one. A detail or end for an id that is not live is no error and changes nothing, so a hook that arrives late, or after the shell restarted, never brings an ended session back as a Persistent Activity. The cost is that an Activity lost to a restart returns at the next prompt.
  - The id is `claude-` and `session_id` when that is a valid id (the alphabet of ADR 0042), else `claude-` and the first 16 bytes of its SHA-256 in hex, so every session id gets a valid id of its own, and the prefix keeps agents and scripts from meeting. The 8 live, the 80 and 160 character cuts and "never log content" are the shell's, applied as for any verb; the client logs and prints nothing of the document.
  - Exit status is 0 or 1, never 2 (Claude Code takes 2 as a block) and never 3. 0: told, nothing to show, no shell running, or `live` off. The agent runs on machines without Kanade, and a hook error on every tool call there would be noise for no fault. 1 with one line on stderr: not a hook document, no `session_id`, an event not mapped, a refusal like the 8 live, or no answer within 2 s. Claude Code takes a non-zero other than 2 as a non-blocking error.
  - The wait for the shell is 2 s for the whole hook, not per verb (`cli::call_within` is given what is left of one deadline), and covers the connect, which runs on a thread so a shell with a full queue cannot hold it, and the write. A `StopFailure` for an Activity not live says three verbs within that one bound. The tool name, error type and folder name are cut to their lengths before they go over the socket. `PreToolUse` is set `"async": true` in the documented configuration, as Claude Code's hooks reference allows for command hooks, so the busiest event never waits on the shell; a late arrival after a session ended changes nothing, and one just after `PermissionRequest` can replace its "Waiting for you" line until the next event.
- **Reposts are coalesced** in the `Board`, which stays pure with time given. A change (`progress`, `detail`, `touch`) that leaves the Activity as it was is not posted. A change within `REPOST` (250 ms) of that Activity's last post is held, the newest kept, and posted when the time is up. `start` and `end` always post at once, `end` carrying the latest. The `live` Module's thread already sleeps on a condition variable until the earliest stale Activity; it now also wakes for the earliest held post, and sleeps with no timeout while none is live or held, so it adds no idle wakeup.

## Alternatives

**A throttle in each client or in `hook`.** A script posting from a loop would still redraw the Island per call; the bound belongs where the Island is posted.

**Start the Activity from any event.** Simple and robust to a restart, but with async hooks or parallel tool calls an event can arrive after `Stop` and leave a Persistent Activity until `live.stale`.

**Exit non-zero for no shell.** Shows a hook error for each event of every session on a machine where Kanade does not run.

**A hook per agent as a Module, or a plugin.** One reader of one document does not need a Module of its own; `live` owns the verbs, and `docs/wiki/Agent-hooks.md` shows another agent how to use them.

## Amendment: an opt-in permission decider

The default `activity hook claude` is unchanged: it still returns within its 2 s bound, exits 0 or 1, never 2, and still sets no buttons. `PermissionRequest` still only sets the detail to `Waiting for you`.

`activity hook claude decide` is a separate argv, used only as the replacement for the `PermissionRequest` hook entry, with a longer Claude timeout (90 s). The shell still never exits 2. This process does:

- If the focused window's app id is a best-effort match for Claude or Codex, from one `Windows` ask of the list niri already has, it exits 0 and shows nothing. A terminal whose title mentions the agent is not that match: the match is the app id.
- Otherwise it posts Deny, Allow and Always on the session's Live Activity, the three labels ADR 0110 allows, and sets the detail to the command, one line, cut. The command is never logged. `ui.describe` redacts that detail unless the mode is content. The pills stay until the press, the Activity ends, or the wait ends.
- It waits with the existing `pressed-wait`, 60 s. Allow exits 0. Deny exits 2. No answer exits 2. A prompt that was not shown does not allow the command either.
- Always remembers that tool name for this process only, not on disk, and not the command. A later `PermissionRequest` for the same tool name exits 0 with no prompt.

## Consequences

- ADR 0042's note that nothing throttles reposts is superseded: a client redraws the Island at most four times a second per Activity.
- `progress` held for up to 250 ms shows late by that much; a bar moving faster than four steps a second shows its newest.
- A turn the user interrupts sends no `Stop`; `SessionEnd` or `live.stale` ends it. `Waiting for you` stays until the next tool or the end.
- Claude Code's events and fields may change; an unknown event is exit 1 and a missing field is exit 1, so a change shows as a hook error, not a wrong Activity.
