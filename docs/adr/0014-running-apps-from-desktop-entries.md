# 14. Running apps matched to desktop entries

Status: accepted (roadmap 10 Desktop/data, #143). Resolves the `docs/design.md` Open item "Define Dock `app_id` ↔ `.desktop` matching + override shape".

## Context

The Dock (#144) shows each running app once, with its name and icon, beside the pinned ones, and launches or focuses it. niri reports windows, each with a Wayland `app_id` that may be missing, may change, and often differs from the `.desktop` file that names the app (`dolphin` is `org.kde.dolphin.desktop`, Electron apps set a `StartupWMClass`). The design keeps raw niri objects out of the Dock: `windows` owns the running state. Amane's `DesktopApp` has neither the desktop file id nor `StartupWMClass`, so under ADR 0011 Kanade reads the entries itself.

## Decision

- **`windows` reads the island's niri stream.** Kanade already holds one EventStream (`src/sources/niri.rs`); its window events go to `windows`, which keeps each window's id, `app_id`, focus and urgency, and publishes the `Windows` Service only when the grouped result changes. A title or layout change wakes nothing. Losing the stream forgets every window; niri lists them again on reconnect.
- **A Running app** is one desktop entry's open windows, ordered by when their first window opened. Windows that match no entry group by `app_id`; a window with none stands alone. Nothing of niri leaves `windows` but window ids, which the Dock needs to focus one.
- **Matching, first hit wins**:
  1. the override in `windows.apps`, which is final: one naming a missing entry leaves the app unmatched rather than guessed;
  2. the file id `<app_id>.desktop`;
  3. `StartupWMClass` equal to the `app_id`;
  4. 2 or 3 ignoring ASCII case;
  5. the last dot-separated part of the file id, ignoring case.

  Within a rule the entry read first wins: the XDG data dirs in order, the user's first, then by path. An exact rule beats a looser one even for an entry read later.
- **Entries** are the `[Desktop Entry]` sections of `applications/**/*.desktop` under `XDG_DATA_HOME` and `XDG_DATA_DIRS`, with the spec's file ids (`kde/dolphin.desktop` is `kde-dolphin.desktop`) and the first id found shadowing the rest. `Hidden=true` deletes the entry, also shadowing a system one; `NoDisplay=true` is kept, since such apps still open windows. A file that cannot be read, or has no `[Desktop Entry]` with `Type` and `Name`, is skipped and shadows nothing. String values have the spec's escapes (`\s`, `\n`, `\t`, `\r`, `\\`) decoded.
- **No polling, no inotify.** Entries are read when the first window comes, again when an `app_id` comes that none matches (each such `app_id` once, so an unknown game does not rescan on every window), and on `kanade config reload`.
- **Pinned apps** (#144): the Dock hands `windows` its `dock.pinned` ids, and `windows` publishes each with its entry, by file id only, beside the running apps. Pins read entries too, and a pinned id none has rescans once, like an unknown `app_id`. A published entry carries its icon's file, resolved once, and how it launches, per the Desktop Entry spec (`src/sources/launch.rs`): `DBusActivatable=true` activates over D-Bus and runs `Exec` only when the bus says it could not deliver `Activate` (no such service, or it failed to start one): an unanswered or lost call may still start the app, so it is not retried; `Exec` is split into arguments with `%c`, `%i` and `%k` expanded and file/URL codes dropped, run in `Path`, and for `Terminal=true` through `xdg-terminal-exec`. niri spawns the command, so the app gets niri's environment.
- **Launcher apps** (#183): the Launcher lists the same entries, those without `NoDisplay=true` that can launch, and starts them the same way. It reads them when its Module starts and again whenever it opens, so an app installed since shows, still without polling.
- **Override shape**: `[windows.apps]`, `app_id = "desktop file id"`, the `.desktop` optional. Keys are exact `app_id`s. Layers merge by `app_id`.

## Alternatives

**Amane's app list.** It lacks file ids and `StartupWMClass`, so it cannot do rules 2, 3 or 5 or name an override target; an upstream change would block #143 on Amane.

**Fuzzy matching on `Name` or `Exec`.** Catches a few more apps but guesses wrong silently (several entries share a binary). An unmatched app still shows, with its `app_id`, and an override fixes it for good.

**Watching the applications dirs with inotify.** Exact, but one watch per dir and subdir, for a case the miss-triggered rescan and reload already cover.

**A regex or glob key in overrides.** Steam's `steam_app_<n>` is the main case; it can come later as its own key without changing this shape.

## Consequences

- An app installed after its own window opened stays unmatched until another unknown `app_id` comes or the config reloads.
- An app that changes its `app_id` while open moves to its new app; the Dock sees one app leave and another arrive.
- `kanade status` lists the `app_id`s that matched nothing, which is what an override is written for.
