# Agent hooks

A coding agent can show a session as a live Activity on the Island: it starts with your prompt, shows the tool it runs, says when it waits for you, and ends when the turn does. It is the `live` Module's `kanade activity` (see [CLI](CLI.md)), fed by the agent's own hooks.

Only Claude Code is supported, as its [hooks reference](https://code.claude.com/docs/en/hooks) describes its events. No other agent is claimed: see the last section for any that can run a command on an event.

## Claude Code

Add this to `~/.claude/settings.json` (or a project's `.claude/settings.json`), merging with any `hooks` you have:

```json
{
  "hooks": {
    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "kanade activity hook claude", "timeout": 5 }] }],
    "PreToolUse": [{ "hooks": [{ "type": "command", "command": "kanade activity hook claude", "timeout": 5 }] }],
    "PermissionRequest": [{ "hooks": [{ "type": "command", "command": "kanade activity hook claude", "timeout": 5 }] }],
    "Notification": [{ "hooks": [{ "type": "command", "command": "kanade activity hook claude", "timeout": 5 }] }],
    "Stop": [{ "hooks": [{ "type": "command", "command": "kanade activity hook claude", "timeout": 5 }] }],
    "StopFailure": [{ "hooks": [{ "type": "command", "command": "kanade activity hook claude", "timeout": 5 }] }],
    "SessionEnd": [{ "hooks": [{ "type": "command", "command": "kanade activity hook claude", "timeout": 5 }] }]
  }
}
```

`kanade` must be on the `PATH` Claude Code runs with. Each hook reads its event from stdin, so there is nothing else to configure; use as few of the events as you like.

| Claude Code event | The Activity |
|---|---|
| `UserPromptSubmit` | starts, titled `Claude Code: <project folder>`, afresh if it was live |
| `PreToolUse` | detail line: the tool's name, like `Bash` or `Edit` |
| `PermissionRequest`, `Notification` | detail line: `Waiting for you`. Of notifications only a permission prompt, an elicitation and an agent waiting for input count; an idle prompt comes after the turn ended |
| `Stop`, `SessionEnd` | ends as done, shown for 10 s on the focused output |
| `StopFailure` | ends as failed, with the error type as the line (`Rate limit`), also when it was not live |

What it does and does not do:

- One Activity per Claude Code session: its id is `claude-` and the session's id, or a digest of it when that has characters an id cannot. At most 8 Activities are live, so a ninth session is not shown and its hook exits 1 with that reason.
- Nothing of the document is used but the event, the session's id, the folder name, the tool's name, the notification type and the error type. Your prompt, tool input and Claude's replies are read past and never shown or logged.
- A tool, wait or end event for a session that is not live changes nothing, so a late event never brings an ended session back. Restarting Kanade mid-turn loses the Activity until your next prompt. A turn you interrupt sends no `Stop`; `SessionEnd`, or `live.stale` (30 min), ends it.
- The `Waiting for you` line stays until the next tool or the end, as no event says you answered.
- Many calls a second are held to about 4 posts a second per session, so a busy agent does not redraw the Island for each.
- The hook is bounded so it cannot hold the agent: it waits at most 2 s for the shell. It exits 0 when it told the shell, when the event needs nothing shown, and when no shell is running or `live` is off (Claude Code runs on machines without Kanade). It exits 1, with one line on stderr that holds none of the document, for something not a Claude Code event, an event it does not map, or a refusal like the 8 live. It never exits 2, which Claude Code takes as a block; Claude Code shows a non-zero exit as a hook error and goes on.

## Other agents

`kanade activity` has no agent in it: any agent or script that can run a command on its events can use the same verbs.

```sh
kanade activity start agent-1 "My agent"        # a turn starts
kanade activity detail agent-1 "Running tests"  # what it does now
kanade activity progress agent-1 40%            # how far along, if it knows
kanade activity end agent-1                     # done; `fail agent-1 "why"` for a failure
```

An id is 1 to 64 letters, digits, `.`, `_` or `-`; give each session its own. Whatever the agent's hook feeds in must come from its documentation. Kanade reads only Claude Code's own events and does not guess at other formats.
