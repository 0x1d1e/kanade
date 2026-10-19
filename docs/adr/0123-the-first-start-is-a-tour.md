# 123. The first start is a tour

Status: accepted and implemented.

## Decision

- One pass after the first start of the shell with a focused output. The Welcome Surface explains how to open the Island and Surfaces, names the Launcher and Keys keybinds, and points to the Keys Surface for the user's niri binds.
- Enter, Space or Right advances; Left goes back; Escape dismisses. The final advance and dismissal both complete the tour.
- `$XDG_STATE_HOME/kanade/welcome-tour` gates later starts, following the durability pattern of `$XDG_STATE_HOME/kanade/health-notice` in ADR 0088. It is written only when the user finishes or dismisses. If the write fails, the tour still closes, and a later shell start may offer it again.
- The tour is opened by the always-on Island Module and is not withheld when `keys-surface` is off. It does not choose settings, write niri config, or replace the health notice. Settings stays the place for choices.

Files: `src/surfaces/welcome.rs` owns the tour and completion marker; `src/island/presentation.rs`, `src/island/geometry.rs`, `src/view/forms.rs`, and `src/view/input.rs` register and dispatch its Surface; `src/modules/catalog.rs` starts it after the core Island setup.
