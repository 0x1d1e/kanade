# 118. Notification sounds are chosen in layers

Status: accepted. Extends the opt-in notification sound recorded in `docs/design.md` (`notifications.sound`, a `sound-file`/`sound-name` hint, else `message-new-instant`, through `pw-play` or `canberra-gtk-play` under `setpriv`); no earlier ADR holds it, so this one records the whole. Nothing there changes for a config that sets none of the new keys.

## Context

The sound was the sender's hint or a built-in default, silenced only per rule with `sound = false`. Wanted: a default sound of the user's, one per urgency, a volume, and a rule that gives an app its own sound.

## Decision

- **Layers, the first that says something wins.** (1) the matching rule's `sound`; (2) the sender's hints (`sound-file`, else `sound-name`; `suppress-sound` silences everything below it); (3) the sound of the notification's urgency, `notifications.sound_low`, `sound_normal`, `sound_critical` (the urgency a rule gives counts); (4) `notifications.sound_file`; (5) `message-new-instant`. A sender's hint that is no good still plays nothing, as before. A rule's explicit sound beats even `suppress-sound`: the user's rule is the one thing Kanade does not let an app override. The rest of the sender's say is kept: it names the sound unless the user did.
- **A choice is a file, a name or `off`.** An absolute path (a leading `~/` is the home directory) must be an existing regular file; anything without a `/` must be a plain theme name (letters, digits, `.`, `_`, `-`, no leading `-` or `.`, 64 long at most); `off` (any case) is silence for that layer and everything below it. A layer of the user's whose file is missing or unreadable, whose name no installed player can find (pw-play only the freedesktop theme; canberra-gtk-play resolves names itself), or whose spelling is none of these, is skipped with a note on stderr and a warning in `kanade doctor`, and the next layer plays: never an error, never a crash. A path is never executed or put through a shell; it is one argument of the player. A rule's `sound` is checked as the file is read (a bad spelling refuses the rule by number, like every rule key), the file only as it plays, so a file that appears later is used.
- **Rule `sound` is a union.** `false` (or `"off"`) is silent, `true` says nothing (the sound it would have), a string is this sound. `volume` (0-100 %) is its own key, over `sound_volume`. First match still wins, whole.
- **Volume is the sound's stream, nothing else.** `notifications.sound_volume`, 0-100, default 100. `pw-play --volume=<0..1>` takes it linear; `canberra-gtk-play -V` takes dB, so the same factor as `20*log10`. At 100 no option is passed; at 0 nothing plays. It never reads or sets the system's or any app's volume. The volume applies to every layer, a sender's file included.
- **Unchanged.** Off by default; never under DND (Critical included); only for a notification new to the list; one sound at a time, 5 s, `setpriv`; no audio library.
- **Settings** shows the switch, `sound_file` (a text field) and `sound_volume` (a stepper). Per-urgency sounds and per-rule sounds are config-file only, like the rules.
- **No new verb.** `kanade notify` does not exist; `notify-send` from the user's own shell tests a sound, and a verb of Kanade's own would only wrap it.

## Alternatives

**A table `[notifications.sounds]` with `low`, `normal`, `critical`.** One key group, but the registry's keys are flat scalars with defaults shown verbatim in the wiki; three flat keys fit and need no new Kind.

**`suppress-sound` beaten by the urgency sound or `sound_file`.** Defeats a sender that asked for quiet for the sake of a taste; the user can say so in a rule.

**Perceptual (cubic) volume.** Closer to a slider's feel, but surprising against the dB the other player gets; linear amplitude is one factor both agree on.
