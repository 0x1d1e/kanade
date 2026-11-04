# 147. Umbriel compositor support and config editor

Status: accepted. Extends compositor support to Umbriel alongside niri (ADR 0028, ADR 0119). [ADR 0149](0149-umbriel-starts-a-shell-recovery-parent.md) adds a recovery parent behind the existing Umbriel `kanade` autostart entry, including lock intent that survives a rendering-shell crash.

## Context

Kanade was originally designed around niri as its sole Wayland compositor. Umbriel is an independent scrolling and tiling Wayland compositor developed to offer a lightweight, highly configurable alternative with native overview, smooth scrolling column layouts, and IPC socket control.

Users choosing Umbriel need the same seamless experience Kanade delivers under niri:
1. **Compositor state awareness**: Tracking window focus, workspace changes, overview state, and active keyboard layouts without polling.
2. **Config editor with undo**: Editing Umbriel's `config.toml` safely from Kanade Settings, preserving comments, formatting, and table ordering, with validation and backup recovery.
3. **Appearance synchronization**: Seamless Light and Dark mode switching across the shell, GTK, Qt, and Umbriel itself, harmonizing colors, typography, cursor themes, and icon themes.

## Decision

- **Umbriel IPC event stream**: Kanade connects to Umbriel's unix socket (`$UMBRIEL_SOCKET` or `$XDG_RUNTIME_DIR/umbriel-$WAYLAND_DISPLAY.sock`) on a background supervisor thread (`src/sources/umbriel.rs`), subscribing to `overview`, `windows`, `workspaces`, `outputs`, and `keyboard_layout` events. The `Umbriel` Service state reflects active window titles, workspace names, and overview status.
- **Window and Strip Surfaces** (extends ADRs 0132 and 0133): Umbriel snapshots become the same focused-window, workspace and strip models the Surfaces use under niri. Service-owned bridges preserve window UUIDs and workspace IDs behind sequential UI IDs; workspace moves resolve the latest index and output. Seat-active focus wins, with the focused workspace's remembered window retained while the Island holds the keyboard. Widths use logical output dimensions. Strip fitting watches Umbriel events, and tile selection resolves the UUID before focusing through IPC. Umbriel's consume-or-expel action stays enabled for a single tile and is labelled "Join / split"; its meaning differs from niri's "Expel bottom".
- **External screenshots**: Area and output captures use `slurp`/`grim`, then offer the saved PNG to `wl-copy --type image/png`. Clipboard helper failure leaves screenshot creation successful; window capture remains unsupported under Umbriel.
- **AST-based configuration engine**: The `umbriel_config` module (`src/umbriel_config/`) uses `toml_edit` to parse and mutate Umbriel's `config.toml`. Edits are staged in memory and applied atomically only when saved, preserving user comments, indentation, and key order. Changes are backed up to a ring in `~/.local/state/kanade/umbriel-config/` allowing unbounded undo.
- **Dedicated Settings page**: The Settings interface provides an Umbriel page (`src/settings/view/umbriel_page.rs` and `src/settings/umbriel.rs`) with sections for Layout (mode, gap, extent presets), Appearance (border width, corner radius, blur radius/passes, shadows), and Input (keyboard layout and repeat, touchpad gestures, mouse, and cursor theme/size).
- **Default typography and tokens**:
  - The default shell font fallback is set to `"JetBrainsMono Nerd Font"` (`src/theme.rs:FONT`), applied across shell widgets when `appearance.font` is unset.
  - GTK and Qt appearance adapters configure `"JetBrainsMono Nerd Font 11"` (`gtk-font-name` in GTK `settings.ini`, `[Fonts] general` in `qt*ct.conf`, and `font-name` in desktop settings).
- **Light/Dark appearance pairing**:
  - **Cursors**: Dark mode applies `"Bibata-Modern-Classic"` (fallback to installed cursor); Light mode applies `"Bibata-Modern-Ice"` (fallback to `Bibata-Modern-Classic` or installed cursor). Applied to `gtk-cursor-theme-name`, Qt cursor config, and desktop `cursor-theme`.
  - **Icons**: Dark mode applies `"Papirus-Dark"` (for Qt `icon_theme` and GTK `gtk-icon-theme-name`, fallback `Papirus`); Light mode applies `"Papirus"`.
  - **Widget themes**: Dark mode applies GTK theme `adw-gtk3-dark` and Qt style `Darkly`; Light mode applies GTK theme `adw-gtk3` and Qt style `Darkly` (or system default).
  - **Compositor palette**: Umbriel color schemes are generated from wallpaper Matugen roles into `~/.config/umbriel/kanade.toml` and linked via `[include.optional] files = ["kanade.toml"]`.
- **Restore paths**: When appearance synchronization is disabled, original values of themes, icons, cursors, and fonts are cleanly restored across GTK, Qt, and desktop settings without leaving orphaned entries.
- **Base distribution config**: `dist/umbriel/config.toml` ships with default autostart of Kanade, scrolling layout presets, blur layer rules for Kanade surfaces, JetBrainsMono Nerd Font typography, and Bibata cursor settings (`[input.cursor] theme = "Bibata-Modern-Classic", size = 24`).
- **Compositor IPC spawn**: When Umbriel is active, desktop entry `Exec` commands and dropped files launch via Umbriel's Unix IPC socket (`{"cmd":"action","action":"spawn","command":"<shell command>"}`), ensuring apps receive the compositor's environment rather than Kanade's without failing on missing `NIRI_SOCKET`.

## Autodetection and output settings

`sources::compositor::active()` chooses a reachable Umbriel socket first, then niri, or neither. Startup follows only that compositor's event stream; a missing socket ends its reconnect loop. Window, Strip, the Dock, capture, launcher and keybinding selection share this decision. Under Umbriel the Dock takes that snapshot's windows and focuses one by its bridged id; closing one from the Dock does nothing, since the IPC client has no close that names a window. Settings navigation, keyboard traversal, launcher destinations and search expose only the active compositor page; opening `compositor` or the other compositor routes to the active page.

The Desk's overview visibility and moving wallpaper's overview pause share `sources::compositor::overview()` and `watch_overview()`: each owning Module subscribes to the selected compositor's Service at start and takes its initial snapshot. No inactive compositor events or IPC polling drive these consumers, and a Module that is off registers no watcher. This extends the niri-only overview paths in ADRs 0137 and 0144 to Umbriel.

Both compositor pages use the same Outputs canvas, monitor cards and trial controller. Umbriel's `outputs` reply supplies the shared monitor model. Trials send `output-set` actions and never write the config. Keep writes `[output."<connector>"]` properties (`enabled`, `mode`, `position`, `scale`, `transform`, `vrr`) through the validated Umbriel editor, then requests `umbriel msg config-reload`; reset and undo reload too. VRR uses `disabled`, `always`, or `fullscreen`. Umbriel's pending recovery file is `umbriel-outputs-pending.json`, separate from niri's record. A rejected live action leaves no trial or config edit; an unanswered action retains recovery state. Umbriel versions without `output-set` can display monitors but reject trials explicitly.

## Auth (extends ADR 0146)

Auth uses the selected compositor's confirmed, present focused output and overview state bridged into `IslandService`. Umbriel tracks explicit output, seat-window or workspace focus separately from navigation's remembered/first-output fallback, and accepts it only on an output in the current output snapshot. Cached window re-derivation cannot confirm focus; stream loss clears confirmation. The shared bridge updates confirmation even when navigation focus stays the same. No second compositor watcher or output fallback is added. A never-shown request waits without holding the keyboard or accepting a password until focus exists and overview ends; the existing Island watcher retries it. Registration backoff remains separate from prompt retry.

Cancellation of the active request shuts down its helper and resets the password before the next request opens. Cancelling a queued request leaves the active prompt intact. Pending or held lock cancels every request, clears prompt state and prevents deferred opening, typing and submission. Each helper attempt has an identity reserved before its worker starts, so cancellation before connection and late outcomes cannot attach to or settle a later request, even if a cookie is reused. Queue, prompt, visit and helper transitions are serialized. Lock onset takes the same guard before the lock request mutex, synchronously cancels Auth before publishing pending lock or calling the compositor, and holds it through that request. Input mutations and each helper password write take the guard too; a helper that stops reading has a bounded write timeout. The guard is released before waiting for compositor news.

Disposable subprocess regressions exercise shared-focus placement with niri focus absent, niri placement, real Umbriel output/window/workspace events through the production Island bridge, missing/removed focus and overview retries through Service derivation, cancellation and helper shutdown, pending lock and late outcomes. A controlled fake-helper exchange reproduced a password write after pending lock before Service derivation; it now observes EOF, and the unlocked exchange still receives its answer. A controlled input interleaving verifies that lock onset waits for an edit already past its unlocked check and then clears it. They use synthetic requests and local socket pairs, without agent registration, PAM or a running shell. Live acceptance of this revision remains **NOT RUN**; these regressions are not live acceptance.

## Consequences

- Kanade seamlessly supports both niri and Umbriel sessions.
- Keybindings in `src/sources/binds.rs` load from either compositor's active configuration file.
- Appearance switching updates GTK, Qt, and Umbriel atomically with full revert capabilities on sync disable.
