# 151. Claude integration removed

Status: accepted. Supersedes ADR 0058's agent-hook reader and permission decider, and ADR 0145 in full.

## Context

Kanade grew two pieces around one vendor's product: a reader of Claude Code's hook documents (`activity hook claude`) with an opt-in permission decider (`decide`), and a `usage` Module that drew Claude's plan utilization as five-hour and seven-day rings. Generic Live Activities (`kanade activity` start/progress/detail/end/fail/touch/actions/pressed/wait/list/run, ADR 0042) already cover what a script, build or download needs, and they name no agent.

Three things made the Claude-specific paths untenable. Claude Code's hook semantics moved under them: the decider answered a `PermissionRequest` with an exit status the current protocol no longer reads as the same verdict, so a hook wired as documented could silently stop permitting or blocking what the user saw. The plan-usage Module fetched a vendor endpoint with a bearer key read from a private file, which is a credential, network and product tie the shell had no reason to keep. And both required reading one vendor's documents as the shell's contract, so every change on that side was Kanade's bug.

## Decision

- **The Claude Code hook reader and decider are removed.** `activity hook <agent>` and `activity hook <agent> decide` no longer parse; the shell does not read a hook document, decide a permission, or keep a tool name allowed in memory for `decide`. The `remember` and `remembered` verbs that existed only for the decider's "always" press are removed with it.
- **The Claude plan-usage Module is removed.** No `usage` Module, no five-hour or seven-day rings, no API endpoint, no `usage-key` file, no `kanade usage` or `kanade usage key`. `Kind::Usage`, `Detail::Usage`, `Detail::UsageNotice`, the `Usage` struct and their forms, Satellites and shape measurements go with it. Nothing fetches the vendor endpoint or stores its key.
- **Generic Live Activities stay as they are.** `activity` start, progress, detail, end, fail, touch, actions, pressed, pressed-wait, wait, list and run, the `live` Module, its coalesced reposts (ADR 0058's second decision, which stands), its bounds, its presses and its Satellite forms are untouched. A program that wants to show a session can post one with the documented verbs itself.
- **User Hooks stay.** The `hooks.<event>` commands (ADR 0055) and their environment filtering are unchanged, including the generic credential-name redaction that keeps a variable such as `ANTHROPIC_API_KEY` out of a hook's child. That redaction names providers only because credentials do; it is not part of the removed integration and stays.
- **Dictation stays.** Its Gemini consent and key path are a separate, opted-in Module (ADR 0111) and are not touched by this.
- **The docs and code claims follow the code.** The Agent hooks wiki page is deleted, and the CLI, Configuration, README, Home and `CONTEXT.md` no longer describe either path. ADR 0058 and ADR 0145 remain as history, their Status lines pointing here.

## Alternatives

**Keep the reader, drop only `decide`.** The reader was the smaller half, but it still bound the shell to one vendor's event names and document fields, and its wiki page had to teach that vendor's settings file. Removing half would leave the larger maintenance surface in place for no product gain.

**Reimplement the decider against current Claude semantics.** That is a vendor protocol, not a Kanade feature, and it would need its own Module, its own documentation and its own tracking. A user who wants a permission prompt on the Island can read the agent's hooks and post a Live Activity with actions; Kanade runs nothing for a press and stays out of the verdict.

**Keep the usage rings behind a generic endpoint setting.** The rings were not a shell capability; they rendered one subscription's numbers with one vendor's authorization scheme. A generic HTTP-to-Activity Module was not asked for and would have widened the credential and network surface rather than shrinking it.

## Consequences

`kanade activity` has no agent verb, `kanade usage` is an unknown verb, and the `usage` Module is absent from `kanade module list` and the Settings Modules page. A configuration that still sets `[modules] usage = ...` or a hook entry calling `kanade activity hook claude` now gets an unknown-verb refusal from the agent side and an unknown Module key warning from the config side; nothing else was live to migrate, since both paths were opt-in and stateless apart from the removed key file. Existing `$XDG_STATE_HOME/kanade/usage-key`, if present, is no longer read and may be deleted; Kanade never writes it again. ADR 0150's account of Usage forms describes the design of its time and no longer describes the tree.
