# 83. An app's launcher entry progress is a Live Activity

Status: accepted. Builds on [ADR 0042](0042-live-activities-from-outside.md) (Live Activities), [ADR 0059](0059-notification-progress-as-a-live-activity.md) (progress from a notification), [ADR 0014](0014-running-apps-from-desktop-entries.md) (desktop entries) and [ADR 0004](0004-wake-sources-on-announcements.md) (wake sources).

## Context

Apps with a long job but no notification (a browser download, a file manager copy) tell docks about it with the Unity launcher entry signal: `com.canonical.Unity.LauncherEntry.Update(s app_uri, a{sv} properties)`, with `app_uri` like `application://firefox.desktop` and properties `progress` (double, 0 to 1), `progress-visible`, `count`, `count-visible`, `urgent`. The sender sends it from its own object path. A signal may carry only the properties that changed. Kanade showed none of it. Not tested against a live app in this change; the signature is from libunity's launcher entry documentation as the Dock shells that read it use it.

## Decision

- A new Module `launcher-entry`, on by default, requires `live`. Its thread (`sources/launcher_entry.rs`) matches `Update` on that interface, takes it only from the app it names where the bus can say who owns that app's name (`GetNameOwner` of the desktop file id without `.desktop`, asked when a sender differs and for a name nobody owned every 30 s), and blocks on it, so it wakes nothing at idle (ADR 0004). It is a Module of its own, not a part of `live`, so a user can turn the bus listening off and keep `kanade activity`.
- Each entry keeps a small state (`progress-visible`, `progress`), merged from the properties present and of the right kind (a bool, a number); a NaN or out-of-range amount is clamped to 0 to 1. At most 16 apps are tracked.
  - visible and under 1: `live::show` with the id `launcher.<desktop file id>`, the app's desktop entry name as the title, no detail, the amount in whole percent;
  - amount 1: `live::finish`, the Transient done form (ADR 0042), and the state is dropped;
  - not visible: `live::withdraw`, and the state is dropped.
  `count` and `urgent` are not shown: a count needs a badge on the Dock, which is not trivial for its draw and hit paths, and an urgent flag has no Activity form.
- The signal is any process's. It is bounded like a client's: the app is a `.desktop` file id of `[A-Za-z0-9._-]`, at most 48 characters, from an `application://` URI, else ignored; the title is the desktop entry's `Name` (read when an app is first met, the entries read again for an unknown app no oftener than every 30 s), else the file id without `.desktop`, never the signal's text. Nothing from a signal is logged.
- The ids begin `launcher.`, reserved like `notification.`: a client's `activity` verbs refuse them. At most `LAUNCHED` = 2 of the 8 live ones are launcher entries, the oldest giving way; a client keeps at least 3 (8 - 3 notifications - 2). A launcher entry is withdrawn after `live.stale` without a new signal, as any live one.

## Alternatives

**Part of `live`.** One switch less, but a user who wants no listening on the bus loses `kanade activity` too.

**Show `count` and `urgent` as a Dock badge.** Left out; the Dock has no badge form, and building one is more than this change.

**A share equal to the notifications'.** Both are driven by other programs; keeping them separate stops a download flood from evicting a notification's progress and the reverse.

## Consequences

- A program that claims to be an app can show an Activity titled with that app's name; the bounds above keep it to two Activities and plain text. The signal's sender is checked against the app only where the app can be identified on the bus: the desktop file id without `.desktop` is the well-known name a D-Bus activatable app owns, so when that name has an owner the signal must come from it. An app whose id D-Bus cannot name, or whose name nobody owns, is taken as the sender, so the check narrows the exposure without losing an app that does not own a name.

- An app that sends visible progress and dies leaves its Activity up to `live.stale`.
- Files: `src/sources/launcher_entry.rs`, `src/sources/live.rs`, `src/modules/catalog.rs`.
