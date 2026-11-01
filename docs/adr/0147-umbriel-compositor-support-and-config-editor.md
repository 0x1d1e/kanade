# 147. Umbriel compositor support and config editor

Status: accepted. Extends compositor support to Umbriel alongside niri (ADR 0028, ADR 0119).

## Context

Kanade was originally designed around niri as its sole Wayland compositor. Umbriel is an independent scrolling and tiling Wayland compositor developed to offer a lightweight, highly configurable alternative with native overview, smooth scrolling column layouts, and IPC socket control.

Users choosing Umbriel need the same seamless experience Kanade delivers under niri:
1. **Compositor state awareness**: Tracking window focus, workspace changes, overview state, and active keyboard layouts without polling.
2. **Config editor with undo**: Editing Umbriel's `config.toml` safely from Kanade Settings, preserving comments, formatting, and table ordering, with validation and backup recovery.
3. **Appearance synchronization**: Seamless Light and Dark mode switching across the shell, GTK, Qt, and Umbriel itself, harmonizing colors, typography, cursor themes, and icon themes.

## Decision

- **Umbriel IPC event stream**: Kanade connects to Umbriel's unix socket (`$UMBRIEL_SOCKET` or `$XDG_RUNTIME_DIR/umbriel-$WAYLAND_DISPLAY.sock`) on a background supervisor thread (`src/sources/umbriel.rs`), subscribing to `overview`, `windows`, `workspaces`, and `keyboard_layout` events. The `Umbriel` Service state reflects active window titles, workspace names, and overview status.
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

## Consequences

- Kanade seamlessly supports both niri and Umbriel sessions.
- Keybindings in `src/sources/binds.rs` load from either compositor's active configuration file.
- Appearance switching updates GTK, Qt, and Umbriel atomically with full revert capabilities on sync disable.
