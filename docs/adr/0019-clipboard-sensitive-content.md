# 19. Clipboard sensitive content

Status: accepted (roadmap 12 Polish, #162). Answers the clipboard sensitive-content policy of `docs/design.md` Security/privacy, the prerequisite for persistence. Amended by ADR 0028: with data-control in `crates/runtime`, sensitivity is filtered directly at the Wayland offer level. Offers with `x-kde-passwordManagerHint` are dropped and destroyed immediately without reading, superseding the `wl-paste` version check (`SENSITIVE_SINCE`), `CLIPBOARD_STATE` environment checks, and the doctor gate.

Validated (#162) on niri with wl-clipboard 2.3.0: `wl-copy --sensitive` between two plain copies left only the plain ones in the history; a `wl-paste` printing version 2.2.1 started no watcher, and stderr, `kanade status` and `kanade doctor` said why. A watcher killed after its `wl-paste` was swapped for one acting as 2.2 was not started again, and a sensitive copy after it was not kept.

## Context

The `clipboard` Module (#136, ADR 0011) keeps a history of what was copied. Selection announcements arrive from the runtime's data-control protocol (`ext_data_control_manager_v1` or `zwlr_data_control_manager_v1`, ADR 0028). A password manager's copy must never enter it. On Wayland the convention is KDE's: the owner offers the `x-kde-passwordManagerHint` type next to the content, with `secret` as its data, as `wl-copy --sensitive` does.

Historically, Kanade saw no offer itself when using `wl-paste --watch`: wl-paste ran Kanade per selection with the content on stdin and `CLIPBOARD_STATE` set (`hand_over`). Under runtime data-control, Kanade receives the data offer and its offered MIME types directly before requesting transfer.

## Decision

- **A selection marked sensitive is never read.** When the data control device receives a selection offer, its MIME types are checked before calling `receive`. If `x-kde-passwordManagerHint` is present among the offered types, the offer is immediately destroyed without binding a pipe or reading data.
- **Offer-level filtering supersedes tool version gating.** Because sensitivity is verified on the Wayland data offer itself before any bytes are requested, `wl-paste --version`, `SENSITIVE_SINCE`, `Provider::Paste`, the doctor check, `--clipboard-selection`, and `hand_over` are retired.
- **The hint is the only signal.** Kanade does not guess at content (a pattern for keys or passwords) or at the copying app, which Wayland does not name.
- **Persistence, when it comes, stores only what this lets in.** An entry is in the history only after passing this, so a store of the history needs no second filter.

## Alternatives

**Check the offered types from Kanade.** `wl-paste --list-types` runs after the selection was handed over, so it may describe a newer one: a sensitive selection replaced at once by a public one would be kept. A Wayland data-control client of Kanade's own would see the offer, but breaks Amane owning the Wayland seam (ADR 0011).

**Keep history on an older wl-paste, warning only.** Fails open: every password copied lands in the history.

**Read the hint's data, keeping `public`.** Only Kanade's own reads would see it, and they race as above; wl-paste 2.3 treats any hint as sensitive, which errs safe.

**Content heuristics.** Miss most passwords and drop ordinary text; a false sense of safety.

## Consequences

- wl-clipboard 2.3 or later is needed for any history. A distribution still shipping 2.2 gets none until it updates.
- A copy whose owner does not offer the hint, like a script's `wl-copy` without `--sensitive`, is kept like any other, a password too. Removing the entry or clearing the history is the remedy; the wiki says so.
- A watcher already running goes on as the wl-paste it started as: replacing the file does not change it, and the next start checks the new one. A file replaced between the check and the start is not caught.
