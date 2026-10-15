# 110. External Live Activities carry actions and hear them pressed

Status: accepted. Extends ADR 0042 and 0058 (Live Activities from outside), 0055 and 0107 (the shell runs no program because it was told to) and 0097 (the shell holds no call). Supersedes nothing.

## Context

A Live Activity (ADR 0042) is read-only. A coding agent that waits for a permission, or a build the user wants to cancel, has to ask the user elsewhere. Reminders, the timer and the session countdown already draw Actions as pills on Peek and run them by key (`Activity.actions`, which the Arbiter never reads); a Live Activity could do the same if its poster could set them. The cost is trust: the text and the buttons are another program's, and a press must not turn the shell into a launcher of whatever that program names (ADR 0055, 0107).

## Decision

- **A press is delivered back to the poster; the shell runs nothing.** Actions carry no argv. A press is queued for the program that posted the Activity, which reads it over the same socket and does what it likes in its own process.
- **Verbs** of `kanade activity`:
  - `actions <id> [<key>=<label>]...` replaces the set; none clears it.
  - `pressed <id>` takes the oldest press waiting: prints its key and exits 0, or exits 1 with nothing.
  - `wait <id> [--timeout <seconds>]` runs in the client: it asks `pressed` every 250 ms until there is a press, or the timeout passes. The shell answers each call at once and holds none (ADR 0097), so it keeps no thread or socket for a waiter. The polling is not free: each ask is a connect and an IPC handled on the shell's draw thread, about 4 wakeups a second for as long as anyone waits (for `run --action cancel`, the command's lifetime). A call that fails for now (a timeout, a full queue) is asked again; a refusal, or no shell, ends the waiting.
  - `run <id> <title> --action <key>=<label>... -- <command>...` sets the actions and, for the action keyed `cancel`, sends SIGTERM to the command when it is pressed, and again at each further press while it runs, for a command that ignores the first. The signal goes to the direct child only, not its process group: a `sh -c` wrapper dies, a grandchild may live on. `run` reads presses only for `cancel`, so it takes no other action beside it: it would read and lose their presses before their own reader came for them.
- **Bounds.** At most 3 actions. A key is 1 to 32 of `[a-z0-9_-]`, checked in `Request::parse`. A label is one line cut to 24 characters by the same `line()` as titles (control and bidirectional override characters become spaces; invisible format characters, Unicode Cf, are removed from a label), never empty, and never logged. Keys differ in a set. The shell refuses, with a message, the labels `Cancel`, `Now`, `Dismiss`, `Stop` and `Confirm`, compared without regard to case, after removing format characters (zero-width, joiners, direction marks) and trimming space, punctuation and symbols from both ends (`Cancel.` is `Cancel`). This is best effort: a look-alike in other scripts can pass. It is not the defence: the pill is always neutral and a press only messages the poster, so a misleading label cannot make the shell do anything. The mailbox holds at most 4 presses; a newer one drops the oldest.
- **Lifetime.** Actions die with the Activity: `end` and `fail` clear them, the withdrawal after `live.stale` removes them, a new `start` of an id resets the set and takes a new generation. The mailbox is dropped with the Board's entry. Each start has a generation (`Job::serial`, never 0 for a client's); a pill's press names `<id>#<generation>` and is ignored when it is not the start now live, as a session countdown's serial is.
- **A press shows as pending.** The press sets the set to one disabled "..." pill (an empty key, which no poster has, so pressing it does nothing). The poster's next word for the id (`detail`, `progress`, `end`, `fail`, `actions`) clears it; otherwise it lapses after 10 s and the set is pressable again. Time is given to the Board, never read; the `live` thread sleeps until the pending press lapses, so there is no timer of its own. A press is posted at once, not held back by the 250 ms repost rule.
- **Presentation.** Pills are always the neutral `action_pill`, never the filled `main_pill`; the poster chooses no style and no tone and a destructive action is not marked. They show on Peek only. A Peek is 300 pt wide: the percent gives way to the pills, and as many as leave the title 72 pt are drawn, and the first is cut with an ellipsis to leave it too. No layout was rewritten; actions beyond what fits are not reachable by the pointer (see the gap below). `kanade ui` names each pill `action`, `action#2`, ... with the label as content, and `ui press` goes through the real click.
- **Where the data lives.** Actions are the Activity's `actions`, as for every Kind, beside `Detail::Live(Job)` and not in it; the Board's entry owns them, and what it posts is the set as the Island shows it (the pending form included). `Job` carries only the generation, so that the view can name it in a press. `Job` equality still drives repost suppression; the Board compares the pair of job and shown set.
- **Gestures.** `gesture::check` is an allow-list and `activity` is not in it, so `actions`, `pressed` and `wait` are refused like `run` and `hook`.

## Known gap

Pills are pointer-only on Peek in v1. There is no Live Surface, so the keyboard cannot reach a pill: `kanade ui press` works, but no key does. A Peek that holds fewer pills than actions hides the rest. A Live Surface (a list of live Activities with their actions, keyboard-reachable) is the way to fix both and is left until asked for.

## Alternatives

**The shell runs a command per action (argv from the poster).** A way for any local process to have the shell start a program with its own environment and the user's pointer behind it; it also breaks ADR 0055's closed set. Refused.

**A long-held call for `wait` in the shell.** Holds a socket and a thread in the shell for a client that may never return; ADR 0097 chose polling from the client.

**A callback socket or command per Activity.** The poster must listen on something; a file, a FIFO or an address is another surface to secure. The mailbox needs none.

## Consequences

- The poster does the work and can be as wrong as it likes: a stale pill, a press nobody reads, a poster that died all end in the Activity going stale or ending, and nothing the shell did.
- A poster that does not read presses leaves the last 4 in a mailbox of a few bytes until the Activity ends.
- Ids are not owned: any local process can `pressed` (which takes the press, destructively) or `start` over another poster's id, stealing its presses or wiping its actions. That stays inside the user-only socket model (ADR 0098), where any process of the user can already post anything.
- A label can still mislead (it is the poster's text), but never as one of the five words the shell uses itself; any local process of the user could already post titles (ADR 0042).
