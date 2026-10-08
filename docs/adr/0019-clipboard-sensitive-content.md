# 19. Clipboard sensitive content

Status: accepted (roadmap 12 Polish, #162). Answers the clipboard sensitive-content policy of `docs/design.md` Security/privacy, the prerequisite for persistence.

Validated (#162) on niri with wl-clipboard 2.3.0: `wl-copy --sensitive` between two plain copies left only the plain ones in the history; a `wl-paste` printing version 2.2.1 started no watcher, and stderr, `kanade status` and `kanade doctor` said why.

## Context

The `clipboard` Module (#136, ADR 0011) keeps a history of what was copied, through `wl-paste --watch`. A password manager's copy must never enter it. On Wayland the convention is KDE's: the owner offers the `x-kde-passwordManagerHint` type next to the content, with `secret` as its data, as `wl-copy --sensitive` does.

Kanade sees no offer itself: wl-paste does, and runs Kanade per selection with the content on stdin and `CLIPBOARD_STATE` set (`hand_over`). Checked in wl-clipboard's source:

| wl-paste | `CLIPBOARD_STATE` of a selection offering the hint |
|---|---|
| 2.1 and older | unset |
| 2.2 | `data`, as for any selection |
| 2.3 and later | `sensitive` |

2.3 marks a selection sensitive whenever the hint type is offered, without reading whether it says `secret` or `public`.

## Decision

- **A selection marked sensitive is never read.** Only one whose `CLIPBOARD_STATE` is exactly `data` is handed to Kanade; `sensitive`, `nil`, `clear`, a state Kanade does not know, or none at all hands over `-` alone and leaves the content in wl-paste. Unset fails closed: it can only come from a wl-paste too old to tell.
- **Only a wl-paste that tells runs.** At start the `clipboard` Module runs `wl-paste --version`; older than 2.3, or a version it cannot read, and it starts no watcher, keeps no history, says why on stderr and in `kanade status`. `kanade doctor` warns the same.
- **The hint is the only signal.** Kanade does not guess at content (a pattern for keys or passwords) or at the copying app, which Wayland does not name.
- **Persistence, when it comes, stores only what this lets in.** An entry is in the history only after passing this, so a store of the history needs no second filter.

## Alternatives

**Check the offered types from Kanade.** `wl-paste --list-types` runs after the selection was handed over, so it may describe a newer one: a sensitive selection replaced at once by a public one would be kept. A Wayland data-control client of Kanade's own would see the offer, but breaks Amane owning the Wayland seam (ADR 0011).

**Keep history on an older wl-paste, warning only.** Fails open: every password copied lands in the history.

**Read the hint's data, keeping `public`.** Only Kanade's own reads would see it, and they race as above; wl-paste 2.3 treats any hint as sensitive, which errs safe.

**Content heuristics.** Miss most passwords and drop ordinary text; a false sense of safety.

## Consequences

- wl-clipboard 2.3 or later is needed for any history. A distribution still shipping 2.2 gets none until it updates.
- A copy whose owner does not offer the hint, like a script's `wl-copy` without `--sensitive`, is kept like any other, a password too. Removing the entry or clearing the history is the remedy; the README says so.
- wl-paste is checked once per start: one downgraded below 2.3 while Kanade runs is followed until the next start.
