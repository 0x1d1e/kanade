# 80. Desktop accessibility from the settings portal

Status: accepted. Builds on [ADR 0051](0051-paper-finish.md) and [ADR 0057](0057-blur-finish.md) (the finishes) and [ADR 0004](0004-wake-sources-on-announcements.md) (wake sources).

## Context

Reduced motion was only `reduced_motion` or `KANADE_REDUCED_MOTION`, so a user who had told the desktop they want less motion still got springs. The desktop's answer is in the settings portal, `org.freedesktop.portal.Desktop` `/org/freedesktop/portal/desktop`, interface `org.freedesktop.portal.Settings`, namespace `org.freedesktop.appearance`. Checked against the portal's interface description (`data/org.freedesktop.portal.Settings.xml` in xdg-desktop-portal, version 2 of the interface): `reduced-motion` and `contrast` are `u`, `0` no preference, `1` reduced motion or higher contrast, unknown values to be treated as `0`; `ReadAll(as) -> a{sa{sv}}` filters by namespace; `SettingChanged(s, s, v)` announces a change. The WebFetch tool was not available; the file was fetched with curl from the project's repository. Not tested against a live portal.

No portal reader existed: the `color-scheme` code in `appearance/adapters.rs` reads and writes gsettings, to sync the desktop's preference, not to follow it.

## Decision

- `sources/accessibility.rs` follows the two keys: it subscribes to `SettingChanged`, then reads the namespace with `ReadAll`, so no change falls between. A change that differs from what was held reloads the config (`reload::reload`). Anything but a number `1` is no preference. The thread blocks on the signal; the portal sends it rarely, so it adds no idle wakeup of its own beyond the session bus connection every source shares.
- The preferences enter at the one place `KANADE_REDUCED_MOTION` already does, `config::read`, never `config::layers`, so nothing is written to the user's files and Settings, which compares against `layers`, shows what the files say.
  - `reduced-motion` = 1 sets the island's motion to reduced, as `reduced_motion = true`.
  - `contrast` = 1 turns the translucent finishes (liquid glass, blur) into the solid one of `appearance.tone` (`Finish::solid`): paper for light, the tinted solid for dark. The two solid finishes already hold their ink at 7:1 and 4.5:1 (ADRs 0050, 0051), so they stay as the user chose. The saved `appearance.finish` is untouched.
- One switch, `appearance.accessibility`, default on, decides whether the portal is followed. It is a switch, not a tri-state `reduced_motion`, so existing files with `reduced_motion = true|false` keep reading as before and no `Kind` is added. The cost: a file's `reduced_motion = false` cannot override a desktop that asks; `appearance.accessibility = false` does. Precedence for motion: `KANADE_REDUCED_MOTION`, then `reduced_motion = true`, then the portal, then off. A desktop with no portal, or no preference, behaves as before.
- The thread starts after the Modules, only if the switch is on at start. Turning it on later takes a restart; turning it off applies at once, as the overlay checks it on every read.
- It is a setting of the core, not a Module: it has no window, verb or Service, and the Module list stays the user-facing features.

## Consequences

- A desktop that sets higher contrast turns a liquid glass or blur Island into a solid one live, and back.
- The contrast response is the finish only. Kanade's text sizes, Settings window and the app adapters do not change with `contrast`.
- `kanade config get appearance.finish` shows the file's value, not the effective one.
- Files: `src/sources/accessibility.rs`, `src/config.rs`, `src/look.rs`, `src/modules/mod.rs`.
