# 124. Notification silence

Status: accepted.

## Decision

- Per-app silence is a control on the Notifications Surface and a row on the Dock menu. It is not a `notifications.rules` entry and is not written to settings. A state file under `$XDG_STATE_HOME/kanade/` remembers it. A matching config rule still wins, applied as the notification arrives. Silence means no Banner and no sound, Critical included, history kept. Unsilence restores the normal path for the next notification.
- `notifications.fullscreen_silence` (default on): while a fullscreen window covers the focused output, a non-Critical Banner waits and no notification sound plays. A Critical Banner, and one a rule lets bypass Do Not Disturb, still shows. Waiting Banners show when the output is no longer covered. The skipped sound is not played later. History is unchanged. Do Not Disturb still drops what it drops.

Files: `src/sources/notifications/silence.rs`, `src/sources/notifications/policy.rs`, `src/banners/stack.rs`, `src/config.rs`.
